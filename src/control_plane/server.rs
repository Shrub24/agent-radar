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
//! The daemon speaks only the normalized [`RuntimeProvider`] seam. tmux
//! transport and decoding remain inside the adapter.

use std::io::{ErrorKind, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::control_plane::communication::{self, CommunicationStore};
use crate::control_plane::containers;
use crate::control_plane::lifecycle::{self as subjects, Bootstrap};
use crate::control_plane::ops::{self, Method};
use crate::control_plane::protocol::{self, Code, PROTOCOL_VERSION, Refusal, Request, Response};
use crate::control_plane::public_contracts::observation::{
    ObservationGetParams, ObservationGetResponse,
};
use crate::control_plane::public_contracts::{PingResult, TaskLaneStatus};
use crate::control_plane::reconcile;
use crate::control_plane::registry::{
    self, AcquireRequest, Channel, CloseAdmission, CloseOutcome, CloseResult, LocalProcfsVerifier,
    LocationEvidence, ProcessVerifier, RegistrationRequest, Registry, RegistryLocation, SpawnEdge,
};
use crate::control_plane::store::{self, Category, RequestOutcome, RequestRecord, Store};
use crate::control_plane::transport::{
    Dispatch, EventReady, IngestDispatch, ListenerSource, Transport,
};
use crate::lifecycle::{self, CloseRequest, DirectClose};
use crate::model::SessionUuid;
use crate::model::{FleetObservation, ProcessIdentity};
use crate::process_control::{self, Acquired};
use crate::runtime::{
    BackendIdentity, CreateOutcome, CreateRequest, CreatedKind, CreatedLocation, FocusOutcome,
    InputOutcome, InputPayload, InputRequest, LaunchOutcome, LaunchRequest, OutputOutcome,
    OutputRead, OutputRequest, PaneExitObservation, Primitives, ReportOutcome, ReportRequest,
    RuntimeProvider, SplitDirection, Target, validate_command,
};
use crate::{TmuxConfig, TmuxRuntime};

mod admission;
mod child_close;
mod handoff;
mod input;
mod launch;
mod places;
mod reads;
mod registration;
mod subject_control;
mod subject_reads;
mod sweeper;
mod verification;
use admission::*;
use child_close::*;
use handoff::*;
use input::*;
use launch::*;
use places::*;
use reads::*;
use registration::*;
use subject_control::*;
use sweeper::*;
use verification::*;

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
/// Set by the signal handlers [`install_signal_handlers`] registers, and read by
/// the serve loop, which is at most one poll away from noticing. Only the daemon
/// process installs handlers; a library user that never calls
/// [`install_signal_handlers`] is never interrupted by this.
static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Asks the daemon to stop. The signal is the operator's; this is the same
/// request from inside the process, and it is what [`install_signal_handlers`]
/// sets from the signal handler.
pub fn interrupted() -> bool {
    INTERRUPTED
        .get()
        .is_some_and(|flag| flag.load(Ordering::SeqCst))
}

/// Arranges for `SIGINT` and `SIGTERM` to stop the daemon.
///
/// Called by the `daemon` subcommand and by nothing else: a process that has not
/// asked for this must not have its signals hijacked. A client that hung up
/// mid-response cannot kill the daemon with `SIGPIPE`, because the Rust runtime
/// already ignores it.
pub fn install_signal_handlers() -> Result<(), String> {
    let flag = INTERRUPTED.get_or_init(Arc::default);
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(flag))
            .map_err(|error| format!("cannot handle signal {signal}: {error}"))?;
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
                write!(f, "control backend `{name}`: not a backend; use `tmux`")
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
        },
    };
    match name.as_str() {
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
    /// implements ([`RuntimeProvider::primitives`]); nothing here restates
    /// them. An adapter that cannot serve the configuration it was given is a
    /// startup refusal, never a backend that quietly serves another one.
    fn adapter(&self) -> Result<Arc<dyn RuntimeProvider>, BackendConfigError> {
        match self {
            Self::Tmux { server } => TmuxRuntime::new(TmuxConfig::for_server(server))
                .map(|runtime| Arc::new(runtime) as Arc<dyn RuntimeProvider>)
                .map_err(|problem| BackendConfigError::AdapterConfiguration {
                    backend: "tmux",
                    problem,
                }),
        }
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
    communications: Arc<CommunicationStore>,
    task_ingest: Option<Arc<crate::control_plane::ingest::TaskIngest>>,
    task_ingest_error: Option<String>,
    backend: Option<Arc<dyn RuntimeProvider>>,
    verifier: Arc<dyn ProcessVerifier>,
    stopping: Arc<AtomicBool>,
    /// The binary and socket a tracked launch starts as `radar run --launch`. `None` when this process has
    /// no executable path it can name, which refuses tracked launches.
    bootstrap: Option<Bootstrap>,
    /// Unix milliseconds of the last completed recovery sweep pass; 0 before one.
    sweep_at: Arc<std::sync::atomic::AtomicI64>,
    /// The daemon's shared observation sampler. `None` when no backend is wired:
    /// there is nothing to observe, and the read refuses rather than publishing
    /// an empty fleet as a successful one.
    hub: Option<Arc<crate::control_plane::observation::Hub>>,
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
        let PhysicalBackend::Tmux { server } = &backend;
        crate::service::ensure_tmux_server(server)?;
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

    /// Binds `socket`, keeping records under `state`, creating and verifying both
    /// directories.
    ///
    /// `Err` is the diagnostic for a control plane that must not serve: a socket
    /// directory that is a symlink, foreign-owned or not `0700`, a socket file
    /// another daemon answers on, a bind that failed, or a state directory that
    /// failed the same checks.
    pub fn bind_at(socket: &Path, state: &Path) -> Result<Self, String> {
        let backend = Arc::new(
            TmuxRuntime::new(TmuxConfig::for_server(crate::config::DEFAULT_TMUX_SERVER))
                .expect("the documented tmux defaults are usable"),
        );
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
        let communications = match CommunicationStore::open(state) {
            Ok(store) => Arc::new(store),
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
        // The sampler observes the same backend the reads do, and is started
        // below once `Core` exists to hold it.
        let hub_backend = backend.clone();
        let hub_tasks = task_ingest.clone();
        let hub_task_error = task_ingest_error.clone();
        let hub_store = store.clone();
        let verification = Arc::new(VerificationBudget {
            active: Mutex::new(0),
            workers: Mutex::new(Vec::new()),
        });
        let bootstrap = bootstrap_for(socket);
        let registry = Arc::new(registry);
        let core = Arc::new(Mutex::new(Core {
            store,
            registry: Arc::clone(&registry),
            communications,
            task_ingest: task_ingest.clone(),
            task_ingest_error,
            backend,
            verifier,
            stopping: Arc::clone(&stopping),
            sweep_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            bootstrap,
            hub: None,
        }));
        // One sampler for every display, sampling only while a reader asks. The
        // lifecycle sweep keeps its own coverage, so a daemon with no display
        // attached still observes what its managed state needs.
        if let Some(provider) = hub_backend {
            let hub = Arc::new(crate::control_plane::observation::Hub::start(
                provider,
                Arc::clone(&registry),
                hub_tasks,
                hub_task_error,
                hub_store,
                &stopping,
            ));
            locked(&core).hub = Some(hub);
        }
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
        // One reconciliation at start, because no external baseline carries over
        // from the last daemon: durable operations may be unresolved and nothing
        // has established their effects yet. It dispatches nothing, and it is not
        // the periodic pass — the sweeper above is that.
        {
            let (store, registry, verifier) = {
                let core = locked(&core);
                (
                    core.store.clone(),
                    Arc::clone(&core.registry),
                    Arc::clone(&core.verifier),
                )
            };
            match reconcile::unresolved(
                &store,
                &registry,
                verifier.as_ref(),
                reconcile::Trigger::DaemonStart,
                store::now_ms(),
            ) {
                Ok(outcome) if outcome.resolved > 0 || !outcome.unresolved.is_empty() => {
                    tracing::info!(
                        examined = outcome.examined,
                        resolved = outcome.resolved,
                        unresolved = outcome.unresolved.len() + outcome.omitted,
                        "reconciled unresolved operations at start"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "startup reconciliation failed"),
            }
        }
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

    /// Stops accepting, lets the requests already being served answer, ends
    /// every connection, joins their threads, and removes the socket file this
    /// daemon created. Idempotent.
    ///
    /// Records are left exactly where they are: they are the only account of what
    /// was asked for, and a daemon that removed them on the way out would make
    /// "did this run" unanswerable.
    pub fn stop(&mut self) {
        let Some(mut transport) = self.transport.take() else {
            return;
        };
        self.stopping.store(true, Ordering::SeqCst);
        // The transport drains first and cuts second: a request already being
        // served answers, and a request that arrives while stopping is refused
        // without being dispatched.
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
            let (identity, contracts, task_status) = {
                let core = locked(core);
                let (identity, backend) = match &core.backend {
                    Some(backend) => (backend.identity(), backend.primitives()),
                    None => (BackendIdentity::named("none"), Primitives::NONE),
                };
                let available = core.task_ingest.is_some();
                (
                    identity,
                    ops::advertised_with_tasks(backend, available)
                        .into_iter()
                        .map(|contract| ops::Contract::as_str(contract).to_string())
                        .collect::<Vec<_>>(),
                    TaskLaneStatus {
                        available,
                        error: core.task_ingest_error.clone(),
                    },
                )
            };
            let result = PingResult {
                protocol: PROTOCOL_VERSION,
                revision: ops::PROTOCOL_REVISION,
                backend: identity.name,
                instance: identity.instance,
                contracts,
                tasks: task_status,
            };
            answer(
                writer,
                Response::ok(
                    &request.id,
                    serde_json::to_value(result).expect("ping result is JSON-serializable"),
                ),
            );
        }
        Method::Request => answer(writer, read_one(core, &request)),
        Method::Requests => answer(writer, read_many(core, &request)),
        Method::Events => answer(writer, read_events(core, &request, stopping)),
        Method::Observation => answer(writer, read_observation(core, &request)),
        Method::Communication => answer(writer, communication_request(core, &request)),
        Method::Tasks => answer(writer, read_tasks(core, &request)),
        Method::Registry => answer(writer, registry_request(core, &request, verification)),
        Method::Claim => answer(writer, claim_request(core, &request, verification)),
        Method::Admit => answer(writer, admit_request(core, &request, verification)),
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
struct ProcessGetParams {
    registration_id: String,
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
struct AgentRetireParams {
    registration_id: String,
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
    /// Where the previous pass stopped, so successive passes cover every recorded
    /// container rather than re-examining the first page.
    #[serde(default)]
    after: Option<String>,
}

/// Unix milliseconds now.
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Where lifecycle reads get their observation evidence: the observation
/// interval and its last completed pass. No pass yet is reported as null, never
/// as success.
///
/// This is steady-state observation, not reconciliation: it runs continuously so
/// lifecycle coverage stays alive with no client connected. Reconciliation is a
/// separate mechanism with its own triggers (`reconcile`).
fn observation_source(core: &Arc<Mutex<Core>>) -> serde_json::Value {
    let last = locked(core).sweep_at.load(Ordering::SeqCst);
    json!({"observation": {
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

fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// A read the backend does not implement refuses explicitly, naming the
/// primitive, rather than answering with a plausible-looking empty result.
fn primitive_refuse(backend: &Arc<dyn RuntimeProvider>, method: &str) -> Option<Refusal> {
    let needed = ops::read_primitives(method);
    needed.missing_name(backend.primitives()).map(|primitive| {
        let (code, message) = ops::read_unsupported(method, primitive);
        Refusal::new(code, message)
    })
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
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fn path_value(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    /// A control naming a claim the subject has moved past targets nothing: the
    /// stale id is refused before any signal, so a reply that arrives late cannot
    /// end the successor the operator never saw.
    #[test]
    fn a_control_naming_a_stale_claim_signals_nothing() {
        let root = std::env::temp_dir().join(format!(
            "radar-stale-claim-{}",
            crate::control_plane::random_uuid()
        ));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .expect("scratch root");
        let registry = Registry::open(&root).expect("registry");
        let claim = crate::control_plane::lifecycle::create(
            &registry,
            "01a10c77-8a6b-7035-8a0e-b1fa607bb507",
            crate::control_plane::lifecycle::LaunchContext {
                executable: "/bin/true".into(),
                argv: Vec::new(),
                cwd: None,
                env: std::collections::BTreeMap::new(),
            },
            None,
            1,
        )
        .expect("claim");
        let missing = json!({"subject_id": claim.subject_id});
        assert!(serde_json::from_value::<SubjectParams>(missing.clone()).is_err());
        let named = serde_json::from_value::<RestartParams>(missing).expect("shape");
        assert!(
            named.parent_launch().is_err(),
            "a restart must be fenced or owned"
        );
        let stale = SubjectParams {
            subject_id: Some(claim.claim_id.clone()),
            claim_id: crate::control_plane::random_uuid(),
            spawn_request_id: None,
        };
        let refusal = cancel_target(&registry, &stale).expect_err("a stale claim is refused");
        assert!(refusal.message.contains("not current"), "{refusal:?}");
        let current = SubjectParams {
            subject_id: Some(claim.claim_id.clone()),
            claim_id: claim.claim_id.clone(),
            spawn_request_id: None,
        };
        assert_eq!(
            cancel_target(&registry, &current)
                .expect("the current claim is admitted")
                .0,
            claim.claim_id
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn identity(pid: i32) -> crate::model::ProcessIdentity {
        crate::model::ProcessIdentity {
            boot_id: "boot".to_string(),
            pid,
            start_ticks: pid as u64,
        }
    }

    /// A pane recorded under a server that no longer serves is provably gone: a
    /// pane cannot outlive the process that owned it. Silence and an unreachable
    /// server claim nothing, so neither counts.
    #[test]
    fn a_pane_recorded_under_an_ended_server_generation_is_gone() {
        assert!(generation_ended(Some(&identity(10)), Some(&identity(11))));
        assert!(!generation_ended(Some(&identity(10)), Some(&identity(10))));
        assert!(!generation_ended(Some(&identity(10)), None));
        assert!(!generation_ended(None, Some(&identity(11))));
    }

    fn event(seq: u64) -> store::Event {
        store::Event {
            subject_id: None,
            claim_id: None,

            lifecycle_event_id: None,
            seq,
            kind: "agent.registered".to_string(),
            registration_id: Some(format!("agent-{seq}")),
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
    fn the_physical_backend_is_the_configured_one_and_only_named_names_serve() {
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
            physical_backend_with(Some(value(" Tmux ")), None, &file),
            Ok(PhysicalBackend::Tmux {
                server: "radar".to_string()
            })
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
            ping["result"]["contracts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == ops::Contract::TasksList.as_str())
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
