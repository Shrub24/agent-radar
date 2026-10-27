//! The thread that owns the [`Session`], and the handle everything else uses.
//!
//! One dedicated thread runs a synchronous mailbox loop over bounded channels.
//! It never waits on the backend, the filesystem or a domain operation: it
//! applies a message, answers it, and publishes the newest state. Delivery to
//! a presentation is the transport's job; here a presentation only holds a
//! [`Subscription`] on a latest-state watch, so a slow one can skip states and
//! can never hold the actor up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::session::{Published, Session};
use crate::control_plane::observation::{Classes, Hub, Snapshot};
use crate::control_plane::public_contracts::ui_session as dto;

const MAILBOX: usize = 256;
/// Queued messages one pass may take before it publishes.
const DRAIN: usize = 32;

/// Why the session could not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiError {
    /// The mailbox is full or the actor did not answer in time. Retryable.
    Busy,
    /// The session has stopped.
    Stopped,
    Refused(dto::RefusalCode),
}

pub enum Effect {
    Dispatch(Box<super::gate::Completion>),
    Reconcile(String),
}

enum Reply<T> {
    Async(tokio::sync::oneshot::Sender<T>),
}

impl<T> Reply<T> {
    fn send(self, value: T) -> Result<(), T> {
        match self {
            Self::Async(sender) => sender.send(value),
        }
    }
}

enum Msg {
    Observe(
        Box<super::publication::Observation>,
        crate::content::Content,
    ),
    ContentFailed(dto::PublicationFailure),
    Encoded(String, Reply<Option<crate::content::Content>>),
    Demand(String, bool, Reply<Result<(), dto::RefusalCode>>),
    Open(
        String,
        dto::AttachmentRole,
        Option<String>,
        Weak<()>,
        Reply<Result<watch::Receiver<bool>, dto::RefusalCode>>,
    ),
    Act(
        String,
        u64,
        Option<u64>,
        dto::UiAction,
        Reply<dto::ActionOutcome>,
    ),
    Finished(dto::CommandOutcome),
    Stop,
}

/// A cheap, cloneable way to talk to the session.
#[derive(Clone)]
pub struct UiHandle {
    tx: SyncSender<Msg>,
    published: watch::Receiver<Arc<Published>>,
    attached: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    processes: Arc<AtomicBool>,
}

/// Owns the actor thread. Dropping it stops the session.
pub struct UiService {
    handle: UiHandle,
    thread: Option<JoinHandle<()>>,
}

impl UiService {
    pub fn start() -> Self {
        Self::with_executor(None)
    }

    pub fn with_executor(
        executor: Option<Box<dyn Fn(Effect) -> dto::CommandOutcome + Send>>,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(MAILBOX);
        let (work_tx, work_rx) = mpsc::sync_channel::<Effect>(8);
        let done_tx = tx.clone();
        let work = executor.map(|execute| {
            thread::Builder::new()
                .name("radar-ui-effects".into())
                .spawn(move || {
                    while let Ok(job) = work_rx.recv() {
                        let id = match &job {
                            Effect::Dispatch(job) => job.request_id.clone(),
                            Effect::Reconcile(id) => id.clone(),
                        };
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(job)))
                                .unwrap_or(dto::CommandOutcome::new(
                                    &id,
                                    dto::CommandStatus::SubmissionUncertain,
                                ));
                        if done_tx.send(Msg::Finished(outcome)).is_err() {
                            break;
                        }
                    }
                })
                .expect("start UI effect worker");
            work_tx
        });
        let session = Session::new();
        let (publish, published) = watch::channel(Arc::new(session.current().clone()));
        let attached = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let processes = Arc::new(AtomicBool::new(false));
        let thread = {
            let attached = Arc::clone(&attached);
            let stopped = Arc::clone(&stopped);
            let processes = Arc::clone(&processes);
            thread::Builder::new()
                .name("radar-ui-session".into())
                .spawn(move || {
                    run(session, rx, publish, &attached, &processes, work);
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
                processes,
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

/// A dispatched effect whose answer is owed.
///
/// It keeps asking the daemon for the request's record, and it says so once: an
/// operation repeatedly re-announced as uncertain tells the operator nothing the
/// first announcement did not, while the repeated publication bumps the state
/// every interval. The entry leaves this map only on a verdict, so an operation
/// that stays in flight is still being asked about.
/// How often an owed answer is looked for.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

struct Owed {
    since: Instant,
    published: bool,
}

impl Owed {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            published: false,
        }
    }

    /// Whether this interval's look for the answer has come round.
    fn due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.since) < RECONCILE_INTERVAL {
            return false;
        }
        self.since = now;
        true
    }

    /// Whether publishing uncertainty now would say something new.
    fn mark_published(&mut self) -> bool {
        if self.published {
            return false;
        }
        self.published = true;
        true
    }
}

fn reap_attachment(session: &mut Session, leases: &mut HashMap<String, Weak<()>>, id: &str) {
    if leases
        .get(id)
        .is_some_and(|lease| lease.strong_count() == 0)
    {
        leases.remove(id);
        session.detach(id);
    }
}

fn run(
    mut session: Session,
    rx: mpsc::Receiver<Msg>,
    publish: watch::Sender<Arc<Published>>,
    attached: &AtomicUsize,
    processes: &AtomicBool,
    work: Option<SyncSender<Effect>>,
) {
    let mut pending = HashMap::<String, Owed>::new();
    let mut process_demand = std::collections::HashSet::new();
    let mut subscriptions: HashMap<String, watch::Sender<bool>> = HashMap::new();
    let mut leases: HashMap<String, Weak<()>> = HashMap::new();
    let mut clock = Instant::now();
    loop {
        let first = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(message) => Some(message),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // A connection that has gone is discovered without asking it to speak:
        // the lease it held is the only thing that had to survive the socket.
        let gone: Vec<String> = leases
            .iter()
            .filter(|(_, lease)| lease.strong_count() == 0)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            leases.remove(&id);
            session.detach(&id);
        }
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
                Msg::Observe(observation, content) => {
                    session.observe_content(&observation, content);
                }
                Msg::ContentFailed(error) => {
                    session.observation_failed(error);
                }
                Msg::Encoded(hash, reply) => {
                    let _ = reply.send(session.encoded_content(&hash));
                }
                Msg::Demand(id, wanted, reply) => {
                    reap_attachment(&mut session, &mut leases, &id);
                    let result = if session.attachments_contain(&id) {
                        if wanted {
                            process_demand.insert(id);
                        } else {
                            process_demand.remove(&id);
                        }
                        Ok(())
                    } else {
                        Err(dto::RefusalCode::Unavailable)
                    };
                    let _ = reply.send(result);
                }
                Msg::Open(id, role, interaction, lease, reply) => {
                    let result = if lease.strong_count() == 0 {
                        Err(dto::RefusalCode::Unavailable)
                    } else {
                        session.attach(&id, role).and_then(|()| {
                            if let Some(interaction) = interaction {
                                let claim = if role == dto::AttachmentRole::Presenter {
                                    session.claim_attachment(&id, &interaction)
                                } else {
                                    Err(dto::RefusalCode::Role)
                                };
                                if let Err(code) = claim {
                                    session.detach(&id);
                                    return Err(code);
                                }
                            }
                            let (alive, receiver) = watch::channel(true);
                            subscriptions.insert(id.clone(), alive);
                            leases.insert(id.clone(), lease);
                            Ok(receiver)
                        })
                    };
                    if result.is_ok() && session.current().update_id != publish.borrow().update_id {
                        publish.send_replace(Arc::new(session.current().clone()));
                    }
                    let _ = reply.send(result);
                }
                Msg::Act(id, number, gate, action, reply) => {
                    reap_attachment(&mut session, &mut leases, &id);
                    let _ = reply.send(session.act_stamped(&id, number, gate, action));
                }
                Msg::Finished(outcome) => {
                    if !matches!(
                        outcome.status,
                        dto::CommandStatus::SubmissionUncertain | dto::CommandStatus::Admitted
                    ) {
                        pending.remove(&outcome.request_id);
                    }
                    session.operation(outcome);
                }
            }
        }
        for job in session.take_completions() {
            let id = job.request_id.clone();
            let status = if work
                .as_ref()
                .is_some_and(|work| work.try_send(Effect::Dispatch(Box::new(job))).is_ok())
            {
                pending.insert(id.clone(), Owed::new(Instant::now()));
                dto::CommandStatus::DispatcherAccepted
            } else {
                dto::CommandStatus::DispatchRefused
            };
            session.operation(dto::CommandOutcome::new(&id, status));
        }
        let expired: Vec<_> = leases
            .iter()
            .filter_map(|(id, lease)| (lease.strong_count() == 0).then_some(id.clone()))
            .collect();
        for id in expired {
            leases.remove(&id);
            session.detach(&id);
        }
        let now = Instant::now();
        for (id, owed) in &mut pending {
            if !owed.due(now) {
                continue;
            }
            if let Some(work) = &work {
                let _ = work.try_send(Effect::Reconcile(id.clone()));
            }
            // The published uncertainty is a statement that the answer has not
            // come, so it is made once per request: repeating it every interval
            // republishes the same fact, costs a redraw and hides nothing new.
            if owed.mark_published() {
                session.operation(dto::CommandOutcome::new(
                    id,
                    dto::CommandStatus::SubmissionUncertain,
                ));
            }
        }
        let elapsed = now.duration_since(clock).as_secs();
        if elapsed > 0 {
            session.tick(elapsed);
            clock += Duration::from_secs(elapsed);
        }
        subscriptions.retain(|id, alive| {
            if session.attachments_contain(id) {
                true
            } else {
                alive.send_replace(false);
                false
            }
        });
        process_demand.retain(|id| session.attachments_contain(id));
        processes.store(!process_demand.is_empty(), Ordering::SeqCst);
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
        let observation = super::publication::Observation::from(snapshot.as_ref());
        let message = match crate::content::Content::encode(&observation) {
            Ok(content) => Msg::Observe(Box::new(observation), content),
            Err(error) => Msg::ContentFailed(error.into()),
        };
        let _ = self.tx.send(message);
    }

    pub fn head(&self) -> super::publication::Head {
        self.published.borrow().head()
    }

    fn enqueue<T>(
        &self,
        build: impl FnOnce(Reply<T>) -> Msg,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, UiError> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.tx
            .try_send(build(Reply::Async(reply)))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => UiError::Busy,
                mpsc::TrySendError::Disconnected(_) => UiError::Stopped,
            })?;
        Ok(answer)
    }

    pub fn enqueue_content(
        &self,
        hash: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<Option<crate::content::Content>>, UiError> {
        self.enqueue(|reply| Msg::Encoded(hash.into(), reply))
    }

    pub fn enqueue_demand(
        &self,
        id: &str,
        processes: bool,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<(), dto::RefusalCode>>, UiError> {
        self.enqueue(|reply| Msg::Demand(id.into(), processes, reply))
    }

    pub fn enqueue_attachment(
        &self,
        id: &str,
        role: dto::AttachmentRole,
        interaction: Option<String>,
        lease: &Arc<()>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<watch::Receiver<bool>, dto::RefusalCode>>,
        UiError,
    > {
        self.enqueue(|reply| {
            Msg::Open(
                id.to_string(),
                role,
                interaction,
                Arc::downgrade(lease),
                reply,
            )
        })
    }

    pub fn enqueue_action(
        &self,
        id: &str,
        number: u64,
        gate: Option<u64>,
        action: dto::UiAction,
    ) -> Result<tokio::sync::oneshot::Receiver<dto::ActionOutcome>, UiError> {
        self.enqueue(|reply| Msg::Act(id.to_string(), number, gate, action, reply))
    }

    pub async fn open_duplex(
        &self,
        id: &str,
        role: dto::AttachmentRole,
        interaction: Option<String>,
    ) -> Result<Subscription, UiError> {
        let lease = Arc::new(());
        let alive = self
            .enqueue_attachment(id, role, interaction, &lease)?
            .await
            .map_err(|_| UiError::Stopped)?
            .map_err(UiError::Refused)?;
        let mut published = self.published.clone();
        published.mark_changed();
        Ok(Subscription {
            published,
            known_head: None,
            alive,
            _lease: lease,
        })
    }
}

/// One attachment's publication stream: the latest state, skipping heads the
/// connection was too slow to take. Dropping it releases the attachment.
pub struct Subscription {
    published: watch::Receiver<Arc<Published>>,
    /// The last head this connection fully wrote; everything after it is news.
    known_head: Option<super::publication::Head>,
    alive: watch::Receiver<bool>,
    /// Held for the attachment's lifetime: dropping it is the release signal.
    _lease: Arc<()>,
}

impl Subscription {
    pub async fn prepare_frame(&mut self, session_only: bool) -> Option<super::publication::Frame> {
        loop {
            if !*self.alive.borrow() {
                return None;
            }
            {
                let current = self.published.borrow_and_update();
                let sent = self.known_head.as_ref().map(|head| head.update_id);
                if sent.is_none_or(|sent| current.update_id > sent) {
                    return Some(current.frame(self.known_head.as_ref(), session_only));
                }
            }
            tokio::select! {
                result = self.published.changed() => if result.is_err() { return None; },
                _ = self.alive.changed() => return None,
            }
        }
    }

    pub fn sent_frame(&mut self, head: &super::publication::Head) {
        self.known_head = Some(head.clone());
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
                    hub.demand(Classes {
                        processes: handle.processes.load(Ordering::SeqCst),
                    });
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
    async fn process_demand_is_attachment_scoped_and_released_on_loss() {
        let service = UiService::start();
        let handle = service.handle();
        let a = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let _b = handle
            .open_duplex("b", dto::AttachmentRole::Sidebar, None)
            .await
            .unwrap();
        handle
            .enqueue_demand("a", true)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        handle
            .enqueue_demand("b", true)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        crate::testing::eventually("process demand", || {
            handle.processes.load(Ordering::SeqCst).then_some(())
        });
        // The second owner remains; clearing it releases demand, not merely one
        // boolean overwritten by whichever attachment spoke last.
        drop(a);
        handle
            .enqueue_demand("b", false)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        crate::testing::eventually("process demand released", || {
            (!handle.processes.load(Ordering::SeqCst)).then_some(())
        });
        assert_eq!(
            handle.enqueue_demand("a", true).unwrap().await.unwrap(),
            Err(dto::RefusalCode::Unavailable)
        );
    }

    #[test]
    fn a_released_lease_is_checked_before_action_admission() {
        let mut session = Session::new();
        session.attach("a", dto::AttachmentRole::Dashboard).unwrap();
        let lease = Arc::new(());
        let mut leases = HashMap::from([("a".to_owned(), Arc::downgrade(&lease))]);
        drop(lease);
        reap_attachment(&mut session, &mut leases, "a");
        assert!(matches!(
            session.act("a", 1, A::Move { delta: 1 }),
            Out::Detached
        ));
    }

    #[tokio::test]
    async fn dropping_a_duplex_lease_releases_its_attachment() {
        let service = UiService::start();
        let handle = service.handle();
        let lease = Arc::new(());
        let alive = handle
            .enqueue_attachment("duplex", dto::AttachmentRole::Dashboard, None, &lease)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(*alive.borrow());
        drop(lease);
        crate::testing::eventually("released attachment", || (!*alive.borrow()).then_some(()));
        let outcome = handle
            .enqueue_action("duplex", 1, None, A::Move { delta: 1 })
            .unwrap()
            .await
            .unwrap();
        assert!(matches!(outcome, Out::Detached));
    }

    #[tokio::test]
    async fn an_unsent_candidate_does_not_advance_content_knowledge() {
        let service = UiService::start();
        let handle = service.handle();
        handle.observe(snapshot());
        let mut stream = handle
            .open_duplex("duplex", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let first = stream.prepare_frame(false).await.unwrap();
        assert_eq!(first.content.len(), 3);
        handle
            .enqueue_action("duplex", 1, None, A::Move { delta: 1 })
            .unwrap()
            .await
            .unwrap();
        crate::testing::eventually("new publication", || {
            (handle.head().update_id > first.head.update_id).then_some(())
        });
        let later = stream.prepare_frame(false).await.unwrap();
        assert_eq!(later.content.len(), 3);
        stream.sent_frame(&later.head);
        assert!(later.head.update_id > first.head.update_id);
    }

    #[tokio::test]
    async fn an_enqueued_action_answers_without_blocking_the_caller() {
        let service = UiService::start();
        let handle = service.handle();
        handle.observe(snapshot());
        let _attached = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let answer = handle
            .enqueue_action("a", 1, None, A::Move { delta: 1 })
            .unwrap();
        assert!(matches!(answer.await.unwrap(), Out::Applied { .. }));
    }

    #[test]
    fn a_full_mailbox_rejects_before_an_action_is_enqueued() {
        let (tx, _rx) = mpsc::sync_channel(1);
        assert!(tx.try_send(Msg::Stop).is_ok());
        let session = Session::new();
        let (_, published) = watch::channel(Arc::new(session.current().clone()));
        let handle = UiHandle {
            tx,
            published,
            attached: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicBool::new(false)),
            processes: Arc::new(AtomicBool::new(false)),
        };
        assert!(matches!(
            handle.enqueue_action("a", 1, None, A::Move { delta: 1 }),
            Err(UiError::Busy)
        ));
    }

    #[tokio::test]
    async fn a_subscriber_gets_a_baseline_then_only_newer_state() {
        let service = UiService::start();
        let handle = service.handle();
        handle.observe(snapshot());
        let mut stream = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let baseline = stream.prepare_frame(false).await.unwrap();
        stream.sent_frame(&baseline.head);
        assert_eq!(baseline.content.len(), 3);

        // Actions travel a different path from the publication and never wait on it.
        assert!(matches!(
            handle
                .enqueue_action("a", 1, None, A::Move { delta: 1 })
                .unwrap()
                .await
                .unwrap(),
            Out::Applied { .. }
        ));
        let moved = tokio::time::timeout(Duration::from_secs(2), stream.prepare_frame(false))
            .await
            .expect("an update is pushed without a request")
            .unwrap();
        assert!(moved.head.update_id > baseline.head.update_id);
        assert_eq!(moved.content.len(), 1);
        assert_eq!(moved.head.tree, baseline.head.tree);
    }

    #[tokio::test]
    async fn losing_the_connection_releases_the_attachment_and_its_publication() {
        let service = UiService::start();
        let handle = service.handle();
        let mut stream = handle
            .open_duplex("a", dto::AttachmentRole::Sidebar, None)
            .await
            .unwrap();
        stream.prepare_frame(false).await.unwrap();
        // The same attachment cannot be opened twice: one connection owns it.
        assert_eq!(
            handle
                .open_duplex("a", dto::AttachmentRole::Sidebar, None)
                .await
                .err(),
            Some(UiError::Refused(dto::RefusalCode::Conflict))
        );
        drop(stream);
        crate::testing::eventually("losing the connection detaches its attachment", || {
            (handle.attachments() == 0).then_some(())
        });
        assert_eq!(
            handle
                .enqueue_action("a", 1, None, A::CycleOrder)
                .unwrap()
                .await
                .unwrap(),
            Out::Detached
        );
    }

    async fn create_from_picker(handle: &UiHandle, next: &mut u64) -> String {
        *next += 1;
        let Out::Applied {
            interaction_id: Some(id),
            ..
        } = handle
            .enqueue_action(
                "a",
                *next,
                None,
                A::RunCommand {
                    command: dto::SharedCommand::NewWorkspace,
                    row: None,
                },
            )
            .unwrap()
            .await
            .unwrap()
        else {
            panic!("opens interaction")
        };
        *next += 1;
        assert!(matches!(
            handle
                .enqueue_action(
                    "a",
                    *next,
                    None,
                    A::ProvideChoices {
                        interaction_id: id.clone(),
                        choices: vec!["/project".into()],
                        note: None
                    },
                )
                .unwrap()
                .await
                .unwrap(),
            Out::Applied { .. }
        ));
        *next += 1;
        assert!(matches!(
            handle
                .enqueue_action(
                    "a",
                    *next,
                    None,
                    A::ClaimPresentation {
                        interaction_id: id.clone()
                    },
                )
                .unwrap()
                .await
                .unwrap(),
            Out::Applied { .. }
        ));
        *next += 1;
        let Out::Applied {
            request_id: Some(request),
            ..
        } = handle
            .enqueue_action(
                "a",
                *next,
                None,
                A::ResolveInteraction {
                    interaction_id: id,
                    result: dto::InteractionResult::Project {
                        path: "/project".into(),
                    },
                },
            )
            .unwrap()
            .await
            .unwrap()
        else {
            panic!("returns a stable request id")
        };
        request
    }

    async fn operation(stream: &mut Subscription, id: &str, status: dto::CommandStatus) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let publication = stream.prepare_frame(false).await.expect("service alive");
                stream.sent_frame(&publication.head);
                let session: dto::SessionState = publication
                    .content
                    .iter()
                    .find(|content| content.hash == publication.head.session)
                    .or_else(|| {
                        publication
                            .content
                            .iter()
                            .find(|content| content.hash == publication.head.session)
                    })
                    .expect("session payload")
                    .read()
                    .unwrap();
                if session
                    .operation
                    .as_ref()
                    .is_some_and(|op| op.request_id == id && op.status == status)
                {
                    break;
                }
            }
        })
        .await
        .expect("operation status published");
    }

    #[tokio::test]
    async fn full_or_absent_dispatchers_refuse_without_holding_the_gate() {
        let (release, held) = mpsc::sync_channel::<()>(1);
        let service = UiService::with_executor(Some(Box::new(move |effect| {
            let Effect::Dispatch(job) = effect else {
                panic!("not a reconciliation")
            };
            let _ = held.recv();
            dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Completed)
        })));
        let handle = service.handle();
        let mut stream = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let mut next = 0;
        let mut last = String::new();
        for _ in 0..10 {
            last = create_from_picker(&handle, &mut next).await;
        }
        operation(&mut stream, &last, dto::CommandStatus::DispatchRefused).await;
        assert!(stream.published.borrow().state.blocking.is_none());
        drop(release); // releases the held worker without leaving a test thread behind
        drop(stream);
        drop(service);

        let service = UiService::start();
        let handle = service.handle();
        let mut stream = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let request = create_from_picker(&handle, &mut 0).await;
        operation(&mut stream, &request, dto::CommandStatus::DispatchRefused).await;
    }

    #[tokio::test]
    async fn an_uncertain_completion_is_read_by_request_id_never_dispatched_again() {
        let dispatched = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&dispatched);
        let service = UiService::with_executor(Some(Box::new(move |effect| match effect {
            Effect::Dispatch(job) => {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = job;
                panic!("worker completion lost after possible admission");
            }
            Effect::Reconcile(id) => dto::CommandOutcome::answering(
                &id,
                dto::CommandStatus::Completed,
                serde_json::json!({"recorded": true}),
            ),
        })));
        let handle = service.handle();
        let mut stream = handle
            .open_duplex("a", dto::AttachmentRole::Dashboard, None)
            .await
            .unwrap();
        let request = create_from_picker(&handle, &mut 0).await;
        operation(
            &mut stream,
            &request,
            dto::CommandStatus::SubmissionUncertain,
        )
        .await;
        operation(&mut stream, &request, dto::CommandStatus::Completed).await;
        assert_eq!(dispatched.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod owed_tests {
    use super::*;

    #[test]
    fn uncertainty_is_published_once_and_still_looked_for_every_interval() {
        let start = Instant::now();
        let mut owed = Owed::new(start);
        assert!(!owed.due(start), "not before the interval has passed");
        let first = start + RECONCILE_INTERVAL;
        assert!(owed.due(first), "the interval asks again");
        assert!(
            owed.mark_published(),
            "the first announcement says something"
        );
        let second = first + RECONCILE_INTERVAL;
        assert!(owed.due(second), "a slow operation is still asked about");
        assert!(
            !owed.mark_published(),
            "a repeated announcement would rewrite the state with the same fact"
        );
    }
}
