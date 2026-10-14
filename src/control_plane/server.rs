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

use std::io::{ErrorKind, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::control_plane::containers;
use crate::control_plane::lifecycle::{self as subjects, Bootstrap};
use crate::control_plane::ops::{self, Method};
use crate::control_plane::protocol::{self, Code, PROTOCOL_VERSION, Refusal, Request, Response};
use crate::control_plane::registry::{
    self, AcquireRequest, Channel, CloseAdmission, CloseOutcome, CloseResult, LocalProcfsVerifier,
    LocationEvidence, ProcessVerifier, RegistrationRequest, Registry, RegistryLocation, SpawnEdge,
};
use crate::control_plane::store::{self, Category, RequestOutcome, RequestRecord, Store};
use crate::control_plane::transport::{
    ConnectionSet, Dispatch, EventReady, IngestDispatch, ListenerSource, Transport,
};
use crate::lifecycle::{self, CloseRequest, DirectClose};
use crate::model::SessionUuid;
use crate::model::{FleetObservation, ProcessIdentity};
use crate::process_control::{self, Acquired};
use crate::runtime::{
    BackendIdentity, CLOSE_CAPABILITY, CreateOutcome, CreateRequest, CreatedKind, CreatedLocation,
    EXIT_OBSERVATION_CAPABILITY, FocusOutcome, InputOutcome, InputPayload, InputRequest,
    LAUNCH_CONTEXT_CAPABILITY, LIFECYCLE_TRACKING_CAPABILITY, LaunchOutcome, LaunchRequest,
    OutputOutcome, OutputRead, OutputRequest, PaneExitObservation, ReportOutcome, ReportRequest,
    RuntimeProvider, SplitDirection, Target, validate_command,
};
use crate::{HerdrConfig, HerdrRuntime, TmuxConfig, TmuxRuntime};

/// The socket file's name inside the directory the daemon owns.
const SOCKET_NAME: &str = "control.sock";

/// The most records one `requests` answer carries, and what it carries when the
/// caller names no limit.
const MAX_LISTED: usize = 200;
const DEFAULT_LISTED: usize = 50;
/// The most characters one container label carries.
///
/// A label is Radar's own name for a container and is shown in the tree, so it is
/// bounded where it is accepted rather than truncated where it is displayed.
const MAX_LABEL_CHARS: usize = 128;
/// The registry returns at most this many records in one page, agents and spawn
/// edges alike.
const MAX_REGISTRY_PAGE: usize = registry::MAX_LISTED;

/// One reconciliation pass writes one observation per container, each its own
/// durable append, so a pass is capped rather than sized by the fleet.
const MAX_RECONCILE: usize = 128;
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
    rustix::process::getuid().as_raw()
}

/// The physical backend this daemon serves. A process serves exactly one.
///
/// This is the daemon's *physical* backend, not Radar's client transport
/// (`[runtime] backend = "direct" | "daemon"`), which chooses how a dashboard
/// reaches a daemon that is already running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhysicalBackend {
    /// Herdr, through the adapter in [`crate::herdr`]. The default.
    Herdr,
    /// A tmux server the operator named, never tmux's default server: a tmux
    /// selection that names no server is refused instead of reaching the one
    /// tmux would pick for itself.
    Tmux { server: String },
}

/// Why a configured physical backend cannot serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendConfigError {
    /// The configured name matches no backend.
    UnknownBackend { name: String },
    /// `tmux` was selected with an empty server name.
    TmuxServerMissing,
    /// The configuration file could not be read, so no backend is chosen.
    ConfigFile { problem: String },
    /// The backend is named in configuration but the adapter cannot serve that
    /// configuration, so no effect may be attempted through it.
    AdapterConfiguration {
        backend: &'static str,
        problem: String,
    },
}

impl std::fmt::Display for BackendConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBackend { name } => {
                write!(
                    f,
                    "control backend `{name}`: not a backend; use `herdr` or `tmux`"
                )
            }
            Self::TmuxServerMissing => write!(
                f,
                "control backend `tmux`: the server name is empty; set `daemon.tmux_server` or `RADAR_TMUX_SERVER`"
            ),
            Self::ConfigFile { problem } => write!(f, "configuration: {problem}"),
            Self::AdapterConfiguration { backend, problem } => {
                write!(f, "control backend `{backend}`: {problem}")
            }
        }
    }
}

/// The physical backend the daemon serves: the environment's choice when it sets
/// one, else the configuration file's, else tmux on the `radar` server.
///
/// An unknown name is refused rather than served, and nothing here falls back to
/// another backend.
pub fn physical_backend() -> Result<PhysicalBackend, BackendConfigError> {
    let file = crate::config::Config::load()
        .map_err(|problem| BackendConfigError::ConfigFile { problem })?
        .daemon;
    physical_backend_with(
        std::env::var_os("RADAR_CONTROL_BACKEND").as_deref(),
        std::env::var_os("RADAR_TMUX_SERVER").as_deref(),
        &file,
    )
}

/// [`physical_backend`] over an environment and a configuration the caller
/// supplies, so the rule can be read and tested without touching either.
pub fn physical_backend_with(
    backend: Option<&std::ffi::OsStr>,
    tmux_server: Option<&std::ffi::OsStr>,
    file: &crate::config::DaemonConfig,
) -> Result<PhysicalBackend, BackendConfigError> {
    let name = match backend.map(|value| value.to_string_lossy().trim().to_ascii_lowercase()) {
        Some(name) if !name.is_empty() => name,
        _ => match file.backend {
            crate::config::ControlBackend::Tmux => "tmux".to_string(),
            crate::config::ControlBackend::Herdr => "herdr".to_string(),
        },
    };
    match name.as_str() {
        "herdr" => Ok(PhysicalBackend::Herdr),
        "tmux" => {
            let server = tmux_server
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|server| !server.is_empty())
                .unwrap_or_else(|| file.tmux_server.trim().to_string());
            if server.is_empty() {
                return Err(BackendConfigError::TmuxServerMissing);
            }
            Ok(PhysicalBackend::Tmux { server })
        }
        _ => Err(BackendConfigError::UnknownBackend { name }),
    }
}

impl PhysicalBackend {
    /// The adapter this backend is served through.
    ///
    /// Capabilities are the adapter's own declaration for the backend it
    /// implements ([`RuntimeProvider::capabilities`]); nothing here restates
    /// them. An adapter that cannot serve the configuration it was given is a
    /// startup refusal, never a backend that quietly serves another one.
    fn adapter(&self) -> Result<Arc<dyn RuntimeProvider>, BackendConfigError> {
        match self {
            Self::Herdr => Ok(Arc::new(HerdrRuntime::new(HerdrConfig::default()))),
            Self::Tmux { server } => TmuxRuntime::new(TmuxConfig::for_server(server))
                .map(|runtime| Arc::new(runtime) as Arc<dyn RuntimeProvider>)
                .map_err(|problem| BackendConfigError::AdapterConfiguration {
                    backend: "tmux",
                    problem,
                }),
        }
    }
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
    task_ingest: Option<Arc<crate::control_plane::ingest::TaskIngest>>,
    task_ingest_error: Option<String>,
    backend: Option<Arc<dyn RuntimeProvider>>,
    verifier: Arc<dyn ProcessVerifier>,
    stopping: Arc<AtomicBool>,
    /// What a tracked launch runs as its bootstrap. `None` when this process has
    /// no executable path it can name, which refuses tracked launches.
    bootstrap: Option<Bootstrap>,
    /// Unix milliseconds of the last completed recovery sweep pass; 0 before one.
    sweep_at: Arc<std::sync::atomic::AtomicI64>,
}

fn stop_backend_for(connections: &ConnectionSet) {
    connections.close_all();
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
    task_socket: Option<(PathBuf, (u64, u64))>,
    stopping: Arc<AtomicBool>,
    transport: Option<Transport>,
    verification: Arc<VerificationBudget>,
    sweeper: Option<JoinHandle<()>>,
    /// The inode this daemon bound, or `None` for a socket systemd passed.
    socket_identity: Option<(u64, u64)>,
    core: Arc<Mutex<Core>>,
}

impl Daemon {
    /// Binds the socket at [`socket_path`], keeping records under [`state_dir`],
    /// through the physical backend [`physical_backend`] selects.
    ///
    /// A configured backend this build cannot serve is refused here, before the
    /// socket is bound and before any directory is prepared: no other backend is
    /// served in its place.
    pub fn bind() -> Result<Self, String> {
        let backend = physical_backend().map_err(|error| error.to_string())?;
        // The adapter validates the server name before anything is started under it.
        let adapter = backend.adapter().map_err(|error| error.to_string())?;
        if let PhysicalBackend::Tmux { server } = &backend {
            crate::service::ensure_tmux_server(server)?;
        }
        let task_socket = crate::bus::socket_path();
        Self::bind_with_backend_and_verifier_and_ingest(
            &socket_path(),
            &state_dir(),
            Some(adapter),
            Arc::new(LocalProcfsVerifier),
            Some(&task_socket),
        )
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
        Self::bind_with_backend_and_verifier_and_ingest(socket, state, backend, verifier, None)
    }

    fn bind_with_backend_and_verifier_and_ingest(
        socket: &Path,
        state: &Path,
        backend: Option<Arc<dyn RuntimeProvider>>,
        verifier: Arc<dyn ProcessVerifier>,
        task_socket_path: Option<&Path>,
    ) -> Result<Self, String> {
        // The socket's own directory is what must be ours, not the socket path:
        // binding in a directory anyone can write is the thing being refused.
        prepare_directory(socket_directory(socket))?;
        // A listener systemd passed belongs to the socket unit, which removes its
        // path on stop, so this daemon neither binds nor unlinks it.
        let (listener, socket_identity) = match inherited_listener(socket)? {
            Some(listener) => (listener, None),
            None => {
                let listener = bind_socket(socket)?;
                let metadata = std::fs::symlink_metadata(socket)
                    .map_err(|error| format!("control socket {}: {error}", socket.display()))?;
                (listener, Some((metadata.dev(), metadata.ino())))
            }
        };
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
        let (event_notification, event_signal) =
            crate::control_plane::transport::event_notifications();
        let store_notification = event_notification.clone();
        store.set_event_notifier(Arc::new(move || store_notification.notify()));
        let ready_store = store.clone();
        let event_ready: Arc<EventReady> =
            Arc::new(move |line| event_read_ready(&ready_store, line));
        let registry = match Registry::open(state) {
            Ok(registry) => registry,
            Err(error) => {
                let _ = remove_socket_if_owned(socket, socket_identity);
                return Err(error);
            }
        };
        let (task_listener, task_socket, task_ingest, task_ingest_error) = if let Some(path) =
            task_socket_path
        {
            let path = path.to_path_buf();
            if path == socket {
                (
                    None,
                    None,
                    None,
                    Some("bus and control sockets must have different paths".to_string()),
                )
            } else {
                let bind = crate::control_plane::transport::prepare_socket_directory(
                    socket_directory(&path),
                    "bus",
                )
                .and_then(|()| {
                    crate::control_plane::transport::bind_socket(
                        &path,
                        "bus",
                        "another Radar owns it",
                    )
                    .map_err(|error| error.to_string())
                });
                match bind {
                    Ok(listener) => {
                        let identity = std::fs::symlink_metadata(&path)
                            .map(|metadata| (metadata.dev(), metadata.ino()));
                        match identity {
                            Ok(identity) if listener.set_nonblocking(true).is_ok() => (
                                Some(listener),
                                Some((path, identity)),
                                Some(Arc::new(crate::control_plane::ingest::TaskIngest::new())),
                                None,
                            ),
                            Ok(identity) => {
                                let _ = remove_socket_if_owned(&path, Some(identity));
                                (
                                    None,
                                    None,
                                    None,
                                    Some("bus listener could not be made nonblocking".to_string()),
                                )
                            }
                            Err(error) => (
                                None,
                                None,
                                None,
                                Some(format!("bus socket {}: {error}", path.display())),
                            ),
                        }
                    }
                    Err(error) => (None, None, None, Some(error)),
                }
            }
        } else {
            (None, None, None, None)
        };
        registry.attach_journal(store.clone());
        let stopping = Arc::new(AtomicBool::new(false));
        let verification = Arc::new(VerificationBudget {
            active: Mutex::new(0),
            workers: Mutex::new(Vec::new()),
        });
        let bootstrap = bootstrap_for(socket);
        let core = Arc::new(Mutex::new(Core {
            store,
            registry: Arc::new(registry),
            task_ingest: task_ingest.clone(),
            task_ingest_error,
            backend,
            verifier,
            stopping: Arc::clone(&stopping),
            sweep_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            bootstrap,
        }));
        let dispatch_core = Arc::clone(&core);
        let dispatch_stopping = Arc::clone(&stopping);
        let dispatch_verification = Arc::clone(&verification);
        let dispatch: Arc<Dispatch> = Arc::new(move |line, id, writer| {
            handle_line(
                &line,
                id,
                &dispatch_core,
                &writer,
                &dispatch_verification,
                &dispatch_stopping,
            );
        });
        let mut sources = vec![ListenerSource::control(
            listener,
            dispatch,
            event_signal,
            event_notification,
            event_ready,
        )];
        if let (Some(listener), Some(ingest)) = (task_listener, task_ingest) {
            let dispatch_ingest = Arc::clone(&ingest);
            let ingest_dispatch: Arc<IngestDispatch> =
                Arc::new(move |line, id| dispatch_ingest.ingest(id, &line));
            let disconnect_ingest = Arc::clone(&ingest);
            let disconnect = Arc::new(move |id| disconnect_ingest.disconnect(id));
            sources.push(ListenerSource::ingest(
                listener,
                ingest_dispatch,
                disconnect,
            ));
        }
        let transport = match Transport::start_services(sources, Arc::clone(&stopping)) {
            Ok(transport) => transport,
            Err(error) => {
                let _ = remove_socket_if_owned(socket, socket_identity);
                if let Some((path, identity)) = &task_socket {
                    let _ = remove_socket_if_owned(path, Some(*identity));
                }
                return Err(format!("control transport: {error}"));
            }
        };
        let sweeper = spawn_sweeper(&core, &stopping, &verification);
        Ok(Self {
            path: socket.to_path_buf(),
            state: state.to_path_buf(),
            task_socket,
            stopping,
            transport: Some(transport),
            verification,
            sweeper,
            socket_identity,
            core,
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

    fn core_store(&self) -> Option<Store> {
        Some(locked(&self.core).store.clone())
    }

    /// Stops accepting, ends every connection, joins their threads, and removes
    /// the socket file this daemon created. Idempotent.
    ///
    /// Records are left exactly where they are: they are the only account of what
    /// was asked for, and a daemon that removed them on the way out would make
    /// "did this run" unanswerable.
    pub fn stop(&mut self) {
        let Some(mut transport) = self.transport.take() else {
            return;
        };
        self.stopping.store(true, Ordering::SeqCst);
        stop_backend_for(&transport.connections);
        transport.stop();
        if let Some(sweeper) = self.sweeper.take() {
            let _ = sweeper.join();
        }
        self.verification.detach_remaining();
        if let Some(core) = self.core_store() {
            let _ = core.finish_journal();
        }
        let _ = remove_socket_if_owned(&self.path, self.socket_identity);
        if let Some((path, identity)) = &self.task_socket {
            let _ = remove_socket_if_owned(path, Some(*identity));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
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
    stopping: &Arc<AtomicBool>,
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
            let (identity, capabilities, task_status) = {
                let core = locked(core);
                let (identity, backend) = match &core.backend {
                    Some(backend) => (backend.identity(), backend.capabilities()),
                    None => (BackendIdentity::named("none"), &[][..]),
                };
                let available = core.task_ingest.is_some();
                (
                    identity,
                    ops::advertised_with_tasks(backend, available),
                    json!({"available": available, "error": core.task_ingest_error}),
                )
            };
            answer(
                writer,
                Response::ok(
                    &request.id,
                    json!({
                        "protocol": PROTOCOL_VERSION,
                        "backend": identity.name,
                        "instance": identity.instance,
                        "capabilities": capabilities,
                        "tasks": task_status,
                    }),
                ),
            );
        }
        Method::Request => answer(writer, read_one(core, &request)),
        Method::Requests => answer(writer, read_many(core, &request)),
        Method::Events => answer(writer, read_events(core, &request, stopping)),
        Method::Tasks => answer(writer, read_tasks(core, &request)),
        Method::Registry => answer(writer, registry_request(core, &request, verification)),
        Method::Bootstrap => answer(writer, bootstrap_request(core, &request, verification)),
        Method::Wake => answer(writer, wake_request(core, &request, verification)),
        Method::Read => answer(writer, read_backend(core, &request)),
        Method::Operation(operation) => {
            let response = execute(operation, &request, core, verification);
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
struct TasksListParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
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
struct LifecycleGetParams {
    subject_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleListParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerGetParams {
    container_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerListParams {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerReconcileParams {
    #[serde(default)]
    limit: Option<usize>,
}

/// Unix milliseconds now.
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Where lifecycle reads get their recovery evidence: the sweep interval and its
/// last completed pass. No pass yet is reported as null, never as success.
fn sweep_source(core: &Arc<Mutex<Core>>) -> serde_json::Value {
    let last = locked(core).sweep_at.load(Ordering::SeqCst);
    json!({"recovery_sweep": {
        "interval_ms": SWEEP_INTERVAL.as_millis() as u64,
        "last_pass_at": (last > 0).then(|| crate::control_plane::store::format_millis(last)),
    }})
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildCloseParams {
    spawn_request_id: String,
    source: String,
    incarnation: String,
    intent: registry::CloseIntent,
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
    /// The registered birth identity, present only when one was claimed. It is
    /// same-UID process evidence for matching a pane, not a credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    claim: Option<crate::model::ProcessIdentity>,
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

/// Run one evidence read on a bounded worker. The same global budget bounds
/// backend inventory, foreground evidence and procfs verification for close.
fn run_bounded<T: Send + 'static>(
    budget: &Arc<VerificationBudget>,
    stopping: &Arc<AtomicBool>,
    deadline: Instant,
    name: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ()> {
    if deadline.saturating_duration_since(Instant::now()).is_zero()
        || stopping.load(Ordering::SeqCst)
    {
        return Err(());
    }
    budget.reap();
    let Some(permit) = budget.acquire() else {
        return Err(());
    };
    let (send, receive) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _permit = permit;
            let result = work();
            let _ = send.send(result);
        })
        .map_err(|_| ())?;
    budget.track(worker);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stopping.load(Ordering::SeqCst) {
            return Err(());
        }
        match receive.recv_timeout(remaining.min(VERIFY_POLL)) {
            Ok(result) => return Ok(result),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Verifies that a pane's live foreground process is the exact process the bound
/// child registered.
///
/// A pane is closed on a live occupant only when the birth identity read now is
/// the identity that was claimed: a matching PID alone is not an identity. Any
/// other answer is a refusal, and the caller repeats it unless dead-pane evidence
/// replaces this gate.
fn verify_live_occupant(
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    pane_id: &str,
    claim: &crate::model::ProcessIdentity,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Result<(), Refusal> {
    let foreground = match bounded_foreground(
        backend,
        pane_id,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(evidence) => evidence,
        Err(()) => {
            return Err(Refusal::new(
                Code::BackendUnavailable,
                "foreground process evidence is unavailable",
            ));
        }
    };
    let pid = match foreground {
        crate::model::ForegroundEvidence::NonShell { pid, .. } if pid > 0 => pid,
        crate::model::ForegroundEvidence::NonShell { .. } => {
            return Err(Refusal::new(
                Code::Refused,
                "foreground process PID is invalid",
            ));
        }
        crate::model::ForegroundEvidence::Shell
        | crate::model::ForegroundEvidence::Inconclusive => {
            return Err(Refusal::new(
                Code::Refused,
                "foreground process evidence is inconclusive",
            ));
        }
    };
    let observed = verify_birth_identity(
        pid,
        verifier,
        stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    );
    match observed {
        Ok(observed)
            if pid == claim.pid
                && observed.pid == pid
                && observed.boot_id == claim.boot_id
                && observed.start_ticks == claim.start_ticks =>
        {
            Ok(())
        }
        _ => Err(Refusal::new(
            Code::Refused,
            "foreground process birth identity is unavailable or changed",
        )),
    }
}

/// What the dead-pane evidence says about a pane the live gate could not verify.
enum DeadPane {
    /// A retained pane of this edge, in the very instance that created it: its
    /// cleanup is defined.
    Cleanup(registry::SpawnExitObservation),
    /// Nothing here says the pane is dead and this edge's: the live refusal
    /// stands.
    Unresolved,
    /// The instance that created the recorded pane cannot be established, which
    /// nothing here may infer: refuse for that reason rather than adopt a pane
    /// that merely carries the same id.
    Unfenced(Refusal),
}

/// Evidence that this edge's recorded pane is a retained, already-dead pane whose
/// cleanup is defined, or why it is not.
///
/// The live gate protects a process that may be this child's and alive; a pane
/// reported dead now has no such occupant. What replaces the birth-identity match
/// is that the daemon created this location (the edge names this backend and
/// server instance and was created in it), that the pane's command exit is
/// already a recorded fact of this edge, and that the backend reports the pane
/// dead in a fresh read. History alone never closes anything, and a legacy or
/// re-pointed edge is not eligible.
fn dead_pane_evidence(
    registry: &Arc<Registry>,
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    close: &registry::ChildCloseRequest,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> DeadPane {
    if !backend
        .capabilities()
        .contains(&EXIT_OBSERVATION_CAPABILITY)
    {
        return DeadPane::Unresolved;
    }
    let Ok(Some(edge)) = registry.spawn_edge(&close.spawn_request_id) else {
        return DeadPane::Unresolved;
    };
    if edge.version != registry::SPAWN_VERSION {
        return DeadPane::Unresolved;
    }
    let Some(location) = edge.location.as_ref() else {
        return DeadPane::Unresolved;
    };
    let identity = backend.identity();
    if location.backend != identity.name
        || location.instance != identity.instance
        || location.pane.as_deref() != Some(pane_id)
    {
        return DeadPane::Unresolved;
    }
    // A pane id outlives the server that answered the create, so the pane is only
    // this edge's while the very process that served the create is still the one
    // serving: a restart under the same name hands the same id to a pane that was
    // never Radar's to close.
    let Some(created_in) = edge.instance.clone() else {
        return DeadPane::Unfenced(Refusal::new(
            Code::Refused,
            "the spawn edge records no verified backend instance for its pane",
        ));
    };
    match serving_identity(backend, verifier, stopping, budget) {
        Some(serving) if serving == created_in => {}
        _ => {
            return DeadPane::Unfenced(Refusal::new(
                Code::Refused,
                "the backend instance serving now is not the one that created the recorded pane",
            ));
        }
    }
    let Some(exit) = edge.exit else {
        return DeadPane::Unresolved;
    };
    match bounded_exit_observation(backend, pane_id, stopping, budget) {
        Ok(PaneExitObservation::Exited { .. }) => DeadPane::Cleanup(exit),
        // A pane reported running, undetermined, or unreadable is not a pane whose
        // command has exited: the live refusal stands.
        Ok(PaneExitObservation::Running | PaneExitObservation::Unknown) | Err(()) => {
            DeadPane::Unresolved
        }
    }
}

/// The verified birth identity of the process serving `backend` now, read within
/// the daemon's verification budget.
///
/// Both reads — the backend's own answer for its serving process and the process
/// behind that pid — run bounded on the shared budget, so a stalled backend or
/// verifier is abandoned at the deadline instead of holding the worker that is
/// answering a caller. A claim that cannot be read, a process that is gone and a
/// pid the verifier does not confirm are all none, so neither a recorded
/// instance nor a compared one is ever a guess.
fn serving_identity(
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Option<crate::model::ProcessIdentity> {
    let deadline = Instant::now() + VERIFICATION_TIMEOUT;
    let reader = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    let pid = run_bounded(
        budget,
        stopping,
        deadline,
        "radar-serving-process",
        move || reader.server_process(&cancel),
    )
    .ok()??;
    verify_birth_identity(pid, verifier, stopping, budget, deadline)
        .ok()
        .filter(|observed| observed.pid == pid)
}

fn bounded_close_dispatch(
    backend: &Arc<dyn RuntimeProvider>,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::runtime::CloseOutcome, ()> {
    let backend = Arc::clone(backend);
    let pane_id = pane_id.to_string();
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-child-close-dispatch",
        move || backend.close_outcome(&crate::runtime::CloseTarget::Pane(pane_id), &cancel),
    )
}

fn bounded_inventory(
    backend: &Arc<dyn RuntimeProvider>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<FleetObservation, ()> {
    let backend = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-close-inventory",
        move || backend.inventory(&cancel),
    )?
    .map_err(|_| ())
}

fn bounded_foreground(
    backend: &Arc<dyn RuntimeProvider>,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::model::ForegroundEvidence, ()> {
    let backend = Arc::clone(backend);
    let pane_id = pane_id.to_string();
    let cancel = Arc::clone(stopping);
    run_bounded(
        budget,
        stopping,
        deadline,
        "radar-close-foreground",
        move || backend.foreground_evidence(&pane_id, &cancel),
    )
}

/// Closes the pane an unbound launch left behind, fenced to the very instance
/// that created it, and records what happened as that launch's own durable fact.
///
/// Closing a pane ends whatever still runs in it, so a pane is closed only once
/// nothing this daemon launched can still be alive there: the launch's process is
/// verified gone, or the pane itself is a retained, already-exited pane of this
/// edge. Anything less leaves the pane to the operator and says why. This reports
/// a cleanup, never a process exit.
///
/// Only a cleanup that was dispatched is recorded: a confirmed close, a backend
/// refusal and an unanswered dispatch are each durable facts of the launch. An
/// eligibility refusal dispatches nothing, so it records nothing and a later
/// admitted cancel re-decides it against the evidence it finds.
fn cleanup_unbound_pane(
    registry: &Arc<Registry>,
    backend: &Arc<dyn RuntimeProvider>,
    verifier: &Arc<dyn ProcessVerifier>,
    subject_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    now_ms: i64,
) -> Option<subjects::PaneCleanup> {
    use subjects::PaneCleanup;

    let Ok(Some(subject)) = subjects::read(registry, subject_id) else {
        return Some(PaneCleanup::Refused(
            "the tracked launch is unavailable".into(),
        ));
    };
    // The first outcome is the one that stands, so a second cancel reports it
    // rather than closing anything again.
    if let Some(recorded) = subject.pane_cleanup.clone() {
        return Some(recorded);
    }
    if subject.state == subjects::State::Bound {
        // A bound launch's pane belongs to its registered child, so no cancel
        // has anything to say about it.
        return None;
    }
    if subject.state != subjects::State::Revoked {
        return Some(PaneCleanup::Refused(
            "the launch is not a revoked one, so its pane is not this cancel's to remove".into(),
        ));
    }
    // A subject with no spawn edge never had a pane of this daemon's: an
    // enrolled lead's pane belongs to the operator who enrolled it.
    let Ok(Some(edge)) = registry.spawn_edge(&subject.spawn_request_id) else {
        return None;
    };
    if edge.version != registry::SPAWN_VERSION {
        return Some(PaneCleanup::Refused(
            "a legacy spawn edge names no pane of this daemon's".into(),
        ));
    }
    let Some(location) = edge.location.as_ref() else {
        return Some(PaneCleanup::Refused("the launch records no pane".into()));
    };
    let Some(pane) = location.pane.clone() else {
        return Some(PaneCleanup::Refused("the launch records no pane".into()));
    };
    let identity = backend.identity();
    if location.backend != identity.name || location.instance != identity.instance {
        return Some(PaneCleanup::Refused(
            "the recorded pane belongs to a different backend instance".into(),
        ));
    }
    // A pane id outlives the server that answered the create, so the pane is only
    // this launch's while the very process that served the create still serves.
    let Some(created_in) = edge.instance.clone() else {
        return Some(PaneCleanup::Refused(
            "the launch records no verified backend instance for its pane".into(),
        ));
    };
    match serving_identity(backend, verifier, stopping, budget) {
        Some(serving) if serving == created_in => {}
        _ => {
            return Some(PaneCleanup::Refused(
                "the backend instance serving now is not the one that created the recorded pane"
                    .into(),
            ));
        }
    }
    if !backend
        .capabilities()
        .contains(&EXIT_OBSERVATION_CAPABILITY)
    {
        return Some(PaneCleanup::Refused(
            "this backend cannot report whether the pane's command has exited".into(),
        ));
    }
    let exited = subject.exited_at.is_some()
        || (edge.exit.is_some()
            && matches!(
                bounded_exit_observation(backend, &pane, stopping, budget),
                Ok(PaneExitObservation::Exited { .. })
            ));
    if !exited {
        return Some(PaneCleanup::Refused(
            "the launch's process is not verified gone; closing its pane would end it unclaimed"
                .into(),
        ));
    }
    let reader = Arc::clone(backend);
    let cancel = Arc::clone(stopping);
    let outcome = run_bounded(
        budget,
        stopping,
        Instant::now() + VERIFICATION_TIMEOUT,
        "radar-pane-cleanup",
        move || reader.close_outcome(&crate::runtime::CloseTarget::Pane(pane), &cancel),
    );
    let outcome = match outcome {
        Ok(crate::runtime::CloseOutcome::Completed) => PaneCleanup::Cleaned,
        Ok(crate::runtime::CloseOutcome::Refused(message)) => PaneCleanup::Refused(message),
        Ok(crate::runtime::CloseOutcome::Unknown(message)) => PaneCleanup::Unknown(message),
        // The dispatch may have reached the backend and its answer was lost.
        Err(()) => PaneCleanup::Unknown("the cleanup did not finish inside its deadline".into()),
    };
    match subjects::record_pane_cleanup(registry, subject_id, outcome, now_ms) {
        Ok(recorded) => Some(recorded),
        // An unrecorded outcome is never claimed as a cleanup: a replay must not
        // read "cleaned" for a close nothing committed.
        Err(error) => Some(PaneCleanup::Unknown(format!(
            "the cleanup outcome could not be recorded: {error}"
        ))),
    }
}

/// Reads one pane's exit state now, bounded and cancellable like every other
/// read this path makes. A pane the backend does not answer for is unknown.
fn bounded_exit_observation(
    backend: &Arc<dyn RuntimeProvider>,
    pane_id: &str,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
) -> Result<PaneExitObservation, ()> {
    let backend = Arc::clone(backend);
    let panes = vec![pane_id.to_string()];
    let cancel = Arc::clone(stopping);
    let observations = run_bounded(
        budget,
        stopping,
        Instant::now() + VERIFICATION_TIMEOUT,
        "radar-close-exit-observation",
        move || backend.exit_observations(&panes, &cancel),
    )?;
    observations.get(pane_id).cloned().ok_or(())
}

fn verify_birth_identity(
    pid: i32,
    verifier: &Arc<dyn ProcessVerifier>,
    stopping: &Arc<AtomicBool>,
    budget: &Arc<VerificationBudget>,
    deadline: Instant,
) -> Result<crate::model::ProcessIdentity, registry::ProcessVerification> {
    let verifier = Arc::clone(verifier);
    match run_bounded(
        budget,
        stopping,
        deadline,
        "radar-verify-birth-identity",
        move || verifier.inspect(pid),
    ) {
        Ok(Ok(Some(identity))) => Ok(identity),
        Ok(Ok(None)) => Err(registry::ProcessVerification::Absent),
        Ok(Err(_)) | Err(()) => Err(registry::ProcessVerification::Unavailable),
    }
}

fn process_evidence(
    registration: &registry::Registration,
    verification: registry::ProcessVerification,
) -> ProcessEvidence {
    ProcessEvidence {
        claimed: registration.request.process.is_some(),
        claim: registration.request.process.clone(),
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
    // The same capability the wire `mux.observe` read needs: a backend that
    // cannot answer that read cannot confirm a location here either.
    if capabilities_refuse(&backend, protocol::OBSERVE_METHOD).is_some()
        || cancel.load(Ordering::SeqCst)
    {
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
fn observe_spawn_exits(
    core: &Arc<Mutex<Core>>,
    edges: &[SpawnEdge],
) -> Result<Vec<SpawnEdge>, String> {
    let (registry, backend, cancel) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let Some(backend) = backend else {
        return Ok(edges.to_vec());
    };
    if cancel.load(Ordering::SeqCst)
        || !backend
            .capabilities()
            .contains(&EXIT_OBSERVATION_CAPABILITY)
    {
        return Ok(edges.to_vec());
    }
    let pending: Vec<_> = edges
        .iter()
        .filter(|edge| edge.version == registry::SPAWN_VERSION && edge.exit.is_none())
        .filter_map(|edge| {
            edge.location
                .as_ref()
                .and_then(|location| location.pane.as_ref())
                .cloned()
        })
        .collect();
    if pending.is_empty() {
        return Ok(edges.to_vec());
    }
    let observations = backend.exit_observations(&pending, &cancel);
    edges
        .iter()
        .map(|edge| {
            if edge.version != registry::SPAWN_VERSION || edge.exit.is_some() {
                return Ok(edge.clone());
            }
            let Some(pane) = edge
                .location
                .as_ref()
                .and_then(|location| location.pane.as_ref())
            else {
                return Ok(edge.clone());
            };
            let Some(PaneExitObservation::Exited { code, signal }) = observations.get(pane) else {
                return Ok(edge.clone());
            };
            registry.record_spawn_exit(&edge.request_id, *code, *signal, store::now_ms())
        })
        .collect()
}

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

/// Revalidate the exact managed child against fresh pane and process evidence,
/// persist the request only after the checks succeed, then dispatch one pane
/// close.
///
/// A live occupant is closed only after its birth identity is verified. A pane
/// whose command has already exited has no such occupant to protect, so a failed
/// live verification is retried against dead-pane evidence and closed as
/// `already_exited`; without that evidence the live refusal stands. Neither
/// outcome says the registered process exited.
fn execute_child_close(
    close: &registry::ChildCloseRequest,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    budget: &Arc<VerificationBudget>,
) -> Response {
    let (registry, backend, verifier, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    // A recorded close is answered from its own record before anything is
    // re-verified or dispatched: a replay repeats the one outcome this request id
    // reached, even when the pane it closed is no longer there to verify.
    if let Some(existing) = match registry.child_close(&close.request_id) {
        Ok(existing) => existing,
        Err(error) => return registry_error(request, error),
    } {
        if !existing.answers(close) {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "spawn.close request id already names different contents",
                ),
            );
        }
        return close_response(request, &existing);
    }
    let location = match registry.validate_child_close_target(close) {
        Ok(location) => location,
        Err(error) => return registry_error(request, error),
    };
    let pane_id = location.pane.as_deref().expect("validated pane target");
    let registration = match registry.child_registration(&close.source, &close.incarnation) {
        Ok(Some(registration)) => registration,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "bound child registration is not recorded"),
            );
        }
        Err(error) => return registry_error(request, error),
    };
    let Some(claim) = registration.request.process.clone() else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "bound child registration has no process identity claim",
            ),
        );
    };
    let Some(backend) = backend else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                "spawn.close requires a mux backend",
            ),
        );
    };
    if stopping.load(Ordering::SeqCst) {
        return Response::refused(&request.id, &Refusal::new(Code::Busy, "daemon is stopping"));
    }
    if let Some(refusal) = capabilities_refuse(&backend, protocol::OBSERVE_METHOD) {
        return Response::refused(&request.id, &refusal);
    }
    let inventory = match bounded_inventory(
        &backend,
        &stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(inventory) => inventory,
        Err(_) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BackendUnavailable,
                    "fresh mux inventory is unavailable",
                ),
            );
        }
    };
    let Some(pane) = inventory.pane(pane_id) else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "recorded child pane is absent from fresh inventory",
            ),
        );
    };
    if location
        .workspace
        .as_deref()
        .is_some_and(|id| id != pane.location.workspace_id)
        || location
            .tab
            .as_deref()
            .is_some_and(|id| id != pane.location.tab_id)
    {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, "recorded child pane containment changed"),
        );
    }
    let already_dead =
        match verify_live_occupant(&backend, &verifier, pane_id, &claim, &stopping, budget) {
            Ok(()) => None,
            Err(refusal) => {
                match dead_pane_evidence(
                    &registry, &backend, &verifier, close, pane_id, &stopping, budget,
                ) {
                    DeadPane::Cleanup(exit) => Some(exit),
                    DeadPane::Unresolved => return Response::refused(&request.id, &refusal),
                    DeadPane::Unfenced(fence) => {
                        return Response::refused(&request.id, &fence);
                    }
                }
            }
        };
    // A close backend is required before recording admission; otherwise this
    // request cannot reach a truthful dispatch result.
    if !backend.capabilities().contains(&CLOSE_CAPABILITY) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                format!(
                    "spawn.close needs the `{CLOSE_CAPABILITY}` capability; this backend does not \
                     provide it"
                ),
            ),
        );
    }
    if stopping.load(Ordering::SeqCst) {
        return Response::refused(&request.id, &Refusal::new(Code::Busy, "daemon is stopping"));
    }
    // Admission decides the one dispatch this request id may make: a caller that
    // raced this one for the same id dispatches nothing, and reads the outcome
    // recorded here instead.
    match registry.record_child_close(close, store::now_ms()) {
        Ok(CloseAdmission::Admitted(_)) => {}
        Ok(CloseAdmission::Recorded(recorded)) => return close_response(request, &recorded),
        Err(error) => return registry_error(request, error),
    }

    let result = match bounded_close_dispatch(
        &backend,
        pane_id,
        &stopping,
        budget,
        Instant::now() + VERIFICATION_TIMEOUT,
    ) {
        Ok(crate::runtime::CloseOutcome::Completed) => match already_dead {
            Some(exit) => CloseResult::AlreadyExited(exit),
            None => CloseResult::Completed,
        },
        Ok(crate::runtime::CloseOutcome::Refused(message)) => CloseResult::Refused(message),
        Ok(crate::runtime::CloseOutcome::Unknown(message)) => CloseResult::Unknown(Some(message)),
        // The dispatch may have reached the backend and its answer was lost.
        Err(()) => CloseResult::Unknown(None),
    };
    // The outcome is durable before this call answers: the effect is claimed only
    // from what the backend said, and a dispatch that said nothing leaves the
    // record unknown for every later replay to read.
    let recorded = match registry.record_child_close_outcome(&close.request_id, result) {
        Ok(recorded) => recorded,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Internal,
                    format!("the child close outcome was not recorded: {error}"),
                ),
            );
        }
    };
    close_response(request, &recorded)
}

/// The daemon's answer for one recorded close.
///
/// The durable outcome decides the shape, so a replay — including one after a
/// restart — answers as the original call did. A confirmed close reports the mux
/// pane closure and nothing else: no answer here claims a process or process
/// group exit, and an unknown outcome stays unknown rather than being retried.
fn close_response(request: &Request, close: &registry::ChildClose) -> Response {
    match close.outcome {
        CloseOutcome::Completed => Response::ok(
            &request.id,
            json!({"close": {
                "outcome": "completed",
                "intent": close.intent,
                "pane": close.location.pane.as_deref(),
            }}),
        ),
        CloseOutcome::AlreadyExited => Response::ok(
            &request.id,
            json!({"close": {
                "outcome": "already_exited",
                "intent": close.intent,
                "pane": close.location.pane.as_deref(),
                "exit_observation": close.exit.clone().map(|exit| {
                    registry::PublicSpawnExitObservation::Exited {
                        code: exit.code,
                        signal: exit.signal,
                        observed_at: exit.observed_at,
                    }
                }),
            }}),
        ),
        CloseOutcome::Refused => Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                close
                    .message
                    .as_deref()
                    .unwrap_or("the backend refused the close"),
            ),
        ),
        CloseOutcome::Unknown => Response::refused(
            &request.id,
            // A close that may have been dispatched keeps that uncertainty on the
            // wire: `outcome_unknown` is the code a caller reads as "an effect
            // may exist", so it is never reported as a pre-dispatch refusal.
            &Refusal::new(
                Code::OutcomeUnknown,
                close
                    .message
                    .as_deref()
                    .unwrap_or("managed child close outcome is unknown"),
            ),
        ),
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
            let facts = match registry.published_facts(&params.agent_id, Channel::Facts, now_ms) {
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
                    "execution": execution, "assignment": assignment, "facts": facts,
                    "context": context,
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
                let facts = match registry.published_facts(
                    &registration.agent_id,
                    Channel::Facts,
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
                    "facts": facts,
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
            let stored = match observe_spawn_exits(core, &[stored]) {
                Ok(mut edges) => edges.pop().expect("one spawn edge was supplied"),
                Err(error) => return registry_error(request, error),
            };
            // Location presence is separate from exit evidence: a missing pane
            // never becomes an exit observation.
            let inventory = observed_inventory(core);
            let evidence = location_evidence(inventory.as_ref(), &stored);
            Response::ok(&request.id, json!({"spawn": stored.topology(evidence)}))
        }
        "lifecycle.get" => {
            let params: LifecycleGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            if SessionUuid::parse(&params.subject_id).is_none() {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`subject_id` must be a canonical UUID"),
                );
            }
            let (registry, verifier) = {
                let core = locked(core);
                (Arc::clone(&core.registry), Arc::clone(&core.verifier))
            };
            match subjects::read(&registry, &params.subject_id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(Code::NotFound, "launch subject is not recorded"),
                    );
                }
                Err(error) => return registry_error(request, error),
            }
            // Observe exit by verified identity before answering, as spawn reads do.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64);
            if let Err(error) =
                subjects::observe_exit(&registry, &params.subject_id, verifier.as_ref(), now)
            {
                return registry_error(request, error);
            }
            match subjects::read(&registry, &params.subject_id) {
                Ok(Some(subject)) => Response::ok(
                    &request.id,
                    json!({"subject": subjects::public_view(&subject), "source": sweep_source(core)}),
                ),
                Ok(None) => Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "launch subject is not recorded"),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "lifecycle.list" => {
            let params: LifecycleListParams = match decode_params(request) {
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
            if params
                .after
                .as_deref()
                .is_some_and(|after| SessionUuid::parse(after).is_none())
            {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`after` must be a canonical UUID"),
                );
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            let (registry, verifier) = {
                let core = locked(core);
                (Arc::clone(&core.registry), Arc::clone(&core.verifier))
            };
            // One record beyond the page tells whether a continuation exists.
            let read = match subjects::list_after(&registry, params.after.as_deref(), limit + 1) {
                Ok(read) => read,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let page: Vec<String> = read
                .iter()
                .take(limit)
                .map(|subject| subject.subject_id.clone())
                .collect();
            let now = unix_millis();
            let mut views = Vec::with_capacity(page.len());
            for subject_id in &page {
                if let Err(error) =
                    subjects::observe_exit(&registry, subject_id, verifier.as_ref(), now)
                {
                    return registry_error(request, error);
                }
                match subjects::read(&registry, subject_id) {
                    Ok(Some(subject)) => views.push(subjects::public_view(&subject)),
                    Ok(None) => {}
                    Err(error) => return registry_error(request, error),
                }
            }
            let next_after = if has_more { page.last().cloned() } else { None };
            Response::ok(
                &request.id,
                json!({
                    "subjects": views,
                    "has_more": has_more,
                    "next_after": next_after,
                    "source": sweep_source(core),
                }),
            )
        }
        "topology.get" => {
            let params: ContainerGetParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            if SessionUuid::parse(&params.container_id).is_none() {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`container_id` must be a canonical UUID"),
                );
            }
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            match containers::read(&registry, &params.container_id) {
                Ok(Some(container)) => {
                    Response::ok(&request.id, json!({"container": container.public()}))
                }
                Ok(None) => Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "container is not recorded"),
                ),
                Err(error) => registry_error(request, error),
            }
        }
        "topology.list" => {
            let params: ContainerListParams = match decode_params(request) {
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
            if params
                .after
                .as_deref()
                .is_some_and(|after| SessionUuid::parse(after).is_none())
            {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BadParams, "`after` must be a canonical UUID"),
                );
            }
            let limit = params.limit.unwrap_or(50).min(MAX_REGISTRY_PAGE);
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            // One record beyond the page tells whether a continuation exists.
            let read = match containers::list_after(&registry, params.after.as_deref(), limit + 1) {
                Ok(read) => read,
                Err(error) => return registry_error(request, error),
            };
            let has_more = read.len() > limit;
            let page: Vec<_> = read
                .iter()
                .take(limit)
                .map(|container| container.public())
                .collect();
            let next_after = if has_more {
                page.last().map(|container| container.container_id.clone())
            } else {
                None
            };
            Response::ok(
                &request.id,
                json!({"containers": page, "has_more": has_more, "next_after": next_after}),
            )
        }
        "topology.reconcile" => {
            let params: ContainerReconcileParams = match decode_params(request) {
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
                Some(limit) if limit > MAX_RECONCILE => {
                    return Response::refused(
                        &request.id,
                        &Refusal::new(
                            Code::BadParams,
                            format!("`limit` must be at most {MAX_RECONCILE}"),
                        ),
                    );
                }
                _ => {}
            }
            let limit = params.limit.unwrap_or(MAX_RECONCILE).min(MAX_RECONCILE);
            let backend = {
                let core = locked(core);
                core.backend.clone()
            };
            // A reconciliation needs an inventory to reconcile against: without
            // one it refuses rather than recording that everything is missing.
            let Some(backend) = backend else {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::BackendUnavailable, "no mux backend is wired"),
                );
            };
            let Some(inventory) = observed_inventory(core) else {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::BackendUnavailable,
                        "the backend did not answer an inventory read",
                    ),
                );
            };
            let (registry, verifier, stopping) = {
                let core = locked(core);
                (
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                    Arc::clone(&core.stopping),
                )
            };
            // Every handle in the snapshot belongs to the incarnation serving it,
            // so an unverified serving process confirms nothing.
            let incarnation = serving_identity(&backend, &verifier, &stopping, verification);
            let inventory = containers::Inventory::from_observation(
                &inventory,
                &backend.identity(),
                incarnation,
            );
            let at = store::format_millis(unix_millis());
            match containers::reconcile(&registry, &inventory, &at, limit) {
                Ok(outcome) => Response::ok(&request.id, json!({"reconciled": outcome})),
                Err(error) => registry_error(request, error),
            }
        }
        "spawn.close" => {
            let params: ChildCloseParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let close = registry::ChildCloseRequest {
                request_id: request.id.clone(),
                spawn_request_id: params.spawn_request_id,
                source: params.source,
                incarnation: params.incarnation,
                intent: params.intent,
            };
            if let Err(error) = close.validate() {
                return Response::refused(&request.id, &Refusal::new(Code::BadParams, error));
            }
            execute_child_close(&close, request, core, verification)
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
            let candidates = match observe_spawn_exits(core, &candidates) {
                Ok(edges) => edges,
                Err(error) => return registry_error(request, error),
            };
            // One inventory serves the whole page, independently of the single
            // batch exit-status read above.
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
    if request.method == protocol::OBSERVE_METHOD
        && !request.params.as_object().is_some_and(|o| o.is_empty())
    {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`{}` accepts no params", protocol::OBSERVE_METHOD),
            ),
        );
    }
    let pane = if request.method == protocol::PROCESS_INFO_METHOD {
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
                &Refusal::new(
                    Code::BadParams,
                    format!("`{}` accepts only pane_id", protocol::PROCESS_INFO_METHOD),
                ),
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
                &Refusal::new(
                    Code::BadParams,
                    format!("`{}` accepts no params", protocol::OBSERVE_METHOD),
                ),
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

/// The public operation projection omits launch environment values while the
/// private stored record retains them for exact replay comparison.
fn public_record(record: RequestRecord) -> serde_json::Value {
    let mut value = serde_json::to_value(record).expect("request records serialize");
    if let Some(params) = value.get_mut("params") {
        if let Some(request) = params.get_mut("request")
            && let Some(object) = request.as_object_mut()
        {
            object.remove("env");
            object.remove("argv");
        }
        if let Some(object) = params.as_object_mut() {
            object.remove("env");
        }
    }
    value
}

/// One record, by id.
fn read_one(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let id = match request.required_string("id") {
        Ok(id) => id,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.get(&id) {
        Ok(Some(record)) => Response::ok(&request.id, json!({ "request": public_record(record) })),
        Ok(None) => Response::refused(
            &request.id,
            &Refusal::new(Code::NotFound, format!("no request {id}")),
        ),
        Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
    }
}

const DEFAULT_TASKS_LIMIT: usize = 100;
const MAX_TASKS_LIMIT: usize = 256;

fn read_tasks(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let params: TasksListParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let limit = params.limit.unwrap_or(DEFAULT_TASKS_LIMIT);
    if limit == 0 || limit > MAX_TASKS_LIMIT {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`limit` must be between 1 and {MAX_TASKS_LIMIT}"),
            ),
        );
    }
    let (ingest, error) = {
        let core = locked(core);
        (core.task_ingest.clone(), core.task_ingest_error.clone())
    };
    let Some(ingest) = ingest else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BackendUnavailable,
                error.unwrap_or_else(|| "task ingest is not configured".to_string()),
            ),
        );
    };
    let (revision, sessions, next_after) = ingest.list(params.after.as_deref(), limit);
    Response::ok(
        &request.id,
        json!({"revision": revision, "sessions": sessions, "next_after": next_after}),
    )
}

/// Recent records, newest first, bounded.
const DEFAULT_EVENT_LIMIT: usize = 100;
const MAX_EVENT_LIMIT: usize = 500;
const MAX_EVENT_WAIT_MS: u64 = 1_000;
const MAX_EVENTS_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsReadParams {
    #[serde(default)]
    after: u64,
    #[serde(default)]
    epoch: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    wait_ms: Option<u64>,
}

fn event_read_ready(store: &Store, line: &[u8]) -> bool {
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(line) else {
        return true;
    };
    if request.get("method").and_then(serde_json::Value::as_str) != Some("events.read") {
        return true;
    }
    let Some(params) = request.get("params") else {
        return true;
    };
    let Ok(params) = serde_json::from_value::<EventsReadParams>(params.clone()) else {
        return true;
    };
    let wait_ms = params.wait_ms.unwrap_or_default();
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if wait_ms == 0 || wait_ms > MAX_EVENT_WAIT_MS || limit == 0 || limit > MAX_EVENT_LIMIT {
        return true;
    }
    store.event_ready(params.after, params.epoch.as_deref())
}

fn read_events(core: &Arc<Mutex<Core>>, request: &Request, stopping: &Arc<AtomicBool>) -> Response {
    let params: EventsReadParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 || limit > MAX_EVENT_LIMIT {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`limit` must be between 1 and {MAX_EVENT_LIMIT}"),
            ),
        );
    }
    let wait = params.wait_ms.unwrap_or(0);
    if wait > MAX_EVENT_WAIT_MS {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("`wait_ms` must not exceed {MAX_EVENT_WAIT_MS}"),
            ),
        );
    }
    let after = params.after;
    let requested_epoch = params.epoch.as_deref();
    let store = {
        let core = locked(core);
        core.store.clone()
    };
    let deadline = Instant::now() + Duration::from_millis(wait);
    loop {
        let generation = store.event_generation();
        let (epoch, tail, earliest, events, delivery_pending) = match store.event_snapshot() {
            Ok(cursor) => cursor,
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        // Expiry is measured against the journal head: an `after` past it names
        // a position this journal never had.
        if requested_epoch.is_some_and(|requested| requested != epoch)
            || after > tail
            || after.saturating_add(1) < earliest
        {
            return Response::ok(
                &request.id,
                json!({"cursor_expired": {
                    "epoch": epoch, "cursor": {"epoch": epoch, "sequence": tail}, "earliest": earliest
                }}),
            );
        }
        // The cursor is where a consumer resumes, so it names the last event
        // actually delivered — or the request's own `after` when this answer
        // delivered none. The head is reported separately, and `more` says
        // whether a resumed read has anything left: a bounded prefix and a byte
        // bound can no longer look like an empty journal.
        let (available, cursor) = deliverable(events, after, limit, MAX_EVENTS_RESPONSE_BYTES);
        let more = tail > cursor || delivery_pending;
        if !available.is_empty()
            || wait == 0
            || stopping.load(Ordering::SeqCst)
            || Instant::now() >= deadline
        {
            return Response::ok(
                &request.id,
                json!({
                    "epoch": epoch, "events": available, "cursor": {"epoch": epoch, "sequence": cursor}, "tail": tail, "more": more, "earliest": earliest
                }),
            );
        }
        // Long polls occupy a bounded connection worker, but no Core/store lock.
        // Short condvar slices make stop cancellation prompt without starving joins.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = store.wait_for_event(generation, remaining.min(Duration::from_millis(25)));
    }
}

/// The prefix one `events.read` answer carries, and the position a consumer
/// resumes from.
///
/// The prefix is bounded by the caller's `limit` and by the bytes one answer
/// line may hold; the cursor is the last event actually delivered, so following
/// it can never skip an event either bound held back.
fn deliverable(
    events: Vec<store::Event>,
    after: u64,
    limit: usize,
    bytes: usize,
) -> (Vec<store::Event>, u64) {
    let mut available = Vec::new();
    let mut used = 0usize;
    for event in events
        .into_iter()
        .filter(|event| event.seq > after)
        .take(limit)
    {
        used += serde_json::to_vec(&event).map_or(0, |encoded| encoded.len());
        if used > bytes {
            break;
        }
        available.push(event);
    }
    let cursor = available.last().map_or(after, |event| event.seq);
    (available, cursor)
}

fn read_many(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    // Every record not yet completed, however old: a stop gate must see those,
    // and the recency window below cannot show them.
    if request
        .params
        .get("unfinished")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        let core = locked(core);
        return match core.store.unfinished() {
            Ok(records) => Response::ok(
                &request.id,
                json!({ "requests": records.into_iter().take(MAX_LISTED).map(public_record).collect::<Vec<_>>() }),
            ),
            Err(message) => Response::refused(&request.id, &Refusal::new(Code::Internal, message)),
        };
    }
    let limit = match request.limit(DEFAULT_LISTED, MAX_LISTED) {
        Ok(limit) => limit,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let core = locked(core);
    match core.store.recent(limit) {
        Ok(records) => Response::ok(
            &request.id,
            json!({ "requests": records.into_iter().map(public_record).collect::<Vec<_>>() }),
        ),
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
            return Response::ok(&request.id, json!({"request": public_record(record)}));
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
    Response::ok(&request.id, json!({"request": public_record(record)}))
}

/// Validates, records and dispatches one effectful operation without holding
/// the store lock over backend I/O.
fn execute(
    operation: ops::Operation,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match operation {
        ops::Operation::Focus => execute_focus(request, core),
        ops::Operation::Close => execute_close(request, core),
        ops::Operation::Create => execute_create(request, core, verification),
        ops::Operation::CreateContainer => execute_container_create(request, core, verification),
        ops::Operation::DeleteContainer => execute_container_delete(request, core, verification),
        ops::Operation::ReloadContainer => execute_container_reload(request, core, verification),
        ops::Operation::Input => execute_input(request, core),
        ops::Operation::Report => execute_report(request, core),
        ops::Operation::Spawn => execute_spawn(request, core, verification),
        ops::Operation::Enroll => execute_enroll(request, core, verification),
        ops::Operation::Restart => execute_restart(request, core, verification),
        ops::Operation::Cancel | ops::Operation::Stop => {
            execute_subject_control(request, core, operation, verification)
        }
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
                let mut result = json!({ "request": public_record(record.clone()) });
                // A replayed creation names the container its first answer named,
                // so a caller recovering from a lost answer reads the identity it
                // already made instead of searching the tree for it. No other
                // operation's answer carries a container, so a replay of one stays
                // the answer the first call gave.
                if let ops::Operation::CreateContainer = operation {
                    let container = containers::created_by(&core.registry, &request.id);
                    result["container"] = json!(container.map(|container| container.public()));
                }
                return Admission::Answered(Response::ok(&request.id, result));
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
/// no generic close route may signal it; managed close uses subject cancellation followed by spawn.close.
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
    let registrations = || locked(core).registry.public_registrations();
    let done = match lifecycle::close_unmanaged(
        backend.as_ref(),
        &close_request,
        &cancel,
        registrations,
    ) {
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
/// or the refusal to answer with. `extra` names further params this method
/// accepts beside it.
fn frozen_params(request: &Request, extra: &[&str]) -> Result<serde_json::Value, Response> {
    if request.params.as_object().is_none_or(|params| {
        params.keys().any(|key| {
            !matches!(key.as_str(), "request" | "requester") && !extra.contains(&key.as_str())
        })
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

/// One creation's result: the record to settle, and the container it recorded.
struct Creation {
    record: RequestRecord,
    container: Option<containers::Container>,
}

/// The label a creation carries, bounded and free of control characters.
///
/// A label is Radar's own name for a container: it is recorded verbatim and shown
/// in the tree, and the backend is never told it. It is bounded here, at the wire
/// boundary, rather than truncated wherever it is displayed.
fn creation_label(request: &Request) -> Result<Option<String>, Response> {
    let label = match request.optional_string("label") {
        Ok(label) => label,
        Err(refusal) => return Err(Response::refused(&request.id, &refusal)),
    };
    let Some(label) = label.filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                format!("a container label is at most {MAX_LABEL_CHARS} characters"),
            ),
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "a container label cannot carry control characters",
            ),
        ));
    }
    Ok(Some(label))
}

/// Dispatches a primitive create: a location is made, and nothing is started in
/// it.
///
/// The record's target is the location the request changes — the pane being split,
/// or the workspace a new tab joins — so a competing mutation of that location
/// cannot run beside it. A new workspace changes nothing that exists, so it has no
/// target and shares only the unscoped mutations' lane.
fn execute_create(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    match dispatch_creation(ops::Operation::Create, None, request, core, verification) {
        Ok(creation) => write_settled(request, creation.record, core),
        Err(response) => response,
    }
}

/// Dispatches a creation Radar records as one of its own containers.
///
/// The creation is the same primitive one; what this verb adds is that the
/// container, its label and its provenance are Radar's records, and that the
/// caller is answered with the container it made.
fn execute_container_create(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let label = match creation_label(request) {
        Ok(label) => label,
        Err(response) => return response,
    };
    match dispatch_creation(
        ops::Operation::CreateContainer,
        label,
        request,
        core,
        verification,
    ) {
        Ok(creation) => write_created(request, creation, core),
        Err(response) => response,
    }
}

/// Writes a settled creation together with the container it recorded.
///
/// The container travels with the record so a caller acts on the identity it made
/// instead of joining a handle to whatever the tree shows next.
fn write_created(request: &Request, creation: Creation, core: &Arc<Mutex<Core>>) -> Response {
    let core = locked(core);
    match core.store.write(&creation.record) {
        Ok(()) => Response::ok(
            &request.id,
            json!({
                "request": public_record(creation.record),
                "container": creation.container.map(|container| container.public()),
            }),
        ),
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// Validates, records and dispatches one creation, and records the container it
/// made.
///
/// `mux.create` asks a backend for a primitive and `topology.create` for the same
/// creation as Radar's own hierarchy verb, so the two share every step: the
/// request is validated here, the runtime's answer is turned into a container
/// record before the creation is reported, and only the operation a caller is
/// recorded under differs. A container is recorded for the identity the request
/// asked for and no other, and a runtime that names nothing records nothing.
fn dispatch_creation(
    operation: ops::Operation,
    label: Option<String>,
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Result<Creation, Response> {
    // The primitive create carries the request alone; the hierarchy verb carries
    // the label its container is recorded under.
    let extra: &[&str] = match operation {
        ops::Operation::CreateContainer => &["label", "recipe"],
        _ => &[],
    };
    let frozen = frozen_params(request, extra)?;
    let create: CreateRequest = match serde_json::from_value(frozen) {
        Ok(create) => create,
        Err(error) => {
            return Err(Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a create request: {error}"),
                ),
            ));
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
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the create target is not a valid identifier",
            ),
        ));
    }
    // The command this creation's slot runs, if it carries one, is parsed and
    // checked here: a recipe this backend cannot carry refuses before a pane
    // exists, exactly as a spawn is preflighted.
    let recipe = creation_recipe(request, &create)?;
    if let Some(recipe) = &recipe
        && let Err(refusal) = preflight_recipe(core, &create, recipe)
    {
        return Err(Response::refused(&request.id, &refusal));
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Err(Response::refused(&request.id, &refusal)),
    };
    let now = store::now_ms();
    let method = operation.as_str();
    let (backend, cancel) =
        match admit(operation, request, target.as_deref(), &requester, now, core) {
            Admission::Answered(response) => return Err(response),
            Admission::Unsupported(message) => {
                return Err(execute_unsupported(
                    operation, request, &message, target, core,
                ));
            }
            Admission::Dispatch(backend, cancel) => (backend, cancel),
        };
    let record = RequestRecord::new(&request.id, method, requester, target, now)
        .with_params(request.params.clone());
    // The incarnation serving this create is verified before it is dispatched, so
    // the container is bound to the generation that made it rather than to
    // whatever a later server puts at the same handle.
    let verifier = Arc::clone(&locked(core).verifier);
    let incarnation = serving_identity(&backend, &verifier, &cancel, verification);
    // Creation is confirmed, not re-observed: the runtime says it made the
    // location, nothing here lists to prove it exists.
    let done = match backend.create(&create, &cancel) {
        // The identity the answer named travels as data, so a client acts on what
        // it created without reading the effect prose.
        CreateOutcome::Completed(created) => {
            // The container is recorded before the creation is reported, so a
            // caller never reads an owned location that no record names. A failed
            // append is reported rather than hidden: the effect happened, and the
            // caller must not be told the topology is recorded when it is not.
            let now = store::now_ms();
            let registry = {
                let core = locked(core);
                Arc::clone(&core.registry)
            };
            let backend_identity = backend.identity();
            let id = store::random_uuid();
            let mut notes: Vec<String> = Vec::new();
            // A recipe is held privately before the record that says so is
            // written, so no record claims a command the daemon does not have. It
            // is held only for a create that named a pane: a recipe runs in a pane
            // slot, and a create that named nothing has no slot to run it in.
            let held = match (recipe.as_ref(), created.pane_id.as_deref()) {
                (Some(recipe), Some(_)) => match containers::write_recipe(&registry, &id, recipe) {
                    Ok(()) => true,
                    Err(error) => {
                        notes.push(format!("the recipe was not held: {error}"));
                        false
                    }
                },
                (Some(_), None) => {
                    notes.push("the runtime named no pane, so the recipe was not held".to_string());
                    false
                }
                (None, _) => false,
            };
            let recording = containers::record_created(
                &registry,
                containers::Confirmed {
                    id,
                    recipe: held,
                    request: &create,
                    created: &created,
                    backend: &backend_identity,
                    incarnation: incarnation.clone(),
                    label,
                    created_by: Some(request.id.clone()),
                },
                &store::format_millis(now),
            );
            let (container, _) = match (recording, created.is_empty()) {
                (Err(error), _) => {
                    notes.push(format!("the created container was not recorded: {error}"));
                    (None, false)
                }
                (Ok(_), true) => {
                    notes.push(
                        "the runtime confirmed the creation without naming what it created"
                            .to_owned(),
                    );
                    (None, true)
                }
                (Ok(mut recorded), false) => (recorded.pop(), true),
            };
            // The recipe runs in the pane the create named, launched by the daemon
            // outside any spawn: the child carries no spawn token, and this launch
            // is what a later reload requires to be gone before running it again.
            let launched = match (held, created.pane_id.as_deref()) {
                (true, Some(pane_id)) => recipe.as_ref().map(|recipe| {
                    backend.launch(
                        &LaunchRequest {
                            pane_id: pane_id.to_string(),
                            executable: recipe.executable.clone(),
                            argv: recipe.argv.clone(),
                            spawn_token: None,
                            cwd: recipe.cwd.clone(),
                            env: recipe.env.clone(),
                        },
                        &cancel,
                    )
                }),
                _ => None,
            };
            let mut effects = created.effects();
            let (outcome, category) = match &launched {
                Some(LaunchOutcome::Completed) => {
                    effects.push("launch".into());
                    (RequestOutcome::Completed, None)
                }
                Some(LaunchOutcome::Refused(message)) => {
                    notes.push(format!("the recipe was not launched: {message}"));
                    (RequestOutcome::Refused, Some(Category::BACKEND_REFUSED))
                }
                Some(LaunchOutcome::Unknown(message)) => {
                    notes.push(format!(
                        "the recipe launch is unknown and is not retried: {message}"
                    ));
                    (RequestOutcome::Unknown, Some(Category::BACKEND_UNAVAILABLE))
                }
                None => (RequestOutcome::Completed, None),
            };
            let note = (!notes.is_empty()).then(|| notes.join("; "));
            return Ok(Creation {
                record: record
                    .created(created.identity(create.created_kind()))
                    .complete(outcome, category, note.as_deref(), effects, now),
                container,
            });
        }
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
    Ok(Creation {
        record: done,
        container: None,
    })
}

/// Deletes one container Radar records: the hierarchy's own verb for removing a
/// workspace, a tab or a pane slot.
///
/// The container is what is deleted and the frozen target is what the operator
/// confirmed, so a container replaced in the meantime is not the one acted on.
/// The close is the same guarded dispatch a generic close runs — positively
/// unmanaged, still the frozen identity, unregistered — and what a deletion would
/// take with it is read from the daemon's own records before anything is
/// dispatched. Pane removal is recorded as a removal; no process is claimed to
/// have exited, no child to have been unregistered, and no assignment to have
/// completed.
fn execute_container_delete(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let delete: containers::DeleteRequest = match serde_json::from_value(frozen) {
        Ok(delete) => delete,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a container deletion: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&delete.container_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the container id is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::DeleteContainer;
    let method = operation.as_str();
    let target = delete.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    // Everything below refuses into the record: a confirmed deletion is readable
    // by its request id whatever it decided.
    let registry = {
        let core = locked(core);
        Arc::clone(&core.registry)
    };
    let container = match containers::read(&registry, &delete.container_id) {
        Ok(Some(container)) => container,
        Ok(None) => {
            return write_refused(
                request,
                record,
                &format!(
                    "container {} is not recorded: nothing was deleted",
                    delete.container_id
                ),
                core,
            );
        }
        Err(error) => return write_refused(request, record, &error, core),
    };
    // A container this daemon already removed is history, and one whose binding
    // names no verified serving instance cannot be fenced: neither is deleted.
    if let Some(at) = container.removed_at.as_deref() {
        return write_refused(
            request,
            record,
            &format!(
                "container {} was removed at {at}: nothing was deleted",
                container.id
            ),
            core,
        );
    }
    let Some(binding) = container.binding.clone() else {
        return write_refused(
            request,
            record,
            &format!(
                "container {} records no binding: nothing was deleted",
                container.id
            ),
            core,
        );
    };
    let expected = container.kind.target(&binding.handle);
    if delete.frozen.target != expected {
        return write_refused(
            request,
            record,
            &format!(
                "{} is not container {}'s location ({}): nothing was deleted",
                delete.frozen.target.description(),
                container.id,
                expected.description()
            ),
            core,
        );
    }
    let verifier = Arc::clone(&locked(core).verifier);
    let serving = serving_identity(&backend, &verifier, &cancel, verification);
    let Some(recorded) = binding.incarnation.clone() else {
        return write_refused(
            request,
            record,
            &format!(
                "container {} records no verified serving instance: nothing was deleted",
                container.id
            ),
            core,
        );
    };
    if serving.as_ref() != Some(&recorded) {
        return write_refused(
            request,
            record,
            &format!(
                "container {} was recorded under another serving instance: nothing was deleted",
                container.id
            ),
            core,
        );
    }
    // What the deletion would take with it, from the daemon's own records: the
    // container, everything recorded inside it, and any worker registered into
    // one of those places.
    let members = match containers::subtree(&registry, &container.id) {
        Ok(members) => members,
        Err(message) => return write_refused(request, record, &message, core),
    };
    let mut places: Vec<(containers::Kind, String)> = Vec::new();
    for id in &members {
        match containers::read(&registry, id) {
            Ok(Some(member)) => {
                if let Some(binding) = member.binding.as_ref() {
                    places.push((member.kind, binding.handle.clone()));
                }
            }
            Ok(None) => {}
            Err(error) => return write_refused(request, record, &error, core),
        }
    }
    let registrations = match locked(core).registry.public_registrations() {
        Ok(registrations) => registrations,
        Err(message) => return write_refused(request, record, &message, core),
    };
    // A live managed process is named rather than closed around: the operator
    // stops it first, and the stop reports the exit while the deletion reports
    // the removal.
    let occupied = lifecycle::registrations_in(&registrations, |location| {
        places.iter().any(|(kind, handle)| match kind {
            containers::Kind::Workspace => location.workspace.as_deref() == Some(handle),
            containers::Kind::Tab => location.tab.as_deref() == Some(handle),
            containers::Kind::Pane => location.pane.as_deref() == Some(handle),
        })
    });
    if !occupied.is_empty() {
        let named: Vec<String> = occupied
            .iter()
            .map(|registration| match registration.label.as_deref() {
                Some(label) => format!("{label} ({})", registration.agent_id),
                None => registration.agent_id.clone(),
            })
            .collect();
        return write_refused(
            request,
            record,
            &format!(
                "deleting {} would end {}: stop it first, then delete; nothing was deleted",
                delete.frozen.target.description(),
                named.join(", ")
            ),
            core,
        );
    }
    // The guarded dispatch a generic close uses, unchanged: one fresh inventory,
    // the frozen identity, and a registry that must answer.
    let registrations = || locked(core).registry.public_registrations();
    let settled = match lifecycle::close_unmanaged(
        backend.as_ref(),
        &delete.frozen,
        &cancel,
        registrations,
    ) {
        DirectClose::Completed => {
            let now = store::now_ms();
            let at = store::format_millis(now);
            let mut removed: Vec<containers::Container> = Vec::new();
            let mut failures: Vec<String> = Vec::new();
            for id in &members {
                match containers::record_removed(&registry, id, &at) {
                    Ok(container) => removed.push(container),
                    Err(error) => failures.push(format!("{id}: {error}")),
                }
            }
            // The close is the runtime's confirmation that it removed the place,
            // not a re-observation of it, and the removals are the daemon's own
            // records: neither claims anything about a process.
            let note = if failures.is_empty() {
                format!(
                    "{} was closed by the runtime; {} container record(s) marked removed",
                    delete.frozen.target.description(),
                    removed.len()
                )
            } else {
                format!(
                    "{} was closed by the runtime; removals were not recorded for: {}",
                    delete.frozen.target.description(),
                    failures.join("; ")
                )
            };
            let settled = record.complete(
                RequestOutcome::Completed,
                None,
                Some(&note),
                vec!["close".into(), "container.removal".into()],
                now,
            );
            return write_deleted(request, settled, removed, core);
        }
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
    write_deleted(request, settled, Vec::new(), core)
}

/// Reloads one pane container's slot: the recipe it was created with runs again
/// in the pane it was created in.
///
/// Reload requires the previous command to be gone — the pane's own retained exit
/// evidence, read now — so nothing is replaced while it may still be running. It
/// records its own outcome and claims no session restart, no new container and no
/// assignment fact: it is one slot's command run again.
fn execute_container_reload(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let reload: containers::ReloadRequest = match serde_json::from_value(frozen) {
        Ok(reload) => reload,
        Err(error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`request` is not a container reload: {error}"),
                ),
            );
        }
    };
    if !valid_identifier(&reload.container_id) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "the container id is not a valid identifier",
            ),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    let operation = ops::Operation::ReloadContainer;
    let method = operation.as_str();
    let target = reload.container_id.clone();
    let (backend, cancel) = match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(&request.id, method, requester, Some(target), now)
        .with_params(request.params.clone());
    let registry = {
        let core = locked(core);
        Arc::clone(&core.registry)
    };
    let container = match containers::read(&registry, &reload.container_id) {
        Ok(Some(container)) => container,
        Ok(None) => {
            return reload_refused(
                request,
                record,
                &format!(
                    "container {} is not recorded: nothing was reloaded",
                    reload.container_id
                ),
                None,
                core,
            );
        }
        Err(error) => return reload_refused(request, record, &error, None, core),
    };
    // Everything a reload requires is decided here, and every refusal names what
    // is missing: only a pane slot runs a recipe, only a container Radar created
    // has one, and the recipe itself must still be held.
    let eligible = (|| -> Result<(containers::Binding, containers::Recipe), String> {
        if container.kind != containers::Kind::Pane {
            return Err("only a pane slot runs a recipe".to_string());
        }
        if container.provenance != containers::Provenance::Created {
            return Err(
                "this container is not one Radar created: it has no recipe to run".to_string(),
            );
        }
        if let Some(at) = container.removed_at.as_deref() {
            return Err(format!("container {} was removed at {at}", container.id));
        }
        let binding = container
            .binding
            .clone()
            .ok_or_else(|| format!("container {} records no binding", container.id))?;
        let verifier = Arc::clone(&locked(core).verifier);
        let serving = serving_identity(&backend, &verifier, &cancel, verification);
        let Some(recorded) = binding.incarnation.clone() else {
            return Err(format!(
                "container {} records no verified serving instance",
                container.id
            ));
        };
        if serving.as_ref() != Some(&recorded) {
            return Err(format!(
                "container {} was recorded under another serving instance",
                container.id
            ));
        }
        if !container.recipe {
            return Err(format!("container {} records no recipe", container.id));
        }
        // The record says a recipe is held and the private store has none: the two
        // disagree, and a command is never invented to run.
        let recipe = containers::read_recipe(&registry, &container.id)?
            .ok_or_else(|| format!("the recipe held for container {} is missing", container.id))?;
        Ok((binding, recipe))
    })();
    let (binding, recipe) = match eligible {
        Ok(ready) => ready,
        Err(message) => {
            return reload_refused(
                request,
                record,
                &format!("{message}: nothing was reloaded"),
                Some(container),
                core,
            );
        }
    };
    // The pane's own retained exit is the evidence that the previous command is
    // gone. A command still running, or one whose exit cannot be established, is
    // not replaced: the operator stops it first.
    let pane = binding.handle.clone();
    let observed = backend.exit_observations(std::slice::from_ref(&pane), &cancel);
    match observed.get(&pane) {
        Some(PaneExitObservation::Exited { .. }) => {}
        Some(PaneExitObservation::Running) => {
            return reload_refused(
                request,
                record,
                &format!("pane {pane} is still running its command: stop it first, then reload"),
                Some(container),
                core,
            );
        }
        _ => {
            return reload_refused(
                request,
                record,
                &format!(
                    "pane {pane} does not report an exited command: not replacing what may still be running"
                ),
                Some(container),
                core,
            );
        }
    }
    let launched = backend.launch(
        &LaunchRequest {
            pane_id: pane.clone(),
            executable: recipe.executable.clone(),
            argv: recipe.argv.clone(),
            // A container's own launch is not a spawn child: no token, and the
            // variable is not put in its environment.
            spawn_token: None,
            cwd: recipe.cwd.clone(),
            env: recipe.env.clone(),
        },
        &cancel,
    );
    let settled = match launched {
        LaunchOutcome::Completed => record.complete(
            RequestOutcome::Completed,
            None,
            Some(&format!(
                "the recorded recipe was run again in {pane}; the container is unchanged"
            )),
            vec!["launch".into()],
            store::now_ms(),
        ),
        LaunchOutcome::Refused(message) => record.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
        LaunchOutcome::Unknown(message) => record.complete(
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(&message),
            Vec::new(),
            store::now_ms(),
        ),
    };
    write_reloaded(request, settled, Some(container), core)
}

/// Answers an admitted reload with a refusal that is recorded before it is
/// written.
fn reload_refused(
    request: &Request,
    record: RequestRecord,
    message: &str,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_reloaded(
        request,
        record.complete(
            RequestOutcome::Refused,
            Some(Category::REFUSED),
            Some(message),
            Vec::new(),
            store::now_ms(),
        ),
        container,
        core,
    )
}

/// Answers a settled reload with the container it acted on, unchanged.
fn write_reloaded(
    request: &Request,
    record: RequestRecord,
    container: Option<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_settled_with(
        request,
        record,
        json!({
            "container": container.map(|container| container.public()),
        }),
        core,
    )
}

/// Writes a settled operation with the public facts its own answer names beside
/// the record, so a caller acts on what its request did rather than on what the
/// next read happens to show.
fn write_settled_with(
    request: &Request,
    record: RequestRecord,
    extra: serde_json::Value,
    core: &Arc<Mutex<Core>>,
) -> Response {
    let core = locked(core);
    match core.store.write(&record) {
        Ok(()) => {
            let mut body = json!({ "request": public_record(record) });
            if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    body.insert(key.clone(), value.clone());
                }
            }
            Response::ok(&request.id, body)
        }
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// Answers an admitted deletion with a refusal that is recorded before it is
/// written, so a caller reads back what the daemon decided.
fn write_refused(
    request: &Request,
    record: RequestRecord,
    message: &str,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_deleted(
        request,
        record.complete(
            RequestOutcome::Refused,
            Some(Category::REFUSED),
            Some(message),
            Vec::new(),
            store::now_ms(),
        ),
        Vec::new(),
        core,
    )
}

/// Writes a settled deletion together with the containers it marked removed.
///
/// The containers travel with the record so a caller reads what its own request
/// removed rather than joining handles to whatever the tree shows next.
fn write_deleted(
    request: &Request,
    record: RequestRecord,
    removed: Vec<containers::Container>,
    core: &Arc<Mutex<Core>>,
) -> Response {
    write_settled_with(
        request,
        record,
        json!({
            "removed": removed
                .into_iter()
                .map(|container| container.public())
                .collect::<Vec<_>>(),
        }),
        core,
    )
}

/// Validates, records and dispatches one pane input.
///
/// Input is what a client asks a pane to receive: literal text, or named key
/// presses, in one of the two shapes and never both. The payload is validated
/// here, at the wire boundary, before anything is recorded or dispatched; what
/// survives validation is input this seam can carry without deciding for the
/// caller what a terminal would do with it.
fn execute_input(request: &Request, core: &Arc<Mutex<Core>>) -> Response {
    let frozen = match frozen_params(request, &[]) {
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
    let frozen = match frozen_params(request, &[]) {
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
    parent_session_id: String,
    executable: String,
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

/// Asks the selected backend whether it can carry this launch, before any record
/// is written.
fn preflight_launch(core: &Arc<Mutex<Core>>, params: &SpawnParams) -> Result<(), Refusal> {
    preflight_command(
        core,
        "preflight",
        &params.executable,
        &params.argv,
        params.cwd.as_deref(),
        &params.env,
    )
}

/// Asks the selected backend whether it can carry one command in one pane, before
/// any record is written. The probe names no token: what is checked is the command
/// and the context it needs.
fn preflight_command(
    core: &Arc<Mutex<Core>>,
    pane_id: &str,
    executable: &str,
    argv: &[String],
    cwd: Option<&str>,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(), Refusal> {
    let launch_context = LaunchRequest {
        pane_id: pane_id.to_string(),
        executable: executable.to_string(),
        argv: argv.to_vec(),
        spawn_token: None,
        cwd: cwd.map(str::to_string),
        env: env.clone(),
    };
    let preflight = {
        let core = locked(core);
        core.backend.as_ref().map(|backend| {
            if (!env.is_empty() || cwd.is_some())
                && !backend.capabilities().contains(&LAUNCH_CONTEXT_CAPABILITY)
            {
                Err("selected backend does not support launch context".to_string())
            } else {
                backend.validate_launch(&launch_context)
            }
        })
    };
    match preflight {
        Some(Err(message)) => Err(Refusal::new(Code::BadParams, message)),
        None if cwd.is_some() || !env.is_empty() => Err(Refusal::new(
            Code::BackendUnavailable,
            "selected backend does not support launch context",
        )),
        _ => Ok(()),
    }
}

/// The recipe a creation carries: the command its pane slot runs.
///
/// A recipe is held privately and reported only as held. It is refused on a
/// creation that makes no pane, because a recipe runs in a pane slot, and only the
/// hierarchy verb accepts one: the primitive create takes no Radar annotations.
fn creation_recipe(
    request: &Request,
    create: &CreateRequest,
) -> Result<Option<containers::Recipe>, Response> {
    let Some(recipe) = request.params.get("recipe") else {
        return Ok(None);
    };
    if !matches!(create, CreateRequest::PaneSplit { .. }) {
        return Err(Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "a recipe runs in a pane slot: only a pane creation carries one",
            ),
        ));
    }
    serde_json::from_value(recipe.clone())
        .map(Some)
        .map_err(|error| {
            Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    format!("`recipe` is not a container recipe: {error}"),
                ),
            )
        })
}

/// Asks the selected backend whether it can carry a container's recipe, before
/// anything is created.
fn preflight_recipe(
    core: &Arc<Mutex<Core>>,
    create: &CreateRequest,
    recipe: &containers::Recipe,
) -> Result<(), Refusal> {
    let CreateRequest::PaneSplit { pane_id, .. } = create else {
        return Err(Refusal::new(
            Code::BadParams,
            "a recipe runs in a pane slot: only a pane creation carries one",
        ));
    };
    preflight_command(
        core,
        pane_id,
        &recipe.executable,
        &recipe.argv,
        recipe.cwd.as_deref(),
        &recipe.env,
    )
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
fn execute_spawn(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: SpawnParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`request` is not a valid spawn request".to_string(),
                ),
            );
        }
    };
    // The command and the parent are validated at the wire boundary, before
    // anything is recorded: a request this seam cannot carry creates no pane.
    if let Err(message) = validate_command(&params.executable, &params.argv) {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    if let Err(refusal) = preflight_launch(core, &params) {
        return Response::refused(&request.id, &refusal);
    }
    let intent = registry::SpawnRequest {
        request_id: request.id.clone(),
        parent: params.parent.clone(),
        parent_session_id: params.parent_session_id.clone(),
    };
    if let Err(message) = intent.validate() {
        return Response::refused(&request.id, &Refusal::new(Code::BadParams, message));
    }
    let tracks_launches = locked(core).backend.as_ref().is_some_and(|backend| {
        backend
            .capabilities()
            .contains(&LIFECYCLE_TRACKING_CAPABILITY)
    });
    if tracks_launches && let Err(message) = subjects::check_bounds(&requested_launch(&params)) {
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
        let context = match core.registry.context(&params.parent, store::now_ms()) {
            Ok(Some(context)) => context,
            Ok(None) => {
                return Response::refused(
                    &request.id,
                    &Refusal::new(Code::Refused, "parent has no published session context"),
                );
            }
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        };
        if context.context.freshness != registry::Freshness::Fresh
            || context.context.session.as_deref() != Some(&params.parent_session_id)
        {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "parent session context is absent, stale or mismatched",
                ),
            );
        }
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
    spawn_child(
        request,
        core,
        verification,
        &params,
        &intent,
        parent_pane,
        None,
    )
}

/// Creates the pane a child runs in and launches it there, once its parent pane is
/// resolved. A restart reuses this with the lead's own pane as the parent pane and
/// its resume recipe as the launch, so both paths share one durable sequence.
fn spawn_child(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
    params: &SpawnParams,
    intent: &registry::SpawnRequest,
    parent_pane: String,
    session: Option<String>,
) -> Response {
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
    if let Err(message) = locked(core).registry.record_spawn(intent, now) {
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
    // A backend that tracks its launches gets the subject before any effect, so
    // the bootstrap's correlation is recorded before the launch can run it.
    let tracking = if backend
        .capabilities()
        .contains(&LIFECYCLE_TRACKING_CAPABILITY)
    {
        match track_launch(
            core,
            &request.id,
            requested_launch(params),
            session.clone(),
            now,
        ) {
            Ok(tracking) => Some(tracking),
            Err(message) => {
                let settled = record.complete(
                    RequestOutcome::Refused,
                    None,
                    Some(&message),
                    Vec::new(),
                    store::now_ms(),
                );
                let _ = write_settled(request, settled, core);
                return Response::refused(&request.id, &Refusal::new(Code::Internal, message));
            }
        }
    } else {
        None
    };
    // The pane is a split of the parent's own pane, unfocused: a child is
    // created where its parent is, and creating one must not move the operator's
    // cursor.
    // The process serving this backend is read and verified on the daemon-wide
    // verification budget before the create is dispatched: a pane belongs to the
    // generation serving then, or to one that replaced it, so a later close can
    // tell the two apart instead of trusting a reused pane id. A verifier that
    // stalls is abandoned at the deadline, and a capture that cannot be verified
    // records nothing.
    let verifier = Arc::clone(&locked(core).verifier);
    let instance = serving_identity(&backend, &verifier, &cancel, verification);
    let create_request = CreateRequest::PaneSplit {
        pane_id: parent_pane,
        direction: SplitDirection::Right,
        focus: false,
    };
    let created = backend.create(&create_request, &cancel);
    let (created_outcome, named, mut created_message) = match &created {
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
        .map(|named| created_location(named, &backend.identity()));
    // The pane this launch was dispatched into is recorded as one of this
    // daemon's containers before the edge and the launch are recorded: a spawned
    // agent's slot is topology, not only a location on an edge.
    if let Some(named) = named {
        let registry = {
            let core = locked(core);
            Arc::clone(&core.registry)
        };
        let at = store::format_millis(store::now_ms());
        if let Err(error) = containers::record_created(
            &registry,
            containers::Confirmed {
                id: store::random_uuid(),
                recipe: false,
                request: &create_request,
                created: named,
                backend: &backend.identity(),
                incarnation: instance.clone(),
                label: None,
                created_by: Some(request.id.clone()),
            },
            &at,
        ) {
            created_message = Some(format!("the created container was not recorded: {error}"));
        }
    }
    let mut effects = named.map(CreatedLocation::effects).unwrap_or_default();
    let pane = named.and_then(|named| named.identity(CreatedKind::Pane));
    // An instance identity is recorded only for a pane: an edge with no location
    // has a generation no close could use.
    let instance = if pane.is_some() { instance } else { None };
    // The create is recorded before the launch is attempted: a confirmed pane is
    // readable even when the launch never answers.
    let written = locked(core).registry.record_spawn_created(
        &request.id,
        created_outcome,
        location.clone(),
        instance,
    );
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
            let target = LaunchRequest {
                pane_id: pane,
                executable: params.executable.clone(),
                argv: params.argv.clone(),
                spawn_token: Some(edge.token.clone()),
                cwd: params.cwd.clone(),
                env: params.env.clone(),
            };
            let launch = match &tracking {
                Some((bootstrap, ticket)) => subjects::wrap(bootstrap, ticket, target),
                None => target,
            };
            backend.launch(&launch, &cancel)
        });
    let (launched_outcome, launch_message) = match &launched {
        Some(LaunchOutcome::Completed) => (Some(RequestOutcome::Completed), None),
        Some(LaunchOutcome::Refused(_)) => (
            Some(RequestOutcome::Refused),
            Some("backend refused child launch".to_string()),
        ),
        Some(LaunchOutcome::Unknown(_)) => (
            Some(RequestOutcome::Unknown),
            Some("child launch outcome is unknown; it will not be retried".to_string()),
        ),
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

/// The bootstrap a tracked launch runs: this binary, serving `socket`. `None`
/// when either path is not text the launch argv can carry.
fn bootstrap_for(socket: &Path) -> Option<Bootstrap> {
    Some(Bootstrap {
        executable: std::env::current_exe().ok()?.to_str()?.to_string(),
        socket: socket.to_str()?.to_string(),
    })
}

/// Records the launch subject for a tracked spawn and attaches it to the edge,
/// returning what the launch needs to run through the bootstrap.
fn track_launch(
    core: &Arc<Mutex<Core>>,
    request_id: &str,
    launch: subjects::LaunchContext,
    session: Option<String>,
    now: i64,
) -> Result<(Bootstrap, subjects::Ticket), String> {
    let (registry, bootstrap) = {
        let core = locked(core);
        (Arc::clone(&core.registry), core.bootstrap.clone())
    };
    let bootstrap =
        bootstrap.ok_or("this daemon has no executable path a tracked launch can run through")?;
    let ticket = subjects::create(&registry, request_id, launch, session, now)?;
    registry.attach_subject(request_id, &ticket.subject_id)?;
    Ok((bootstrap, ticket))
}

fn requested_launch(params: &SpawnParams) -> subjects::LaunchContext {
    subjects::LaunchContext {
        executable: params.executable.clone(),
        argv: params.argv.clone(),
        cwd: params.cwd.clone(),
        env: params.env.clone(),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapParams {
    subject_id: String,
    correlation: String,
    process: ProcessIdentity,
}

/// Captures the process a tracked launch's bootstrap claims, and releases it to
/// run its program.
///
/// Released only when the subject is pending with this correlation, the pane the
/// edge recorded has this process as its foreground, the identity matches the
/// live process, and a pidfd can be opened on it. Every refusal reads the same
/// way, so a caller cannot probe which of those checks failed.
fn bootstrap_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let params: BootstrapParams = match decode_params(request) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (registry, backend, verifier, stopping) = {
        let core = locked(core);
        let Some(backend) = core.backend.clone() else {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "no backend is wired"),
            );
        };
        (
            Arc::clone(&core.registry),
            backend,
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let not_awaiting = || {
        Response::refused(
            &request.id,
            &Refusal::new(Code::Refused, subjects::NOT_AWAITING),
        )
    };
    if !backend
        .capabilities()
        .contains(&LIFECYCLE_TRACKING_CAPABILITY)
    {
        return not_awaiting();
    }
    let subject = match subjects::read(&registry, &params.subject_id) {
        Ok(Some(subject))
            if subject.state == subjects::State::Pending
                && subject.correlation.as_deref() == Some(params.correlation.as_str()) =>
        {
            subject
        }
        Ok(_) => return not_awaiting(),
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let edge = match registry.spawn_edge(&subject.spawn_request_id) {
        Ok(Some(edge)) => edge,
        Ok(None) => return not_awaiting(),
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let Some(pane) = edge
        .location
        .as_ref()
        .and_then(|location| location.pane.clone())
    else {
        return not_awaiting();
    };
    if let Err(refusal) = verify_live_occupant(
        &backend,
        &verifier,
        &pane,
        &params.process,
        &stopping,
        verification,
    ) {
        return Response::refused(&request.id, &refusal);
    }
    // The pidfd is held across the context read, so the read describes the
    // claimed process. A later stop reopens one from the record.
    let _pidfd = match process_control::acquire(&params.process, verifier.as_ref()) {
        Ok(Acquired::Process(pidfd)) => pidfd,
        Ok(Acquired::Absent) => return not_awaiting(),
        Ok(Acquired::Replaced) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::Refused, "process identity has changed"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Refused, error)),
    };
    let Ok(effective) = process_control::effective(params.process.pid) else {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "launch context is unavailable"),
        );
    };
    // The bootstrap runs no program before release, so its context reads describe
    // the claimed process only while that identity still holds.
    if !matches!(
        verifier.inspect(params.process.pid),
        Ok(Some(observed)) if observed == params.process
    ) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "foreground process birth identity is unavailable or changed",
            ),
        );
    }
    // The wake is installed before the launch is captured, so the program's exit
    // is noticed from the moment it can run. A launch whose wake cannot be
    // installed is never released.
    if let Err(message) = install_exit_wake(core, &backend, &params.subject_id, &edge, &stopping) {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, message),
        );
    }
    match subjects::capture(
        &registry,
        &params.subject_id,
        &params.correlation,
        &params.process,
        &effective,
        store::now_ms(),
    ) {
        Ok(()) => Response::ok(&request.id, json!({ "released": true })),
        Err(error) => {
            retire_wake(&backend, &params.subject_id, &stopping);
            Response::refused(&request.id, &Refusal::new(Code::Refused, error))
        }
    }
}

/// Installs the pane-death wake for a launch, naming the server generation its
/// pane was created in. A launch whose generation was not recorded cannot be
/// fenced, so it has no wake.
fn install_exit_wake(
    core: &Arc<Mutex<Core>>,
    backend: &Arc<dyn RuntimeProvider>,
    subject_id: &str,
    edge: &registry::SpawnEdge,
    stopping: &AtomicBool,
) -> Result<(), String> {
    let bootstrap = locked(core)
        .bootstrap
        .clone()
        .ok_or("this daemon has no executable path a tracked launch can run through")?;
    let instance = edge
        .instance
        .as_ref()
        .ok_or("the launch's server generation is unrecorded, so its exit cannot be notified")?;
    let command = subjects::wake_command(&bootstrap, subject_id, instance)?;
    backend.install_exit_wake(subject_id, &command, stopping)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectParams {
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default)]
    spawn_request_id: Option<String>,
}

/// Validates, records and dispatches a cancel or stop of a tracked launch.
///
/// The target is the exact subject, named directly or through the spawn that
/// created it. The record's target is the subject's pane, so a competing
/// mutation of that pane waits behind it. The record names each effect: launch
/// revocation (cancel only), signal dispatch, exit verification, and the pane an
/// unbound launch left behind.
fn execute_subject_control(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    operation: ops::Operation,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: SubjectParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`request` is not a valid subject request".to_string(),
                ),
            );
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let registry = Arc::clone(&locked(core).registry);
    let (subject_id, target) = match cancel_target(&registry, &params) {
        Ok(target) => target,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let now = store::now_ms();
    match admit(operation, request, Some(&target), &requester, now, core) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(operation, request, &message, Some(target), core);
        }
        Admission::Dispatch(..) => {}
    }
    let (verifier, backend, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.verifier),
            core.backend.clone(),
            Arc::clone(&core.stopping),
        )
    };
    let termination = match operation {
        ops::Operation::Stop => subjects::stop(&registry, &subject_id, verifier.as_ref(), now),
        _ => subjects::cancel(&registry, &subject_id, verifier.as_ref(), now),
    };
    // Every cancel asks about the launch's own pane, so a launch already cleaned
    // up reports that instead of closing anything again. A bound launch's pane
    // belongs to its child, and a stop leaves the launch exactly as it is.
    let cleanup = match (&termination, operation, &backend) {
        (Ok(_), ops::Operation::Cancel, Some(backend)) => cleanup_unbound_pane(
            &registry,
            backend,
            &verifier,
            &subject_id,
            &stopping,
            verification,
            now,
        ),
        _ => None,
    };
    let (outcome, category, message, effects) = match termination {
        Ok(termination) => {
            if let (subjects::Exit::Exited, Some(backend)) = (&termination.exit, &backend) {
                retire_wake(backend, &subject_id, &stopping);
            }
            termination_outcome(&termination, operation, cleanup.as_ref())
        }
        Err(error) => (RequestOutcome::Unknown, None, Some(error), Vec::new()),
    };
    let record = RequestRecord::new(
        &request.id,
        operation.as_str(),
        requester,
        Some(target),
        now,
    )
    .with_params(request.params.clone())
    .complete(
        outcome,
        category,
        message.as_deref(),
        effects,
        store::now_ms(),
    );
    write_settled(request, record, core)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollParams {
    agent_id: String,
    session: String,
}

/// What a lifecycle request reports: its outcome, category, message and effects.
type Settlement = (
    RequestOutcome,
    Option<&'static str>,
    Option<String>,
    Vec<String>,
);

fn refusal_settlement(message: impl Into<String>) -> Settlement {
    (
        RequestOutcome::Refused,
        Some(Category::BACKEND_REFUSED),
        Some(message.into()),
        Vec::new(),
    )
}

/// Enrolls a running lead the daemon did not launch as a restartable subject.
///
/// The named registration must claim a process that is verifiably the foreground of
/// its own pane, and its fresh session context must name the operator's session.
/// Nothing is signalled or relaunched: the enrollment records what was verified.
fn execute_enroll(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: EnrollParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`request` is not a valid enrollment".to_string(),
                ),
            );
        }
    };
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    if crate::model::SessionUuid::parse(&params.session).is_none() {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::BadParams,
                "`session` must be a canonical UUID".to_string(),
            ),
        );
    }
    let (registry, verifier, stopping) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let now = store::now_ms();
    let registration = match registry.get(&params.agent_id) {
        Ok(Some(registration)) => registration,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "registration is not recorded".to_string()),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let Some(pane) = registration
        .request
        .location
        .as_ref()
        .and_then(|location| location.pane.clone())
    else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "registration names no pane to enroll".to_string(),
            ),
        );
    };
    let Some(claim) = registration.request.process.clone() else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "registration claims no process to enroll".to_string(),
            ),
        );
    };
    match registry.context(&params.agent_id, now) {
        Ok(Some(context))
            if context.context.freshness == registry::Freshness::Fresh
                && context.context.session.as_deref() == Some(params.session.as_str()) => {}
        Ok(_) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::Refused,
                    "registration's fresh session context does not match the session".to_string(),
                ),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
    let (backend, _cancel) = match admit(
        ops::Operation::Enroll,
        request,
        Some(&pane),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(
                ops::Operation::Enroll,
                request,
                &message,
                Some(pane),
                core,
            );
        }
        Admission::Dispatch(backend, cancel) => (backend, cancel),
    };
    let record = RequestRecord::new(
        &request.id,
        ops::Operation::Enroll.as_str(),
        requester,
        Some(pane.clone()),
        now,
    )
    .with_params(request.params.clone());
    let enrolled =
        match verify_live_occupant(&backend, &verifier, &pane, &claim, &stopping, verification) {
            Err(refusal) => Err(refusal.message),
            Ok(()) => record_enrollment(&registry, &verifier, &params, &claim, &request.id, now),
        };
    let (outcome, category, message, effects) = match enrolled {
        Ok(subject_id) => (
            RequestOutcome::Completed,
            None,
            None,
            vec![format!("enrolled subject {subject_id}")],
        ),
        Err(message) => refusal_settlement(message),
    };
    write_settled(
        request,
        record.complete(
            outcome,
            category,
            message.as_deref(),
            effects,
            store::now_ms(),
        ),
        core,
    )
}

/// Records the enrolled subject for a verified process. The launch is read only
/// while the claimed birth identity still holds, so it describes the claimed process.
fn record_enrollment(
    registry: &registry::Registry,
    verifier: &Arc<dyn ProcessVerifier>,
    params: &EnrollParams,
    claim: &crate::model::ProcessIdentity,
    enrollment_id: &str,
    now: i64,
) -> Result<String, String> {
    let effective = process_control::effective(claim.pid)
        .map_err(|_| "launch context is unavailable".to_string())?;
    if !matches!(verifier.inspect(claim.pid), Ok(Some(observed)) if observed == *claim) {
        return Err("foreground process birth identity is unavailable or changed".into());
    }
    let launch = subjects::captured_launch(&effective)?;
    subjects::enroll_subject(
        registry,
        enrollment_id,
        &params.agent_id,
        claim,
        &params.session,
        launch,
        now,
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartParams {
    subject_id: String,
}

/// Restarts an enrolled or tracked lead: stops its verified process, then launches a
/// replacement that resumes its Pi session in a fresh pane split from the lead's own.
///
/// Refusals before the stop change nothing. The replacement is recorded as the old
/// process's successor before it is launched, so a replay under a new request never
/// launches a second one. Children and their immutable edges are left as they are.
fn execute_restart(
    request: &Request,
    core: &Arc<Mutex<Core>>,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let frozen = match frozen_params(request, &[]) {
        Ok(frozen) => frozen,
        Err(response) => return response,
    };
    let params: RestartParams = match serde_json::from_value(frozen) {
        Ok(params) => params,
        Err(_error) => {
            return Response::refused(
                &request.id,
                &Refusal::new(
                    Code::BadParams,
                    "`request` is not a valid restart request".to_string(),
                ),
            );
        }
    };
    if crate::model::SessionUuid::parse(&params.subject_id).is_none() {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BadParams, "`subject_id` must be a canonical UUID"),
        );
    }
    let requester = match request.optional_string("requester") {
        Ok(requester) => requester,
        Err(refusal) => return Response::refused(&request.id, &refusal),
    };
    let registry = Arc::clone(&locked(core).registry);
    let subject = match subjects::read(&registry, &params.subject_id) {
        Ok(Some(subject)) => subject,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "launch subject is not recorded".to_string()),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    // The anchor is the lead's own pane: a dead lead's pane stays addressable under
    // remain-on-exit, and a replacement is split from it rather than from a parent.
    let anchor = match subject.bound_agent_id.as_deref() {
        Some(agent) => match registry.get(agent) {
            Ok(Some(registration)) => registration
                .request
                .location
                .and_then(|location| location.pane),
            Ok(None) => None,
            Err(error) => {
                return Response::refused(&request.id, &Refusal::new(Code::Internal, error));
            }
        },
        None => None,
    };
    let Some(anchor) = anchor else {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "restart needs the lead's own recorded pane as its anchor".to_string(),
            ),
        );
    };
    let now = store::now_ms();
    let cancel = match admit(
        ops::Operation::Restart,
        request,
        Some(&subject.subject_id),
        &requester,
        now,
        core,
    ) {
        Admission::Answered(response) => return response,
        Admission::Unsupported(message) => {
            return execute_unsupported(
                ops::Operation::Restart,
                request,
                &message,
                Some(subject.subject_id.clone()),
                core,
            );
        }
        Admission::Dispatch(_, cancel) => cancel,
    };
    let record = RequestRecord::new(
        &request.id,
        ops::Operation::Restart.as_str(),
        requester,
        Some(subject.subject_id.clone()),
        now,
    )
    .with_params(request.params.clone());
    let (outcome, category, message, effects) =
        restart_lead(core, &subject, &anchor, &cancel, verification, now);
    write_settled(
        request,
        record.complete(
            outcome,
            category,
            message.as_deref(),
            effects,
            store::now_ms(),
        ),
        core,
    )
}

/// The restart's effects after admission, in order: the checks, the verified stop,
/// the successor record, then the replacement launch. Each stage reports on its own.
fn restart_lead(
    core: &Arc<Mutex<Core>>,
    subject: &subjects::Subject,
    anchor: &str,
    cancel: &AtomicBool,
    verification: &Arc<VerificationBudget>,
    now: i64,
) -> Settlement {
    let (registry, backend, verifier) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            core.backend.clone(),
            Arc::clone(&core.verifier),
        )
    };
    let Some(backend) = backend else {
        return refusal_settlement("no backend is wired");
    };
    if subject.state != subjects::State::Bound {
        return refusal_settlement("the subject is not bound to a live process");
    }
    let (Some(agent), Some(session)) = (
        subject.bound_agent_id.as_deref(),
        subject.session.as_deref(),
    ) else {
        return refusal_settlement("the subject has no session to resume; enroll it first");
    };
    if let Some(successor) = &subject.successor {
        return refusal_settlement(format!(
            "this process is already superseded by spawn {successor}"
        ));
    }
    match backend.inventory(cancel) {
        Ok(fleet)
            if fleet
                .panes
                .iter()
                .any(|pane| pane.location.pane_id == anchor) => {}
        Ok(_) => {
            return refusal_settlement(
                "the lead's pane is no longer present to anchor the replacement",
            );
        }
        Err(error) => {
            return (
                RequestOutcome::Refused,
                Some(Category::BACKEND_UNAVAILABLE),
                Some(error),
                Vec::new(),
            );
        }
    }
    match registry.context(agent, now) {
        Ok(Some(context)) if context.context.session.as_deref() == Some(session) => {}
        Ok(_) => {
            return refusal_settlement(
                "the lead's published session no longer matches its recipe; enroll it again",
            );
        }
        Err(error) => return refusal_settlement(error),
    }
    let resume = match subjects::resume_launch(&subject.launch, session) {
        Ok(resume) => resume,
        Err(message) => return refusal_settlement(message),
    };
    let params = SpawnParams {
        parent: agent.to_string(),
        parent_session_id: session.to_string(),
        executable: resume.executable,
        argv: resume.argv,
        cwd: resume.cwd,
        env: resume.env,
    };
    if let Err(message) = validate_command(&params.executable, &params.argv) {
        return refusal_settlement(message);
    }
    if let Err(message) = subjects::check_bounds(&requested_launch(&params)) {
        return refusal_settlement(message);
    }
    if let Err(refusal) = preflight_launch(core, &params) {
        return refusal_settlement(refusal.message);
    }
    let mut effects = Vec::new();
    let termination = match subjects::stop(&registry, &subject.subject_id, verifier.as_ref(), now) {
        Ok(termination) => termination,
        Err(error) => {
            return (
                RequestOutcome::Unknown,
                Some(Category::BACKEND_UNAVAILABLE),
                Some(error),
                effects,
            );
        }
    };
    if termination.signal_attempted {
        effects.push("signalled previous process".to_string());
    }
    match termination.exit {
        subjects::Exit::Exited => effects.push("previous process exited".to_string()),
        subjects::Exit::Unresolved => {
            return (
                RequestOutcome::Refused,
                Some(Category::BACKEND_REFUSED),
                Some(
                    "the previous process could not be identified; no replacement was launched"
                        .into(),
                ),
                effects,
            );
        }
        subjects::Exit::Unverified(reason) => {
            return (
                RequestOutcome::Unknown,
                Some(Category::BACKEND_UNAVAILABLE),
                Some(format!(
                    "previous process exit is not verified; no replacement was launched: {reason}"
                )),
                effects,
            );
        }
    }
    let successor = store::random_uuid();
    if let Err(error) =
        subjects::set_successor(&registry, &subject.subject_id, &successor, store::now_ms())
    {
        return (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some(error),
            effects,
        );
    }
    effects.push(format!("successor {successor}"));
    let intent = registry::SpawnRequest {
        request_id: successor.clone(),
        parent: params.parent.clone(),
        parent_session_id: params.parent_session_id.clone(),
    };
    let replacement = Request {
        id: successor.clone(),
        method: ops::Operation::Spawn.as_str().to_string(),
        params: serde_json::json!({
            "parent": params.parent,
            "parent_session_id": params.parent_session_id,
            "executable": params.executable,
            "argv": params.argv,
            "cwd": params.cwd,
            "env": params.env,
        }),
    };
    let _ = spawn_child(
        &replacement,
        core,
        verification,
        &params,
        &intent,
        anchor.to_string(),
        Some(session.to_string()),
    );
    match registry.spawn_edge(&successor) {
        Ok(Some(edge)) => match edge.launched {
            RequestOutcome::Completed => {
                effects.push("launched replacement".to_string());
                (RequestOutcome::Completed, None, None, effects)
            }
            RequestOutcome::Unknown => (
                RequestOutcome::Unknown,
                Some(Category::BACKEND_UNAVAILABLE),
                Some("replacement launch is unknown; it will not be retried".into()),
                effects,
            ),
            _ => (
                RequestOutcome::Refused,
                Some(Category::BACKEND_REFUSED),
                Some("replacement was not launched".into()),
                effects,
            ),
        },
        Ok(None) => (
            RequestOutcome::Refused,
            Some(Category::BACKEND_REFUSED),
            Some("replacement was not recorded".into()),
            effects,
        ),
        Err(error) => (
            RequestOutcome::Unknown,
            Some(Category::BACKEND_UNAVAILABLE),
            Some(error),
            effects,
        ),
    }
}

/// The subject a cancel or stop names and the target it is admitted under.
fn cancel_target(registry: &Registry, params: &SubjectParams) -> Result<(String, String), Refusal> {
    let internal = |error: String| Refusal::new(Code::Internal, error);
    let subject = match (&params.subject_id, &params.spawn_request_id) {
        (Some(subject_id), None) => subjects::read(registry, subject_id)
            .map_err(internal)?
            .ok_or_else(|| Refusal::new(Code::NotFound, "launch subject is not recorded"))?,
        (None, Some(request_id)) => {
            let edge = registry
                .spawn_edge(request_id)
                .map_err(internal)?
                .ok_or_else(|| Refusal::new(Code::NotFound, "spawn is not recorded"))?;
            let subject_id = edge.subject_id.clone().ok_or_else(|| {
                Refusal::new(Code::Refused, "spawn is not a lifecycle-tracked launch")
            })?;
            subjects::read(registry, &subject_id)
                .map_err(internal)?
                .ok_or_else(|| Refusal::new(Code::NotFound, "launch subject is not recorded"))?
        }
        _ => {
            return Err(Refusal::new(
                Code::BadParams,
                "exactly one of `subject_id` or `spawn_request_id` is required",
            ));
        }
    };
    Ok((subject.subject_id.clone(), subject.subject_id))
}

/// What a cancel's or stop's effects read as on its record: the outcome the caller
/// may rely on, the message for it, and each effect named separately. Only a
/// cancel names the pane, because only a cancel acts on it.
fn termination_outcome(
    termination: &subjects::Termination,
    operation: ops::Operation,
    cleanup: Option<&subjects::PaneCleanup>,
) -> (
    RequestOutcome,
    Option<&'static str>,
    Option<String>,
    Vec<String>,
) {
    let mut effects = Vec::new();
    if termination.revoked {
        effects.push("launch revoked".to_string());
    }
    if termination.signal_attempted {
        effects.push("sigterm dispatched".to_string());
    }
    if operation == ops::Operation::Cancel {
        match cleanup {
            Some(cleanup) => effects.push(cleanup.effect()),
            None => effects.push("pane kept".to_string()),
        }
    }
    match &termination.exit {
        subjects::Exit::Unresolved => {
            effects.push("process identity unresolved".to_string());
            (RequestOutcome::Completed, None, None, effects)
        }
        subjects::Exit::Exited => {
            effects.push("process exit verified".to_string());
            (RequestOutcome::Completed, None, None, effects)
        }
        subjects::Exit::Unverified(message) => (
            RequestOutcome::Unknown,
            None,
            Some(message.clone()),
            effects,
        ),
    }
}

/// Removes a launch's exit wake once its exit is recorded. Best effort: a slot
/// left behind names a launch that has ended, and the next wake removes it.
fn retire_wake(backend: &Arc<dyn RuntimeProvider>, subject_id: &str, stopping: &AtomicBool) {
    let _ = backend.remove_exit_wake(subject_id, stopping);
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeParams {
    subject_id: String,
    server: crate::model::ProcessIdentity,
}

/// Reconciles one launch after its pane's process may have died.
///
/// How long the recovery sweep waits between passes.
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);

/// Starts the bounded recovery sweep. It re-observes recorded launches on a fixed
/// interval, so a missed exit notification is recovered without any client, and
/// it stops with the daemon.
fn spawn_sweeper(
    core: &Arc<Mutex<Core>>,
    stopping: &Arc<AtomicBool>,
    verification: &Arc<VerificationBudget>,
) -> Option<JoinHandle<()>> {
    let (registry, verifier, sweep_at) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.verifier),
            Arc::clone(&core.sweep_at),
        )
    };
    let stopping = Arc::clone(stopping);
    let core = Arc::clone(core);
    let verification = Arc::clone(verification);
    thread::Builder::new()
        .name("radar-control-sweep".to_string())
        .spawn(move || {
            // What the last topology pass saw, so an unchanged server costs one
            // read and no record.
            let mut seen: Option<String> = None;
            while !stopping.load(Ordering::SeqCst) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_millis() as i64);
                match subjects::sweep(&registry, verifier.as_ref(), now) {
                    Ok(_) => sweep_at.store(now, Ordering::SeqCst),
                    Err(error) => tracing::error!(%error, "launch recovery sweep failed"),
                }
                if let Err(error) = adopt_topology(&core, &registry, &verification, &mut seen) {
                    tracing::error!(%error, "topology adoption failed");
                }
                let deadline = std::time::Instant::now() + SWEEP_INTERVAL;
                while std::time::Instant::now() < deadline && !stopping.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
            }
        })
        .ok()
}

/// Records the places of this daemon's own server as observed containers, and
/// reports the ones that have gone, when the server's places have changed since
/// the last pass.
///
/// Adoption is scoped to the one server the daemon serves, runs under a verified
/// serving incarnation, and dispatches nothing. A failed read leaves `seen`
/// as it was, so the next pass looks again.
fn adopt_topology(
    core: &Arc<Mutex<Core>>,
    registry: &Arc<Registry>,
    verification: &Arc<VerificationBudget>,
    seen: &mut Option<String>,
) -> Result<(), String> {
    let (backend, verifier, stopping) = {
        let core = locked(core);
        (
            core.backend.clone(),
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let Some(backend) = backend else {
        return Ok(());
    };
    // A creation records the container it made after the place exists. Adopting
    // in between would record the same place twice, so the pass waits for it.
    if locked(core)
        .store
        .unfinished()?
        .iter()
        .any(|record| record.method == "topology.create")
    {
        return Ok(());
    }
    let Some(observation) = observed_inventory(core) else {
        return Ok(());
    };
    let incarnation = serving_identity(&backend, &verifier, &stopping, verification);
    let inventory = containers::Inventory::from_observation(
        &observation,
        &backend.identity(),
        incarnation.clone(),
    );
    let signature = format!(
        "{:?}|{:?}|{:?}|{:?}",
        inventory.incarnation, inventory.workspaces, inventory.tabs, inventory.panes
    );
    if seen.as_deref() == Some(signature.as_str()) {
        return Ok(());
    }
    let at = store::format_millis(unix_millis());
    containers::adopt(
        registry,
        &observation,
        &backend.identity(),
        incarnation.as_ref(),
        &at,
    )?;
    containers::reconcile(registry, &inventory, &at, MAX_RECONCILE)?;
    *seen = Some(signature);
    Ok(())
}

/// A wake is honoured only from the server generation that created the pane and
/// is serving now, so a pane id reused by a replaced server is not this launch's
/// pane. It records an exit only when pidfd verification shows the process gone.
fn wake_request(
    core: &Arc<Mutex<Core>>,
    request: &Request,
    verification: &Arc<VerificationBudget>,
) -> Response {
    let params: WakeParams = match decode_params(request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let (registry, backend, verifier, stopping) = {
        let core = locked(core);
        let Some(backend) = core.backend.clone() else {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::BackendUnavailable, "no backend is wired"),
            );
        };
        (
            Arc::clone(&core.registry),
            backend,
            Arc::clone(&core.verifier),
            Arc::clone(&core.stopping),
        )
    };
    let subject = match subjects::read(&registry, &params.subject_id) {
        Ok(Some(subject)) => subject,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "launch subject is not recorded"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let edge = match registry.spawn_edge(&subject.spawn_request_id) {
        Ok(Some(edge)) => edge,
        Ok(None) => {
            return Response::refused(
                &request.id,
                &Refusal::new(Code::NotFound, "spawn is not recorded"),
            );
        }
        Err(error) => return Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    };
    let serving = serving_identity(&backend, &verifier, &stopping, verification);
    if edge.instance.as_ref() != Some(&params.server) || serving.as_ref() != Some(&params.server) {
        return Response::refused(
            &request.id,
            &Refusal::new(
                Code::Refused,
                "wake names a server generation that does not serve this launch",
            ),
        );
    }
    match subjects::observe_exit_from(
        &registry,
        &params.subject_id,
        verifier.as_ref(),
        store::now_ms(),
        subjects::Observer::TmuxHook,
    ) {
        Ok(exited) => {
            if exited {
                retire_wake(&backend, &params.subject_id, &stopping);
            }
            Response::ok(&request.id, json!({ "exited": exited }))
        }
        Err(error) => Response::refused(&request.id, &Refusal::new(Code::Internal, error)),
    }
}

/// The location a create answered with, in the coordinates this daemon records.
///
/// The backend names itself, so a location recorded through one daemon is
/// distinguishable from the same coordinates read through another.
fn created_location(named: &CreatedLocation, backend: &BackendIdentity) -> RegistryLocation {
    RegistryLocation {
        backend: backend.name.clone(),
        instance: backend.instance.clone(),
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
            json!({ "request": public_record(record), "spawn": edge.public() }),
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
        (Ok(Some(edge)), Some(record)) => Response::ok(
            &id,
            json!({ "request": public_record(serde_json::from_value(record).expect("stored public record shape")), "spawn": edge.public() }),
        ),
        _ => Response::ok(&id, result),
    }
}

/// Writes one settled record and answers with it.
fn write_settled(request: &Request, record: RequestRecord, core: &Arc<Mutex<Core>>) -> Response {
    let core = locked(core);
    match core.store.write(&record) {
        Ok(()) => Response::ok(&request.id, json!({ "request": public_record(record) })),
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
    crate::control_plane::transport::verify_socket_directory(directory, "control")
}

/// Creates `directory` `0700` where it is missing, then requires it to pass
/// [`trusted_directory`].
///
/// This is the daemon's half of the rule: only a daemon may create the socket
/// directory, and it checks the same rule a client will check later.
fn prepare_directory(directory: &Path) -> Result<(), String> {
    crate::control_plane::transport::prepare_socket_directory(directory, "control")
}

/// Binds `path`, first clearing a socket file a dead daemon left behind.
///
/// A file that answers a connection belongs to a running daemon and is left
/// untouched — the caller is told it is already running rather than served a
/// second listener on one path; a refused connection means the file is stale and
/// it is removed.
fn bind_socket(path: &Path) -> Result<UnixListener, String> {
    crate::control_plane::transport::bind_socket(path, "control", "a daemon is already running")
}

/// The listener systemd passed this process for `socket`, when the socket unit
/// started it. The passed socket must be the only one, and must be bound to the
/// configured path; anything else is refused rather than served.
fn inherited_listener(socket: &Path) -> Result<Option<UnixListener>, String> {
    let for_us = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok())
        == Some(std::process::id());
    if !for_us {
        return Ok(None);
    }
    match std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|count| count.parse::<u32>().ok())
    {
        None => return Ok(None),
        Some(1) => {}
        Some(count) => {
            return Err(format!(
                "systemd passed {count} sockets; the control daemon serves exactly one"
            ));
        }
    }
    // SAFETY: systemd's socket activation contract passes the first listener at
    // descriptor 3 to the process it names in LISTEN_PID, and this branch runs
    // only when that names this process. Nothing else in the process owns fd 3.
    let listener = unsafe { <UnixListener as std::os::fd::FromRawFd>::from_raw_fd(3) };
    let bound = listener
        .local_addr()
        .map_err(|error| format!("control socket passed by systemd: {error}"))?;
    if bound.as_pathname() != Some(socket) {
        return Err(format!(
            "control socket passed by systemd is bound to {}, not {}",
            bound
                .as_pathname()
                .map_or_else(|| "no path".to_string(), |path| path.display().to_string()),
            socket.display()
        ));
    }
    Ok(Some(listener))
}

/// Removes the socket pathname only when it still names the inode this daemon
/// bound. A replacement daemon or administrator-created path is left untouched,
/// and a socket this daemon did not bind (`None`) is never removed.
fn remove_socket_if_owned(path: &Path, identity: Option<(u64, u64)>) -> Result<(), String> {
    let Some(identity) = identity else {
        return Ok(());
    };
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
    use std::os::unix::fs::PermissionsExt;

    fn path_value(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    fn event(seq: u64) -> store::Event {
        store::Event {
            subject_id: None,
            lifecycle_event_id: None,
            seq,
            kind: "agent.registered".to_string(),
            agent_id: Some(format!("agent-{seq}")),
            spawn: None,
            request_id: None,
            at: "2026-01-01T00:00:00.000Z".to_string(),
        }
    }

    /// A prefix the limit or the byte bound cut short still says where to resume:
    /// the last event it delivered, never the journal head.
    #[test]
    fn a_bounded_event_prefix_resumes_after_the_last_event_it_delivered() {
        let events = vec![event(1), event(2), event(3), event(4)];
        let (delivered, cursor) = deliverable(events.clone(), 0, 2, MAX_EVENTS_RESPONSE_BYTES);
        assert_eq!(sequences(&delivered), vec![1, 2]);
        assert_eq!(cursor, 2);

        // The same bound in bytes: the events that fit are delivered, and the
        // one that did not is what the next read starts from.
        let one = serde_json::to_vec(&event(1)).expect("an event").len();
        let (delivered, cursor) = deliverable(events.clone(), 0, 500, one * 2 + 1);
        assert_eq!(sequences(&delivered), vec![1, 2]);
        assert_eq!(cursor, 2);

        // Nothing delivered at all: the request's own `after`, so a resumed read
        // repeats the position rather than skipping past it.
        let (delivered, cursor) = deliverable(events.clone(), 1, 500, one - 1);
        assert!(delivered.is_empty());
        assert_eq!(cursor, 1);

        let (delivered, cursor) = deliverable(events, 2, 500, MAX_EVENTS_RESPONSE_BYTES);
        assert_eq!(sequences(&delivered), vec![3, 4]);
        assert_eq!(cursor, 4);
    }

    fn sequences(events: &[store::Event]) -> Vec<u64> {
        events.iter().map(|event| event.seq).collect()
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
    fn an_inherited_socket_path_is_never_unlinked_and_a_bound_one_is() {
        let directory = std::env::temp_dir().join(format!(
            "radar-socket-owner-{}-{}",
            std::process::id(),
            store::random_uuid()
        ));
        std::fs::create_dir_all(&directory).expect("a directory");
        let path = directory.join("control.sock");
        let listener = UnixListener::bind(&path).expect("a socket");
        let metadata = std::fs::symlink_metadata(&path).expect("metadata");
        let identity = (metadata.dev(), metadata.ino());

        // A socket systemd passed is the unit's to remove: this daemon keeps out
        // of its way and leaves the path for the socket unit.
        remove_socket_if_owned(&path, None).expect("no-op");
        assert!(path.exists(), "an inherited socket path was unlinked");

        // The socket this daemon bound is removed on the way out.
        remove_socket_if_owned(&path, Some(identity)).expect("removed");
        assert!(!path.exists(), "a bound socket path was left behind");
        drop(listener);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_physical_backend_is_the_configured_one_and_herdr_only_when_named() {
        let value = std::ffi::OsStr::new;
        let file = crate::config::DaemonConfig::default();
        // Unset and empty fall back to the configured backend, which is tmux on
        // the `radar` server by default.
        for backend in [None, Some(value(""))] {
            assert_eq!(
                physical_backend_with(backend, None, &file),
                Ok(PhysicalBackend::Tmux {
                    server: "radar".to_string()
                }),
                "{backend:?}"
            );
        }
        assert_eq!(
            physical_backend_with(Some(value(" Herdr ")), None, &file),
            Ok(PhysicalBackend::Herdr)
        );
    }

    #[test]
    fn the_environment_names_the_server_and_the_file_supplies_it_when_unset() {
        let value = std::ffi::OsStr::new;
        let file = crate::config::DaemonConfig {
            backend: crate::config::ControlBackend::Tmux,
            tmux_server: "work".to_string(),
        };
        assert_eq!(
            physical_backend_with(None, None, &file),
            Ok(PhysicalBackend::Tmux {
                server: "work".to_string()
            })
        );
        assert_eq!(
            physical_backend_with(Some(value("tmux")), Some(value(" probe ")), &file),
            Ok(PhysicalBackend::Tmux {
                server: "probe".to_string()
            })
        );
    }

    #[test]
    fn an_empty_server_name_never_reaches_tmux_default() {
        let value = std::ffi::OsStr::new;
        let file = crate::config::DaemonConfig {
            backend: crate::config::ControlBackend::Tmux,
            tmux_server: "   ".to_string(),
        };
        assert_eq!(
            physical_backend_with(Some(value("tmux")), Some(value("")), &file),
            Err(BackendConfigError::TmuxServerMissing)
        );
    }

    #[test]
    fn an_unknown_physical_backend_names_itself() {
        assert_eq!(
            physical_backend_with(
                Some(std::ffi::OsStr::new(" Docker ")),
                None,
                &crate::config::DaemonConfig::default()
            ),
            Err(BackendConfigError::UnknownBackend {
                name: "docker".to_string()
            })
        );
    }

    #[test]
    fn a_name_that_is_not_text_is_refused_rather_than_the_default() {
        use std::os::unix::ffi::OsStrExt;
        // Nothing that is not a backend name may be read as the default: an
        // unreadable name is still a name that has to be refused.
        assert!(matches!(
            physical_backend_with(
                Some(std::ffi::OsStr::from_bytes(b"tmux\xff")),
                Some(std::ffi::OsStr::new("probe")),
                &crate::config::DaemonConfig::default()
            ),
            Err(BackendConfigError::UnknownBackend { .. })
        ));
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
    #[test]
    fn daemon_ingests_publishers_and_serves_the_volatile_task_read_on_one_transport() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::unix::net::UnixStream as StdUnixStream;

        fn call(socket: &Path, method: &str, params: serde_json::Value) -> serde_json::Value {
            let mut stream = StdUnixStream::connect(socket).expect("connect to control socket");
            serde_json::to_writer(
                &mut stream,
                &json!({
                    "version": PROTOCOL_VERSION,
                    "id": store::random_uuid(),
                    "method": method,
                    "params": params,
                }),
            )
            .expect("write request");
            stream.write_all(b"\n").expect("terminate request");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("read response");
            serde_json::from_str(&line).expect("response JSON")
        }

        let root = trust_root();
        let control = root.join("control.sock");
        let publisher = root.join("publisher.sock");
        let state = root.join("state");
        let mut daemon = Daemon::bind_with_backend_and_verifier_and_ingest(
            &control,
            &state,
            Some(Arc::new(
                crate::tmux::TmuxRuntime::new(crate::tmux::TmuxConfig {
                    executable: PathBuf::from("tmux"),
                    server: "test".to_string(),
                    command_timeout: Duration::from_secs(1),
                })
                .expect("valid tmux adapter"),
            )),
            Arc::new(LocalProcfsVerifier),
            Some(&publisher),
        )
        .expect("bind daemon and publisher listener");
        let ping = call(&control, "daemon.ping", json!({}));
        assert!(
            ping["result"]["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == ops::TASKS_LIST_CONTRACT)
        );

        let event_tail = call(&control, "events.read", json!({"after": 0, "limit": 500}))["result"]
            ["tail"]
            .as_u64()
            .expect("read event tail");
        let mut publisher_stream = StdUnixStream::connect(&publisher).expect("connect publisher");
        publisher_stream
            .write_all(
                b"{\"type\":\"hello\",\"v\":1,\"session\":\"pi-session-a\",\"pane\":\"%8\"}\n",
            )
            .expect("publish hello");
        publisher_stream
            .write_all(b"{\"type\":\"tasks\",\"tasks\":[]}\n")
            .expect("publish task snapshot");

        let page = call(&control, "tasks.list", json!({"limit": 10}));
        assert!(page["result"]["revision"].as_u64().is_some());
        assert_eq!(page["result"]["sessions"][0]["session"], "pi-session-a");
        assert_eq!(page["result"]["sessions"][0]["connected"], true);
        assert_eq!(page["result"]["sessions"][0]["tasks"], json!([]));
        assert!(
            page["result"]["sessions"][0]["last_updated_at"]
                .as_u64()
                .is_some()
        );
        let client = crate::control_plane::client::DaemonRuntime::handshake(&control)
            .expect("dashboard client handshake");
        let snapshot = client.task_snapshot().expect("dashboard task snapshot");
        assert!(snapshot.revision > 0);
        assert!(
            matches!(snapshot.events.first(), Some(crate::bus::BusEvent::Connected { session, .. }) if session == "pi-session-a")
        );

        publisher_stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("set replacement observation timeout");
        let mut replacement = StdUnixStream::connect(&publisher).expect("reconnect publisher");
        replacement
            .write_all(b"{\"type\":\"hello\",\"v\":1,\"session\":\"pi-session-a\"}\n")
            .expect("publish replacement hello");
        replacement
            .write_all(
                b"{\"type\":\"tasks\",\"tasks\":[{\"id\":\"fresh\",\"state\":\"running\"}]}\n",
            )
            .expect("publish replacement snapshot");
        let mut byte = [0u8; 1];
        assert_eq!(
            publisher_stream
                .read(&mut byte)
                .expect("observe replaced connection"),
            0
        );
        let page = call(&control, "tasks.list", json!({"limit": 10}));
        assert_eq!(page["result"]["sessions"][0]["tasks"][0]["id"], "fresh");

        let events = call(
            &control,
            "events.read",
            json!({"after": event_tail, "limit": 500}),
        );
        assert_eq!(events["result"]["events"], json!([]));
        assert_eq!(events["result"]["tail"], event_tail);

        replacement
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("set disconnect observation timeout");
        replacement
            .write_all(b"{malformed json}\n")
            .expect("write malformed publisher line");
        assert_eq!(
            replacement.read(&mut byte).expect("observe protocol close"),
            0
        );
        let page = call(&control, "tasks.list", json!({"limit": 10}));
        assert_eq!(page["result"]["sessions"], json!([]));
        daemon.stop();
        let _ = std::fs::remove_dir_all(root);
    }

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
