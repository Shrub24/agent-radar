//! The daemon: the socket, the listener and the connection rules.
//!
//! One process owns the socket, and every connection is served on its own
//! thread, so a client that asks slowly cannot hold up the one asking now.
//! Operations execute synchronously on that bounded connection worker: once
//! accepted, each is recorded before the connection waits for its response.
//!
//! The listener refuses a socket directory that is not a real `0700` directory of
//! this user — a directory someone else can write is a directory someone else
//! can serve from — and a socket file another daemon answers on, while a stale
//! file left by a dead daemon is cleared. Shutdown is deliberate: stop accepting,
//! end the connections, join their threads, remove the socket file, and leave the
//! records where they are, because the records are what a later question is
//! answered from.
//!
//! The daemon speaks only the normalized [`RuntimeProvider`] seam. Herdr
//! transport and decoding remain inside the adapter.

use std::io::{BufReader, ErrorKind, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::control_plane::ops::{self, Method};
use crate::control_plane::protocol::{self, Code, PROTOCOL_VERSION, Refusal, Request, Response};
use crate::control_plane::registry::{
    self, AcquireRequest, Channel, LocalProcfsVerifier, LocationEvidence, ProcessVerifier,
    RegistrationRequest, Registry, RegistryLocation, SpawnEdge,
};
use crate::control_plane::store::{self, Category, RequestOutcome, RequestRecord, Store};
use crate::lifecycle::{self, CloseRequest, DirectClose};
use crate::model::FleetObservation;
use crate::runtime::{
    CreateOutcome, CreateRequest, CreatedKind, CreatedLocation, FocusOutcome, InputOutcome,
    InputPayload, InputRequest, LaunchOutcome, LaunchRequest, OutputOutcome, OutputRead,
    OutputRequest, ReportOutcome, ReportRequest, RuntimeProvider, SplitDirection, Target,
    validate_command,
};
use crate::{HerdrConfig, HerdrRuntime};

/// The socket file's name inside the directory the daemon owns.
const SOCKET_NAME: &str = "control.sock";

/// The word the daemon reports for the mux backend it was given: what liveness
/// answers, and what a spawn edge records as the backend of the pane it created.
const WIRED_BACKEND: &str = "runtime";

/// How many connections are served at once. A connection beyond this is closed
/// rather than queued, so a client that opens sockets in a loop cannot grow the
/// daemon's threads without bound.
const MAX_CONNECTIONS: usize = 16;

/// Socket writes time out even when a connected client stops reading.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a connection may sit without sending a line before it is closed. A
/// client that is thinking is free to think; a client that has gone away must
/// not hold its thread for the daemon's lifetime.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// The most records one `requests` answer carries, and what it carries when the
/// caller names no limit.
const MAX_LISTED: usize = 200;
const DEFAULT_LISTED: usize = 50;
/// The registry returns at most this many records in one page, agents and spawn
/// edges alike.
const MAX_REGISTRY_PAGE: usize = registry::MAX_LISTED;
/// Keep response payloads below the protocol line limit, including JSON framing.
///
/// A page holds at most [`MAX_REGISTRY_PAGE`] records of a few kilobytes each, so
/// no page reaches this bound today. It stays as the protocol's own limit, where
/// a field allowed to grow past its current bound would otherwise push one page
/// past what a client can read in a single line.
const MAX_LIST_RESPONSE_BYTES: usize = protocol::MAX_LINE_BYTES - 1024;
/// How long a socket worker may wait for injected process verification. The
/// local procfs verifier answers in microseconds; the bound is for an injected
/// or unresponsive source, and a verification that misses it is `unavailable`.
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(3);
/// The daemon-wide bound on process-verifier calls executing at once.
///
/// A call that misses the three-second verification deadline, or whose
/// connection ends, keeps its slot until the injected verifier actually returns,
/// so repeated requests
/// cannot accumulate blocked verifier threads. The limit is therefore also the
/// ceiling on uncancellable verifier threads a stop request may leave behind:
/// the daemon never joins a verifier that is still blocked, and a verifier that
/// blocks forever holds its slot for the life of the process.
pub const MAX_VERIFICATION_JOBS: usize = 4;
/// How often a worker waiting on verification re-checks whether the daemon is
/// stopping, so shutdown does not wait out [`VERIFICATION_TIMEOUT`].
const VERIFY_POLL: Duration = Duration::from_millis(50);

/// The most output text one `output` answer carries, and the most rows it may
/// ask for. A line can be very long and the daemon's write has a deadline, so a
/// read is bounded here whether or not the backend bounded its own snapshot.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_LINES: u32 = 500;

/// How long the accept loop waits before it re-checks whether it should stop.
const POLL_MS: i32 = 200;

/// Set by a signal handler, read by the accept loop.
///
/// A signal handler may only touch memory that needs no allocation, so the
/// handler sets this and the loop — which is at most one poll away — acts on it.
/// Only the daemon process installs handlers; a library user that never calls
/// [`install_signal_handlers`] is never interrupted by this.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Asks the daemon to stop. The signal is the operator's; this is the same
/// request from inside the process, and it is what [`install_signal_handlers`]
/// sets from the signal handler.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

extern "C" fn handle_signal(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Arranges for `SIGINT` and `SIGTERM` to stop the daemon, and for `SIGPIPE` to
/// be ignored.
///
/// Called by the `daemon` subcommand and by nothing else: a process that has not
/// asked for this must not have its signals hijacked. `SA_RESTART` is left off
/// deliberately, so an interrupted poll returns and the loop notices the flag at
/// once rather than after its timeout. Ignoring `SIGPIPE` is what keeps a client
/// that hung up mid-response from killing the daemon.
pub fn install_signal_handlers() -> Result<(), String> {
    // SAFETY: `sigaction` is initialized to zero and then filled in field by
    // field; the handler is a plain `extern "C"` function that only stores to an
    // atomic, which is all a signal handler may do.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_signal as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(format!(
                    "cannot handle signal {signal}: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    Ok(())
}

/// The socket path, in the documented order: `RADAR_CONTROL_SOCKET`, else
/// `$XDG_RUNTIME_DIR/agent-radar/control.sock`, else
/// `/tmp/agent-radar-<uid>/control.sock`.
pub fn socket_path() -> PathBuf {
    socket_path_in(
        std::env::var_os("RADAR_CONTROL_SOCKET")
            .filter(|value| !value.is_empty())
            .as_deref(),
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .as_deref(),
        current_uid(),
    )
}

/// [`socket_path`] over an environment the caller supplies, so the rule can be
/// read and tested without touching the process environment.
pub fn socket_path_in(
    control_socket: Option<&std::ffi::OsStr>,
    xdg_runtime_dir: Option<&std::ffi::OsStr>,
    uid: u32,
) -> PathBuf {
    if let Some(path) = control_socket {
        return PathBuf::from(path);
    }
    match xdg_runtime_dir {
        Some(runtime_dir) => Path::new(runtime_dir).join("agent-radar").join(SOCKET_NAME),
        None => Path::new("/tmp")
            .join(format!("agent-radar-{uid}"))
            .join(SOCKET_NAME),
    }
}

/// The state root the daemon keeps its records under: `RADAR_CONTROL_STATE`, else
/// `$XDG_STATE_HOME/agent-radar/control`, else `~/.local/state/agent-radar/control`.
///
/// With none of the three set — no override, no state home and no home directory
/// — the temporary directory's own `agent-radar-<uid>` name is used, which is the
/// same last resort the socket path has. A record has to land somewhere; landing
/// somewhere private and unnamed by the documentation is better than refusing to
/// serve.
pub fn state_dir() -> PathBuf {
    state_dir_in(
        std::env::var_os("RADAR_CONTROL_STATE")
            .filter(|value| !value.is_empty())
            .as_deref(),
        std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .as_deref(),
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .as_deref(),
        current_uid(),
    )
}

/// [`state_dir`] over an environment the caller supplies.
pub fn state_dir_in(
    control_state: Option<&std::ffi::OsStr>,
    xdg_state_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    uid: u32,
) -> PathBuf {
    if let Some(path) = control_state {
        return PathBuf::from(path);
    }
    let root = match xdg_state_home {
        Some(state_home) => PathBuf::from(state_home),
        None => match home {
            Some(home) => PathBuf::from(home).join(".local").join("state"),
            None => return Path::new("/tmp").join(format!("agent-radar-{uid}")),
        },
    };
    root.join("agent-radar").join("control")
}

/// The current uid, the value the socket directory must be owned by.
fn current_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// The listener's shared state: every connection and every worker it started.
#[derive(Default)]
struct Connections {
    /// Live connections by the id they were accepted under, kept so stopping can
    /// end their blocked reads and so a finished one frees its slot.
    live: Vec<(u64, UnixStream)>,
    workers: Vec<JoinHandle<()>>,
}

/// Daemon-wide process-verification admission. A detached call keeps its slot
/// until the injected verifier returns, so timeouts cannot grow outstanding work.
struct VerificationBudget {
    active: Mutex<usize>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl VerificationBudget {
    fn acquire(self: &Arc<Self>) -> Option<VerificationPermit> {
        let mut active = locked(&self.active);
        if *active >= MAX_VERIFICATION_JOBS {
            return None;
        }
        *active += 1;
        Some(VerificationPermit(Arc::clone(self)))
    }

    fn track(&self, worker: JoinHandle<()>) {
        let mut workers = locked(&self.workers);
        let mut unfinished = Vec::with_capacity(workers.len() + 1);
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                unfinished.push(worker);
            }
        }
        unfinished.push(worker);
        *workers = unfinished;
    }

    fn reap(&self) {
        let mut workers = locked(&self.workers);
        let mut unfinished = Vec::with_capacity(workers.len());
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                unfinished.push(worker);
            }
        }
        *workers = unfinished;
    }

    fn detach_remaining(&self) {
        let mut workers = locked(&self.workers);
        let all = std::mem::take(&mut *workers);
        for worker in all {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                drop(worker);
            }
        }
    }
}

struct VerificationPermit(Arc<VerificationBudget>);

impl Drop for VerificationPermit {
    fn drop(&mut self) {
        let mut active = locked(&self.0.active);
        *active -= 1;
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned mutex means some other thread panicked while holding it. The
    // state behind this one is a store and a list; the daemon keeps serving from
    // it rather than taking the whole control plane down with one bad request.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What every connection shares: the durable records.
struct Core {
    store: Store,
    registry: Arc<Registry>,
    backend: Option<Arc<dyn RuntimeProvider>>,
    verifier: Arc<dyn ProcessVerifier>,
    stopping: Arc<AtomicBool>,
}

fn stop_backend_for(connections: &Arc<Mutex<Connections>>) {
    let connections = locked(connections);
    for (_, stream) in &connections.live {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

/// The control-plane daemon.
///
/// [`Daemon::bind_at`] binds the socket and starts accepting at once, so a caller
/// that only needs a daemon for the length of a test binds it, drives it, and
/// calls [`Daemon::stop`]. [`Daemon::run`] is for the process that should stay up
/// until it is signalled.
pub struct Daemon {
    path: PathBuf,
    state: PathBuf,
    stopping: Arc<AtomicBool>,
    connections: Arc<Mutex<Connections>>,
    verification: Arc<VerificationBudget>,
    accept: Option<JoinHandle<()>>,
    socket_identity: (u64, u64),
}

impl Daemon {
    /// Binds the socket at [`socket_path`], keeping records under [`state_dir`].
    pub fn bind() -> Result<Self, String> {
        let backend = Arc::new(HerdrRuntime::new(HerdrConfig::default()));
        Self::bind_with_backend(&socket_path(), &state_dir(), Some(backend))
    }

    /// Binds a daemon with no backend, for protocol/store-only tests.
    pub fn bind_without_backend(socket: &Path, state: &Path) -> Result<Self, String> {
        Self::bind_with_backend(socket, state, None)
    }

    /// Binds a production Herdr adapter at custom test/application paths.
    pub fn bind_herdr_at(socket: &Path, state: &Path, config: HerdrConfig) -> Result<Self, String> {
        Self::bind_with_backend(socket, state, Some(Arc::new(HerdrRuntime::new(config))))
    }

    /// Binds `socket`, keeping records under `state`, creating and verifying both
    /// directories.
    ///
    /// `Err` is the diagnostic for a control plane that must not serve: a socket
    /// directory that is a symlink, foreign-owned or not `0700`, a socket file
    /// another daemon answers on, a bind that failed, or a state directory that
    /// failed the same checks.
    pub fn bind_at(socket: &Path, state: &Path) -> Result<Self, String> {
        let backend = Arc::new(HerdrRuntime::new(HerdrConfig::default()));
        Self::bind_with_backend(socket, state, Some(backend))
    }

    /// Binds with an injected normalized runtime backend.
    pub fn bind_with_backend(
        socket: &Path,
        state: &Path,
        backend: Option<Arc<dyn RuntimeProvider>>,
    ) -> Result<Self, String> {
        Self::bind_with_backend_and_verifier(socket, state, backend, Arc::new(LocalProcfsVerifier))
    }

    /// Binds with injected runtime and process verifier seams for integration
    /// tests and alternate local evidence providers.
    pub fn bind_with_backend_and_verifier(
        socket: &Path,
        state: &Path,
        backend: Option<Arc<dyn RuntimeProvider>>,
        verifier: Arc<dyn ProcessVerifier>,
    ) -> Result<Self, String> {
        // The socket's own directory is what must be ours, not the socket path:
        // binding in a directory anyone can write is the thing being refused.
        prepare_directory(socket_directory(socket))?;
        let listener = bind_socket(socket)?;
        let metadata = std::fs::symlink_metadata(socket)
            .map_err(|error| format!("control socket {}: {error}", socket.display()))?;
        let socket_identity = (metadata.dev(), metadata.ino());
        if let Err(error) = listener.set_nonblocking(true) {
            let _ = remove_socket_if_owned(socket, socket_identity);
            return Err(format!("control socket {}: {error}", socket.display()));
        }
        let store = match Store::open(state) {
            Ok(store) => store,
            Err(error) => {
                let _ = remove_socket_if_owned(socket, socket_identity);
                return Err(error);
            }
        };
        let registry = match Registry::open(state) {
            Ok(registry) => registry,
            Err(error) => {
                let _ = remove_socket_if_owned(socket, socket_identity);
                return Err(error);
            }
        };
        let stopping = Arc::new(AtomicBool::new(false));
        let verification = Arc::new(VerificationBudget {
            active: Mutex::new(0),
            workers: Mutex::new(Vec::new()),
        });
        let core = Arc::new(Mutex::new(Core {
            store,
            registry: Arc::new(registry),
            backend,
            verifier,
            stopping: Arc::clone(&stopping),
        }));
        let connections = Arc::new(Mutex::new(Connections::default()));
        let result = thread::Builder::new()
            .name("radar-control-accept".to_string())
            .spawn({
                let listener = match listener.try_clone() {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = remove_socket_if_owned(socket, socket_identity);
                        return Err(format!("control socket {}: {error}", socket.display()));
                    }
                };
                let core = Arc::clone(&core);
                let stopping = Arc::clone(&stopping);
                let connections = Arc::clone(&connections);
                let verification = Arc::clone(&verification);
                move || accept_loop(listener, core, stopping, connections, verification)
            });
        let accept = match result {
            Ok(accept) => accept,
            Err(error) => {
                let _ = remove_socket_if_owned(socket, socket_identity);
                return Err(format!("control accept thread: {error}"));
            }
        };
        Ok(Self {
            path: socket.to_path_buf(),
            state: state.to_path_buf(),
            stopping,
            connections,
            verification,
            accept: Some(accept),
            socket_identity,
        })
    }

    /// The socket this daemon serves on.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The state root this daemon keeps records under.
    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    /// Serves until the process is signalled or another thread stops it, then
    /// stops cleanly.
    ///
    /// The accept thread is already running: this is the process's own wait, not
    /// the serving itself.
    pub fn run(&mut self) -> Result<(), String> {
        while !self.stopping.load(Ordering::SeqCst) && !interrupted() {
            thread::sleep(Duration::from_millis(POLL_MS as u64));
        }
        self.stop();
        Ok(())
    }

    /// Stops accepting, ends every connection, joins their threads, and removes
    /// the socket file this daemon created. Idempotent.
    ///
    /// Records are left exactly where they are: they are the only account of what
    /// was asked for, and a daemon that removed them on the way out would make
    /// "did this run" unanswerable.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.stopping.store(true, Ordering::SeqCst);
        stop_backend_for(&self.connections);
        let _ = accept.join();
        let (live, workers): (Vec<(u64, UnixStream)>, Vec<JoinHandle<()>>) = {
            let mut connections = locked(&self.connections);
            (
                std::mem::take(&mut connections.live),
                std::mem::take(&mut connections.workers),
            )
        };
        // Shut down reply streams as well as reads before joining, so a slow
        // client cannot keep a worker blocked while the daemon is stopping.
        for (_, connection) in &live {
            let _ = connection.shutdown(Shutdown::Both);
        }
        for worker in workers {
            let _ = worker.join();
        }
        self.verification.detach_remaining();
        let _ = remove_socket_if_owned(&self.path, self.socket_identity);
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Accepts connections until stopped, one thread each. A connection beyond
/// [`MAX_CONNECTIONS`] is closed instead of served.
fn accept_loop(
    listener: UnixListener,
    core: Arc<Mutex<Core>>,
    stopping: Arc<AtomicBool>,
    connections: Arc<Mutex<Connections>>,
    verification: Arc<VerificationBudget>,
) {
    let mut next_id: u64 = 0;
    while !stopping.load(Ordering::SeqCst) {
        if !connection_waiting(&listener) {
            continue;
        }
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                // The backlog this poll woke for is drained; poll again.
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                // A signal arrives as an error and is not a reason to stop on its
                // own: the stopping flag decides. Anything else means the
                // listening socket itself failed, so stop rather than spin.
                Err(error) if error.kind() == ErrorKind::Interrupted => break,
                Err(_) => return,
            };
            let Ok(seen) = stream.try_clone() else {
                continue;
            };
            let id = {
                let mut connections = locked(&connections);
                if connections.live.len() >= MAX_CONNECTIONS {
                    // Dropping the stream closes it, so a client over the limit
                    // sees an ended connection rather than a served one.
                    continue;
                }
                // Finished workers are dropped rather than joined here, so
                // reconnects cannot grow the list over a daemon's lifetime.
                connections.workers.retain(|worker| !worker.is_finished());
                let id = next_id;
                next_id += 1;
                connections.live.push((id, seen));
                id
            };
            verification.reap();
            let spawned = thread::Builder::new()
                .name(format!("radar-control-{id}"))
                .spawn({
                    let core = Arc::clone(&core);
                    let stopping = Arc::clone(&stopping);
                    let connections = Arc::clone(&connections);
                    let verification = Arc::clone(&verification);
                    move || serve(stream, id, &core, &stopping, &connections, &verification)
                });
            match spawned {
                Ok(worker) => locked(&connections).workers.push(worker),
                // No thread came up: close the connection rather than hold a
                // slot for it until the daemon stops.
                Err(_) => end_connection(&connections, id),
            }
        }
    }
}

/// Whether the listener has a connection waiting, without accepting it.
///
/// A non-blocking listener reports "nothing yet" as an error, so the poll is what
/// separates "wait for a client" from "spin on an empty backlog" — and it is also
/// what bounds how long a stop request waits to be noticed.
fn connection_waiting(listener: &UnixListener) -> bool {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized `pollfd` for one descriptor, and the timeout is a
    // plain integer.
    let ready = unsafe { libc::poll(&mut descriptor, 1, POLL_MS) };
    ready > 0 && (descriptor.revents & libc::POLLIN) != 0
}

/// Serves one connection: read lines, answer each, and keep the connection while
/// the client keeps sending usable requests.
fn serve(
    stream: UnixStream,
    id: u64,
    core: &Arc<Mutex<Core>>,
    stopping: &Arc<AtomicBool>,
    connections: &Arc<Mutex<Connections>>,
    verification: &Arc<VerificationBudget>,
) {
    if stream.set_read_timeout(Some(IDLE_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        end_connection(connections, id);
        return;
    }
    let Ok(writer) = stream.try_clone() else {
        end_connection(connections, id);
        return;
    };
    let writer = Arc::new(Mutex::new(writer));
    let mut reader = BufReader::new(stream);
    while !stopping.load(Ordering::SeqCst) {
        match protocol::read_line(&mut reader) {
            Ok(protocol::Line::Data(line)) => handle_line(&line, id, core, &writer, verification),
            // An over-long line is drained already, so the connection is still at
            // a line boundary and can keep serving.
            Ok(protocol::Line::TooLong) => answer(
                &writer,
                Response::refused(
                    "",
                    &Refusal::new(
                        Code::BadRequest,
                        format!("the line exceeds {} bytes", protocol::MAX_LINE_BYTES),
                    ),
                ),
            ),
            Ok(protocol::Line::End) => break,
            // A read that failed — an ended connection, an idle timeout, a
            // shutdown while this thread was blocked — ends this connection and
            // leaves the others alone.
            Err(_) => break,
        }
    }
    end_connection(connections, id);
}

/// Forgets a connection that has finished, so its slot is free for the next one.
fn end_connection(connections: &Arc<Mutex<Connections>>, id: u64) {
    locked(connections).live.retain(|(held, _)| *held != id);
}

/// Turns one line into one answer.
///
/// A request that cannot be decoded is refused here and the connection keeps
/// serving. Operations execute synchronously on the bounded connection worker.
fn handle_line(
    line: &[u8],
    _id: u64,
    core: &Arc<Mutex<Core>>,
    writer: &Arc<Mutex<UnixStream>>,
    verification: &Arc<VerificationBudget>,
) {
    let request = match protocol::decode_request(line) {
        Ok(request) => request,
        Err(refusal) => {
            // The id is echoed when the line names one, so a client can still
            // correlate a request the daemon refused to read as a request.
            answer(
                writer,
                Response::refused(&protocol::peek_id(line), &refusal),
            );
            return;
        }
    };
    let Some(method) = ops::classify(&request.method) else {
        answer(
            writer,
            Response::refused(
                &request.id,
                &Refusal::new(
                    Code::UnknownMethod,
                    format!("unknown method `{}`", request.method),
                ),
            ),
        );
        return;
    };
    match method {
        Method::Ping => {
            let (backend, mut capabilities) = {
                let core = locked(core);
                match &core.backend {
                    Some(backend) => (WIRED_BACKEND, backend.capabilities().to_vec()),
                    None => ("none", Vec::new()),
                }
            };
            capabilities.push("agent_registry");
            answer(
                writer,
                Response::ok(
                    &request.id,
                    json!({
                        "protocol": PROTOCOL_VERSION,
                        "backend": backend,
                        "capabilities": capabilities,
                    }),
                ),
            );
        }
        Method::Request => answer(writer, read_one(core, &request)),
        Method::Requests => answer(writer, read_many(core, &request)),
        Method::Registry => answer(writer, registry_request(core, &request, verification)),
        Method::Read => answer(writer, read_backend(core, &request)),
        Method::Operation(operation) => {
            let response = execute(operation, &request, core);
            answer(writer, response);
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentGetParams {
    agent_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentListParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRetireParams {
    agent_id: String,
    channel: Channel,
    writer_handle: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnGetParams {
    request_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnListParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Process evidence as a client reads it: whether an identity was claimed, and,
/// when it was checked, the labelled outcome of that check. Publication freshness
/// is a separate field on each channel, and a claim is never a verification.
#[derive(Clone, Debug, serde::Serialize)]
struct ProcessEvidence {
    claimed: bool,
    verification: registry::ProcessVerification,
}

fn decode_params<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, Response> {
    serde_json::from_value(request.params.clone()).map_err(|error| {
        Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, format!("params are unusable: {error}")),
        )
    })
}

/// Maps a registry failure onto a protocol code.
///
/// The registry reports a missing name and a damaged store through one message
/// space, so the two cases a client must tell apart are chosen here: a name that
/// does not exist is `not_found`, and every other failure — a refused transition
/// or an invalid stored record — is a refusal whose message names the cause. A
/// corrupt record is never answered as a successful omission.
fn registry_error(request: &Request, error: String) -> Response {
    let code = if error.contains("is not registered") || error.contains("has no writer") {
        Code::NotFound
    } else {
        Code::Refused
    };
    Response::refused(&request.id, &Refusal::new(code, error))
}

/// Runs one process verification on its own thread, bounded by `deadline`.
///
/// The verifier may read procfs and can, in a test, block; the connection worker
/// polls so a stop request is noticed in [`VERIFY_POLL`] rather than after the
/// whole deadline. A verification that does not finish in time is `unavailable`,
/// which is explicitly not evidence of absence.
fn verify_bounded(
    process: Option<crate::model::ProcessIdentity>,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> registry::ProcessVerification {
    if deadline.saturating_duration_since(Instant::now()).is_zero()
        || stopping.load(Ordering::SeqCst)
    {
        return registry::ProcessVerification::Unavailable;
    }
    budget.reap();
    let Some(permit) = budget.acquire() else {
        return registry::ProcessVerification::Unavailable;
    };
    let Some(process) = process else {
        drop(permit);
        return registry::ProcessVerification::Unavailable;
    };
    let (send, receive) = mpsc::sync_channel(1);
    let verifier = Arc::clone(verifier);
    let spawned = thread::Builder::new()
        .name("radar-agent-verify".into())
        .spawn(move || {
            let _permit = permit;
            let result = registry::verify_identity(Some(&process), verifier.as_ref());
            let _ = send.send(result);
        });
    match spawned {
        Ok(worker) => budget.track(worker),
        Err(_) => return registry::ProcessVerification::Unavailable,
    }
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return registry::ProcessVerification::Unavailable;
        }
        match receive.recv_timeout(remaining.min(VERIFY_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return registry::ProcessVerification::Unavailable;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stopping.load(Ordering::SeqCst) {
                    return registry::ProcessVerification::Unavailable;
                }
            }
        }
    }
}

fn process_evidence(
    registration: &registry::Registration,
    verification: registry::ProcessVerification,
) -> ProcessEvidence {
    ProcessEvidence {
        claimed: registration.request.process.is_some(),
        verification,
    }
}

/// What the backend reports right now, or `None` when it could not be asked.
///
/// A topology read is answered from the daemon's own records whether or not a
/// backend can be consulted, so a backend that is missing, unobservable, failing
/// or stopping leaves a recorded location unconfirmed instead of refusing the
/// read. One observation serves one answer, and the call runs outside every
/// registry lock because it reaches the runtime.
fn observed_inventory(core: &Arc<Mutex<Core>>) -> Option<FleetObservation> {
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let backend = backend?;
    // The same capability the wire `observe` read needs: a backend that cannot
    // answer that read cannot confirm a location here either.
    if capabilities_refuse(&backend, "observe").is_some() || cancel.load(Ordering::SeqCst) {
        return None;
    }
    match backend.inventory(&cancel) {
        Ok(inventory) if !cancel.load(Ordering::SeqCst) => Some(inventory),
        _ => None,
    }
}

/// What one read's observation says about a recorded location.
///
/// A pane's absence is evidence about a pane, never about the process that ran
/// in it, and an edge that recorded no pane has nothing to check — so the read
/// reports the disagreement, or its lack of confirmation, and claims nothing
/// else.
fn location_evidence(inventory: Option<&FleetObservation>, edge: &SpawnEdge) -> LocationEvidence {
    let recorded = edge
        .location
        .as_ref()
        .and_then(|location| location.pane.as_deref());
    match (inventory, recorded) {
        (Some(inventory), Some(pane)) if inventory.pane(pane).is_some() => {
            LocationEvidence::Present
        }
        (Some(_), Some(_)) => LocationEvidence::Absent,
        _ => LocationEvidence::Unavailable,
    }
}

fn registry_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match request.method.as_str() {
        "agent.register" => {
            let registration: RegistrationRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.register(&registration, store::now_ms()) {
                Ok(record) => Response::ok(&request.id, json!({"registration": record.public()})),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.acquire" => {
            let acquire: AcquireRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.acquire_writer(&acquire, store::now_ms()) {
                Ok(binding) => Response::ok(&request.id, json!({"writer": binding})),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.publish" => {
            let publish: registry::PublishRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.publish(&publish, store::now_ms()) {
                Ok(record) => Response::ok(
                    &request.id,
                    json!({"channel": record.view(store::now_ms(), core.registry.serving_epoch())}),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.context" => {
            let publish: registry::ContextRequest = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            let now_ms = store::now_ms();
            match core.registry.publish_context(&publish, now_ms) {
                Ok(result) => Response::ok(
                    &request.id,
                    json!({
                        // The write response returns the credential only to its
                        // publisher; agent.get/list use the credential-free projection.
                        "writer": result.record.writer,
                        "context": result.record.public(now_ms, core.registry.serving_epoch()),
                        "warning": result.warning,
                    }),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.retire" => {
            let params: AgentRetireParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let core = locked(core);
            match core.registry.retire(
                &params.agent_id,
                params.channel,
                &params.writer_handle,
                store::now_ms(),
            ) {
                Ok(record) => Response::ok(
                    &request.id,
                    json!({"channel": record.view(store::now_ms(), core.registry.serving_epoch())}),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "agent.get" => {
            let params: AgentGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            let registration = match registry.get(&params.agent_id) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::NotFound, "agent is not registered"),
                    );
                }
                Err(error) => return registry_error(request, error),
            };
            // Verification is external evidence, so it runs outside every
            // registry lock and is bounded: a slow verifier must not hold the
            // worker past the deadline or a stop request.
            let deadline = Instant::now() + VERIFICATION_TIMEOUT;
            let process = registration.request.process.clone();
            let verified = verify_bounded(process, &verifier, &stopping, verification, deadline);
            if stopping.load(Ordering::SeqCst) {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Busy, "daemon is stopping"),
                );
            }
            let now_ms = store::now_ms();
            let execution =
                match registry.published_facts(&params.agent_id, Channel::Execution, now_ms) {
                    Ok(value) => value,
                    Err(error) => return registry_error(request, error),
                };
            let assignment =
                match registry.published_facts(&params.agent_id, Channel::Assignment, now_ms) {
                    Ok(value) => value,
                    Err(error) => return registry_error(request, error),
                };
            let context = match registry.context(&params.agent_id, now_ms) {
                Ok(value) => value,
                Err(error) => return registry_error(request, error),
            };
            Response::ok(
                &request.id,
                json!({"agent": {
                    "registration": registration.public(),
                    "process": process_evidence(&registration, verified),
                    "execution": execution, "assignment": assignment, "context": context,
                }}),
            )
        }
        "agent.list" => {
            let params: AgentListParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            // One page reads its registry entries once, then verifies each with a
            // single shared deadline so a page of blocked verifiers cannot hold
            // the worker, or the shutdown that joins it, without bound.
            let deadline = Instant::now() + VERIFICATION_TIMEOUT;
            // One record beyond the page tells whether a continuation exists,
            // without reading the whole fleet to page a bounded answer.
            let read = match registry.list_after(params.after.as_deref(), limit + 1) {
                Ok(records) => records,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let candidates: Vec<_> = read.into_iter().take(limit).collect();
            let now_ms = store::now_ms();
            let mut entries = Vec::new();
            let mut used = 0usize;
            for registration in &candidates {
                let process = registration.request.process.clone();
                let verified =
                    verify_bounded(process, &verifier, &stopping, verification, deadline);
                let execution = match registry.published_facts(
                    &registration.agent_id,
                    Channel::Execution,
                    now_ms,
                ) {
                    Ok(value) => value,
                    Err(error) => return registry_error(request, error),
                };
                let assignment = match registry.published_facts(
                    &registration.agent_id,
                    Channel::Assignment,
                    now_ms,
                ) {
                    Ok(value) => value,
                    Err(error) => return registry_error(request, error),
                };
                let context = match registry.context(&registration.agent_id, now_ms) {
                    Ok(value) => value,
                    Err(error) => return registry_error(request, error),
                };
                let entry = json!({
                    "registration": registration.public(),
                    "process": process_evidence(registration, verified),
                    "execution": execution,
                    "assignment": assignment,
                    "context": context,
                });
                let size = serde_json::to_vec(&entry).map_or(usize::MAX, |bytes| bytes.len() + 1);
                if used + size > MAX_LIST_RESPONSE_BYTES {
                    if entries.is_empty() {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(
                                Code::Refused,
                                "one agent record exceeds the registry response byte bound",
                            ),
                        );
                    }
                    break;
                }
                used += size;
                entries.push(entry);
            }
            // The continuation follows the last key actually returned, so a page
            // cut short by the byte bound resumes without skipping a record.
            let next = (entries.len() < candidates.len() || has_more).then(|| {
                entries
                    .last()
                    .expect("a continuation has a returned record")["registration"]["agent_id"]
                    .as_str()
                    .expect("public registration id")
                    .to_owned()
            });
            Response::ok(&request.id, json!({"agents": entries, "next": next}))
        }
        "spawn.get" => {
            let params: SpawnGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let stored = {
                let core = locked(core);
                match core.registry.spawn_edge(&params.request_id) {
                    Ok(Some(edge)) => edge,
                    Ok(None) => {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(Code::NotFound, "spawn edge is not recorded"),
                        );
                    }
                    Err(error) => return registry_error(request, error),
                }
            };
            // The observation is external evidence, so it runs outside every
            // registry lock, like the process verification `agent.get` does.
            let inventory = observed_inventory(core);
            let evidence = location_evidence(inventory.as_ref(), &stored);
            Response::ok(&request.id, json!({"spawn": stored.topology(evidence)}))
        }
        "spawn.list" => {
            let params: SpawnListParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            match params.limit {
                Some(0) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::BadParams, "`limit` must be positive"),
                    );
                }
                Some(limit) if limit > MAX_REGISTRY_PAGE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_REGISTRY_PAGE}"),
                        ),
                    );
                }
                _ => {}
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            // One record beyond the page tells whether a continuation exists,
            // without reading the whole topology to page a bounded answer.
            let read = {
                let core = locked(core);
                match core
                    .registry
                    .spawn_edges_after(params.after.as_deref(), limit + 1)
                {
                    Ok(edges) => edges,
                    Err(error) => return registry_error(request, error),
                }
            };
            let has_more = read.len() > limit;
            let candidates: Vec<_> = read.into_iter().take(limit).collect();
            // One observation serves the whole page: the backend is asked once,
            // however many edges the page holds.
            let inventory = observed_inventory(core);
            let mut edges = Vec::new();
            let mut used = 0usize;
            for edge in &candidates {
                let entry = json!(edge.topology(location_evidence(inventory.as_ref(), edge)));
                let size = serde_json::to_vec(&entry).map_or(usize::MAX, |bytes| bytes.len() + 1);
                if used + size > MAX_LIST_RESPONSE_BYTES {
                    if edges.is_empty() {
                        return Response::refused(
                            &request.id,
                            &Refusal::new(
                                Code::Refused,
                                "one spawn edge exceeds the topology response byte bound",
                            ),
                        );
                    }
                    break;
                }
                used += size;
                edges.push(entry);
            }
            // The continuation follows the last key actually returned, so a page
            // cut short by the byte bound resumes without skipping an edge.
            let next = (edges.len() < candidates.len() || has_more).then(|| {
                edges.last().expect("a continuation has a returned edge")["request_id"]
                    .as_str()
                    .expect("a public request id")
                    .to_owned()
            });
            Response::ok(&request.id, json!({"spawns": edges, "next": next}))
        }
        _ => Response::refused(
            &request.id,
            &Refusal::new(Code::UnknownMethod, "unknown registry method"),
        ),
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// A read the backend does not implement refuses explicitly, naming the
/// capability, rather than answering with a plausible-looking empty result.
fn capabilities_refuse(backend: &Arc<dyn RuntimeProvider>, method: &str) -> Option<Refusal> {
    let capability = ops::read_capability(method);
    (!backend.capabilities().contains(&capability)).then(|| {
        let (code, message) = ops::read_unsupported(method, capability);
        Refusal::new(code, message)
    })
}

/// Serves one mux read from the backend.
///
/// A read records nothing: nothing here can be executed later, so a failure is
/// only ever this answer, and the caller is told which part of the backend is
/// missing rather than given an empty result it might believe.
fn read_backend(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    if request.method == protocol::OUTPUT_METHOD {
        return read_output(core, request);
    }
    if request.method == "observe" && !request.params.as_object().is_some_and(|o| o.is_empty()) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`observe` accepts no params"),
        );
    }
    let pane = if request.method == "process_info" {
        let pane = match request.required_string("pane_id") {
            Ok(pane) => pane,
            Err(refusal) => return Response::refused(&request.id, &refusal),
        };
        if request
            .params
            .as_object()
            .is_none_or(|object| object.keys().any(|key| key != "pane_id"))
        {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BadParams, "`process_info` accepts only pane_id"),
            );
        }
        Some(pane)
    } else {
        if !request
            .params
            .as_object()
            .is_some_and(|object| object.is_empty())
        {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BadParams, "`observe` accepts no params"),
            );
        }
        None
    };
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let Some(backend) = backend else {
        let (code, message) = ops::read_unavailable(&request.method);
        return Response::refused(&request.id, &Refusal::new(code, message));
    };
    if let Some(refusal) = capabilities_refuse(&backend, &request.method) {
        return Response::refused(&request.id, &refusal);
    }
    if let Some(pane_id) = pane {
        if !valid_identifier(&pane_id) {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BadParams, "`pane_id` is not a valid identifier"),
            );
        }
        let evidence = backend.foreground_evidence(&pane_id, &cancel);
        if cancel.load(Ordering::SeqCst) {
            Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
            )
        } else {
            Response::ok(&request.id, json!({"evidence": evidence}))
        }
    } else {
        match backend.inventory(&cancel) {
            Ok(_) if cancel.load(Ordering::SeqCst) => Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
            ),
            Ok(inventory) => Response::ok(&request.id, json!({"inventory": inventory})),

            Err(error) => {
                Response::refused(&request.id, &Refusal::new(Code::BackendUnavailable, error))
            }
        }
    }
}

/// Serves one bounded output read of one pane.
///
/// The snapshot is bounded here as well as by the backend: an answer the daemon
/// cannot write inside its deadline is no answer, so the text is cut to what one
/// line can carry and the `truncated` flag says so.
fn read_output(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let allowed: &[&str] = &["pane_id", "source", "lines", "ansi"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "output accepts only pane_id, source, lines and ansi",
            ),
        );
    }
    let output: OutputRequest = match serde_json::from_value(request.params.clone()) {
        Ok(output) => output,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`output` params are unusable: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&output.pane_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`pane_id` is not a valid identifier"),
        );
    }
    if output
        .lines
        .is_some_and(|lines| lines == 0 || lines > MAX_OUTPUT_LINES)
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`lines` must be between 1 and {MAX_OUTPUT_LINES}"),
            ),
        );
    }
    let (backend, cancel) = {
        let core = locked(core);
        (core.backend.clone(), Arc::clone(&core.stopping))
    };
    let Some(backend) = backend else {
        let (code, message) = ops::read_unavailable(&request.method);
        return Response::refused(&request.id, &Refusal::new(code, message));
    };
    if let Some(refusal) = capabilities_refuse(&backend, &request.method) {
        return Response::refused(&request.id, &refusal);
    }
    match backend.output(&output, &cancel) {
        OutputOutcome::Completed(_) if cancel.load(Ordering::SeqCst) => Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "daemon is stopping"),
        ),
        OutputOutcome::Completed(read) => {
            Response::ok(&request.id, json!({ "output": bounded(read) }))
        }
        OutputOutcome::Refused(message) => {
            Response::refused(&request.id, &Refusal::new(Code::Refused, message))
        }
        OutputOutcome::Unknown(message) => Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, message),
        ),
    }
}

/// One read with its text cut to what a single answer line carries.
fn bounded(read: OutputRead) -> OutputRead {
    if read.text.len() <= MAX_OUTPUT_BYTES {
        return read;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !read.text.is_char_boundary(end) {
        end -= 1;
    }
    OutputRead {
        text: read.text[..end].to_string(),
        truncated: true,
        revision: read.revision,
    }
}

/// One record, by id.
fn read_one(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let id = match request.required_string("id") {
        Ok(id) => id,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.get(&id) {
        Ok(Some(record)) => Response::ok(&request.id, json!({ "request": record })),
        Ok(None) => Response::refused(
            &request.id,
            &Refusal::new(Code::NotFound, format!("no request {id}")),
        ),
        Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
    }
}

/// Recent records, newest first, bounded.
fn read_many(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let limit = match request.limit(DEFAULT_LISTED, MAX_LISTED) {
        Ok(limit) => limit,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.recent(limit) {
        Ok(records) => Response::ok(&request.id, json!({ "requests": records })),
        Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
    }
}

/// Records an operation this daemon cannot serve, without executing it.
///
/// The record is the answer either way: a client asking for a primitive this
/// backend does not implement gets a refusal it can read, not silence and not a
/// plausible-looking effect.
fn execute_unsupported(
    operation: ops::Operation,
    request: &Request,
    message: &str,
    target: Option<String>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let now = store::now_ms();
    let requester = request.optional_string("requester").ok().flatten();
    let core = locked(core);
    match core.store.get(&request.id) {
        Ok(Some(record))
            if record.method == operation.as_str()
                && record.params.as_ref() == Some(&request.params) =>
        {
            return Response::ok(&request.id, json!({"request": record}));
        }
        Ok(Some(_)) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, "request id already names different contents"),
            );
        }
        Ok(None) => {}
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
    match core.store.unresolved(target.as_deref(), now) {
        Ok(Some(held)) => {
            let message = format!(
                "request for this target is already in flight as {}",
                held.id
            );
            let record = RequestRecord::new(
                &request.id,
                operation.as_str(),
                requester.clone(),
                target.clone(),
                now,
            )
            .with_params(request.params.clone())
            .complete(
                RequestOutcome::Refused,
                Some(Category::IN_FLIGHT),
                Some(&message),
                Vec::new(),
                now,
            );
            if let Err(error) = core.store.create(&record) {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
            return Response::refused(&request.id, &Refusal::new(Code::Refused, message));
        }
        Ok(None) => {}
        Err(error) => {
            return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
        }
    }
    let record = RequestRecord::new(&request.id, operation.as_str(), requester, target, now)
        .with_params(request.params.clone())
        .complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(message),
            Vec::new(),
            now,
        );
    if let Err(error) = core.store.create(&record) {
        return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
    }
    Response::ok(&request.id, json!({"request": record}))
}

/// Validates, records and dispatches one effectful operation without holding
/// the store lock over backend I/O.
fn execute(operation: ops::Operation, request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    match operation {
        ops::Operation::Focus => execute_focus(request, core),
        ops::Operation::Close => execute_close(request, core),
        ops::Operation::Create => execute_create(request, core),
        ops::Operation::Input => execute_input(request, core),
        ops::Operation::Report => execute_report(request, core),
        ops::Operation::Spawn => execute_spawn(request, core),
    }
}

/// What the admission every effectful request shares decided.
enum Admission {
    /// The request is already answered: a replay, a content conflict, an
    /// in-flight refusal, or a store error.
    Answered(Response),
    /// The operation cannot be served: no backend is wired, or the wired backend
    /// does not declare the capability it needs. The message says which.
    Unsupported(String),
    /// Admitted and recorded as started: dispatch to this backend.
    Dispatch(Arc<dyn RuntimeProvider>, Arc<AtomicBool>),
}

/// The admission every effectful request shares: exact same-ID replay, the
/// same-ID-different-contents conflict, conflicting unresolved targets across
/// methods, the capability the operation needs, and the Started record written
/// before any dispatch. Never holds the store lock over backend I/O.
///
/// A request this backend cannot serve returns [`Admission::Unsupported`] before
/// any record claims to be started, so an unimplemented primitive is never
/// recorded as having been dispatched.
fn admit(
    operation: ops::Operation,
    request: &Request,
    target: Option<&str>,
    requester: &Option<String>,
    now: i64,
    core: &Arc<Mutex<Core>>,
) -> Admission {
    let method = operation.as_str();
    let (held, backend, cancel) = {
        let core = locked(core);
        let existing = match core.store.get(&request.id) {
            Ok(record) => record,
            Err(error) => {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        };
        if let Some(record) = &existing {
            let same = record.method == method
                && record.target.as_deref() == target
                && record.requester == *requester
                && record.params.as_ref() == Some(&request.params);
            if same {
                return Admission::Answered(Response::ok(
                    &request.id,
                    json!({ "request": record }),
                ));
            }
            let message = if record.params.is_none() {
                "legacy record cannot establish content-equivalent replay"
            } else {
                "request id already names different contents"
            };
            return Admission::Answered(Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, message),
            ));
        }
        let held = match core.store.unresolved(target, now) {
            Ok(held) => held,
            Err(error) => {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        };
        let backend = core.backend.clone();
        let capable = backend
            .as_ref()
            .is_some_and(|backend| operation.ready_for(backend.capabilities()).is_ok());
        // Only a request that will be dispatched is recorded as started: an
        // operation this backend cannot serve leaves no claim that it ran.
        if held.is_none() && capable {
            let pending = RequestRecord::new(
                &request.id,
                method,
                requester.clone(),
                target.map(str::to_string),
                now,
            )
            .with_params(request.params.clone());
            if let Err(error) = core.store.create(&pending) {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
            let started = pending.claim().start();
            if let Err(error) = core.store.write(&started) {
                return Admission::Answered(Response::refused(
                    &request.id,
                    &Refusal::new(Code::Internal, error),
                ));
            }
        }
        (held, backend, Arc::clone(&core.stopping))
    };
    if let Some(held) = held {
        let message = format!(
            "{method} for this target is already in flight as {}",
            held.id
        );
        let record = RequestRecord::new(
            &request.id,
            method,
            requester.clone(),
            target.map(str::to_string),
            now,
        )
        .with_params(request.params.clone())
        .complete(
            RequestOutcome::Refused,
            Some(Category::IN_FLIGHT),
            Some(&message),
            Vec::new(),
            now,
        );
        let core = locked(core);
        if let Err(error) = core.store.create(&record) {
            return Admission::Answered(Response::refused(
                &request.id,
                &Refusal::new(Code::Internal, error),
            ));
        }
        return Admission::Answered(Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, message),
        ));
    }
    match backend {
        None => Admission::Unsupported(operation.unavailable_message()),
        Some(backend) => match operation.ready_for(backend.capabilities()) {
            Ok(()) => Admission::Dispatch(backend, cancel),
            Err(message) => Admission::Unsupported(message),
        },
    }
}

/// Validates, records and dispatches focus.
fn execute_focus(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let allowed: &[&str] = &["target", "target_kind", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "focus accepts only target, target_kind, and requester",
            ),
        );
    }
    let target = match request.required_string("target") {
        Ok(target) => target,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    if !valid_identifier(&target) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`target` is not a valid identifier"),
        );
    }
    let runtime_target = match request.params.get("target_kind").and_then(|v| v.as_str()) {
        Some("pane") => Target::Pane(target.clone()),
        Some("workspace") => Target::Workspace(target.clone()),
        _ => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`target_kind` must be `pane` or `workspace`",
                ),
            );
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Focus;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let outcome = backend.focus_outcome(&runtime_target, &cancel);
    let done = match outcome {
        FocusOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            None,
            vec!["focus".into()],
            store::now_ms(),
        ),
        FocusOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        FocusOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// Validates, records and dispatches one guarded close.
///
/// The caller's frozen identity travels as one [`CloseRequest`], so the daemon
/// applies exactly the policy Radar's direct close applies — fresh inventory,
/// frozen-identity match, positive-unmanaged containment — and never a second
/// implementation of it. A location its owner manages is refused here; there is
/// no daemon route that closes it directly, and managed close stays owner-routed.
fn execute_close(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let allowed: &[&str] = &["request", "requester"];
    if request
        .params
        .as_object()
        .is_none_or(|params| params.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "close accepts only request and requester"),
        );
    }
    let Some(frozen) = request.params.get("request") else {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`request` is required"),
        );
    };
    let close_request: CloseRequest = match serde_json::from_value(frozen.clone()) {
        Ok(close_request) => close_request,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a frozen close request: {error}"),
                ),
            );
        }
    };
    let target = close_request.target.id().to_string();
    if !valid_identifier(&target) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the close target is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Close;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    // A backend confirmation says the close was performed, not that the location
    // is gone: nothing here re-reads the post-state, so the record says so.
    let done = match lifecycle::close_unmanaged(backend.as_ref(), &close_request, &cancel) {
        DirectClose::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            Some("the runtime confirmed the close; its removal was not re-observed"),
            vec!["close".into()],
            store::now_ms(),
        ),
        DirectClose::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        DirectClose::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// The frozen normalized request a create or input carries under `request`,
/// or the refusal to answer with.
fn frozen_params(request: &Request) -> Result<serde_json::Value, Response> {
    if request.params.as_object().is_none_or(|params| {
        params
            .keys()
            .any(|key| !matches!(key.as_str(), "request" | "requester"))
    }) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("{} accepts only request and requester", request.method),
            ),
        ));
    }
    request.params.get("request").cloned().ok_or_else(|| {
        Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`request` is required"),
        )
    })
}

/// Validates, records and dispatches one creation.
///
/// Creation is a mux primitive: a location is made, and nothing is started in
/// it. The record's target is the location the request changes — the pane being
/// split, or the workspace a new tab joins — so a competing mutation of that
/// location cannot run beside it. A new workspace changes nothing that exists,
/// so it has no target and shares only the unscoped mutations' lane.
fn execute_create(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let create: CreateRequest = match serde_json::from_value(frozen) {
        Ok(create) => create,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a create request: {error}"),
                ),
            );
        }
    };
    // The scope is named by the caller, so a create never lands wherever the
    // multiplexer happens to have focus.
    let target = match &create {
        CreateRequest::Workspace { .. } => None,
        CreateRequest::Tab { workspace_id, .. } => Some(workspace_id.clone()),
        CreateRequest::PaneSplit { pane_id, .. } => Some(pane_id.clone()),
    };
    if target
        .as_deref()
        .is_some_and(|target| !valid_identifier(target))
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the create target is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Create;
    let method = operation.as_str();
    let (backend, cancel) =
        match admit(operation, request, target.as_deref(), &requester, now, core) {
            Admission::Answered(response) => return response,
            Admission::Unsupported(message) => {
                return execute_unsupported(operation, request, &message, target, core);
            }
            Admission::Dispatch(backend, cancel) => (backend, cancel),
        };
    let record = RequestRecord::new(&request.id, method, requester, target, now)
        .with_params(request.params.clone());
    // Creation is confirmed, not re-observed: the runtime says it made the
    // location, nothing here lists to prove it exists.
    let done = match backend.create(&create, &cancel) {
        // The identity the answer named travels as data, so a client acts on what
        // it created without reading the effect prose.
        CreateOutcome::Completed(created) => record
            .created(created.identity(create.created_kind()))
            .complete(
                RequestOutcome::Completed,
                None,
                created
                    .is_empty()
                    .then_some("the runtime confirmed the creation without naming what it created"),
                created.effects(),
                store::now_ms(),
            ),
        CreateOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        CreateOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// Validates, records and dispatches one pane input.
///
/// Input is what a client asks a pane to receive: literal text, or named key
/// presses, in one of the two shapes and never both. The payload is validated
/// here, at the wire boundary, before anything is recorded or dispatched; what
/// survives validation is input this seam can carry without deciding for the
/// caller what a terminal would do with it.
fn execute_input(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let input: InputRequest = match serde_json::from_value(frozen) {
        Ok(input) => input,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not an input request: {error}"),
                ),
            );
        }
    };
    if let Err(message) = input.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let effect = match &input.payload {
        InputPayload::Text { .. } => "sent text",
        InputPayload::Keys { .. } => "sent keys",
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let target = input.pane_id.clone();
    let now = store::now_ms();
    let operation = ops::Operation::Input;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let done = match backend.input(&input, &cancel) {
        InputOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            None,
            vec![effect.to_string()],
            store::now_ms(),
        ),
        InputOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        InputOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// Validates, records and dispatches one publisher's report.
///
/// A report is a recorded mutation like any other — admitted once, started before
/// dispatch, never replayed — but it changes no mux state: it carries one
/// publisher's own facts to the backend as the caller gave them. Nothing here
/// derives an agent fact, merges the report into a store or claims a location; the
/// record keeps the request's own params, so what a client reads back is what it
/// reported. The record's target is the location reported about, so a report
/// conflicts with another mutation of that pane instead of racing it.
///
/// Where the backend cannot report, the capability gate refuses before dispatch
/// and the record says so, rather than the daemon substituting an operation.
fn execute_report(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let report: ReportRequest = match serde_json::from_value(frozen) {
        Ok(report) => report,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a report: {error}"),
                ),
            );
        }
    };
    if let Err(message) = report.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let target = report.target().to_string();
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Report;
    let method = operation.as_str();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let done = match backend.report(&report, &cancel) {
        ReportOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            None,
            vec![report.effect()],
            store::now_ms(),
        ),
        ReportOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        ReportOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_settled(request, done, core)
}

/// The wire shape of one spawn request: the parent runtime subject, and the
/// command the caller resolved for the child.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnParams {
    parent: String,
    executable: String,
    #[serde(default)]
    argv: Vec<String>,
}

/// Validates, records and dispatches one managed child spawn.
///
/// One operation, and four durable facts in order: the intent with its private
/// token, the pane under the parent's own registered pane, the command typed
/// into it, and the request's record. Each is written before the next is
/// attempted, so a spawn that stops half-way leaves what it established readable
/// rather than an unattributable pane; nothing is cleaned up on a later failure,
/// because an operator can see and close a pane and cannot see one deleted
/// silently. No assignment fact and no process lifecycle is touched: creating a
/// child says nothing about keeping it alive.
fn execute_spawn(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: SpawnParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a spawn request: {error}"),
                ),
            );
        }
    };
    // The command and the parent are validated at the wire boundary, before
    // anything is recorded: a request this seam cannot carry creates no pane.
    if let Err(message) = validate_command(&params.executable, &params.argv) {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let intent = registry::SpawnRequest {
        request_id: request.id.clone(),
        parent: params.parent.clone(),
    };
    if let Err(message) = intent.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    // The child is created beside the pane its parent registered: a spawn never
    // lands wherever the multiplexer happens to have focus, and a parent with no
    // recorded pane is refused rather than guessed from a title or a label.
    let parent_pane = {
        let core = locked(core);
        let parent = match core.registry.get(&params.parent) {
            Ok(Some(parent)) => parent,
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::NotFound,
                        format!("parent {} is not a registered agent", params.parent),
                    ),
                );
            }
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        match parent
            .request
            .location
            .as_ref()
            .and_then(|location| location.pane.clone())
        {
            Some(pane) => pane,
            None => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::Refused,
                        format!(
                            "parent {} names no pane to create the child under",
                            params.parent
                        ),
                    ),
                );
            }
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::Spawn;
    let (backend, cancel) = match admit(
        operation,
        request,
        Some(&parent_pane),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return spawn_answer(request, response, core),
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(parent_pane), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(
        &request.id,
        operation.as_str(),
        requester,
        Some(parent_pane.clone()),
        now,
    )
    .with_params(request.params.clone());
    // The intent, its parent and the token the child will present are durable
    // before the first effect: a spawn that dies here leaves a readable intent,
    // not nothing to reconcile.
    if let Err(message) = locked(core).registry.record_spawn(&intent, now) {
        let settled = record.complete(
            RequestOutcome::Refused,
            None,
            Some(&message),
            Vec::new(),
            store::now_ms(),
        );
        let _ = write_settled(request, settled, core);
        return Response::refused(&request.id, &Refusal::new(Code::Refused, message));
    }
    // The pane is a split of the parent's own pane, unfocused: a child is
    // created where its parent is, and creating one must not move the operator's
    // cursor.
    let created = backend.create(
        &CreateRequest::PaneSplit {
            pane_id: parent_pane,
            direction: SplitDirection::Right,
            focus: false,
        },
        &cancel,
    );
    let (created_outcome, named, created_message) = match &created {
        CreateOutcome::Completed(named) => (
            RequestOutcome::Completed,
            Some(named),
            named.is_empty().then(|| {
                "the runtime confirmed the creation without naming what it created".to_string()
            }),
        ),
        CreateOutcome::Refused(message) => (RequestOutcome::Refused, None, Some(message.clone())),
        CreateOutcome::Unknown(message) => (RequestOutcome::Unknown, None, Some(message.clone())),
    };
    let location = named
        .filter(|named| !named.is_empty())
        .map(created_location);
    let mut effects = named.map(CreatedLocation::effects).unwrap_or_default();
    let pane = named.and_then(|named| named.identity(CreatedKind::Pane));
    // The create is recorded before the launch is attempted: a confirmed pane is
    // readable even when the launch never answers.
    let written =
        locked(core)
            .registry
            .record_spawn_created(&request.id, created_outcome, location.clone());
    let edge = match written {
        Ok(edge) => edge,
        Err(error) => {
            // Recording an effect is part of doing it: the launch is not
            // attempted past a step the edge could not write, and the store
            // record still names the pane the create confirmed, so a pane that
            // exists is not an unattributable one.
            let settled = record.created(pane).complete(
                RequestOutcome::Unknown,
                None,
                Some(&error),
                effects,
                store::now_ms(),
            );
            let _ = write_settled(request, settled, core);
            return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
        }
    };
    // A command runs only in a pane the create named: with no pane there is
    // nothing to type into, and a launch is never claimed for a step that did not
    // happen.
    let launched = location
        .as_ref()
        .and_then(|location| location.pane.clone())
        .map(|pane| {
            backend.launch(
                &LaunchRequest {
                    pane_id: pane,
                    executable: params.executable.clone(),
                    argv: params.argv.clone(),
                    spawn_token: edge.token.clone(),
                },
                &cancel,
            )
        });
    let (launched_outcome, launch_message) = match &launched {
        Some(LaunchOutcome::Completed) => (Some(RequestOutcome::Completed), None),
        Some(LaunchOutcome::Refused(message)) => {
            (Some(RequestOutcome::Refused), Some(message.clone()))
        }
        Some(LaunchOutcome::Unknown(message)) => {
            (Some(RequestOutcome::Unknown), Some(message.clone()))
        }
        None => (None, None),
    };
    let edge = match launched_outcome {
        Some(outcome) => {
            let written = locked(core)
                .registry
                .record_spawn_launched(&request.id, outcome);
            match written {
                Ok(edge) => edge,
                Err(error) => {
                    let settled = record.created(pane).complete(
                        RequestOutcome::Unknown,
                        None,
                        Some(&error),
                        effects,
                        store::now_ms(),
                    );
                    let _ = write_settled(request, settled, core);
                    return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
                }
            }
        }
        None => edge,
    };
    // The request's own outcome is the step that stopped it: completed only when
    // everything the daemon attempted completed, and never claiming a step that
    // did not run.
    let (outcome, category, message) = match (created_outcome, launched_outcome) {
        (RequestOutcome::Completed, Some(RequestOutcome::Completed)) => {
            (RequestOutcome::Completed, None, None)
        }
        (RequestOutcome::Completed, Some(RequestOutcome::Refused)) => (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            launch_message.as_deref(),
        ),
        // A create that named no pane left nothing to launch into.
        (RequestOutcome::Completed, None) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            created_message.as_deref(),
        ),
        (RequestOutcome::Completed, Some(RequestOutcome::Unknown)) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            launch_message.as_deref(),
        ),
        (RequestOutcome::Refused, _) => (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            created_message.as_deref(),
        ),
        (RequestOutcome::Unknown, _) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            created_message.as_deref(),
        ),
    };
    if launched_outcome == Some(RequestOutcome::Completed) {
        effects.push("launched child".to_string());
    }
    let done = record
        .created(pane)
        .complete(outcome, category, message, effects, store::now_ms());
    settle_spawn(request, done, &edge, core)
}

/// The location a create answered with, in the coordinates this daemon records.
fn created_location(named: &CreatedLocation) -> RegistryLocation {
    RegistryLocation {
        backend: WIRED_BACKEND.to_string(),
        instance: None,
        workspace: named.workspace_id.clone(),
        tab: named.tab_id.clone(),
        pane: named.pane_id.clone(),
    }
}

/// Writes one settled spawn and answers with both records it authored.
///
/// What the request did and the edge it authored are separate durable records,
/// and the effects a caller acts on are the edge's: created, launched and bound
/// as they stand.
fn settle_spawn(
    request: &Request,
    record: RequestRecord,
    edge: &registry::SpawnEdge,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let core = locked(core);
    match core.store.write(&record) {
        Ok(()) => Response::ok(
            &request.id,
            json!({ "request": record, "spawn": edge.public() }),
        ),
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// A spawn that was already answered: its record, and the edge that request id
/// authored.
///
/// A replay, an in-flight refusal and a content conflict are all answered from
/// the records the first attempt wrote, so a caller reads the outcomes it read
/// the first time. A response carrying no record — a refusal, a store error — is
/// answered as itself.
fn spawn_answer(request: &Request, response: Response, core: &Arc<Mutex<Core>>) -> Response {
    let (id, result) = match response {
        Response::Result { id, result } => (id, result),
        other => return other,
    };
    let edge = {
        let core = locked(core);
        core.registry.spawn_edge(&request.id)
    };
    match (edge, result.get("request").cloned()) {
        (Ok(Some(edge)), Some(record)) => {
            Response::ok(&id, json!({ "request": record, "spawn": edge.public() }))
        }
        _ => Response::ok(&id, result),
    }
}

/// Writes one settled record and answers with it.
fn write_settled(request: &Request, record: RequestRecord, core: &Arc<Mutex<Core>>) -> Response {
    let core = locked(core);
    match core.store.write(&record) {
        Ok(()) => Response::ok(&request.id, json!({ "request": record })),
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// Writes one response line, under the connection's write lock so two answers
/// cannot interleave on one connection.
fn answer(writer: &Arc<Mutex<UnixStream>>, response: Response) {
    let mut stream = locked(writer);
    let _ = stream.write_all(&response.line());
    let _ = stream.flush();
}

/// The directory a socket path names: its parent, or the working directory when
/// it names no parent at all.
fn socket_directory(socket: &Path) -> &Path {
    match socket.parent() {
        Some(directory) if !directory.as_os_str().is_empty() => directory,
        _ => Path::new("."),
    }
}

fn directory_diagnostic(directory: &Path, error: std::io::Error) -> String {
    format!("control socket directory {}: {error}", directory.display())
}

/// The trust rule for a socket path that must already exist, read-only.
///
/// Nothing here creates, changes or removes anything, so a caller that must not
/// touch the filesystem can apply the whole rule *before* it connects: the path
/// is a real socket of this user, in a real private directory of this user's.
/// The directory half is [`trusted_directory`], the same rule [`prepare_directory`]
/// applies to the directory the daemon binds in — a daemon could not have bound
/// a socket this rejects, so a path that fails it belongs to someone else, or is
/// a leftover nobody vouched for.
///
/// The socket's own checks are the client's half: the daemon creates its socket
/// file itself, so only the directory rule is shared code. A mode of exactly
/// `0700` is required rather than merely "not group- or other-writable", because
/// that is what the daemon requires to serve at all; a stricter client check can
/// therefore never reject a socket the daemon could have bound.
///
/// `Err` is the diagnostic naming the check that failed.
pub fn trusted_socket(socket: &Path) -> Result<(), String> {
    trusted_directory(socket_directory(socket))?;
    let metadata = std::fs::symlink_metadata(socket)
        .map_err(|error| format!("control socket {}: {error}", socket.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("control socket {} is a symlink", socket.display()));
    }
    if !metadata.file_type().is_socket() {
        return Err(format!(
            "control socket {} is not a socket",
            socket.display()
        ));
    }
    // Unreachable through a directory this user owns privately, which is the
    // point of requiring one: the check does not assume it.
    if metadata.uid() != current_uid() {
        return Err(format!(
            "control socket {} is owned by uid {}",
            socket.display(),
            metadata.uid()
        ));
    }
    Ok(())
}

/// The read-only directory rule: an existing real directory of this user, mode
/// `0700`, reached without following a symlink.
fn trusted_directory(directory: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| directory_diagnostic(directory, error))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "control socket directory {} is a symlink",
            directory.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "control socket directory {} is not a directory",
            directory.display()
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(format!(
            "control socket directory {} is owned by uid {}",
            directory.display(),
            metadata.uid()
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(format!(
            "control socket directory {} is not mode 0700",
            directory.display()
        ));
    }
    Ok(())
}

/// Creates `directory` `0700` where it is missing, then requires it to pass
/// [`trusted_directory`].
///
/// This is the daemon's half of the rule: only a daemon may create the socket
/// directory, and it checks the same rule a client will check later.
fn prepare_directory(directory: &Path) -> Result<(), String> {
    if !directory.exists() {
        let diagnostic = |error| directory_diagnostic(directory, error);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(diagnostic)?;
        // The umask can take bits off the creation mode, and the mode is what
        // makes the socket ours, so what is required is set explicitly.
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(diagnostic)?;
    }
    trusted_directory(directory)
}

/// Binds `path`, first clearing a socket file a dead daemon left behind.
///
/// A file that answers a connection belongs to a running daemon and is left
/// untouched — the caller is told it is already running rather than served a
/// second listener on one path; a refused connection means the file is stale and
/// it is removed.
fn bind_socket(path: &Path) -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Err(format!(
                "control socket {} exists and is not a socket",
                path.display()
            ));
        }
        Ok(_) => match UnixStream::connect(path) {
            Ok(_) => {
                return Err(format!(
                    "control socket {}: a daemon is already running",
                    path.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)
                    .map_err(|error| format!("control socket {}: {error}", path.display()))?;
            }
            Err(error) => return Err(format!("control socket {}: {error}", path.display())),
        },
        Err(_) => {}
    }
    UnixListener::bind(path).map_err(|error| format!("control socket {}: {error}", path.display()))
}

/// Removes the socket pathname only when it still names the inode this daemon
/// bound. A replacement daemon or administrator-created path is left untouched.
fn remove_socket_if_owned(path: &Path, identity: (u64, u64)) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if (metadata.dev(), metadata.ino()) == identity => std::fs::remove_file(path)
            .map_err(|error| format!("control socket {}: {error}", path.display())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("control socket {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_value(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    #[test]
    fn the_socket_path_follows_the_documented_order() {
        let uid = 4242;
        assert_eq!(
            socket_path_in(
                Some(std::ffi::OsStr::new("/run/me/control.sock")),
                Some(std::ffi::OsStr::new("/run/user/4242")),
                uid
            ),
            path_value("/run/me/control.sock")
        );
        assert_eq!(
            socket_path_in(None, Some(std::ffi::OsStr::new("/run/user/4242")), uid),
            path_value("/run/user/4242/agent-radar/control.sock")
        );
        assert_eq!(
            socket_path_in(None, None, uid),
            path_value("/tmp/agent-radar-4242/control.sock")
        );
    }

    #[test]
    fn the_state_path_follows_the_documented_order() {
        let uid = 4242;
        assert_eq!(
            state_dir_in(
                Some(std::ffi::OsStr::new("/var/me/control")),
                Some(std::ffi::OsStr::new("/home/me/.state")),
                Some(std::ffi::OsStr::new("/home/me")),
                uid
            ),
            path_value("/var/me/control")
        );
        assert_eq!(
            state_dir_in(
                None,
                Some(std::ffi::OsStr::new("/home/me/.state")),
                Some(std::ffi::OsStr::new("/home/me")),
                uid
            ),
            path_value("/home/me/.state/agent-radar/control")
        );
        assert_eq!(
            state_dir_in(None, None, Some(std::ffi::OsStr::new("/home/me")), uid),
            path_value("/home/me/.local/state/agent-radar/control")
        );
        assert_eq!(
            state_dir_in(None, None, None, uid),
            path_value("/tmp/agent-radar-4242")
        );
    }

    #[test]
    fn a_directory_that_is_not_ours_is_refused_rather_than_loosened() {
        let directory = std::env::temp_dir().join(format!(
            "radar-control-dir-{}-{}",
            std::process::id(),
            store::random_uuid()
        ));
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
            .expect("a mode");
        let refusal = prepare_directory(&directory).expect_err("a refusal");
        assert!(refusal.contains("not mode 0700"), "{refusal}");
        // A symlink to a good directory is still refused: the path must be the
        // directory itself, not something pointing at one.
        let link = std::env::temp_dir().join(format!(
            "radar-control-link-{}-{}",
            std::process::id(),
            store::random_uuid()
        ));
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("a mode");
        std::os::unix::fs::symlink(&directory, &link).expect("a symlink");
        let refusal = prepare_directory(&link).expect_err("a refusal");
        assert!(refusal.contains("symlink"), "{refusal}");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_socket_this_user_owns_privately_is_trusted() {
        let root = trust_root();
        let socket = root.join("control.sock");
        let _listener = UnixListener::bind(&socket).expect("a listener");
        // Nothing here creates or changes anything: the rule is a read.
        trusted_socket(&socket).expect("a trusted socket");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_socket_path_nothing_vouches_for_is_refused() {
        let root = trust_root();

        // Nothing at the path: no daemon is serving there.
        let refusal = trusted_socket(&root.join("absent.sock")).expect_err("a refusal");
        assert!(refusal.contains("No such file or directory"), "{refusal}");

        let real = root.join("real.sock");
        let _listener = UnixListener::bind(&real).expect("a listener");

        // A symlink at the name, even to a socket this user owns, is not the
        // socket the check vouches for.
        let linked = root.join("linked.sock");
        std::os::unix::fs::symlink(&real, &linked).expect("a symlink");
        let refusal = trusted_socket(&linked).expect_err("a refusal");
        assert!(refusal.contains("is a symlink"), "{refusal}");

        // A regular file wearing a socket's name.
        let file = root.join("file.sock");
        std::fs::write(&file, b"not a socket").expect("a file");
        let refusal = trusted_socket(&file).expect_err("a refusal");
        assert!(refusal.contains("is not a socket"), "{refusal}");

        // A directory anyone in the group can write is refused even though the
        // socket in it is this user's: that is the rule that stops a socket
        // planted ahead of the daemon from being believed.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o770)).expect("a mode");
        let refusal = trusted_socket(&real).expect_err("a refusal");
        assert!(refusal.contains("is not mode 0700"), "{refusal}");

        // When the system temporary directory belongs to another uid, exercise
        // the ownership rejection. Sandboxed builders may run as an unprivileged
        // uid and own /tmp themselves; the preceding mode check still exercises
        // the trust boundary there without assuming a particular account.
        let elsewhere = Path::new("/tmp");
        let temp_metadata = std::fs::symlink_metadata(elsewhere).expect("system temp metadata");
        if temp_metadata.uid() != current_uid() {
            let candidate = elsewhere.join(format!("radar-trust-{}.sock", store::random_uuid()));
            let refusal = trusted_socket(&candidate).expect_err("a refusal");
            assert!(refusal.contains("is owned by uid"), "{refusal}");
        }

        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A private directory for the trust-check tests.
    fn trust_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "radar-control-trust-{}-{}",
            std::process::id(),
            store::random_uuid()
        ));
        std::fs::create_dir_all(&root).expect("a directory");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("a mode");
        root
    }
}
