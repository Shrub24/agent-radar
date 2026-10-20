//! Non-blocking collection of normalized runtime observations.
//!
//! This module owns the refresh schedule. One refresh is in flight at a time and
//! it runs on its own thread, asking the injected
//! [`RuntimeProvider`](crate::runtime::RuntimeProvider) for the inventory and
//! then for foreground evidence on exactly the panes the reconciler reports as
//! continuity candidates. [`Collector::tick`] never blocks: it applies a
//! finished refresh to the caller-owned [`ObservationState`], which keeps the
//! reconciler single-owner and lock-free, and starts the next refresh once the
//! poll interval has elapsed.
//!
//! The runtime's transport is not this module's business. Which executable
//! runs, what its arguments, output shapes and sockets are, and how a stalled
//! command is killed and reaped all live in the adapter ([`crate::herdr`]). A
//! provider is anything that answers with Radar's normalized facts, so this
//! path is also driven by an in-memory fake in tests.
//!
//! Two properties the main loop depends on:
//!
//! - **Input and quit stay responsive.** No provider call runs on the calling
//!   thread, and a call still outstanding at shutdown is abandoned through
//!   cancellation rather than waited for.
//! - **A stalled call leaks nothing.** [`Collector::shutdown`] cancels and
//!   joins the worker, so no thread outlives the process.
//!
//! Whole-source failures are distinguishable from continuity evidence: an
//! `Err` from the inventory is a source diagnostic (the caller's last-good
//! inventory stays stale), while unreadable foreground evidence is
//! [`ForegroundEvidence::Inconclusive`] and never proves that either the source
//! or the pane disappeared.
//!
//! A registry-serving backend's agents are joined to panes here too: the
//! registrations are read after the inventory, and a registration's pane is
//! sampled in the same refresh so the join can be proven by its birth identity.
//!
//! Two normalized facts are completed here rather than by an adapter. The
//! active assignment's age is measured once per refresh against one read of the
//! clock, so decoded facts stay clockless and the rows of a single frame stay
//! comparable. Local `/proc` facts are composed into the foreground evidence the
//! provider returned, so every adapter gets them the same way; that includes the
//! process's resources, the one fact with a memory — interval CPU is the
//! difference between two readings of the same process, and the history is
//! carried from one refresh to the next and pruned to the processes the last one
//! saw. A refresh's process table is read once, before the roots are sampled, so
//! every root summarises its descendants from the same snapshot at the same
//! instant.
//!
//! Which panes get queried comes from
//! [`ObservationState::continuity_candidates`], which the reconciler re-arms on
//! every successful inventory. Because candidates are chosen before that
//! inventory is applied, a retained pane's evidence is refreshed on alternate
//! cycles: retention itself is immediate, and its shell/non-shell confirmation
//! lands within two poll intervals.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::{FleetObservation, ForegroundEvidence};
use crate::observation::ObservationState;
use crate::procfs::{self, Sampler};
use crate::registry_observation::{self, RegistryRead};
use crate::runtime::RuntimeProvider;

/// Reads the registry a refresh joins its agents from.
///
/// The daemon owns its registry and reads it in process with the fleet it just
/// collected, so the source is given that fleet: a spawn edge's location is
/// confirmed against the observation of the same refresh rather than a second
/// listing. `None` is a registry that could not be read, which fails the
/// refresh's registry source and leaves the last-good projection standing.
pub type RegistrySource = Arc<
    dyn Fn(&FleetObservation, &AtomicBool) -> Result<Option<RegistryRead>, String> + Send + Sync,
>;

/// Interval between refreshes when [`CollectorConfig::poll_interval`] is left
/// at its default.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How long one refresh may run, end to end, across the inventory, the registry,
/// its topology and every candidate's foreground evidence.
///
/// The main loop cancels a refresh that outlives this, and cancellation reaches
/// every runtime call, so the bound holds even when one call would wait longer.
const REFRESH_DEADLINE: Duration = Duration::from_secs(30);

/// Collection settings: the refresh schedule alone.
///
/// Running the runtime — its executable and the deadline on one command — is
/// the adapter's configuration, because polling belongs to the collector and
/// the runtime's transport does not.
#[derive(Clone, Debug)]
pub struct CollectorConfig {
    /// Delay between refreshes. The first refresh starts on the first
    /// [`Collector::tick`], so startup does not wait out an interval.
    pub poll_interval: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Which source a failed collection pass could not read.
///
/// The fleet and the registry are separate sources: a registry read that fails
/// leaves the last-good inventory and the last-good registry projection
/// standing, so it is reported against the registry rather than as a fleet
/// failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedSource {
    /// The runtime could not be read: there is no new inventory at all.
    Fleet,
    /// The registry could not be read: the fleet observation may be fine.
    Registry,
}

/// One finished collection cycle, ready to be applied to the reconciler.
enum RefreshOutcome {
    Success {
        observation: FleetObservation,
        /// Foreground evidence per continuity-candidate pane. A candidate with
        /// no usable answer carries [`ForegroundEvidence::Inconclusive`].
        evidence: Vec<(String, ForegroundEvidence)>,
    },
    Failure {
        diagnostic: String,
        source: FailedSource,
    },
}

/// A refresh running on its own thread.
struct Refresh {
    /// Set by [`Collector::shutdown`] or by the deadline; the worker turns it into
    /// a cancellation.
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    outcomes: Receiver<RefreshOutcome>,
    worker: JoinHandle<()>,
}

/// Collects runtime observations for the main loop, one refresh at a time.
///
/// Call [`Collector::tick`] on every loop iteration (at least as often as the
/// poll interval, so a due refresh is noticed promptly) and
/// [`Collector::shutdown`] before exit; dropping the collector shuts down too.
pub struct Collector {
    config: CollectorConfig,
    /// The runtime this collector reads. Injected at assembly, so the same path
    /// serves the production adapter and a test fake.
    provider: Arc<dyn RuntimeProvider>,
    /// The registry this collector joins agents from, read with the fleet of the
    /// refresh it belongs to.
    registry: RegistrySource,
    /// The resource sampling history, one for the run.
    ///
    /// It outlives a refresh — interval CPU is the difference between two
    /// readings of the same process — and a refresh runs on its own thread, so
    /// it is shared with that thread rather than handed over: one lock, held for
    /// the length of the refresh that sampling belongs to, which is the only
    /// time any thread touches it.
    sampler: Arc<Mutex<Sampler>>,
    /// When the next refresh may start; already due at construction.
    next_refresh: Instant,
    in_flight: Option<Refresh>,
    stopped: bool,
}

impl Collector {
    /// A collector that runs nothing until the first [`Self::tick`].
    pub fn new(
        config: CollectorConfig,
        provider: impl RuntimeProvider + 'static,
        registry: RegistrySource,
    ) -> Self {
        Self::sampling(config, provider, registry, Sampler::new())
    }

    /// A collector sampling with `sampler`, as tests and callers that must not
    /// read this machine's process table supply.
    pub fn sampling(
        config: CollectorConfig,
        provider: impl RuntimeProvider + 'static,
        registry: RegistrySource,
        sampler: Sampler,
    ) -> Self {
        Self {
            config,
            provider: Arc::new(provider),
            registry,
            sampler: Arc::new(Mutex::new(sampler)),
            next_refresh: Instant::now(),
            in_flight: None,
            stopped: false,
        }
    }

    /// Advances collection without blocking.
    ///
    /// Applies a finished refresh to `state` (success and its evidence
    /// together, or the failure diagnostic) and starts the next refresh when
    /// the poll interval has elapsed and none is in flight. `processes` asks for
    /// foreground evidence for every reported pane rather than for continuity
    /// candidates alone, which is what the process view reads. Returns whether
    /// `state` changed, so a caller can skip a redraw. Never waits for the
    /// provider: a refresh is applied by the tick that finds it finished.
    pub fn tick(&mut self, state: &mut ObservationState, processes: bool) -> bool {
        if self.stopped {
            return false;
        }
        let applied = self.drain(state);
        if self.in_flight.is_none() && Instant::now() >= self.next_refresh {
            // A whole-fleet process sweep covers every pane; otherwise the
            // candidates are the continuity retentions plus the live agent panes
            // an agent row's staleness mark needs.
            let candidates = if processes {
                state.pane_ids()
            } else {
                state.evidence_candidates()
            };
            self.start_refresh(candidates);
        }
        applied
    }

    /// Applies a finished refresh and enforces the in-flight deadline, without
    /// starting another.
    ///
    /// A sampler that must not collect while nothing is reading still has to
    /// reap the refresh it already started and apply its outcome, so the
    /// worker it owns never outlives the demand that asked for it.
    pub fn drain(&mut self, state: &mut ObservationState) -> bool {
        if self.stopped {
            return false;
        }
        let applied = self.apply_finished(state);
        // The deadline is enforced here, on the caller's thread, so a refresh
        // stuck in one call is still cancelled. Tick at least as often as the
        // poll interval for this to hold promptly.
        if let Some(refresh) = self
            .in_flight
            .as_ref()
            .filter(|r| Instant::now() >= r.deadline)
        {
            refresh.cancel.store(true, Ordering::SeqCst);
        }
        applied
    }

    /// Whether a collection is currently in flight, so the view can animate
    /// its spinner without waiting on the command.
    pub fn is_refreshing(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Stops collecting: cancels an in-flight refresh, waits until its worker
    /// has abandoned it, and makes later ticks no-ops. Idempotent.
    pub fn shutdown(&mut self) {
        self.stopped = true;
        if let Some(refresh) = self.in_flight.take() {
            refresh.cancel.store(true, Ordering::SeqCst);
            self.join(refresh);
        }
    }

    /// Applies one finished refresh, if one is waiting.
    fn apply_finished(&mut self, state: &mut ObservationState) -> bool {
        let Some(refresh) = self.in_flight.as_mut() else {
            return false;
        };
        let outcome = match refresh.outcomes.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            // The worker always hands over exactly one outcome before
            // returning, so this is only reachable through a panic in it.
            // Collection fails visibly instead of silently stopping.
            Err(TryRecvError::Disconnected) => RefreshOutcome::Failure {
                diagnostic: "collection worker stopped unexpectedly".to_string(),
                source: FailedSource::Fleet,
            },
        };
        let refresh = self.in_flight.take().expect("checked above");
        self.join(refresh);

        match outcome {
            RefreshOutcome::Success {
                observation,
                evidence,
            } => {
                // Inventory first: it rebuilds the retained set that the
                // evidence then refines.
                state.apply_success(observation);
                for (pane_id, evidence) in evidence {
                    state.apply_evidence(&pane_id, evidence);
                }
            }
            RefreshOutcome::Failure { diagnostic, source } => match source {
                FailedSource::Fleet => state.apply_failure(diagnostic),
                FailedSource::Registry => state.apply_registry_failure(diagnostic),
            },
        }
        self.next_refresh = Instant::now() + self.config.poll_interval;
        true
    }

    /// Waits for a worker that has already handed over its outcome or has been
    /// cancelled, so no thread outlives the caller.
    fn join(&self, refresh: Refresh) {
        // A panicking worker unwinds after its child is cleaned up; the failure
        // diagnostic above is the visible report of that.
        let _ = refresh.worker.join();
    }

    /// Starts one refresh off the calling thread.
    fn start_refresh(&mut self, candidates: Vec<String>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + REFRESH_DEADLINE;
        let (sender, outcomes) = mpsc::channel();
        let worker = {
            let provider = Arc::clone(&self.provider);
            let registry = Arc::clone(&self.registry);
            let cancel = Arc::clone(&cancel);
            let sampler = Arc::clone(&self.sampler);
            thread::Builder::new()
                .name("radar-collector".to_string())
                .spawn(move || {
                    let _ = sender.send(collect(
                        provider.as_ref(),
                        &registry,
                        &candidates,
                        &cancel,
                        &sampler,
                        deadline,
                    ));
                })
                .expect("collector thread")
        };
        self.in_flight = Some(Refresh {
            cancel,
            deadline,
            outcomes,
            worker,
        });
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Runs one refresh: the inventory, the registry, then evidence for each pane.
///
/// Registered agents are read after the inventory and joined after the evidence,
/// so a registration's pane is sampled in this refresh whether or not an agent
/// was ever seen on it. The backend's own agent records are kept as containment
/// evidence only. A refresh that was cancelled, by its deadline or by shutdown,
/// is a failure, never a partial success.
fn collect(
    provider: &dyn RuntimeProvider,
    registry_source: &RegistrySource,
    candidates: &[String],
    cancel: &AtomicBool,
    sampler: &Mutex<Sampler>,
    deadline: Instant,
) -> RefreshOutcome {
    let mut observation = match provider.inventory(cancel) {
        Ok(observation) => observation,
        Err(diagnostic) => {
            return RefreshOutcome::Failure {
                diagnostic,
                source: FailedSource::Fleet,
            };
        }
    };
    let registry = match registry_source(&observation, cancel) {
        Ok(registry) => registry,
        Err(diagnostic) => {
            return RefreshOutcome::Failure {
                diagnostic,
                source: FailedSource::Registry,
            };
        }
    };
    // A set: a pane named by the continuity candidates and by a registration is
    // sampled once.
    let mut panes: BTreeSet<String> = candidates.iter().cloned().collect();
    if let Some(read) = &registry {
        panes.extend(read.agents.iter().filter_map(|agent| {
            let pane = agent.registration.location.as_ref()?.pane.as_ref()?;
            observation.pane(pane).map(|_| pane.clone())
        }));
    }
    // One PATH lookup per distinct program name for the whole refresh, and one
    // entry point for the local facts composed onto the evidence below.
    let mut binaries = procfs::BinaryIndex::new();
    // The sampling history is this refresh's for as long as it samples: the
    // collector starts no other refresh, so the lock is never contended. A
    // worker that panicked while holding it leaves counters and timestamps
    // behind, and the worst a half-updated history can do is report one interval
    // as unknown — which is not a reason to stop measuring.
    let mut sampler = sampler
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // One instant for the whole refresh, so every interval in one frame is
    // measured over the same clock, and one snapshot of the process table for
    // every root in it: descendants are summed from that scan rather than the
    // table being walked once per pane.
    let now = Instant::now();
    if !panes.is_empty() {
        sampler.scan(now, cancel);
    }
    let evidence: Vec<(String, ForegroundEvidence)> = panes
        .iter()
        .map(|pane_id| {
            (
                pane_id.clone(),
                evidence_for(provider, pane_id, cancel, &mut binaries, &mut sampler, now),
            )
        })
        .collect();
    // The refresh is the unit the history is bounded by, and this is after every
    // root has summed its descendants: the processes this refresh did not read do
    // not come back, and their counters are not kept for them.
    sampler.finish();
    drop(sampler);
    // The clock is checked as well as the flag: a refresh that finished past its
    // deadline is never applied, even before the main loop has cancelled it.
    if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
        return RefreshOutcome::Failure {
            diagnostic: format!("collection did not finish within {REFRESH_DEADLINE:?}"),
            source: FailedSource::Fleet,
        };
    }
    // Containers are read with the registry facts, and they are the daemon's
    // records rather than the backend's: an inventory never carries them, so a
    // refresh that read none presents none.
    let containers = registry
        .as_ref()
        .map(|read| read.containers.clone())
        .unwrap_or_default();
    let lifecycle_subjects = registry
        .as_ref()
        .map(|read| {
            read.lifecycle
                .iter()
                .map(|subject| crate::model::LifecycleObservation {
                    subject_id: subject.subject_id.clone(),
                    bound_registration_id: subject.bound_registration_id.clone(),
                    process: subject.process.clone(),
                    state: subject.state,
                    session: subject.session.clone(),
                    successor: subject.successor.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    // Presented agents come from the registry alone. Where there is no registry,
    // none are presented; the backend's own records stay as containment evidence.
    let metadata_agents = std::mem::take(&mut observation.agents);
    observation.agents = registry.map_or_else(Vec::new, |read| {
        registry_observation::project(&observation, &provider.identity(), read, &evidence)
    });
    observation.metadata_agents = metadata_agents;
    observation.containers = containers;
    observation.lifecycle_subjects = lifecycle_subjects;
    stamp_assignment_ages(&mut observation);
    RefreshOutcome::Success {
        observation,
        evidence,
    }
}

/// Measures every agent's assignment age against one read of the clock.
///
/// The decoded facts stay clockless and fixture-driven, so the age is stamped
/// here, where the observation is produced; one instant for the whole
/// observation keeps the rows of a single frame comparable.
fn stamp_assignment_ages(observation: &mut FleetObservation) {
    let now = now_unix_ms();
    for agent in &mut observation.agents {
        agent.facts.stamp_assigned_age(now);
    }
}

/// Unix milliseconds at the wall clock.
///
/// A clock that cannot be read (one set before the epoch) reports zero, which
/// [`crate::model::HerdsmanFacts::stamp_assigned_age`] saturates to no age rather
/// than a negative one.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Foreground evidence for one pane.
///
/// The provider answers with what the runtime reports; this machine's own facts
/// about that process are composed in here, where the process id and the
/// program name the runtime reported are already in hand, so nothing above the
/// seam has to know about `/proc` or `PATH` and every adapter gets the
/// composition the same way. An inconclusive answer has no process to look up
/// and is passed through unchanged.
fn evidence_for(
    provider: &dyn RuntimeProvider,
    pane_id: &str,
    cancel: &AtomicBool,
    binaries: &mut procfs::BinaryIndex,
    sampler: &mut Sampler,
    now: Instant,
) -> ForegroundEvidence {
    let mut evidence = provider.foreground_evidence(pane_id, cancel);
    if let ForegroundEvidence::NonShell {
        pid, name, local, ..
    } = &mut evidence
    {
        *local = procfs::facts(*pid, name.as_deref(), binaries, sampler, now);
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FleetObservation, ForegroundEvidence};
    use crate::runtime::{BackendIdentity, CloseTarget, Target};

    #[test]
    fn the_poll_interval_defaults_to_one_second() {
        assert_eq!(
            CollectorConfig::default().poll_interval,
            Duration::from_secs(1)
        );
    }

    /// A backend that answers an empty inventory and nothing else: the refresh
    /// has no panes to sample, so only its own clock decides the outcome.
    struct EmptyInventory;

    impl RuntimeProvider for EmptyInventory {
        fn identity(&self) -> BackendIdentity {
            BackendIdentity::named("test")
        }

        fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
            Ok(FleetObservation {
                metadata_agents: Vec::new(),
                containers: Vec::new(),
                lifecycle_subjects: Vec::new(),
                workspaces: Vec::new(),
                tabs: Vec::new(),
                panes: Vec::new(),
                agents: Vec::new(),
            })
        }

        fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
            ForegroundEvidence::Inconclusive
        }

        fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
            Err("not used".into())
        }

        fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
            Err("not used".into())
        }
    }

    /// A refresh that completes at or after its deadline is refused even though
    /// nothing cancelled it: the flag is clear and the work succeeded. The same
    /// backend with the deadline ahead is applied, so the refusal is the clock's.
    #[test]
    fn a_refresh_that_finishes_at_its_deadline_is_refused_even_uncancelled() {
        let cancel = AtomicBool::new(false);
        let sampler = Mutex::new(Sampler::new());
        // This daemon has no registry to read, which is not a failure of the
        // fleet: the two sources fail independently.
        let no_registry: RegistrySource =
            Arc::new(|_fleet: &FleetObservation, _cancel: &AtomicBool| Ok(None));
        let ahead = Instant::now() + Duration::from_secs(60);
        assert!(matches!(
            collect(&EmptyInventory, &no_registry, &[], &cancel, &sampler, ahead),
            RefreshOutcome::Success { .. }
        ));
        assert!(!cancel.load(Ordering::SeqCst));
        assert!(matches!(
            collect(
                &EmptyInventory,
                &no_registry,
                &[],
                &cancel,
                &sampler,
                Instant::now()
            ),
            RefreshOutcome::Failure { .. }
        ));
    }
}
