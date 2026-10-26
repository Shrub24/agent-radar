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
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;

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
}

struct Link {
    action: UnixStream,
    reader: BufReader<UnixStream>,
    attachment: String,
    next_action: u64,
}

struct Inner {
    socket: PathBuf,
    role: dto::AttachmentRole,
    interaction: Option<String>,
    incarnation: AtomicU64,
    displayed_gate: AtomicU64,
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
    pub fn poll(&self) -> Option<Arc<Mirror>> {
        if self.inner.pending.load(Ordering::SeqCst) > 0 {
            return None;
        }
        let latest = locked(&self.inner.latest).clone()?;
        let resync = self.inner.resync.swap(false, Ordering::SeqCst);
        if latest.update_id < self.inner.floor.load(Ordering::SeqCst) {
            self.inner.resync.fetch_or(resync, Ordering::SeqCst);
            return None;
        }
        if latest.update_id == self.inner.delivered.load(Ordering::SeqCst) && !resync {
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

/// One attachment: attach, subscribe, and publish until the stream ends.
fn session(inner: &Inner) -> Result<(), String> {
    let attachment = random_uuid();
    inner.incarnation.fetch_add(1, Ordering::SeqCst);
    let (mut action, mut reader) = dial(inner)?;
    // Update ids belong to one daemon's counter; a new attachment starts over.
    inner.floor.store(0, Ordering::SeqCst);
    inner.delivered.store(0, Ordering::SeqCst);
    *locked(&inner.latest) = None;
    let attached = exchange(
        &mut action,
        &mut reader,
        "ui.attach",
        json!({"attachment_id": attachment, "role": inner.role, "interaction_id": inner.interaction}),
    )?;
    let attached: dto::AttachResult =
        serde_json::from_value(attached).map_err(|error| error.to_string())?;
    let mut cache: Option<(String, Arc<Vec<dto::TreeRow>>)> = None;
    let baseline = publish(inner, &attachment, &mut cache, attached.publication)?;

    let (mut stream, _) = dial(inner)?;
    let (id, line) = request(
        "ui.subscribe",
        json!({"attachment_id": attachment, "known_tree_hash": baseline}),
    );
    stream.write_all(&line).map_err(|error| error.to_string())?;
    // Registered for cancellation before the handshake completes, so a stop or a
    // re-attach ends this attempt instead of waiting out its reply timeout.
    *locked(&inner.subscription) = Some(stream.try_clone().map_err(|error| error.to_string())?);
    // The subscription's first answer is the readiness barrier: the daemon has
    // accepted it and this attachment owns a stream. Reading it through the same
    // buffered reader the push loop uses keeps read-ahead with the frame stream.
    let mut frames = BufReader::new(stream);
    let mut accepted = String::new();
    if frames
        .read_line(&mut accepted)
        .map_err(|error| error.to_string())?
        == 0
    {
        return Err("the subscription closed before it was accepted".to_string());
    }
    let result = answered(&id, &accepted)?;
    let publication: dto::Publication =
        serde_json::from_value(result).map_err(|error| error.to_string())?;
    *locked(&inner.link) = Some(Link {
        action,
        reader,
        attachment: attachment.clone(),
        next_action: 1,
    });
    // A stop requested while this attachment was being made found no stream to
    // cut, so it is honoured here rather than blocking on a quiet one.
    if inner.stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Connected before the baseline is published, so a view that sees a mirror
    // never sees one from a session it is not yet connected to.
    inner.connected.store(true, Ordering::SeqCst);
    publish(inner, &attachment, &mut cache, publication)?;
    // The baseline is in hand, so the subscription is alive: from here it waits
    // without a read timeout, because a quiet fleet is not a failure. Until here
    // it kept the reply timeout, so a subscription the daemon accepts and then
    // never serves is abandoned and retried instead of parking the attach thread
    // for the life of the process.
    let _ = frames.get_ref().set_read_timeout(None);
    inner.wake.notify();

    loop {
        let mut line = String::new();
        let read = frames
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 || inner.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let result = answered(&id, &line)?;
        let publication: dto::Publication =
            serde_json::from_value(result).map_err(|error| error.to_string())?;
        publish(inner, &attachment, &mut cache, publication)?;
    }
}

/// Resolves the rows a publication names and stores it as the newest state.
fn publish(
    inner: &Inner,
    attachment: &str,
    cache: &mut Option<(String, Arc<Vec<dto::TreeRow>>)>,
    publication: dto::Publication,
) -> Result<String, String> {
    let hash = publication.tree.hash;
    let rows = match (publication.tree.rows, cache.as_ref()) {
        (Some(rows), _) => Arc::new(rows),
        (None, Some((held, rows))) if *held == hash => Arc::clone(rows),
        // The frame assumed content this client does not hold: fetch it, and if
        // it has been evicted take a fresh attachment's baseline instead.
        (None, _) => {
            let (mut stream, mut reader) = dial(inner)?;
            let content = exchange(
                &mut stream,
                &mut reader,
                "ui.content.get",
                json!({"hash": hash}),
            )?;
            let content: dto::ContentGetResult =
                serde_json::from_value(content).map_err(|error| error.to_string())?;
            Arc::new(content.rows)
        }
    };
    *cache = Some((hash.clone(), Arc::clone(&rows)));
    *locked(&inner.latest) = Some(Arc::new(Mirror {
        attachment_id: attachment.into(),
        update_id: publication.update_id,
        state: publication.session,
        rows,
    }));
    inner.wake.notify();
    Ok(hash)
}

fn action_loop(inner: &Inner, queue: &mpsc::Receiver<Pending>) {
    loop {
        let action = match queue.recv_timeout(Duration::from_millis(250)) {
            Ok(action) => action,
            Err(RecvTimeoutError::Timeout) if !inner.stop.load(Ordering::SeqCst) => continue,
            Err(_) => return,
        };
        send(inner, action);
        inner.pending.fetch_sub(1, Ordering::SeqCst);
        inner.wake.notify();
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
    if ordinary
        && locked(&inner.latest)
            .as_ref()
            .is_some_and(|m| m.state.gate != pending.gate)
    {
        return;
    }
    let number = link.next_action;
    link.next_action += 1;
    let gate = Some(pending.gate);
    let outcome = exchange(
        &mut link.action,
        &mut link.reader,
        "ui.action",
        json!({"attachment_id": link.attachment, "action_id": number, "gate": gate, "action": action}),
    )
    .and_then(|value| {
        serde_json::from_value::<dto::ActionOutcome>(value).map_err(|error| error.to_string())
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
