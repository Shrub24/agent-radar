//! A frontend's connection to the daemon's shared UI session.
//!
//! Three connections serve one attachment: an ordered action connection, the
//! push subscription, and, rarely, a content fetch. Everything blocking runs on
//! two named threads, so the terminal task never waits on the daemon. The
//! newest published state is kept in one slot and the frontend is woken when it
//! changes; a state it was too slow to read is simply replaced.
//!
//! A lost connection is a lost attachment: the client reconnects under a fresh
//! attachment id and takes the authoritative baseline. It never replays an
//! action whose result it could not read.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;

use super::publication::{Frame, Observation, Open, Reply, Request as UiRequest};
use crate::content::{self, Content};
use crate::control_plane::protocol::PROTOCOL_VERSION;
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::random_uuid;

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_FLOOR: Duration = Duration::from_millis(200);
const RETRY_CEILING: Duration = Duration::from_secs(2);

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

struct Link {
    action: UnixStream,
    reader: BufReader<UnixStream>,
    attachment: String,
    next_action: u64,
    processes: bool,
}

struct Inner {
    socket: PathBuf,
    role: dto::AttachmentRole,
    interaction: Option<String>,
    incarnation: AtomicU64,
    displayed_gate: AtomicU64,
    observed_gate: AtomicU64,
    observed_update: AtomicU64,
    processes: AtomicBool,
    wake: Wake,
    stop: AtomicBool,
    /// Actions queued or in flight.
    pending: AtomicUsize,
    /// The newest update id any action result named.
    floor: AtomicU64,
    /// A refused action left the local replica ahead of the session.
    resync: AtomicBool,
    latest: Mutex<Option<Arc<Mirror>>>,
    delivered: AtomicU64,
    link: Mutex<Option<Link>>,
    subscription: Mutex<Option<UnixStream>>,
    connected: AtomicBool,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct UiClient {
    inner: Arc<Inner>,
    outbox: mpsc::SyncSender<Pending>,
    threads: Vec<JoinHandle<()>>,
}

struct Pending {
    action: dto::UiAction,
    gate: u64,
    incarnation: u64,
}

impl UiClient {
    pub fn start(socket: PathBuf, role: dto::AttachmentRole, wake: Wake) -> Self {
        Self::connect(socket, role, None, wake)
    }

    pub fn presenter(socket: PathBuf, interaction: String, wake: Wake) -> Self {
        Self::connect(
            socket,
            dto::AttachmentRole::Presenter,
            Some(interaction),
            wake,
        )
    }

    fn connect(
        socket: PathBuf,
        role: dto::AttachmentRole,
        interaction: Option<String>,
        wake: Wake,
    ) -> Self {
        let inner = Arc::new(Inner {
            socket,
            role,
            interaction,
            incarnation: AtomicU64::new(0),
            displayed_gate: AtomicU64::new(0),
            observed_gate: AtomicU64::new(0),
            observed_update: AtomicU64::new(0),
            processes: AtomicBool::new(false),
            wake,
            stop: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            floor: AtomicU64::new(0),
            resync: AtomicBool::new(false),
            latest: Mutex::new(None),
            delivered: AtomicU64::new(0),
            link: Mutex::new(None),
            subscription: Mutex::new(None),
            connected: AtomicBool::new(false),
        });
        let (outbox, queue) = mpsc::sync_channel(256);
        let mut threads = Vec::new();
        {
            let inner = Arc::clone(&inner);
            if let Ok(thread) = thread::Builder::new()
                .name("radar-ui-attach".into())
                .spawn(move || attach_loop(&inner))
            {
                threads.push(thread);
            }
        }
        {
            let inner = Arc::clone(&inner);
            if let Ok(thread) = thread::Builder::new()
                .name("radar-ui-actions".into())
                .spawn(move || action_loop(&inner, &queue))
            {
                threads.push(thread);
            }
        }
        Self {
            inner,
            outbox,
            threads,
        }
    }

    /// Queues an action behind every earlier one. Never blocks.
    pub fn submit(&self, action: dto::UiAction) {
        if action.is_ordinary()
            && self.inner.observed_gate.load(Ordering::SeqCst)
                != self.inner.displayed_gate.load(Ordering::SeqCst)
        {
            return;
        }
        self.inner.pending.fetch_add(1, Ordering::SeqCst);
        let pending = Pending {
            action,
            gate: self.inner.displayed_gate.load(Ordering::SeqCst),
            incarnation: self.inner.incarnation.load(Ordering::SeqCst),
        };
        if self.outbox.try_send(pending).is_err() {
            self.inner.pending.fetch_sub(1, Ordering::SeqCst);
            self.inner.resync.store(true, Ordering::SeqCst);
            cut(&self.inner);
        }
    }

    pub fn wants_processes(&self, wanted: bool) {
        self.inner.processes.store(wanted, Ordering::SeqCst);
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
        let deliverable = deliverable(
            &latest,
            pending,
            self.inner.floor.load(Ordering::SeqCst),
            self.inner.delivered.load(Ordering::SeqCst),
            resync,
            self.inner.displayed_gate.load(Ordering::SeqCst),
        );
        if !deliverable {
            self.inner.resync.fetch_or(resync, Ordering::SeqCst);
            return None;
        }
        self.inner
            .delivered
            .store(latest.update_id, Ordering::SeqCst);
        self.inner
            .displayed_gate
            .store(latest.state.gate, Ordering::SeqCst);
        Some(latest)
    }
}

impl Drop for UiClient {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        cut(&self.inner);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Whether the newest publication is shown to this view now.
///
/// A gate change is the one thing that does not wait for this view's own actions:
/// it decides whether what the view is about to submit is still valid, and it is
/// how a blocking interaction opened elsewhere reaches a view that is mid-action.
fn deliverable(
    latest: &Mirror,
    pending: bool,
    floor: u64,
    delivered: u64,
    resync: bool,
    displayed_gate: u64,
) -> bool {
    let gate_changed = latest.state.gate != displayed_gate;
    if gate_changed {
        return true;
    }
    !pending && latest.update_id >= floor && (resync || latest.update_id != delivered)
}

/// Ends the subscription so the attach loop reconnects (or stops).
fn cut(inner: &Inner) {
    if let Some(stream) = locked(&inner.subscription).as_ref() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
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

/// One request and its answer on a connection that serves one at a time.
fn exchange(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let (id, line) = request(method, params);
    stream.write_all(&line).map_err(|error| error.to_string())?;
    let mut answer = String::new();
    if reader
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?
        == 0
    {
        return Err("the daemon closed the connection".into());
    }
    answered(&id, &answer)
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

fn dial(inner: &Inner) -> Result<(UnixStream, BufReader<UnixStream>), String> {
    let stream = UnixStream::connect(&inner.socket).map_err(|error| error.to_string())?;
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
    let reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    Ok((stream, reader))
}

fn attach_loop(inner: &Inner) {
    let mut backoff = RETRY_FLOOR;
    while !inner.stop.load(Ordering::SeqCst) {
        match session(inner) {
            // Served for a while and then lost: start the backoff over.
            Ok(()) => backoff = RETRY_FLOOR,
            Err(_) => backoff = (backoff * 2).min(RETRY_CEILING),
        }
        *locked(&inner.link) = None;
        *locked(&inner.subscription) = None;
        if inner.connected.swap(false, Ordering::SeqCst) {
            inner.wake.notify();
        }
        let mut waited = Duration::ZERO;
        while waited < backoff && !inner.stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
            waited += Duration::from_millis(50);
        }
    }
}

fn binary_exchange(link: &mut Link, request: &UiRequest) -> Result<Reply, String> {
    content::write_frame(&mut link.action, request).map_err(|e| e.to_string())?;
    content::read_frame(&mut link.reader).map_err(|e| e.to_string())
}

fn open(inner: &Inner, mode: Open) -> Result<(UnixStream, BufReader<UnixStream>), String> {
    let (mut stream, mut reader) = dial(inner)?;
    // A stop must be able to cancel even a peer that never answers the opener.
    if matches!(mode, Open::Subscribe { .. }) {
        *locked(&inner.subscription) = Some(stream.try_clone().map_err(|e| e.to_string())?);
    }
    exchange(
        &mut stream,
        &mut reader,
        "ui.open",
        serde_json::to_value(mode).map_err(|e| e.to_string())?,
    )?;
    Ok((stream, reader))
}

/// One attachment owns an ordered action/content link and a push subscription.
fn session(inner: &Inner) -> Result<(), String> {
    let attachment = random_uuid();
    inner.incarnation.fetch_add(1, Ordering::SeqCst);
    inner.observed_update.store(0, Ordering::SeqCst);
    inner.floor.store(0, Ordering::SeqCst);
    inner.delivered.store(0, Ordering::SeqCst);
    *locked(&inner.latest) = None;
    let (action, reader) = open(inner, Open::Actions)?;
    let mut link = Link {
        action,
        reader,
        attachment: attachment.clone(),
        next_action: 1,
        processes: false,
    };
    match binary_exchange(
        &mut link,
        &UiRequest::Attach {
            attachment_id: attachment.clone(),
            role: inner.role,
            interaction_id: inner.interaction.clone(),
        },
    )? {
        Reply::Attached => {}
        other => return Err(format!("attach refused: {other:?}")),
    }
    *locked(&inner.link) = Some(link);
    let (stream, mut frames) = open(
        inner,
        Open::Subscribe {
            attachment_id: attachment.clone(),
            session_only: inner.role == dto::AttachmentRole::Presenter,
        },
    )?;
    let baseline: Frame = content::read_frame(&mut frames).map_err(|e| e.to_string())?;
    if inner.stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    inner.connected.store(true, Ordering::SeqCst);
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    handover(inner, &attachment, frames, baseline)
}

#[derive(Default)]
struct Handover {
    next: Option<Frame>,
    closed: bool,
}

/// The receiver never waits for a content RPC: it can invalidate input and
/// replace the pending target while the bounded materialiser finishes a fetch.
fn handover(
    inner: &Inner,
    attachment: &str,
    mut reader: BufReader<UnixStream>,
    baseline: Frame,
) -> Result<(), String> {
    let slot = (Mutex::new(Handover::default()), Condvar::new());
    let error = Mutex::new(None);
    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut current = Vec::new();
            loop {
                let frame = {
                    let mut state = locked(&slot.0);
                    while state.next.is_none() && !state.closed {
                        state = slot.1.wait(state).unwrap_or_else(|e| e.into_inner());
                    }
                    if state.closed {
                        return;
                    }
                    state.next.take().expect("pending head")
                };
                if let Err(problem) = publish(inner, attachment, &mut current, frame) {
                    *locked(&error) = Some(problem);
                    cut(inner);
                    return;
                }
            }
        });
        let mut frame = baseline;
        let result = loop {
            let mut state = locked(&slot.0);
            if frame.head.update_id >= inner.observed_update.load(Ordering::SeqCst) {
                let _adoption = locked(&inner.latest);
                inner.observed_gate.store(frame.head.gate, Ordering::SeqCst);
                inner
                    .observed_update
                    .store(frame.head.update_id, Ordering::SeqCst);
                state.next = Some(frame);
                slot.1.notify_one();
                inner.wake.notify();
            }
            drop(state);
            match content::read_frame(&mut reader) {
                Ok(next) if !inner.stop.load(Ordering::SeqCst) => frame = next,
                Ok(_) => break Ok(()),
                Err(problem) => break Err(problem.to_string()),
            }
        };
        locked(&slot.0).closed = true;
        slot.1.notify_one();
        let _ = worker.join();
        if let Some(problem) = locked(&error).take() {
            Err(problem)
        } else {
            result
        }
    })
}

/// Materialise exactly one head; only current payloads survive the handover.
fn publish(
    inner: &Inner,
    attachment: &str,
    current: &mut Vec<Content>,
    frame: Frame,
) -> Result<(), String> {
    if frame.head.update_id < inner.observed_update.load(Ordering::SeqCst) {
        return Ok(());
    }
    if locked(&inner.latest)
        .as_ref()
        .is_some_and(|m| m.update_id >= frame.head.update_id)
    {
        return Ok(());
    }
    let hashes = [
        &frame.head.observation,
        &frame.head.tree,
        &frame.head.session,
    ];
    let session_only = inner.role == dto::AttachmentRole::Presenter;
    let mut next = Vec::new();
    for (index, hash) in hashes.into_iter().enumerate() {
        if session_only && index != 2 {
            continue;
        }
        let held = frame
            .content
            .iter()
            .chain(current.iter())
            .find(|c| &c.hash == hash)
            .cloned();
        let payload = match held {
            Some(content) => content,
            None => {
                let mut link = locked(&inner.link);
                let link = link.as_mut().ok_or("content link is not attached")?;
                match binary_exchange(link, &UiRequest::Content { hash: hash.clone() })? {
                    Reply::Content { content } if &content.hash == hash => content,
                    Reply::Expired { .. } => return Err("content_expired".into()),
                    _ => return Err("content fetch refused".into()),
                }
            }
        };
        next.push(payload);
    }
    let session = next
        .iter()
        .find(|c| c.hash == frame.head.session)
        .ok_or("session content missing")?;
    let state: dto::SessionState = session.read().map_err(|e| e.to_string())?;
    if state.gate != frame.head.gate {
        return Err("head gate differs from session".into());
    }
    let previous = locked(&inner.latest).clone();
    let observation = if session_only {
        Arc::new(Observation::default())
    } else if let Some(previous) = previous
        .as_ref()
        .filter(|_| current.iter().any(|c| c.hash == frame.head.observation))
    {
        Arc::clone(&previous.observation)
    } else {
        Arc::new(
            next.iter()
                .find(|c| c.hash == frame.head.observation)
                .ok_or("observation missing")?
                .read()
                .map_err(|e| e.to_string())?,
        )
    };
    let rows = if session_only {
        Arc::new(Vec::new())
    } else if let Some(previous) = previous
        .as_ref()
        .filter(|_| current.iter().any(|c| c.hash == frame.head.tree))
    {
        Arc::clone(&previous.rows)
    } else {
        Arc::new(
            next.iter()
                .find(|c| c.hash == frame.head.tree)
                .ok_or("rows missing")?
                .read()
                .map_err(|e| e.to_string())?,
        )
    };
    let mut latest = locked(&inner.latest);
    if frame.head.update_id < inner.observed_update.load(Ordering::SeqCst) {
        return Ok(());
    }
    *current = next;
    *latest = Some(Arc::new(Mirror {
        attachment_id: attachment.into(),
        update_id: frame.head.update_id,
        state,
        rows,
        observation,
        diagnostic: frame.head.diagnostic,
    }));
    inner.wake.notify();
    Ok(())
}

fn action_loop(inner: &Inner, queue: &mpsc::Receiver<Pending>) {
    loop {
        let action = match queue.recv_timeout(Duration::from_millis(250)) {
            Ok(action) => action,
            Err(RecvTimeoutError::Timeout) if !inner.stop.load(Ordering::SeqCst) => {
                sync_demand(inner);
                continue;
            }
            Err(_) => return,
        };
        send(inner, action);
        sync_demand(inner);
        inner.pending.fetch_sub(1, Ordering::SeqCst);
        inner.wake.notify();
    }
}

fn sync_demand(inner: &Inner) {
    let wanted = inner.processes.load(Ordering::SeqCst);
    let mut link = locked(&inner.link);
    let Some(link) = link.as_mut() else { return };
    if link.processes == wanted {
        return;
    }
    let request = UiRequest::Demand {
        attachment_id: link.attachment.clone(),
        processes: wanted,
    };
    if matches!(binary_exchange(link, &request), Ok(Reply::Demand)) {
        link.processes = wanted;
    }
}

fn send(inner: &Inner, pending: Pending) {
    if pending.incarnation != inner.incarnation.load(Ordering::SeqCst) {
        return;
    }
    let action = pending.action;
    let mut link = locked(&inner.link);
    let Some(link) = link.as_mut() else {
        // Not attached: the action cannot be ordered, so it is not sent. The
        // baseline of the next attachment brings the replica back.
        inner.resync.store(true, Ordering::SeqCst);
        return;
    };
    // Flushing unnumbered input never creates action-number gaps.
    let ordinary = action.is_ordinary();
    if ordinary && inner.observed_gate.load(Ordering::SeqCst) != pending.gate {
        return;
    }
    let number = link.next_action;
    link.next_action += 1;
    let gate = Some(pending.gate);
    let request = UiRequest::Action {
        attachment_id: link.attachment.clone(),
        action_id: number,
        gate,
        action,
    };
    let outcome = binary_exchange(link, &request).and_then(|reply| match reply {
        Reply::Action { outcome } => Ok(outcome),
        Reply::Refused { code, .. } if code == "busy" => Ok(dto::ActionOutcome::Busy),
        other => Err(format!("unexpected action reply: {other:?}")),
    });
    match outcome {
        Ok(dto::ActionOutcome::Applied { update_id, .. })
        | Ok(dto::ActionOutcome::Replayed { update_id, .. }) => {
            inner.floor.fetch_max(update_id, Ordering::SeqCst);
        }
        Ok(dto::ActionOutcome::Refused { .. }) => {
            inner.resync.store(true, Ordering::SeqCst);
        }
        Ok(dto::ActionOutcome::Busy) => {
            link.next_action -= 1;
            inner.resync.store(true, Ordering::SeqCst);
        }
        // The ordering scope is broken or the result is unknown: a new
        // attachment, never a replay under the old numbering.
        Ok(
            dto::ActionOutcome::Gap { .. }
            | dto::ActionOutcome::ReceiptExpired
            | dto::ActionOutcome::Detached,
        )
        | Err(_) => {
            inner.resync.store(true, Ordering::SeqCst);
            cut(inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror(gate: u64, update_id: u64) -> Mirror {
        Mirror {
            attachment_id: "a".into(),
            update_id,
            state: dto::SessionState {
                gate,
                ..dto::SessionState::default()
            },
            rows: std::sync::Arc::new(Vec::new()),
            observation: Arc::new(Observation::default()),
            diagnostic: None,
        }
    }

    #[test]
    fn a_gate_change_reaches_a_view_that_owes_an_answer() {
        assert!(
            deliverable(&mirror(1, 5), true, 4, 3, false, 0),
            "a modal opened elsewhere is not hidden behind this view's own queue"
        );
        assert!(
            !deliverable(&mirror(0, 5), true, 4, 3, false, 0),
            "ordinary state still waits for the view's own answer"
        );
    }

    /// A peer that answers the attach and then never answers the subscription.
    ///
    /// This is the one shape a real daemon will not produce on demand: a
    /// subscription it accepts and leaves silent. It is the case that used to park
    /// the attach thread until the reply timeout, and it is why the socket is
    /// registered for cancellation before the handshake rather than after it.
    fn silent_subscription_peer(
        socket: &std::path::Path,
    ) -> (
        std::thread::JoinHandle<()>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let listener = std::os::unix::net::UnixListener::bind(socket).expect("bind");
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&served);
        let handle = std::thread::spawn(move || {
            let mut served = 0;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                served += 1;
                count.store(served, std::sync::atomic::Ordering::SeqCst);
                if served == 1 {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let answer = json!({"id": request["id"], "result": {"codec": "messagepack"}});
                    let mut writer = stream;
                    writeln!(writer, "{answer}").unwrap();
                    let _: UiRequest = content::read_frame(&mut reader).unwrap();
                    content::write_frame(&mut writer, &Reply::Attached).unwrap();
                    // Keep the action socket alive while accepting its subscription.
                    std::mem::drop(writer);
                    continue;
                }
                // Wait for cancellation, not a wall-clock sleep.
                let mut byte = [0];
                let _ = std::io::Read::read(&mut reader, &mut byte);
                return;
            }
        });
        (handle, served)
    }

    #[test]
    fn a_stalled_subscription_can_be_abandoned_by_dropping_the_client() {
        let directory = std::env::temp_dir().join(format!("radar-stalled-{}", random_uuid()));
        std::fs::create_dir_all(&directory).expect("dir");
        let socket = directory.join("sock");
        let (peer, served) = silent_subscription_peer(&socket);
        let client = UiClient::start(
            socket.clone(),
            dto::AttachmentRole::Dashboard,
            Wake::default(),
        );
        // Both connections have been made, so the client is past its attach and is
        // waiting on a subscription the peer will never answer.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while served.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "the client never reached its subscription"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let started = std::time::Instant::now();
        drop(client);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "dropping a client must end a stalled handshake, not wait out its timeout: {:?}",
            started.elapsed()
        );
        drop(peer);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_new_head_invalidates_input_behind_delayed_action_and_content_replies() {
        use crate::ui_session::session::Session;
        use std::io::Read;
        let directory = std::env::temp_dir().join(format!("radar-handover-{}", random_uuid()));
        std::fs::create_dir_all(&directory).unwrap();
        let socket = directory.join("sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let baseline = Session::new().current().frame(None, false);
        let delayed = Observation {
            collected_at: Some("delayed".into()),
            ..Observation::default()
        };
        let delayed = Content::encode(&delayed).unwrap();
        let newest = Observation {
            collected_at: Some("newest".into()),
            ..Observation::default()
        };
        let newest = Content::encode(&newest).unwrap();
        let one = Content::encode(&dto::SessionState {
            gate: 1,
            ..dto::SessionState::default()
        })
        .unwrap();
        let two = Content::encode(&dto::SessionState {
            gate: 2,
            ..dto::SessionState::default()
        })
        .unwrap();
        let older = Frame {
            head: super::super::publication::Head {
                update_id: 2,
                observation: delayed.hash.clone(),
                session: one.hash.clone(),
                gate: 1,
                ..baseline.head.clone()
            },
            content: vec![one],
        };
        let later = Frame {
            head: super::super::publication::Head {
                update_id: 3,
                observation: newest.hash.clone(),
                session: two.hash.clone(),
                gate: 2,
                ..baseline.head.clone()
            },
            content: vec![newest, baseline.content[1].clone(), two],
        };
        let (heads, pushed) = mpsc::sync_channel::<Frame>(1);
        let (action_started, action_seen) = mpsc::sync_channel(1);
        let (release_action, action_release) = mpsc::sync_channel(1);
        let (fetch_started, fetch_seen) = mpsc::sync_channel(1);
        let (release_fetch, fetch_release) = mpsc::sync_channel(1);
        fn accept_open(
            listener: &std::os::unix::net::UnixListener,
        ) -> (UnixStream, BufReader<UnixStream>) {
            let (mut writer, _) = listener.accept().unwrap();
            writer.set_read_timeout(Some(REPLY_TIMEOUT)).unwrap();
            let mut reader = BufReader::new(writer.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            writeln!(
                writer,
                "{}",
                json!({"id":request["id"], "result":{"codec":"messagepack"}})
            )
            .unwrap();
            (writer, reader)
        }
        let peer = thread::spawn(move || {
            thread::scope(|scope| {
                let (mut writer, mut reader) = accept_open(&listener);
                assert!(matches!(
                    content::read_frame::<_, UiRequest>(&mut reader).unwrap(),
                    UiRequest::Attach { .. }
                ));
                content::write_frame(&mut writer, &Reply::Attached).unwrap();
                scope.spawn(move || {
                assert!(matches!(content::read_frame::<_, UiRequest>(&mut reader).unwrap(), UiRequest::Action { .. }));
                action_started.send(()).unwrap();
                action_release.recv_timeout(REPLY_TIMEOUT).unwrap();
                content::write_frame(&mut writer, &Reply::Action { outcome: dto::ActionOutcome::Applied { update_id: 1, interaction_id: None, request_id: None } }).unwrap();
                assert!(matches!(content::read_frame::<_, UiRequest>(&mut reader).unwrap(), UiRequest::Content { hash } if hash == delayed.hash));
                fetch_started.send(()).unwrap();
                fetch_release.recv_timeout(REPLY_TIMEOUT).unwrap();
                content::write_frame(&mut writer, &Reply::Content { content: delayed }).unwrap();
                let _ = reader.read(&mut [0]);
            });
                let (mut writer, mut reader) = accept_open(&listener);
                content::write_frame(&mut writer, &baseline).unwrap();
                for _ in 0..2 {
                    let frame = pushed.recv_timeout(REPLY_TIMEOUT).unwrap();
                    content::write_frame(&mut writer, &frame).unwrap();
                }
                let _ = reader.read(&mut [0]);
            })
        });
        let client = UiClient::start(socket, dto::AttachmentRole::Dashboard, Wake::default());
        crate::testing::eventually("baseline", || client.poll());
        client.submit(dto::UiAction::CycleOrder);
        action_seen.recv_timeout(REPLY_TIMEOUT).unwrap();
        heads.send(older).unwrap();
        crate::testing::eventually("gate before action receipt", || {
            (client.inner.observed_gate.load(Ordering::SeqCst) == 1).then_some(())
        });
        assert!(
            client.poll().is_none(),
            "an incomplete head is not rendered"
        );
        client.submit(dto::UiAction::CycleOrder);
        assert_eq!(
            client.inner.pending.load(Ordering::SeqCst),
            1,
            "stale input is not queued"
        );
        release_action.send(()).unwrap();
        fetch_seen.recv_timeout(REPLY_TIMEOUT).unwrap();
        heads.send(later).unwrap();
        crate::testing::eventually("gate before content reply", || {
            (client.inner.observed_gate.load(Ordering::SeqCst) == 2).then_some(())
        });
        release_fetch.send(()).unwrap();
        let final_view = crate::testing::eventually("latest coherent head", || {
            let view = client.poll()?;
            assert_ne!(
                view.update_id, 2,
                "superseded fetch must not adopt its old target"
            );
            (view.update_id == 3).then_some(view)
        });
        assert_eq!(
            final_view.observation.collected_at.as_deref(),
            Some("newest")
        );
        assert_eq!(final_view.state.gate, 2);
        drop(client);
        peer.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ordinary_delivery_waits_for_the_floor_and_skips_what_was_seen() {
        assert!(
            !deliverable(&mirror(0, 5), false, 6, 3, false, 0),
            "below the floor"
        );
        assert!(
            !deliverable(&mirror(0, 5), false, 4, 5, false, 0),
            "already delivered"
        );
        assert!(
            deliverable(&mirror(0, 5), false, 4, 5, true, 0),
            "a resync shows it again"
        );
    }
}
