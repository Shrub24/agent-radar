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
                    // The attach answer: enough of a publication for the client to
                    // proceed to the subscription it will never get.
                    let attach = dto::AttachResult {
                        attachment_id: "a".into(),
                        publication: dto::Publication {
                            update_id: 1,
                            tree: dto::TreeProjection {
                                hash: "h".into(),
                                rows: Some(Vec::new()),
                            },
                            state_hash: "s".into(),
                            session: dto::SessionState::default(),
                        },
                    };
                    let id = line
                        .split("\"id\":\"")
                        .nth(1)
                        .and_then(|rest| rest.split('"').next())
                        .unwrap_or_default()
                        .to_string();
                    let answer = serde_json::json!({"id": id, "result": attach});
                    let mut writer = stream;
                    use std::io::Write;
                    let _ = writer.write_all(format!("{answer}\n").as_bytes());
                    continue;
                }
                // The subscription: accepted by silence, for as long as the test
                // leaves this thread alive.
                std::thread::sleep(std::time::Duration::from_secs(30));
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
