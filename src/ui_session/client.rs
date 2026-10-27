//! A frontend's connection to the daemon's shared UI session.
//!
//! One duplex connection serves one attachment: requests go out with their own
//! ids, replies come back correlated, and publications arrive on the same
//! socket without waiting for any of them. Reading, writing, actions, content
//! and process demand are separate tasks, so no one of them can park another.
//!
//! A lost connection is a lost attachment: the client reconnects under a fresh
//! attachment id and takes the authoritative baseline. It never replays an
//! action whose result it could not read.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use super::publication::{DuplexFrame, DuplexMetadata, DuplexOperation, DuplexReply, Observation};
use crate::content::{self, Content};
use crate::control_plane::protocol::PROTOCOL_VERSION;
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::random_uuid;
use crate::control_plane::transport::{self, Tasks};

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_FLOOR: Duration = Duration::from_millis(200);
const RETRY_CEILING: Duration = Duration::from_secs(2);
/// Queued actions. A person cannot fill this faster than the daemon drains it.
const OUTBOX: usize = 256;
/// Requests on the wire: one action plus the bounded auxiliary work.
const OUTGOING: usize = 8;

/// What wakes a frontend's event loop. Cheap to clone; a notification sent
/// while nobody waits is kept for the next wait.
#[derive(Clone, Default)]
pub struct Wake(Arc<Notify>);

impl Wake {
    pub fn notify(&self) {
        self.0.notify_one();
    }

    pub async fn woken(&self) {
        self.0.notified().await;
    }
}

/// One published state as a frontend holds it.
#[derive(Clone, Debug)]
pub struct Mirror {
    pub attachment_id: String,
    pub update_id: u64,
    pub state: dto::SessionState,
    pub rows: Arc<Vec<dto::TreeRow>>,
    pub observation: Arc<Observation>,
    pub diagnostic: Option<dto::PublicationFailure>,
}

struct Inner {
    socket: PathBuf,
    role: dto::AttachmentRole,
    interaction: Option<String>,
    incarnation: AtomicU64,
    observed_gate: AtomicU64,
    observed_update: AtomicU64,
    processes: watch::Sender<bool>,
    wake: Wake,
    /// Actions queued or in flight.
    pending: AtomicUsize,
    /// The newest update id any action result named.
    floor: AtomicU64,
    /// A refused action left the local replica ahead of the session.
    resync: AtomicBool,
    latest: Mutex<Option<Arc<Mirror>>>,
    /// The head this view last displayed: what to deliver next is relative to it.
    shown: Mutex<Shown>,
    /// Correlates requests with their replies on the current connection.
    next_request: AtomicU64,
    connected: AtomicBool,
}

/// The last head a view displayed. Both facts describe one delivery.
#[derive(Clone, Copy, Default)]
struct Shown {
    update_id: u64,
    gate: u64,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct UiClient {
    inner: Arc<Inner>,
    outbox: mpsc::Sender<Pending>,
    /// Drop is the cancellation fallback; normal exits await shutdown.
    supervisor: tokio::task::JoinHandle<()>,
    shutdown: watch::Sender<bool>,
}

struct Pending {
    action: dto::UiAction,
    gate: u64,
    incarnation: u64,
    _count: PendingCount,
}

struct PendingCount(Arc<Inner>);

impl Drop for PendingCount {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::SeqCst);
        self.0.wake.notify();
    }
}

impl UiClient {
    pub fn start(
        runtime: &tokio::runtime::Handle,
        socket: PathBuf,
        role: dto::AttachmentRole,
        wake: Wake,
    ) -> Self {
        Self::connect(runtime, socket, role, None, wake)
    }

    pub fn presenter(
        runtime: &tokio::runtime::Handle,
        socket: PathBuf,
        interaction: String,
        wake: Wake,
    ) -> Self {
        Self::connect(
            runtime,
            socket,
            dto::AttachmentRole::Presenter,
            Some(interaction),
            wake,
        )
    }

    fn connect(
        runtime: &tokio::runtime::Handle,
        socket: PathBuf,
        role: dto::AttachmentRole,
        interaction: Option<String>,
        wake: Wake,
    ) -> Self {
        let (processes, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            socket,
            role,
            interaction,
            incarnation: AtomicU64::new(0),
            observed_gate: AtomicU64::new(0),
            observed_update: AtomicU64::new(0),
            processes,
            wake,
            pending: AtomicUsize::new(0),
            floor: AtomicU64::new(0),
            resync: AtomicBool::new(false),
            latest: Mutex::new(None),
            shown: Mutex::new(Shown::default()),
            next_request: AtomicU64::new(1),
            connected: AtomicBool::new(false),
        });
        let (outbox, queue) = mpsc::channel(OUTBOX);
        let (shutdown, stopping) = watch::channel(false);
        let supervisor = runtime.spawn(attach_loop(Arc::clone(&inner), queue, stopping));
        Self {
            inner,
            outbox,
            supervisor,
            shutdown,
        }
    }

    pub async fn shutdown(&mut self) {
        self.shutdown.send_replace(true);
        if tokio::time::timeout(REPLY_TIMEOUT, &mut self.supervisor)
            .await
            .is_err()
        {
            self.supervisor.abort();
            let _ = (&mut self.supervisor).await;
        }
    }

    /// Queues an action behind every earlier one. Never blocks.
    pub fn submit(&self, action: dto::UiAction) {
        let shown = *locked(&self.inner.shown);
        if action.is_ordinary() && self.inner.observed_gate.load(Ordering::SeqCst) != shown.gate {
            return;
        }
        self.inner.pending.fetch_add(1, Ordering::SeqCst);
        let pending = Pending {
            action,
            gate: shown.gate,
            incarnation: self.inner.incarnation.load(Ordering::SeqCst),
            _count: PendingCount(Arc::clone(&self.inner)),
        };
        if self.outbox.try_send(pending).is_err() {
            self.inner.resync.store(true, Ordering::SeqCst);
        }
    }

    pub fn wants_processes(&self, wanted: bool) {
        self.inner.processes.send_replace(wanted);
    }

    /// Whether every action sent has been answered and the newest state
    /// reflects them: a state taken now would not undo anything.
    pub fn idle(&self) -> bool {
        self.inner.pending.load(Ordering::SeqCst) == 0
            && locked(&self.inner.latest)
                .as_ref()
                .is_none_or(|latest| latest.update_id >= self.inner.floor.load(Ordering::SeqCst))
    }

    /// Whether the session is attached right now.
    pub fn connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    /// The newest published state, once it reflects every action this client
    /// has had answered and nothing is still in flight. Returns each state once.
    ///
    /// A change of gate is exempt from the wait for this client's own actions: a
    /// view still owing an answer must still learn that the session opened or
    /// closed a blocking interaction, because the gate decides whether its queued
    /// input is still valid and whether it has a modal to show. Withholding that
    /// would hide a modal raised elsewhere behind a slow local queue.
    pub fn poll(&self) -> Option<Arc<Mirror>> {
        let pending = self.inner.pending.load(Ordering::SeqCst) > 0;
        let latest = locked(&self.inner.latest).clone()?;
        let resync = self.inner.resync.swap(false, Ordering::SeqCst);
        let shown = *locked(&self.inner.shown);
        let deliverable = deliverable(
            &latest,
            pending,
            self.inner.floor.load(Ordering::SeqCst),
            resync,
            shown,
        );
        if !deliverable {
            self.inner.resync.fetch_or(resync, Ordering::SeqCst);
            return None;
        }
        *locked(&self.inner.shown) = Shown {
            update_id: latest.update_id,
            gate: latest.state.gate,
        };
        Some(latest)
    }
}

impl Drop for UiClient {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

/// Whether the newest publication is shown to this view now.
///
/// A gate change is the one thing that does not wait for this view's own actions:
/// it decides whether what the view is about to submit is still valid, and it is
/// how a blocking interaction opened elsewhere reaches a view that is mid-action.
fn deliverable(latest: &Mirror, pending: bool, floor: u64, resync: bool, shown: Shown) -> bool {
    if latest.state.gate != shown.gate {
        return true;
    }
    !pending && latest.update_id >= floor && (resync || latest.update_id != shown.update_id)
}

fn request(method: &str, params: Value) -> (String, Vec<u8>) {
    let id = random_uuid();
    let mut line = json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    (id, line)
}

fn answered(id: &str, line: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    if value.get("id").and_then(Value::as_str) != Some(id) {
        return Err("answered a different request".into());
    }
    if let Some(error) = value.get("error") {
        return Err(format!(
            "{}: {}",
            error.get("code").and_then(Value::as_str).unwrap_or("error"),
            error.get("message").and_then(Value::as_str).unwrap_or("")
        ));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "answered without a result".into())
}

async fn attach_loop(
    inner: Arc<Inner>,
    mut actions: mpsc::Receiver<Pending>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = RETRY_FLOOR;
    while !*shutdown.borrow() {
        match session(&inner, &mut actions, &mut shutdown).await {
            Ok(()) => backoff = RETRY_FLOOR,
            Err(_) => backoff = (backoff * 2).min(RETRY_CEILING),
        }
        if inner.connected.swap(false, Ordering::SeqCst) {
            inner.wake.notify();
        }
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _ = shutdown.changed() => return,
            () = tokio::time::sleep(backoff) => {}
        }
    }
}

/// One attachment incarnation: dial, open, then serve until it ends.
async fn session(
    inner: &Arc<Inner>,
    actions: &mut mpsc::Receiver<Pending>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let attachment = random_uuid();
    inner.incarnation.fetch_add(1, Ordering::SeqCst);
    inner.observed_update.store(0, Ordering::SeqCst);
    inner.floor.store(0, Ordering::SeqCst);
    *locked(&inner.shown) = Shown::default();
    *locked(&inner.latest) = None;

    let stream = UnixStream::connect(&inner.socket)
        .await
        .map_err(|error| error.to_string())?;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let (id, line) = request(
        "ui.open",
        serde_json::to_value(dto::AttachmentOpen {
            attachment_id: attachment.clone(),
            role: inner.role,
            interaction_id: inner.interaction.clone(),
        })
        .map_err(|error| error.to_string())?,
    );
    write
        .write_all(&line)
        .await
        .map_err(|error| error.to_string())?;
    let mut answer = String::new();
    let opened = tokio::time::timeout(REPLY_TIMEOUT, read.read_line(&mut answer)).await;
    match opened {
        Ok(Ok(0)) => return Err("the daemon closed the connection".into()),
        Ok(Ok(_)) => {
            answered(&id, &answer)?;
        }
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => return Err("the daemon did not answer the opener".into()),
    }

    let (outgoing, requests) = mpsc::channel::<dto::DuplexRequest>(OUTGOING);
    let replies: Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>> = Default::default();
    let (frames, publications) = watch::channel::<Option<DuplexFrame>>(None);

    // Children end with this connection, including when the supervisor itself is
    // cancelled: a leaked reader would hold the socket and its attachment open.
    let mut children = Tasks::default();
    children.spawn(read_loop(
        read,
        Arc::clone(&replies),
        frames,
        Arc::clone(inner),
    ));
    children.spawn(write_loop(write, requests));
    children.spawn(materialise(
        Arc::clone(inner),
        publications,
        outgoing.clone(),
        Arc::clone(&replies),
        attachment.clone(),
    ));
    children.spawn(demand(
        Arc::clone(inner),
        outgoing.clone(),
        Arc::clone(&replies),
    ));
    let result = tokio::select! {
        result = action_pump(inner, actions, &outgoing, &replies) => result,
        () = children.wait() => Err("an attachment task ended".into()),
        _ = shutdown.changed() => Ok(()),
    };
    children.shutdown().await;
    result
}

/// Sends one request and waits for the reply carrying its id.
async fn ask(
    outgoing: &mpsc::Sender<dto::DuplexRequest>,
    replies: &Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
    request: dto::DuplexRequest,
) -> Result<Answered, String> {
    let id = request.id;
    let (reply, answer) = oneshot::channel();
    locked(replies).insert(id, reply);
    if outgoing.send(request).await.is_err() {
        locked(replies).remove(&id);
        return Err("the connection is closed".into());
    }
    let reply = match tokio::time::timeout(REPLY_TIMEOUT, answer).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) => {
            locked(replies).remove(&id);
            return Err("the connection closed before the reply".into());
        }
        Err(_) => {
            locked(replies).remove(&id);
            return Err("the daemon did not answer in time".into());
        }
    };
    Ok(reply)
}

async fn read_loop(
    mut read: BufReader<OwnedReadHalf>,
    replies: Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
    frames: watch::Sender<Option<DuplexFrame>>,
    inner: Arc<Inner>,
) {
    loop {
        // A quiet fleet is normal for this direction; a frame in flight is not.
        let bytes = match transport::read_frame_body(&mut read, transport::FRAME_TIMEOUT).await {
            Ok(bytes) => bytes,
            Err(_) => break,
        };
        let Ok(frame) = content::decode::<DuplexFrame>(&bytes) else {
            break;
        };
        let DuplexFrame { metadata, content } = frame;
        match metadata {
            DuplexMetadata::Reply { id, reply } => {
                if let Some(reply_sender) = locked(&replies).remove(&id) {
                    let _ = reply_sender.send(Answered { reply, content });
                }
            }
            DuplexMetadata::Publication { head } => {
                if head.update_id >= inner.observed_update.load(Ordering::SeqCst) {
                    inner.observed_gate.store(head.gate, Ordering::SeqCst);
                    inner
                        .observed_update
                        .store(head.update_id, Ordering::SeqCst);
                }
                frames.send_replace(Some(DuplexFrame {
                    metadata: DuplexMetadata::Publication { head },
                    content,
                }));
                inner.wake.notify();
            }
        }
    }
}

async fn write_loop(mut write: OwnedWriteHalf, mut requests: mpsc::Receiver<dto::DuplexRequest>) {
    while let Some(next) = requests.recv().await {
        let frame = dto::DuplexRequest {
            id: next.id,
            request: next.request,
        };
        let write_one = async {
            let Ok(segments) = content::Segments::encoded(&frame) else {
                return Err(());
            };
            write.write_all(&segments.0.concat()).await.map_err(|_| ())
        };
        match tokio::time::timeout(REPLY_TIMEOUT, write_one).await {
            Ok(Ok(())) => {}
            _ => return,
        }
    }
}

/// Materialises the newest head, fetching content the head did not carry.
///
/// Content is fetched on this task, never on the reader: a newer head can still
/// invalidate input and replace the pending target while a fetch is in flight.
async fn materialise(
    inner: Arc<Inner>,
    mut publications: watch::Receiver<Option<DuplexFrame>>,
    outgoing: mpsc::Sender<dto::DuplexRequest>,
    replies: Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
    attachment: String,
) {
    let mut current: Vec<Content> = Vec::new();
    while publications.changed().await.is_ok() {
        let Some(frame) = publications.borrow_and_update().clone() else {
            continue;
        };
        if publish(
            &inner,
            &attachment,
            &mut current,
            frame,
            &outgoing,
            &replies,
        )
        .await
        .is_err()
        {
            return;
        }
    }
}

/// One answer as it arrived: the reply and whatever payloads it carried.
struct Answered {
    reply: DuplexReply,
    content: Vec<Content>,
}

async fn fetch(
    outgoing: &mpsc::Sender<dto::DuplexRequest>,
    replies: &Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
    hash: &str,
    id: u64,
) -> Result<Content, String> {
    let answered = ask(
        outgoing,
        replies,
        dto::DuplexRequest {
            id,
            request: DuplexOperation::Content {
                hash: hash.to_owned(),
            },
        },
    )
    .await?;
    match answered.reply {
        DuplexReply::Content { .. } => answered
            .content
            .into_iter()
            .find(|content| content.hash == hash)
            .ok_or_else(|| "content reply carried no payload".to_string()),
        _ => Err("content fetch refused".into()),
    }
}

/// Materialise exactly one head; only the current payloads survive the handover.
async fn publish(
    inner: &Arc<Inner>,
    attachment: &str,
    current: &mut Vec<Content>,
    frame: DuplexFrame,
    outgoing: &mpsc::Sender<dto::DuplexRequest>,
    replies: &Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
) -> Result<(), String> {
    let DuplexFrame { metadata, content } = frame;
    let DuplexMetadata::Publication { head } = metadata else {
        return Ok(());
    };
    if head.update_id < inner.observed_update.load(Ordering::SeqCst) {
        return Ok(());
    }
    if locked(&inner.latest)
        .as_ref()
        .is_some_and(|mirror| mirror.update_id >= head.update_id)
    {
        return Ok(());
    }
    let session_only = inner.role == dto::AttachmentRole::Presenter;
    let hashes = [&head.observation, &head.tree, &head.session];
    let mut next = Vec::new();
    for (index, hash) in hashes.into_iter().enumerate() {
        if session_only && index != 2 {
            continue;
        }
        let held = content
            .iter()
            .chain(current.iter())
            .find(|held| &held.hash == hash)
            .cloned();
        let payload = match held {
            Some(payload) => payload,
            None => fetch(outgoing, replies, hash, next_id(inner)).await?,
        };
        next.push(payload);
    }
    let session = next
        .iter()
        .find(|content| content.hash == head.session)
        .ok_or("session content missing")?;
    let state: dto::SessionState = session.read().map_err(|error| error.to_string())?;
    if state.gate != head.gate {
        return Err("head gate differs from session".into());
    }
    let held = locked(&inner.latest).clone();
    let observation = if session_only {
        Arc::new(Observation::default())
    } else if let Some(previous) = held.as_ref().filter(|_| {
        current
            .iter()
            .any(|content| content.hash == head.observation)
    }) {
        Arc::clone(&previous.observation)
    } else {
        Arc::new(
            next.iter()
                .find(|content| content.hash == head.observation)
                .ok_or("observation missing")?
                .read()
                .map_err(|error| error.to_string())?,
        )
    };
    let rows = if session_only {
        Arc::new(Vec::new())
    } else if let Some(previous) = held
        .as_ref()
        .filter(|_| current.iter().any(|content| content.hash == head.tree))
    {
        Arc::clone(&previous.rows)
    } else {
        Arc::new(
            next.iter()
                .find(|content| content.hash == head.tree)
                .ok_or("rows missing")?
                .read()
                .map_err(|error| error.to_string())?,
        )
    };
    let mut latest = locked(&inner.latest);
    if head.update_id < inner.observed_update.load(Ordering::SeqCst) {
        return Ok(());
    }
    *current = next;
    *latest = Some(Arc::new(Mirror {
        attachment_id: attachment.into(),
        update_id: head.update_id,
        state,
        rows,
        observation,
        diagnostic: head.diagnostic,
    }));
    drop(latest);
    // The baseline is adopted: only now is this view ready to be called attached.
    inner.connected.store(true, Ordering::SeqCst);
    inner.wake.notify();
    Ok(())
}

fn next_id(inner: &Inner) -> u64 {
    inner.next_request.fetch_add(1, Ordering::SeqCst)
}

/// Keeps the daemon sampling the classes this view needs, as a latest wished-for
/// value rather than a repeated poll.
async fn demand(
    inner: Arc<Inner>,
    outgoing: mpsc::Sender<dto::DuplexRequest>,
    replies: Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
) {
    let mut processes = inner.processes.subscribe();
    let mut sent = None;
    loop {
        let wanted = *processes.borrow_and_update();
        if sent != Some(wanted) {
            let id = next_id(&inner);
            let asked = ask(
                &outgoing,
                &replies,
                dto::DuplexRequest {
                    id,
                    request: DuplexOperation::Demand { processes: wanted },
                },
            )
            .await;
            match asked {
                Ok(_) => sent = Some(wanted),
                Err(_) => return,
            }
        }
        if processes.changed().await.is_err() {
            return;
        }
    }
}

/// Sends actions in submission order, one at a time, until the connection ends.
async fn action_pump(
    inner: &Arc<Inner>,
    actions: &mut mpsc::Receiver<Pending>,
    outgoing: &mpsc::Sender<dto::DuplexRequest>,
    replies: &Arc<Mutex<HashMap<u64, oneshot::Sender<Answered>>>>,
) -> Result<(), String> {
    let mut number = 1u64;
    loop {
        let Some(pending) = actions.recv().await else {
            return Ok(());
        };
        if pending.incarnation != inner.incarnation.load(Ordering::SeqCst) {
            inner.resync.store(true, Ordering::SeqCst);
            continue;
        }
        let ordinary = pending.action.is_ordinary();
        if ordinary && inner.observed_gate.load(Ordering::SeqCst) != pending.gate {
            continue;
        }
        let id = next_id(inner);
        let answered = ask(
            outgoing,
            replies,
            dto::DuplexRequest {
                id,
                request: DuplexOperation::Action {
                    action_id: number,
                    gate: Some(pending.gate),
                    action: pending.action,
                },
            },
        )
        .await;
        let outcome = match answered {
            Ok(Answered {
                reply: DuplexReply::Action { outcome },
                ..
            }) => outcome,
            Ok(Answered {
                reply:
                    DuplexReply::Refused {
                        code: crate::control_plane::protocol::Code::Busy,
                        ..
                    },
                ..
            }) => dto::ActionOutcome::Busy,
            Ok(_) => {
                inner.resync.store(true, Ordering::SeqCst);
                return Err("unexpected action reply".into());
            }
            // The result is unknown, not refused: never replayed under this
            // attachment, and never reported as definitely not executed.
            Err(_) => {
                inner.resync.store(true, Ordering::SeqCst);
                return Err("the action result was lost with the connection".into());
            }
        };
        match outcome {
            dto::ActionOutcome::Applied { update_id, .. }
            | dto::ActionOutcome::Replayed { update_id, .. } => {
                number += 1;
                inner.floor.fetch_max(update_id, Ordering::SeqCst);
            }
            // Not accepted, so the number was never consumed.
            dto::ActionOutcome::Busy => {
                inner.resync.store(true, Ordering::SeqCst);
            }
            dto::ActionOutcome::Refused { .. } => {
                number += 1;
                inner.resync.store(true, Ordering::SeqCst);
            }
            dto::ActionOutcome::Gap { .. }
            | dto::ActionOutcome::ReceiptExpired
            | dto::ActionOutcome::Detached => {
                inner.resync.store(true, Ordering::SeqCst);
                return Err("the attachment lost its place in the action order".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_an_action_wait_releases_its_pending_count() {
        let client = UiClient::start(
            &tokio::runtime::Handle::current(),
            PathBuf::from("unused-test-socket"),
            dto::AttachmentRole::Dashboard,
            Wake::default(),
        );
        client.supervisor.abort();
        let inner = &client.inner;
        let (queue, mut actions) = mpsc::channel(1);
        inner.pending.fetch_add(1, Ordering::SeqCst);
        assert!(
            queue
                .send(Pending {
                    action: dto::UiAction::CycleOrder,
                    gate: 0,
                    incarnation: 0,
                    _count: PendingCount(Arc::clone(inner)),
                })
                .await
                .is_ok()
        );
        let (outgoing, mut requests) = mpsc::channel(1);
        let replies = Arc::new(Mutex::new(HashMap::new()));
        let mut pumping = Box::pin(action_pump(inner, &mut actions, &outgoing, &replies));
        tokio::select! {
            request = requests.recv() => assert!(request.is_some()),
            _ = &mut pumping => panic!("the action must await its reply"),
        }
        assert_eq!(inner.pending.load(Ordering::SeqCst), 1);
        drop(pumping);
        assert_eq!(inner.pending.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_content_cannot_replace_an_outstanding_demand_reply() {
        let client = UiClient::start(
            &tokio::runtime::Handle::current(),
            PathBuf::from("unused-test-socket"),
            dto::AttachmentRole::Dashboard,
            Wake::default(),
        );
        client.supervisor.abort();
        let inner = Arc::clone(&client.inner);
        let payloads = [
            Content::encode(&Observation::default()).unwrap(),
            Content::encode(&Vec::<dto::TreeRow>::new()).unwrap(),
            Content::encode(&dto::SessionState::default()).unwrap(),
        ];
        let head = dto::Head {
            update_id: 1,
            observation: payloads[0].hash.clone(),
            tree: payloads[1].hash.clone(),
            session: payloads[2].hash.clone(),
            gate: 0,
            diagnostic: None,
        };
        let (outgoing, mut requests) = mpsc::channel(OUTGOING);
        let replies = Arc::new(Mutex::new(HashMap::new()));
        let mut tasks = Tasks::default();
        tasks.spawn(demand(
            Arc::clone(&inner),
            outgoing.clone(),
            Arc::clone(&replies),
        ));
        let demand = requests.recv().await.unwrap();
        assert!(matches!(demand.request, DuplexOperation::Demand { .. }));
        let mut current = Vec::new();
        let fetching = publish(
            &inner,
            "attachment",
            &mut current,
            DuplexFrame {
                metadata: DuplexMetadata::Publication { head },
                content: Vec::new(),
            },
            &outgoing,
            &replies,
        );
        let serving = async {
            let mut ids = std::collections::HashSet::from([demand.id]);
            for _ in 0..3 {
                let request = requests.recv().await.unwrap();
                assert!(
                    ids.insert(request.id),
                    "content reused a pending request id"
                );
                let DuplexOperation::Content { hash } = request.request else {
                    panic!("expected content recovery");
                };
                let payload = payloads
                    .iter()
                    .find(|payload| payload.hash == hash)
                    .unwrap();
                let reply = locked(&replies).remove(&request.id).unwrap();
                assert!(
                    reply
                        .send(Answered {
                            reply: DuplexReply::Content { hash },
                            content: vec![payload.clone()]
                        })
                        .is_ok()
                );
            }
            assert!(locked(&replies).contains_key(&demand.id));
        };
        let (published, ()) = tokio::join!(fetching, serving);
        published.unwrap();
        assert_eq!(locked(&inner.latest).as_ref().unwrap().update_id, 1);
        tasks.shutdown().await;
    }
}
