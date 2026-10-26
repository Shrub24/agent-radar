//! The thread that owns the [`Session`], and the handle everything else uses.
//!
//! One dedicated thread runs a synchronous mailbox loop over bounded channels.
//! It never waits on the backend, the filesystem or a domain operation: it
//! applies a message, answers it, and publishes the newest state. Delivery to
//! a presentation is the transport's job; here a presentation only holds a
//! [`Subscription`] on a latest-state watch, so a slow one can skip states and
//! can never hold the actor up.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::session::{Published, Session};
use crate::control_plane::observation::{Classes, Hub, Snapshot};
use crate::control_plane::public_contracts::ui_session as dto;

const MAILBOX: usize = 256;
/// Queued messages one pass may take before it publishes.
const DRAIN: usize = 32;
/// How long a caller waits for the actor before it is told the session is busy.
const REPLY: Duration = Duration::from_secs(2);
/// An attachment that never subscribes is released after this long.
const UNSUBSCRIBED: Duration = Duration::from_secs(30);

/// Why the session could not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiError {
    /// The mailbox is full or the actor did not answer in time. Retryable.
    Busy,
    /// The session has stopped.
    Stopped,
    Refused(dto::RefusalCode),
}

type Reply<T> = SyncSender<T>;

enum Msg {
    Observe(Arc<Snapshot>),
    Attach(
        String,
        dto::AttachmentRole,
        Reply<Result<(), dto::RefusalCode>>,
    ),
    Detach(String),
    Act(
        String,
        u64,
        Option<u64>,
        dto::UiAction,
        Reply<dto::ActionOutcome>,
    ),
    Content(String, Reply<Option<Arc<Vec<dto::TreeRow>>>>),
    Subscribe(String, Reply<Result<(), dto::RefusalCode>>),
    Stop,
}

/// A cheap, cloneable way to talk to the session.
#[derive(Clone)]
pub struct UiHandle {
    tx: SyncSender<Msg>,
    published: watch::Receiver<Arc<Published>>,
    attached: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
}

/// Owns the actor thread. Dropping it stops the session.
pub struct UiService {
    handle: UiHandle,
    thread: Option<JoinHandle<()>>,
}

impl UiService {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::sync_channel(MAILBOX);
        let session = Session::new();
        let (publish, published) = watch::channel(Arc::new(session.current().clone()));
        let attached = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let thread = {
            let attached = Arc::clone(&attached);
            let stopped = Arc::clone(&stopped);
            thread::Builder::new()
                .name("radar-ui-session".into())
                .spawn(move || {
                    run(session, rx, publish, &attached);
                    stopped.store(true, Ordering::SeqCst);
                })
                .ok()
        };
        Self {
            handle: UiHandle {
                tx,
                published,
                attached,
                stopped,
            },
            thread,
        }
    }

    pub fn handle(&self) -> UiHandle {
        self.handle.clone()
    }
}

impl Drop for UiService {
    fn drop(&mut self) {
        // Handles are cloned into the daemon and its streams, so the mailbox
        // never disconnects on its own: the loop is told to stop.
        self.handle.stopped.store(true, Ordering::SeqCst);
        let _ = self.handle.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    mut session: Session,
    rx: mpsc::Receiver<Msg>,
    publish: watch::Sender<Arc<Published>>,
    attached: &AtomicUsize,
) {
    let mut unsubscribed: HashMap<String, Instant> = HashMap::new();
    loop {
        let first = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(message) => Some(message),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut first = first;
        for pass in 0..DRAIN {
            let message = if pass == 0 {
                first.take()
            } else {
                rx.try_recv().ok()
            };
            let Some(message) = message else { break };
            match message {
                Msg::Stop => return,
                Msg::Observe(snapshot) => {
                    session.observe(&snapshot.observation, snapshot.tasks.as_ref());
                }
                Msg::Attach(id, role, reply) => {
                    let result = session.attach(&id, role);
                    if result.is_ok() {
                        unsubscribed.insert(id, Instant::now());
                    }
                    let _ = reply.send(result);
                }
                Msg::Detach(id) => {
                    unsubscribed.remove(&id);
                    session.detach(&id);
                }
                Msg::Act(id, number, gate, action, reply) => {
                    let _ = reply.send(session.act_stamped(&id, number, gate, action));
                }
                Msg::Content(hash, reply) => {
                    let _ = reply.send(session.content(&hash));
                }
                Msg::Subscribe(id, reply) => {
                    // One primary subscription per attachment: the marker is
                    // the attachment leaving the unsubscribed set, and a second
                    // request finds it already gone.
                    let result = if unsubscribed.remove(&id).is_some() {
                        Ok(())
                    } else if session.attachments_contain(&id) {
                        Err(dto::RefusalCode::Conflict)
                    } else {
                        Err(dto::RefusalCode::Unavailable)
                    };
                    let _ = reply.send(result);
                }
            }
        }
        let now = Instant::now();
        let expired: Vec<String> = unsubscribed
            .iter()
            .filter(|(_, since)| now.duration_since(**since) > UNSUBSCRIBED)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            unsubscribed.remove(&id);
            session.detach(&id);
        }
        attached.store(session.attachments(), Ordering::SeqCst);
        if session.current().update_id != publish.borrow().update_id {
            publish.send_replace(Arc::new(session.current().clone()));
        }
    }
}

impl UiHandle {
    pub fn attachments(&self) -> usize {
        self.attached.load(Ordering::SeqCst)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn observe(&self, snapshot: Arc<Snapshot>) {
        let _ = self.tx.send(Msg::Observe(snapshot));
    }

    fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> Msg) -> Result<T, UiError> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.tx
            .try_send(build(reply))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => UiError::Busy,
                mpsc::TrySendError::Disconnected(_) => UiError::Stopped,
            })?;
        answer.recv_timeout(REPLY).map_err(|error| match error {
            RecvTimeoutError::Timeout => UiError::Busy,
            RecvTimeoutError::Disconnected => UiError::Stopped,
        })
    }

    /// Registers an attachment and returns the current state as its baseline.
    pub fn attach(
        &self,
        id: &str,
        role: dto::AttachmentRole,
    ) -> Result<dto::AttachResult, UiError> {
        self.ask(|reply| Msg::Attach(id.to_string(), role, reply))?
            .map_err(UiError::Refused)?;
        let current = Arc::clone(&self.published.borrow());
        Ok(dto::AttachResult {
            attachment_id: id.to_string(),
            publication: current.publication(None),
        })
    }

    pub fn detach(&self, id: &str) {
        let _ = self.tx.send(Msg::Detach(id.to_string()));
    }

    pub fn act(
        &self,
        id: &str,
        number: u64,
        gate: Option<u64>,
        action: dto::UiAction,
    ) -> Result<dto::ActionOutcome, UiError> {
        match self.ask(|reply| Msg::Act(id.to_string(), number, gate, action, reply)) {
            Err(UiError::Busy) => Ok(dto::ActionOutcome::Busy),
            other => other,
        }
    }

    pub fn content(&self, hash: &str) -> Result<Option<Arc<Vec<dto::TreeRow>>>, UiError> {
        self.ask(|reply| Msg::Content(hash.to_string(), reply))
    }

    /// Takes the attachment's one primary subscription.
    pub fn subscribe(&self, id: &str, known: Option<String>) -> Result<Subscription, UiError> {
        self.ask(|reply| Msg::Subscribe(id.to_string(), reply))?
            .map_err(UiError::Refused)?;
        let mut published = self.published.clone();
        // Marks the current value unseen so the first frame is the baseline.
        published.mark_changed();
        Ok(Subscription {
            handle: self.clone(),
            attachment: id.to_string(),
            published,
            known,
            sent: None,
        })
    }
}

/// One attachment's push stream: the latest state, skipping what it was too
/// slow to read. Dropping it releases the attachment.
pub struct Subscription {
    handle: UiHandle,
    attachment: String,
    published: watch::Receiver<Arc<Published>>,
    known: Option<String>,
    sent: Option<u64>,
}

impl Subscription {
    /// Waits for a state newer than the last one delivered. `None` means the
    /// session has stopped.
    pub async fn next(&mut self) -> Option<dto::Publication> {
        loop {
            {
                let current = self.published.borrow_and_update();
                if self.sent.is_none_or(|sent| current.update_id > sent) {
                    let publication = current.publication(self.known.as_deref());
                    self.sent = Some(current.update_id);
                    self.known = Some(current.tree_hash.clone());
                    return Some(publication);
                }
            }
            if self.published.changed().await.is_err() {
                return None;
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.handle.detach(&self.attachment);
    }
}

/// Feeds the session from the daemon's observation sampler, and keeps the
/// sampler's demand alive while anything is attached.
pub fn feed(hub: Arc<Hub>, handle: UiHandle, stopping: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    thread::Builder::new()
        .name("radar-ui-feed".into())
        .spawn(move || {
            let mut seen = 0;
            while !stopping.load(Ordering::SeqCst) && !handle.is_stopped() {
                if handle.attachments() > 0 {
                    hub.demand(Classes { processes: true });
                }
                let (revision, snapshot) = hub.current();
                if revision != seen {
                    seen = revision;
                    if let Some(snapshot) = snapshot {
                        handle.observe(snapshot);
                    }
                }
                hub.wait_for_newer(seen, Duration::from_millis(500));
            }
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dto::{ActionOutcome as Out, UiAction as A};

    fn snapshot() -> Arc<Snapshot> {
        let mut observation = crate::observation::ObservationState::new();
        observation.apply_success(crate::model::testing::fleet());
        Arc::new(Snapshot {
            epoch: "e".into(),
            revision: 1,
            collected_at: None,
            sources: Vec::new(),
            tasks: None,
            observation,
        })
    }

    #[tokio::test]
    async fn a_subscriber_gets_a_baseline_then_only_newer_state() {
        let service = UiService::start();
        let handle = service.handle();
        handle.observe(snapshot());
        let attached = handle.attach("a", dto::AttachmentRole::Dashboard).unwrap();
        let mut stream = handle.subscribe("a", None).unwrap();
        let baseline = stream.next().await.unwrap();
        assert!(baseline.update_id >= attached.publication.update_id);
        assert!(baseline.tree.rows.is_some());

        // Actions travel a different path from the stream and never wait on it.
        assert!(matches!(
            handle.act("a", 1, None, A::Move { delta: 1 }).unwrap(),
            Out::Applied { .. }
        ));
        let moved = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("an update is pushed without a request")
            .unwrap();
        assert!(moved.update_id > baseline.update_id);
        // The tree did not change, so its rows are not sent again.
        assert!(moved.tree.rows.is_none());
        assert_eq!(moved.tree.hash, baseline.tree.hash);
    }

    #[tokio::test]
    async fn a_second_subscription_is_refused_and_dropping_one_releases_the_attachment() {
        let service = UiService::start();
        let handle = service.handle();
        handle.attach("a", dto::AttachmentRole::Sidebar).unwrap();
        let stream = handle.subscribe("a", None).unwrap();
        assert_eq!(
            handle.subscribe("a", None).err(),
            Some(UiError::Refused(dto::RefusalCode::Conflict))
        );
        drop(stream);
        let released = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            handle.attachments() == 0
        });
        assert!(released, "losing the subscription detaches its attachment");
        assert_eq!(
            handle.act("a", 1, None, A::CycleOrder).unwrap(),
            Out::Detached
        );
    }
}
