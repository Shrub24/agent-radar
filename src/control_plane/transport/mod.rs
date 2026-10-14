//! Bounded local-socket transport for daemon request lanes.

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdListener, UnixStream as StdStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;

use super::protocol::{self, Code, Refusal, Response};

const MAX_CONNECTIONS: usize = 32;
const CONTROL_LIMIT: usize = 8;
const INGEST_LIMIT: usize = 4;
const WAITING_LIMIT: usize = 8;
const MAX_BLOCKING_HANDLERS: usize = CONTROL_LIMIT + INGEST_LIMIT + WAITING_LIMIT;
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_RECHECK: Duration = Duration::from_millis(100);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Create and verify the private directory used by a daemon-owned socket.
pub(crate) fn prepare_socket_directory(directory: &Path, kind: &str) -> Result<(), String> {
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| format!("{kind} socket directory {}: {error}", directory.display()))?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("{kind} socket directory {}: {error}", directory.display()))?;
    }
    verify_socket_directory(directory, kind)
}

/// Verify a socket directory without creating or changing it.
pub(crate) fn verify_socket_directory(directory: &Path, kind: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("{kind} socket directory {}: {error}", directory.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{kind} socket directory {} is a symlink",
            directory.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "{kind} socket directory {} is not a directory",
            directory.display()
        ));
    }
    let uid = rustix::process::getuid().as_raw();
    if metadata.uid() != uid {
        return Err(format!(
            "{kind} socket directory {} is owned by uid {}",
            directory.display(),
            metadata.uid()
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(format!(
            "{kind} socket directory {} is not mode 0700",
            directory.display()
        ));
    }
    Ok(())
}

/// Bind a private Unix socket, clearing only a stale socket file.
pub(crate) fn bind_socket(path: &Path, kind: &str, occupied: &str) -> Result<StdListener, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Err(format!(
                "{kind} socket {} exists and is not a socket",
                path.display()
            ));
        }
        Ok(_) => match StdStream::connect(path) {
            Ok(_) => return Err(format!("{kind} socket {}: {occupied}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)
                    .map_err(|error| format!("{kind} socket {}: {error}", path.display()))?;
            }
            Err(error) => return Err(format!("{kind} socket {}: {error}", path.display())),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{kind} socket {}: {error}", path.display())),
    }
    StdListener::bind(path).map_err(|error| format!("{kind} socket {}: {error}", path.display()))
}

pub(crate) type Dispatch = dyn Fn(Vec<u8>, u64, Arc<Mutex<StdStream>>) + Send + Sync + 'static;
pub(crate) type IngestDispatch = dyn Fn(Vec<u8>, u64) -> IngestResult + Send + Sync + 'static;
pub(crate) struct IngestResult {
    pub(crate) keep_connection: bool,
    pub(crate) close_connections: Vec<u64>,
}
pub(crate) type Disconnect = dyn Fn(u64) + Send + Sync + 'static;
pub(crate) type EventReady = dyn Fn(&[u8]) -> bool + Send + Sync + 'static;

#[derive(Clone)]
pub(crate) struct EventNotification {
    sender: watch::Sender<(u64, bool)>,
}

#[derive(Clone)]
pub(crate) struct EventSignal {
    receiver: watch::Receiver<(u64, bool)>,
}

pub(crate) fn event_notifications() -> (EventNotification, EventSignal) {
    let (sender, receiver) = watch::channel((0, false));
    (EventNotification { sender }, EventSignal { receiver })
}

impl EventNotification {
    pub(crate) fn notify(&self) {
        self.sender
            .send_modify(|state| state.0 = state.0.wrapping_add(1));
    }

    pub(crate) fn stop(&self) {
        self.sender.send_modify(|state| {
            state.0 = state.0.wrapping_add(1);
            state.1 = true;
        });
    }
}

#[derive(Clone)]
pub(crate) enum Service {
    Control {
        dispatch: Arc<Dispatch>,
        signal: EventSignal,
        ready: Arc<EventReady>,
    },
    Ingest {
        dispatch: Arc<IngestDispatch>,
        disconnect: Arc<Disconnect>,
    },
}

pub(crate) struct ListenerSource {
    listener: StdListener,
    service: Service,
    event_notification: Option<EventNotification>,
}

impl ListenerSource {
    pub(crate) fn control(
        listener: StdListener,
        dispatch: Arc<Dispatch>,
        signal: EventSignal,
        notification: EventNotification,
        ready: Arc<EventReady>,
    ) -> Self {
        Self {
            listener,
            service: Service::Control {
                dispatch,
                signal,
                ready,
            },
            event_notification: Some(notification),
        }
    }

    pub(crate) fn ingest(
        listener: StdListener,
        dispatch: Arc<IngestDispatch>,
        disconnect: Arc<Disconnect>,
    ) -> Self {
        Self {
            listener,
            service: Service::Ingest {
                dispatch,
                disconnect,
            },
            event_notification: None,
        }
    }
}

/// Socket copies retained only so daemon shutdown can interrupt blocked readers.
pub(crate) struct ConnectionSet {
    live: Mutex<HashMap<u64, StdStream>>,
    next_id: Mutex<u64>,
}

impl ConnectionSet {
    pub(crate) fn new() -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            next_id: Mutex::new(0),
        }
    }

    fn register(&self, stream: &StdStream) -> io::Result<Option<(u64, StdStream)>> {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());
        if live.len() >= MAX_CONNECTIONS {
            return Ok(None);
        }
        let seen = stream.try_clone()?;
        let writer = stream.try_clone()?;
        let mut next_id = self
            .next_id
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = *next_id;
        *next_id = next_id.wrapping_add(1);
        live.insert(id, seen);
        Ok(Some((id, writer)))
    }

    fn remove(&self, id: u64) {
        self.live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    }

    fn close(&self, id: u64) {
        if let Some(stream) = self
            .live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&id)
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub(crate) fn close_all(&self) {
        let live = self.live.lock().unwrap_or_else(|error| error.into_inner());
        for stream in live.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Tokio runtime and listener thread. The daemon retains this value and stops it
/// synchronously; request handlers remain synchronous on Tokio's bounded pool.
pub(crate) struct Transport {
    runtime: Option<Arc<Runtime>>,
    accept: Option<JoinHandle<()>>,
    pub(crate) connections: Arc<ConnectionSet>,
    event_notification: Option<EventNotification>,
}

impl Transport {
    pub(crate) fn start_services(
        sources: Vec<ListenerSource>,
        stopping: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let event_notification = sources
            .iter()
            .find_map(|source| source.event_notification.clone());
        let runtime = Arc::new(
            Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(MAX_BLOCKING_HANDLERS)
                .enable_io()
                .enable_time()
                .build()?,
        );
        let connections = Arc::new(ConnectionSet::new());
        let admission = Arc::new(Admission::new());
        let connection_slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let accept_connections = Arc::clone(&connections);
        let thread_runtime = Arc::clone(&runtime);
        let accept = thread::Builder::new()
            .name("radar-transport".to_string())
            .spawn(move || {
                let tasks: Vec<_> = sources
                    .into_iter()
                    .map(|source| {
                        thread_runtime.spawn(accept_loop(
                            source.listener,
                            source.service,
                            Arc::clone(&stopping),
                            Arc::clone(&accept_connections),
                            Arc::clone(&admission),
                            Arc::clone(&connection_slots),
                        ))
                    })
                    .collect();
                thread_runtime.block_on(async move {
                    for task in tasks {
                        let _ = task.await;
                    }
                });
            })?;
        Ok(Self {
            runtime: Some(runtime),
            accept: Some(accept),
            connections,
            event_notification,
        })
    }

    pub(crate) fn stop(&mut self) {
        if let Some(notification) = &self.event_notification {
            notification.stop();
        }
        self.connections.close_all();
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        if let Some(runtime) = self.runtime.take()
            && let Ok(runtime) = Arc::try_unwrap(runtime)
        {
            runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Control,
    Ingest,
    Waiting,
}

#[derive(Clone)]
struct Admission {
    control: Arc<Semaphore>,
    ingest: Arc<Semaphore>,
    waiting: Arc<Semaphore>,
}

impl Admission {
    fn new() -> Self {
        Self {
            control: Arc::new(Semaphore::new(CONTROL_LIMIT)),
            ingest: Arc::new(Semaphore::new(INGEST_LIMIT)),
            waiting: Arc::new(Semaphore::new(WAITING_LIMIT)),
        }
    }

    fn acquire(&self, lane: Lane) -> Option<OwnedSemaphorePermit> {
        let semaphore = match lane {
            Lane::Control => &self.control,
            Lane::Ingest => &self.ingest,
            Lane::Waiting => &self.waiting,
        };
        Arc::clone(semaphore).try_acquire_owned().ok()
    }
}

fn lane(line: &[u8]) -> Lane {
    let Ok(request) = protocol::decode_request(line) else {
        return Lane::Control;
    };
    if request.method == "events.read"
        && request
            .params
            .get("wait_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|wait| wait > 0)
    {
        Lane::Waiting
    } else if matches!(
        request.method.as_str(),
        "agent.acquire" | "agent.register" | "agent.publish" | "agent.retire" | "agent.context"
    ) {
        Lane::Ingest
    } else {
        Lane::Control
    }
}

async fn accept_loop(
    listener: StdListener,
    service: Service,
    stopping: Arc<AtomicBool>,
    connections: Arc<ConnectionSet>,
    admission: Arc<Admission>,
    connection_slots: Arc<Semaphore>,
) {
    let listener = match UnixListener::from_std(listener) {
        Ok(listener) => listener,
        Err(_) => return,
    };
    let mut tasks = JoinSet::new();
    while !stopping.load(Ordering::SeqCst) {
        match tokio::time::timeout(ACCEPT_RECHECK, listener.accept()).await {
            Ok(Ok((stream, _))) => {
                let slot = match Arc::clone(&connection_slots).try_acquire_owned() {
                    Ok(slot) => slot,
                    Err(_) => continue,
                };
                let Ok(std_stream) = stream.into_std() else {
                    continue;
                };
                let Ok(Some((id, writer))) = connections.register(&std_stream) else {
                    continue;
                };
                let Ok(reader) = UnixStream::from_std(std_stream) else {
                    connections.remove(id);
                    continue;
                };
                let service = service.clone();
                let admission = Arc::clone(&admission);
                let connections = Arc::clone(&connections);
                tasks.spawn(async move {
                    serve(reader, writer, id, connections, service, admission).await;
                    drop(slot);
                });
            }
            Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
            Ok(Err(_)) | Err(_) => {}
        }
        while let Some(result) = tasks.try_join_next() {
            let _ = result;
        }
    }
    connections.close_all();
    let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, drain(&mut tasks)).await;
}

async fn drain(tasks: &mut JoinSet<()>) {
    while tasks.join_next().await.is_some() {}
}

async fn serve(
    stream: UnixStream,
    writer: StdStream,
    id: u64,
    connections: Arc<ConnectionSet>,
    service: Service,
    admission: Arc<Admission>,
) {
    let _ = writer.set_write_timeout(Some(WRITE_TIMEOUT));
    let writer = Arc::new(Mutex::new(writer));
    let mut reader = tokio::io::BufReader::new(stream);
    loop {
        let line = match tokio::time::timeout(IDLE_TIMEOUT, read_line(&mut reader)).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        };
        match line {
            ReadLine::TooLong => match &service {
                Service::Control { .. } => {
                    let response = Response::refused(
                        "",
                        &Refusal::new(
                            Code::BadRequest,
                            format!("the line exceeds {} bytes", protocol::MAX_LINE_BYTES),
                        ),
                    );
                    write_response(&writer, &response);
                }
                Service::Ingest { .. } => break,
            },
            ReadLine::Data(line) => {
                let lane = match &service {
                    Service::Control { .. } => lane(&line),
                    Service::Ingest { .. } => Lane::Ingest,
                };
                let Some(permit) = admission.acquire(lane) else {
                    if matches!(&service, Service::Ingest { .. }) {
                        break;
                    }
                    let request_id = serde_json::from_slice::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("id")
                                .and_then(|id| id.as_str())
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    let response = Response::refused(
                        &request_id,
                        &Refusal::new(Code::Busy, format!("the {lane:?} lane is at capacity")),
                    );
                    write_response(&writer, &response);
                    continue;
                };
                match &service {
                    Service::Control {
                        dispatch,
                        signal,
                        ready,
                    } => {
                        let line = wait_for_event(&line, signal, Arc::clone(ready)).await;
                        let dispatch = Arc::clone(dispatch);
                        let writer = Arc::clone(&writer);
                        if tokio::task::spawn_blocking(move || dispatch(line, id, writer))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Service::Ingest { dispatch, .. } => {
                        let dispatch = Arc::clone(dispatch);
                        match tokio::task::spawn_blocking(move || dispatch(line, id)).await {
                            Ok(result) => {
                                for connection_id in result.close_connections {
                                    connections.close(connection_id);
                                }
                                if !result.keep_connection {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
                drop(permit);
            }
        }
    }
    connections.remove(id);
    if let Service::Ingest { disconnect, .. } = service {
        let _ = tokio::task::spawn_blocking(move || disconnect(id)).await;
    }
}

async fn wait_for_event(line: &[u8], signal: &EventSignal, ready: Arc<EventReady>) -> Vec<u8> {
    let Ok(mut request) = serde_json::from_slice::<serde_json::Value>(line) else {
        return line.to_vec();
    };
    if request.get("method").and_then(serde_json::Value::as_str) != Some("events.read") {
        return line.to_vec();
    }
    let wait_ms = request
        .get("params")
        .and_then(|params| params.get("wait_ms"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    if wait_ms == 0 || wait_ms > 1_000 {
        return line.to_vec();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let mut receiver = signal.receiver.clone();
    loop {
        let readiness = {
            let ready = Arc::clone(&ready);
            let line = line.to_vec();
            tokio::task::spawn_blocking(move || ready(&line))
                .await
                .unwrap_or(true)
        };
        if readiness {
            break;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero()
            || !matches!(
                tokio::time::timeout(remaining, receiver.changed()).await,
                Ok(Ok(()))
            )
            || receiver.borrow().1
        {
            break;
        }
    }
    if let Some(params) = request
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
    {
        params.insert("wait_ms".to_string(), serde_json::Value::from(0));
    }
    serde_json::to_vec(&request).unwrap_or_else(|_| line.to_vec())
}

#[derive(Debug)]
enum ReadLine {
    Data(Vec<u8>),
    TooLong,
}

async fn read_line(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<ReadLine>> {
    let mut line = Vec::new();
    let mut overlong = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if line.is_empty() && !overlong {
                None
            } else if overlong {
                Some(ReadLine::TooLong)
            } else {
                Some(ReadLine::Data(line))
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        let content_len = newline.unwrap_or(take);
        if !overlong {
            if line.len().saturating_add(content_len) > protocol::MAX_LINE_BYTES {
                overlong = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..content_len]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            return Ok(Some(if overlong {
                ReadLine::TooLong
            } else {
                ReadLine::Data(line)
            }));
        }
    }
}

fn write_response(writer: &Arc<Mutex<StdStream>>, response: &Response) {
    let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
    if serde_json::to_writer(&mut *writer, response).is_ok() {
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Admission, CONTROL_LIMIT, INGEST_LIMIT, Lane, ReadLine, WAITING_LIMIT, lane, read_line,
    };
    use crate::control_plane::protocol::{self};
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    fn request(method: &str, params: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": protocol::PROTOCOL_VERSION,
            "id": "7f7f7f7f-7f7f-4f7f-8f7f-7f7f7f7f7f7f",
            "method": method,
            "params": params,
        }))
        .expect("request serializes")
    }

    #[test]
    fn request_lanes_are_isolated_and_waiting_reads_are_separate() {
        assert_eq!(lane(&request("agent.register", json!({}))), Lane::Ingest);
        assert_eq!(lane(&request("agent.get", json!({}))), Lane::Control);
        assert_eq!(
            lane(&request("events.read", json!({"wait_ms":1}))),
            Lane::Waiting
        );
        assert_eq!(
            lane(&request("events.read", json!({"wait_ms":0}))),
            Lane::Control
        );
        assert_eq!(lane(&request("lifecycle.stop", json!({}))), Lane::Control);
        assert!(protocol::decode_request(b"bad").is_err());
    }

    #[test]
    fn bounded_framing_accepts_the_limit_and_discards_only_oversized_lines() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("runtime builds");
        let read = |bytes: Vec<u8>| {
            runtime.block_on(async move {
                let (mut writer, reader) = tokio::io::duplex(bytes.len() + 1);
                writer.write_all(&bytes).await.expect("write fixture");
                writer.shutdown().await.expect("close fixture");
                let mut reader = tokio::io::BufReader::new(reader);
                read_line(&mut reader).await.expect("read fixture")
            })
        };
        let cap = protocol::MAX_LINE_BYTES;
        assert!(
            matches!(read(vec![b'x'; cap - 1].into_iter().chain(*b"\n").collect()), Some(ReadLine::Data(line)) if line.len() == cap - 1)
        );
        assert!(
            matches!(read(vec![b'x'; cap].into_iter().chain(*b"\n").collect()), Some(ReadLine::Data(line)) if line.len() == cap)
        );
        assert!(matches!(
            read(vec![b'x'; cap + 1].into_iter().chain(*b"\n").collect()),
            Some(ReadLine::TooLong)
        ));
        assert!(
            matches!(read(b"unterminated".to_vec()), Some(ReadLine::Data(line)) if line == b"unterminated")
        );
    }

    #[test]
    fn each_lane_has_its_own_bounded_admission() {
        let admission = Admission::new();
        let ingest: Vec<_> = (0..INGEST_LIMIT)
            .map(|_| admission.acquire(Lane::Ingest).unwrap())
            .collect();
        assert!(admission.acquire(Lane::Ingest).is_none());
        assert!(admission.acquire(Lane::Control).is_some());
        assert!(admission.acquire(Lane::Waiting).is_some());

        let waiting: Vec<_> = (0..WAITING_LIMIT)
            .map(|_| admission.acquire(Lane::Waiting).unwrap())
            .collect();
        assert!(admission.acquire(Lane::Waiting).is_none());
        assert!(admission.acquire(Lane::Control).is_some());

        let control: Vec<_> = (0..CONTROL_LIMIT)
            .map(|_| admission.acquire(Lane::Control).unwrap())
            .collect();
        assert!(admission.acquire(Lane::Control).is_none());
        drop((ingest, waiting, control));
        assert!(admission.acquire(Lane::Ingest).is_some());
    }
}
