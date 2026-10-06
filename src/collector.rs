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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::{FleetObservation, ForegroundEvidence};
use crate::observation::ObservationState;
use crate::procfs::{self, Sampler};
use crate::runtime::RuntimeProvider;

/// Interval between refreshes when [`CollectorConfig::poll_interval`] is left
/// at its default.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
    },
}

/// A refresh running on its own thread.
struct Refresh {
    /// Set by [`Collector::shutdown`]; the worker turns it into a cancellation.
    cancel: Arc<AtomicBool>,
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
    pub fn new(config: CollectorConfig, provider: impl RuntimeProvider + 'static) -> Self {
        Self::sampling(config, provider, Sampler::new())
    }

    /// A collector sampling with `sampler`, as tests and callers that must not
    /// read this machine's process table supply.
    pub fn sampling(
        config: CollectorConfig,
        provider: impl RuntimeProvider + 'static,
        sampler: Sampler,
    ) -> Self {
        Self {
            config,
            provider: Arc::new(provider),
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
        let applied = self.apply_finished(state);
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
            RefreshOutcome::Failure { diagnostic } => state.apply_failure(diagnostic),
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
        let (sender, outcomes) = mpsc::channel();
        let worker = {
            let provider = Arc::clone(&self.provider);
            let cancel = Arc::clone(&cancel);
            let sampler = Arc::clone(&self.sampler);
            thread::Builder::new()
                .name("radar-collector".to_string())
                .spawn(move || {
                    let _ = sender.send(collect(provider.as_ref(), &candidates, &cancel, &sampler));
                })
                .expect("collector thread")
        };
        self.in_flight = Some(Refresh {
            cancel,
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

/// Runs one refresh: the inventory, then evidence for each candidate.
fn collect(
    provider: &dyn RuntimeProvider,
    candidates: &[String],
    cancel: &AtomicBool,
    sampler: &Mutex<Sampler>,
) -> RefreshOutcome {
    let mut observation = match provider.inventory(cancel) {
        Ok(observation) => observation,
        Err(diagnostic) => return RefreshOutcome::Failure { diagnostic },
    };
    stamp_assignment_ages(&mut observation);
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
    if !candidates.is_empty() {
        sampler.scan(now, cancel);
    }
    let evidence = candidates
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

    #[test]
    fn the_poll_interval_defaults_to_one_second() {
        assert_eq!(
            CollectorConfig::default().poll_interval,
            Duration::from_secs(1)
        );
    }
}
