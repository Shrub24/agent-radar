//! The daemon's shared observation: one sampler, one published snapshot.
//!
//! Every display used to collect for itself — inventory, process samples,
//! foreground evidence, the registry read and the continuity memory that joins
//! them. A second display repeated all of it, and closing a display lost the
//! continuity it held. One daemon sampler now owns that work and publishes an
//! immutable revisioned snapshot every reader shares.
//!
//! Two properties shape this module:
//!
//! - **A snapshot is a read model, not authority.** It carries the daemon's
//!   fleet projection and per-source freshness. It never carries launch values,
//!   environment overlays or private recipes, and publishing a revision appends
//!   nothing to the durable journal.
//! - **Sampling follows demand.** Nothing is collected while no reader has
//!   asked recently. That is safe because the lifecycle sweep keeps its own
//!   coverage: managed state does not depend on a display being attached.
//!
//! What is *not* here, deliberately: durable observation history, deltas between
//! revisions, and per-client view state. Two readers of the same classes get the
//! same revision.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::collector::{Collector, CollectorConfig};
use crate::control_plane::ingest::TaskIngest;
use crate::control_plane::store::{Store, format_millis};
use crate::observation::{ObservationState, SourceFreshness};
use crate::runtime::RuntimeProvider;

/// How long one reader's request keeps a data class demanded.
///
/// A display reads on its own loop; this is the grace period that keeps the
/// sampler running between two of its reads rather than stopping and starting.
pub const DEMAND_WINDOW: Duration = Duration::from_secs(5);

/// The longest a read may be held open waiting for a newer revision.
pub const MAX_OBSERVATION_WAIT_MS: u64 = 30_000;

/// The most task sessions one snapshot carries.
pub const SNAPSHOT_TASK_LIMIT: usize = 64;

/// The most bytes one snapshot response may occupy.
///
/// The transport refuses a line longer than
/// [`crate::control_plane::protocol::MAX_LINE_BYTES`], and a truncated line is
/// not a smaller answer. A snapshot over this bound is refused as its own typed
/// outcome instead of being written as an unreadable line.
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;

/// The shortest gap between two passes that committed events alone brought
/// forward, so a burst of events costs one further sample rather than a storm.
pub const EVENT_WAKE_GAP: Duration = Duration::from_millis(250);

/// How long an idle sampler sleeps before looking for demand again.
const IDLE_POLL: Duration = Duration::from_millis(100);

/// How often the sampler looks at itself while a reader is waiting.
const BUSY_POLL: Duration = Duration::from_millis(20);

/// The data classes a reader can ask for.
///
/// `fleet`, `registry` and `tasks` are always collected: every reader needs them
/// to draw anything, and they are what the snapshot's sources report on.
/// `processes` is the optional one — it is a whole-fleet process sweep, so it is
/// sampled only while a reader asks for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Classes {
    pub processes: bool,
}

impl Classes {
    /// Reads the class names one request asked for.
    ///
    /// An unrecognised name is refused rather than ignored: a reader that asked
    /// for something this daemon does not collect would otherwise wait for data
    /// that can never arrive.
    pub fn parse(names: &[String]) -> Result<Self, String> {
        let mut classes = Self::default();
        for name in names {
            match name.as_str() {
                "fleet" | "registry" | "tasks" => {}
                "processes" => classes.processes = true,
                other => return Err(format!("unknown observation class `{other}`")),
            }
        }
        Ok(classes)
    }
}

/// One source's freshness inside a snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    /// `inventory`, `registry`, `processes` or `tasks`.
    pub name: String,
    pub state: SourceFreshness,
    /// When this source last produced a successful observation, if it ever has.
    pub last_success_at: Option<String>,
}

/// One immutable observation revision.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// The daemon process this revision belongs to.
    ///
    /// A revision number is only meaningful inside one epoch: a restarted daemon
    /// counting from one again must not let a reader holding revision 7 of the
    /// old process mistake a new revision 7 for the one it already had.
    pub epoch: String,
    pub revision: u64,
    /// When this revision was published.
    pub collected_at: Option<String>,
    /// Per-source freshness, so a reader can tell a stale fleet from an
    /// unreadable registry rather than seeing one fleet-wide verdict.
    pub sources: Vec<SourceState>,
    /// Volatile task sessions with the ingest's own revision.
    pub tasks: Option<Value>,
    /// The daemon's fleet projection: last-good inventory, source freshness and
    /// retained continuity.
    pub observation: ObservationState,
}

impl Snapshot {
    /// Whether this snapshot is one the daemon no longer holds.
    pub fn fits(&self) -> bool {
        serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_SNAPSHOT_BYTES)
    }
}

/// The daemon's sampler and the revision it has published.
pub struct Hub {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

struct Shared {
    published: Mutex<Published>,
    /// Signalled whenever a revision is published or demand arrives.
    changed: Condvar,
    demand: Mutex<Demand>,
    /// The daemon's stop flag: the sampler stops with the daemon.
    stopping: Arc<std::sync::atomic::AtomicBool>,
    /// This hub's own stop flag, so dropping a hub stops its sampler without
    /// stopping the daemon it belongs to.
    stopped: std::sync::atomic::AtomicBool,
}

struct Published {
    epoch: String,
    revision: u64,
    snapshot: Option<Arc<Snapshot>>,
}

/// What readers have asked for recently.
#[derive(Default)]
struct Demand {
    at: Option<Instant>,
    processes: bool,
}

impl Demand {
    fn active(&self) -> bool {
        self.at.is_some_and(|at| at.elapsed() < DEMAND_WINDOW)
    }
}

impl Hub {
    /// Starts the daemon's sampler against `provider`.
    ///
    /// The sampler owns its reconciler, so retained continuity belongs to the
    /// daemon and survives a display restart. Nothing is collected until a
    /// reader asks.
    pub(crate) fn start(
        provider: Arc<dyn RuntimeProvider>,
        registry: Arc<crate::control_plane::registry::Registry>,
        tasks: Option<Arc<TaskIngest>>,
        task_error: Option<String>,
        store: Store,
        stopping: &Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let shared = Arc::new(Shared {
            published: Mutex::new(Published {
                epoch: crate::control_plane::random_uuid(),
                revision: 0,
                snapshot: None,
            }),
            changed: Condvar::new(),
            demand: Mutex::new(Demand::default()),
            stopping: Arc::clone(stopping),
            stopped: std::sync::atomic::AtomicBool::new(false),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("radar-observation".to_string())
                .spawn(move || sample_loop(shared, provider, registry, tasks, task_error, store))
                .ok()
        };
        Self { shared, worker }
    }

    /// The epoch of this daemon's revisions.
    pub fn epoch(&self) -> String {
        locked(&self.shared.published).epoch.clone()
    }

    /// The current revision, and the snapshot it names.
    pub fn current(&self) -> (u64, Option<Arc<Snapshot>>) {
        let published = locked(&self.shared.published);
        (published.revision, published.snapshot.clone())
    }

    /// Records that a reader asked for `classes` and returns the current
    /// revision, so a read that has to wait does not miss a publish between
    /// looking and waiting.
    pub fn demand(&self, classes: Classes) -> u64 {
        let revision = {
            let mut demand = locked(&self.shared.demand);
            demand.at = Some(Instant::now());
            if classes.processes {
                demand.processes = true;
            }
            locked(&self.shared.published).revision
        };
        // A sampler that is idle is waiting to be told; this is that telling.
        self.shared.changed.notify_all();
        revision
    }

    /// Waits until a revision newer than `seen` is published, or `timeout`
    /// elapses. Returns whether a newer revision exists.
    ///
    /// A waiter holds no lock and no blocking worker: the condition variable is
    /// the whole mechanism, and the caller re-reads afterwards.
    pub fn wait_for_newer(&self, seen: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut published = locked(&self.shared.published);
        while published.revision <= seen {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            // Short slices so a stopping daemon does not wait out a long poll.
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(published, remaining.min(Duration::from_millis(50)))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            published = next;
            if self.shared.stopped.load(Ordering::SeqCst)
                || self.shared.stopping.load(Ordering::SeqCst)
            {
                return published.revision > seen;
            }
        }
        true
    }

    /// Stops the sampler and wakes it so it notices.
    pub fn shutdown(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
    }

    /// Joins the sampler thread, if this hub owns one.
    pub fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shutdown();
        self.join();
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The sampler thread: collect while a reader is asking, publish what changed.
fn sample_loop(
    shared: Arc<Shared>,
    provider: Arc<dyn RuntimeProvider>,
    registry: Arc<crate::control_plane::registry::Registry>,
    tasks: Option<Arc<TaskIngest>>,
    task_error: Option<String>,
    store: Store,
) {
    // The daemon's own registry, read in process with the fleet this refresh
    // collected. Nothing here verifies a claimed process: a registration's claim
    // is what a pane is matched against, and the verdict belongs to the request
    // that asked for one.
    let registry_source: crate::collector::RegistrySource = Arc::new(move |fleet, _cancel| {
        crate::control_plane::registry_read::read(&registry, Some(fleet), unix_millis()).map(Some)
    });
    let mut collector = Collector::new(CollectorConfig::default(), provider, registry_source);
    let mut state = ObservationState::new();
    let mut last_success: BTreeMap<String, String> = BTreeMap::new();
    let mut last_generation = store.event_generation();
    let mut last_pass: Option<Instant> = None;
    while !shared.stopped.load(Ordering::SeqCst) && !shared.stopping.load(Ordering::SeqCst) {
        let demand = {
            let demand = locked(&shared.demand);
            (demand.active(), demand.processes)
        };
        // A committed event is a wake, not a schedule: it can bring the next
        // pass forward while a reader is waiting, never more often than the gap.
        let generation = store.event_generation();
        let events_woke = demand.0
            && generation != last_generation
            && last_pass.is_none_or(|at| at.elapsed() >= EVENT_WAKE_GAP);
        let applied = if demand.0 {
            collector.tick(&mut state, demand.1)
        } else {
            // Nothing is reading. The refresh already in flight is still reaped
            // and applied, so its worker never outlives the demand that asked.
            collector.drain(&mut state)
        };
        if applied {
            last_pass = Some(Instant::now());
            last_generation = generation;
            let (task_value, task_state) = tasks_snapshot(&tasks, &task_error);
            let at = format_millis(unix_millis());
            let snapshot = Snapshot {
                epoch: String::new(),
                revision: 0,
                collected_at: Some(at.clone()),
                sources: sources(&state, &task_state, demand.1, &mut last_success, &at),
                tasks: task_value,
                observation: state.clone(),
            };
            shared.publish_through(&snapshot);
        }
        let wait = if events_woke {
            Duration::from_millis(1)
        } else if demand.0 {
            BUSY_POLL
        } else {
            IDLE_POLL
        };
        let published = locked(&shared.published);
        let _ = shared
            .changed
            .wait_timeout(published, wait)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    collector.shutdown();
}

impl Shared {
    /// Publishes `snapshot` as the next revision and wakes every waiter.
    ///
    /// The sampler builds a snapshot without knowing the revision it will be, so
    /// this is the one place the epoch and revision are stamped.
    fn publish_through(&self, snapshot: &Snapshot) {
        let mut published = locked(&self.published);
        published.revision += 1;
        let mut next = snapshot.clone();
        next.revision = published.revision;
        next.epoch = published.epoch.clone();
        published.snapshot = Some(Arc::new(next));
        drop(published);
        self.changed.notify_all();
    }
}

/// Per-source freshness for one published revision.
///
/// A source's `last_success_at` is the last time it was current, kept across
/// revisions so a reader can see how old a stale observation is rather than only
/// that it is stale.
fn sources(
    state: &ObservationState,
    tasks: &SourceFreshness,
    processes: bool,
    last_success: &mut BTreeMap<String, String>,
    at: &str,
) -> Vec<SourceState> {
    let mut entries = vec![
        ("inventory", state.source_freshness().clone()),
        ("registry", state.registry_freshness().clone()),
        (
            "processes",
            if processes {
                SourceFreshness::Current
            } else {
                // Not asked for in this revision: no claim either way.
                SourceFreshness::Pending
            },
        ),
        ("tasks", tasks.clone()),
    ];
    let mut sources = Vec::with_capacity(entries.len());
    for (name, freshness) in entries.drain(..) {
        if matches!(freshness, SourceFreshness::Current) {
            last_success.insert(name.to_string(), at.to_string());
        }
        sources.push(SourceState {
            name: name.to_string(),
            state: freshness,
            last_success_at: last_success.get(name).cloned(),
        });
    }
    sources
}

/// The volatile task sessions, with the ingest's own revision.
///
/// A daemon without an ingest reports the source as unavailable and names why,
/// rather than presenting an empty task list as a successful reading.
fn tasks_snapshot(
    tasks: &Option<Arc<TaskIngest>>,
    error: &Option<String>,
) -> (Option<Value>, SourceFreshness) {
    let Some(ingest) = tasks else {
        let diagnostic = error
            .clone()
            .unwrap_or_else(|| "task ingest is not configured".to_string());
        return (None, SourceFreshness::Unavailable { diagnostic });
    };
    let (revision, sessions, _) = ingest.list(None, SNAPSHOT_TASK_LIMIT);
    (
        Some(json!({"revision": revision, "sessions": sessions})),
        SourceFreshness::Current,
    )
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_name_what_a_reader_wants_and_unknown_names_are_refused() {
        let none = Classes::parse(&[]).expect("no classes is a valid request");
        assert!(!none.processes);
        assert!(
            !Classes::parse(&["fleet".into(), "registry".into(), "tasks".into()])
                .expect("the always-collected classes")
                .processes
        );
        assert!(
            Classes::parse(&["processes".into()])
                .expect("the optional class")
                .processes
        );
        // A name this daemon does not collect is refused rather than ignored: a
        // reader waiting for data that can never arrive is worse than a refusal.
        let unknown = Classes::parse(&["processors".into()]).expect_err("an unknown class");
        assert!(unknown.contains("processors"), "{unknown}");
    }

    #[test]
    fn a_source_records_when_it_was_last_current() {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::FleetObservation::default());
        state.apply_registry_failure("unreadable");
        let mut last_success = BTreeMap::new();
        let first = sources(
            &state,
            &SourceFreshness::Current,
            false,
            &mut last_success,
            "T1",
        );
        assert_eq!(first[0].last_success_at.as_deref(), Some("T1"));
        // A failed source is reported as itself, not as a fleet that vanished.
        assert!(matches!(first[1].state, SourceFreshness::Stale { .. }));
        assert_eq!(first[1].last_success_at, None);

        let second = sources(
            &state,
            &SourceFreshness::Current,
            false,
            &mut last_success,
            "T2",
        );
        assert_eq!(second[0].last_success_at.as_deref(), Some("T2"));
        // A class nobody asked for makes no claim either way.
        assert_eq!(second[2].state, SourceFreshness::Pending);
    }

    #[test]
    fn a_snapshot_round_trips_with_its_sources_and_revision() {
        let snapshot = Snapshot {
            epoch: "epoch".into(),
            revision: 7,
            collected_at: Some("2026-01-01T00:00:00.000Z".into()),
            sources: vec![SourceState {
                name: "inventory".into(),
                state: SourceFreshness::Stale {
                    diagnostic: "unreachable".into(),
                },
                last_success_at: Some("2026-01-01T00:00:00.000Z".into()),
            }],
            tasks: None,
            observation: ObservationState::new(),
        };
        let encoded = serde_json::to_value(&snapshot).expect("serialize");
        let decoded: Snapshot = serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(decoded.revision, 7);
        assert_eq!(decoded.epoch, "epoch");
        assert!(matches!(
            decoded.sources[0].state,
            SourceFreshness::Stale { .. }
        ));
    }
}
