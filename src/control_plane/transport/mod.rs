//! Bounded local-socket transport for daemon request lanes.

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdListener, UnixStream as StdStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// Long-lived UI subscriptions. They hold a connection, not a handler or a lane
/// permit, so they are bounded on their own and below `MAX_CONNECTIONS`.
const SUBSCRIPTION_LIMIT: usize = 16;
const MAX_BLOCKING_HANDLERS: usize = CONTROL_LIMIT + INGEST_LIMIT + WAITING_LIMIT;
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_RECHECK: Duration = Duration::from_millis(100);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a stop waits for requests already being served to answer.
///
/// Bounded below the client's own exchange budget and far below systemd's stop
/// timeout. A request that has not answered by then is cut, and the record it
/// was dispatched under — durable before dispatch — is what its caller reads
/// afterwards.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// How often a stop rechecks whether the last request in flight has answered.
const DRAIN_POLL: Duration = Duration::from_millis(5);

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

/// A stream of encoded response lines the server pushes until it ends. Each call
/// waits for the next line; `None` ends the stream.
pub(crate) trait FrameStream: Send {
    fn next<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>> + Send + 'a>>;
}

/// Opens the stream a request asks for. `None` means the method is not a
/// streaming one; otherwise the stream, or the refusal that answers the request.
pub(crate) type StreamOpen =
    dyn Fn(&[u8]) -> Option<Result<Box<dyn FrameStream>, Response>> + Send + Sync + 'static;

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
        streams: Arc<StreamOpen>,
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
        streams: Arc<StreamOpen>,
    ) -> Self {
        Self {
            listener,
            service: Service::Control {
                dispatch,
                signal,
                ready,
                streams,
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
    /// Requests being served right now, so a stop can wait for them instead of
    /// cutting the connections under them.
    in_flight: Arc<AtomicUsize>,
    event_notification: Option<EventNotification>,
}

/// The process-wide signals a connection consults while it serves: whether the
/// daemon is stopping, and how many requests are being served right now.
#[derive(Clone)]
struct Serving {
    stopping: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
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
        let serving = Serving {
            stopping,
            in_flight: Arc::new(AtomicUsize::new(0)),
        };
        let in_flight = Arc::clone(&serving.in_flight);
        let admission = Arc::new(Admission::new());
        let connection_slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let accept_connections = Arc::clone(&connections);
        let accept_serving = serving.clone();
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
                            Arc::clone(&accept_connections),
                            Arc::clone(&admission),
                            Arc::clone(&connection_slots),
                            accept_serving.clone(),
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
            in_flight,
            event_notification,
        })
    }

    pub(crate) fn stop(&mut self) {
        if let Some(notification) = &self.event_notification {
            notification.stop();
        }
        // Requests already being served answer first; only then are the
        // connections cut, which ends the idle ones and anything still running
        // at the deadline.
        self.drain();
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

    /// Waits for the requests being served to answer, up to [`DRAIN_TIMEOUT`].
    ///
    /// Only work in flight is waited for: an idle connection is dropped without
    /// consequence, because a client dials for each call rather than holding a
    /// session. A request that cannot answer within the deadline is cut with its
    /// record standing, which is what makes an unresolved outcome readable
    /// instead of invisible.
    fn drain(&self) {
        let started = std::time::Instant::now();
        while self.in_flight.load(Ordering::SeqCst) > 0 && started.elapsed() < DRAIN_TIMEOUT {
            thread::sleep(DRAIN_POLL);
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
    subscriptions: Arc<Semaphore>,
}

impl Admission {
    fn new() -> Self {
        Self {
            control: Arc::new(Semaphore::new(CONTROL_LIMIT)),
            ingest: Arc::new(Semaphore::new(INGEST_LIMIT)),
            waiting: Arc::new(Semaphore::new(WAITING_LIMIT)),
            subscriptions: Arc::new(Semaphore::new(SUBSCRIPTION_LIMIT)),
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
    // A read that holds its connection open is a waiting read, whatever it is
    // waiting for. It takes a waiting-lane slot rather than a control one, so a
    // display that long-polls for a newer revision cannot starve the effectful
    // requests that share the socket.
    let waiting_read = matches!(request.method.as_str(), "events.read" | "observation.get")
        && request
            .params
            .get("wait_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|wait| wait > 0);
    if waiting_read {
        Lane::Waiting
    } else if matches!(
        request.method.as_str(),
        "agent.acquire"
            | "agent.register"
            | "agent.presence"
            | "agent.release"
            | "agent.publish"
            | "agent.retire"
            | "agent.context"
    ) {
        Lane::Ingest
    } else {
        Lane::Control
    }
}

async fn accept_loop(
    listener: StdListener,
    service: Service,
    connections: Arc<ConnectionSet>,
    admission: Arc<Admission>,
    connection_slots: Arc<Semaphore>,
    serving: Serving,
) {
    let listener = match UnixListener::from_std(listener) {
        Ok(listener) => listener,
        Err(_) => return,
    };
    let mut tasks = JoinSet::new();
    while !serving.stopping.load(Ordering::SeqCst) {
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
                let serving = serving.clone();
                tasks.spawn(async move {
                    serve(reader, writer, id, connections, service, admission, serving).await;
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
    // The connections are cut by `Transport::stop`, after it has waited for the
    // requests in flight: closing them here would cut a request already being
    // served, which is the thing a stop is meant not to do.
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
    serving: Serving,
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
                if let Service::Control { streams, .. } = &service
                    && is_stream_request(&line)
                {
                    if serving.stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    let Some(permit) = Arc::clone(&admission.subscriptions)
                        .try_acquire_owned()
                        .ok()
                    else {
                        let response = Response::refused(
                            &request_id_of(&line),
                            &Refusal::new(Code::Busy, "the subscription limit is reached"),
                        );
                        write_response(&writer, &response);
                        continue;
                    };
                    let open = Arc::clone(streams);
                    let request = line.clone();
                    let opened = tokio::task::spawn_blocking(move || open(&request)).await;
                    match opened {
                        Ok(Some(Ok(stream))) => {
                            push_frames(&mut reader, stream).await;
                            drop(permit);
                            break;
                        }
                        Ok(Some(Err(response))) => {
                            write_response(&writer, &response);
                            continue;
                        }
                        _ => break,
                    }
                }
                let lane = match &service {
                    Service::Control { .. } => lane(&line),
                    Service::Ingest { .. } => Lane::Ingest,
                };
                if serving.stopping.load(Ordering::SeqCst) {
                    // Stopping. A waiting read's connection ends, so its client
                    // reconnects rather than reading a wait that ended because
                    // the daemon left: an empty result would be
                    // indistinguishable from a normal expiry. Anything else is
                    // answered, so its caller learns the request was not
                    // dispatched and may ask again.
                    if matches!(lane, Lane::Waiting) {
                        break;
                    }
                    let request_id = request_id_of(&line);
                    let response = Response::refused(
                        &request_id,
                        &Refusal::new(Code::Busy, "daemon is stopping".to_string()),
                    );
                    write_response(&writer, &response);
                    break;
                }
                let Some(permit) = admission.acquire(lane) else {
                    if matches!(&service, Service::Ingest { .. }) {
                        break;
                    }
                    let request_id = request_id_of(&line);
                    let response = Response::refused(
                        &request_id,
                        &Refusal::new(Code::Busy, format!("the {lane:?} lane is at capacity")),
                    );
                    write_response(&writer, &response);
                    continue;
                };
                serving.in_flight.fetch_add(1, Ordering::SeqCst);
                let keep = match &service {
                    Service::Control {
                        dispatch,
                        signal,
                        ready,
                        ..
                    } => {
                        let line = wait_for_event(&line, signal, Arc::clone(ready)).await;
                        let dispatch = Arc::clone(dispatch);
                        let writer = Arc::clone(&writer);
                        tokio::task::spawn_blocking(move || dispatch(line, id, writer))
                            .await
                            .is_ok()
                    }
                    Service::Ingest { dispatch, .. } => {
                        let dispatch = Arc::clone(dispatch);
                        match tokio::task::spawn_blocking(move || dispatch(line, id)).await {
                            Ok(result) => {
                                for connection_id in result.close_connections {
                                    connections.close(connection_id);
                                }
                                result.keep_connection
                            }
                            Err(_) => false,
                        }
                    }
                };
                serving.in_flight.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
                if !keep {
                    break;
                }
            }
        }
    }
    connections.remove(id);
    if let Service::Ingest { disconnect, .. } = service {
        let _ = tokio::task::spawn_blocking(move || disconnect(id)).await;
    }
}

fn is_stream_request(line: &[u8]) -> bool {
    protocol::decode_request(line).is_ok_and(|request| request.method == "ui.subscribe")
}

/// Pushes a stream's lines until it ends, the peer closes or a write stalls.
///
/// The peer is watched for closure independently of publications, so an idle
/// subscription is released as soon as its client goes. A slow reader costs only
/// its own write deadline: the stream hands over the latest state, never a queue.
async fn push_frames(
    reader: &mut tokio::io::BufReader<UnixStream>,
    mut stream: Box<dyn FrameStream>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut closed = [0u8; 1];
    loop {
        let frame = tokio::select! {
            frame = stream.next() => frame,
            _ = reader.read(&mut closed) => return,
        };
        let Some(mut frame) = frame else { return };
        frame.push(b'\n');
        let write = async {
            let socket = reader.get_mut();
            socket.write_all(&frame).await?;
            socket.flush().await
        };
        if !matches!(tokio::time::timeout(WRITE_TIMEOUT, write).await, Ok(Ok(()))) {
            return;
        }
    }
}

/// The id a line names, so a refusal can answer the request it will not serve.
fn request_id_of(line: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
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
        assert_eq!(lane(&request("agent.presence", json!({}))), Lane::Ingest);
        assert_eq!(lane(&request("agent.release", json!({}))), Lane::Ingest);
        assert_eq!(
            lane(&request("process.get", json!({"registration_id":"x"}))),
            Lane::Control
        );
        assert_eq!(
            lane(&request("events.read", json!({"wait_ms":1}))),
            Lane::Waiting
        );
        assert_eq!(
            lane(&request("events.read", json!({"wait_ms":0}))),
            Lane::Control
        );
        // A long-polling observation read waits in the waiting lane, so eight
        // displays waiting for a revision cannot occupy the control lane.
        assert_eq!(
            lane(&request("observation.get", json!({"wait_ms":1000}))),
            Lane::Waiting
        );
        assert_eq!(
            lane(&request("observation.get", json!({"wait_ms":0}))),
            Lane::Control
        );
        assert_eq!(lane(&request("lifecycle.stop", json!({}))), Lane::Control);
        assert!(protocol::decode_request(b"bad").is_err());
    }

    /// What a stop owes a request it has already accepted: an answer, not a
    /// dropped connection.
    #[test]
    fn a_stop_lets_a_request_in_flight_answer_and_refuses_one_that_arrives_during_it() {
        use super::{
            Dispatch, EventReady, ListenerSource, Transport, bind_socket, event_notifications,
            request_id_of, write_response,
        };
        use crate::control_plane::protocol::{PROTOCOL_VERSION, Response};
        use crate::control_plane::store::random_uuid;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream as StdStream;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let directory = std::env::temp_dir().join(format!(
            "radar-transport-drain-{}-{}",
            std::process::id(),
            random_uuid()
        ));
        std::fs::create_dir_all(&directory).expect("state directory");
        let path = directory.join("control.sock");
        let listener = bind_socket(&path, "test", "already in use").expect("bind");
        // The daemon makes its listener non-blocking before handing it to the
        // transport, and an accepted socket inherits that.
        listener.set_nonblocking(true).expect("non-blocking");
        let (notification, signal) = event_notifications();
        let ready: Arc<EventReady> = Arc::new(|_line: &[u8]| false);

        // The request this test holds is the one the stop must wait for. It is
        // chosen by id rather than by arrival, so the test does not depend on
        // which connection the runtime dispatches first.
        const BUSY: &str = "7f7f7f7f-7f7f-4f7f-8f7f-7f7f7f7f7f7f";
        let held = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let (dispatch_held, dispatch_release) = (Arc::clone(&held), Arc::clone(&release));
        let dispatch: Arc<Dispatch> = Arc::new(
            move |line: Vec<u8>, _id: u64, writer: Arc<Mutex<StdStream>>| {
                let id = request_id_of(&line);
                if id == BUSY {
                    dispatch_held.store(true, Ordering::SeqCst);
                    while !dispatch_release.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                write_response(
                    &writer,
                    &Response::ok(&id, serde_json::json!({"served": true})),
                );
            },
        );

        let stopping = Arc::new(AtomicBool::new(false));
        let mut transport = Transport::start_services(
            vec![ListenerSource::control(
                listener,
                dispatch,
                signal,
                notification,
                ready,
                Arc::new(|_| None),
            )],
            Arc::clone(&stopping),
        )
        .expect("transport starts");

        fn ask(stream: &mut StdStream, id: &str) {
            let line = serde_json::to_vec(&serde_json::json!({
                "version": PROTOCOL_VERSION,
                "id": id,
                "method": "mux.focus",
                "params": {},
            }))
            .expect("request serializes");
            stream.write_all(&line).expect("write request");
            stream.write_all(b"\n").expect("terminate request");
            stream.flush().expect("flush request");
        }
        fn answer(stream: &mut StdStream) -> serde_json::Value {
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("read answer");
            serde_json::from_str(&line).expect("answer decodes")
        }

        // Both connections are accepted before the stop: the one carrying the
        // request in flight, and the one that asks during the drain. An idle
        // connection is exactly what a client holds between calls.
        let mut busy = StdStream::connect(&path).expect("connect");
        let mut later = StdStream::connect(&path).expect("connect");
        ask(&mut busy, BUSY);
        ask(&mut later, "8f8f8f8f-8f8f-4f8f-8f8f-8f8f8f8f8f8f");
        assert_eq!(answer(&mut later)["result"]["served"], true);
        crate::testing::eventually("the held request to be dispatched", || {
            held.load(Ordering::SeqCst).then_some(())
        });

        let finished = Arc::new(AtomicBool::new(false));
        let (stop_finished, stop_stopping) = (Arc::clone(&finished), Arc::clone(&stopping));
        let stop = std::thread::spawn(move || {
            stop_stopping.store(true, Ordering::SeqCst);
            transport.stop();
            stop_finished.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !finished.load(Ordering::SeqCst),
            "the stop is still waiting for the request in flight"
        );

        // A request arriving while stopping is answered, and answered before
        // anything is dispatched, so the caller learns it had no effect and may
        // ask again. A dropped connection could not say that.
        ask(&mut later, "9f9f9f9f-9f9f-4f9f-8f9f-9f9f9f9f9f9f");
        let refusal = answer(&mut later);
        assert_eq!(refusal["error"]["code"], "busy", "{refusal}");
        assert!(
            refusal["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("stopping"),
            "{refusal}"
        );

        // Releasing the request in flight lets it answer, and only then does the
        // stop end.
        release.store(true, Ordering::SeqCst);
        assert_eq!(answer(&mut busy)["result"]["served"], true);
        assert!(
            !finished.load(Ordering::SeqCst),
            "the answer arrived before the stop ended"
        );
        stop.join().expect("stop thread");
        let _ = std::fs::remove_dir_all(&directory);
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
