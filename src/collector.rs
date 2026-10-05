//! Non-blocking `herdr` CLI collection.
//!
//! This module owns every process Radar runs. One refresh is in flight at a
//! time and it runs on its own thread: `herdr api snapshot` first, then
//! `herdr pane process-info --pane <id>` for exactly the panes the reconciler
//! reports as continuity candidates. [`Collector::tick`] never blocks: it
//! applies a finished refresh to the caller-owned [`ObservationState`], which
//! keeps the reconciler single-owner and lock-free, and starts the next
//! refresh once the poll interval has elapsed.
//!
//! Two properties the main loop depends on:
//!
//! - **Input and quit stay responsive.** No command runs on the calling
//!   thread, and a command that outlives [`CollectorConfig::command_timeout`]
//!   is killed and reaped rather than waited for.
//! - **A stalled command leaks nothing.** Both output pipes are drained
//!   concurrently — a command that fills a pipe must not deadlock while we
//!   wait for it to exit — captured output is capped, and
//!   [`Collector::shutdown`] cancels and joins the worker so no child or
//!   thread outlives the process. On Unix commands have isolated process groups,
//!   terminated before joining readers so descendants cannot hold output pipes
//!   open and delay shutdown.
//!
//! Whole-source failures are distinguishable from continuity evidence: a
//! failed or unreadable snapshot is a source diagnostic (the caller's
//! last-good inventory stays stale), while a failed or unreadable
//! process-info answer is [`ForegroundEvidence::Inconclusive`] and never
//! proves that either the source or the pane disappeared.
//!
//! Which panes get queried comes from
//! [`ObservationState::continuity_candidates`], which the reconciler re-arms on
//! every successful inventory. Because candidates are chosen before that
//! inventory is applied, a retained pane's evidence is refreshed on alternate
//! cycles: retention itself is immediate, and its shell/non-shell confirmation
//! lands within two poll intervals.

use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::herdr::{decode_process_info, decode_snapshot};
use crate::model::{FleetObservation, ForegroundEvidence};
use crate::observation::ObservationState;
use crate::procfs;

/// Interval between refreshes when [`CollectorConfig::poll_interval`] is left
/// at its default.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Bound on one `herdr` invocation when [`CollectorConfig::command_timeout`]
/// is left at its default.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a running command is checked for exit, timeout and cancellation.
/// Small enough that cancellation feels immediate, large enough not to spin.
const WAIT_GRANULARITY: Duration = Duration::from_millis(10);

/// Most stdout kept from one command; the rest is discarded.
const STDOUT_CAP: usize = 8 * 1024 * 1024;

/// Most stderr kept from one command (only its first line reaches a diagnostic).
const STDERR_CAP: usize = 8 * 1024;

/// Longest stderr excerpt quoted in a diagnostic.
const DIAGNOSTIC_CHARS: usize = 200;

/// Collection settings.
#[derive(Clone, Debug)]
pub struct CollectorConfig {
    /// The `herdr` executable: a name resolved through `PATH`, or a path.
    pub executable: PathBuf,
    /// Delay between refreshes. The first refresh starts on the first
    /// [`Collector::tick`], so startup does not wait out an interval.
    pub poll_interval: Duration,
    /// Hard bound on one `herdr` invocation. A command still running at the
    /// deadline is killed and reaped.
    pub command_timeout: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("herdr"),
            poll_interval: DEFAULT_POLL_INTERVAL,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
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
    /// Set by [`Collector::shutdown`]; the worker turns it into a kill.
    cancel: Arc<AtomicBool>,
    outcomes: Receiver<RefreshOutcome>,
    worker: JoinHandle<()>,
}

/// Collects `herdr` observations for the main loop, one refresh at a time.
///
/// Call [`Collector::tick`] on every loop iteration (at least as often as the
/// poll interval, so a due refresh is noticed promptly) and
/// [`Collector::shutdown`] before exit; dropping the collector shuts down too.
pub struct Collector {
    config: CollectorConfig,
    /// When the next refresh may start; already due at construction.
    next_refresh: Instant,
    in_flight: Option<Refresh>,
    stopped: bool,
}

impl Collector {
    /// A collector that runs nothing until the first [`Self::tick`].
    pub fn new(config: CollectorConfig) -> Self {
        Self {
            config,
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
    /// `state` changed, so a caller can skip a redraw. Never waits for
    /// `herdr`: a refresh is applied by the tick that finds it finished.
    pub fn tick(&mut self, state: &mut ObservationState, processes: bool) -> bool {
        if self.stopped {
            return false;
        }
        let applied = self.apply_finished(state);
        if self.in_flight.is_none() && Instant::now() >= self.next_refresh {
            let candidates = if processes {
                state.pane_ids()
            } else {
                state.continuity_candidates()
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

    /// Stops collecting: cancels an in-flight command, waits until it is
    /// killed and reaped, and makes later ticks no-ops. Idempotent.
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
    /// cancelled, so no child outlives the caller.
    fn join(&self, refresh: Refresh) {
        // A panicking worker unwinds after killing and reaping its own child;
        // the failure diagnostic above is the visible report of that.
        let _ = refresh.worker.join();
    }

    /// Starts one refresh off the calling thread.
    fn start_refresh(&mut self, candidates: Vec<String>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcomes) = mpsc::channel();
        let worker = {
            let config = self.config.clone();
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("radar-collector".to_string())
                .spawn(move || {
                    let _ = sender.send(collect(&config, &candidates, &cancel));
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

/// Runs one refresh: the snapshot, then evidence for each candidate.
fn collect(config: &CollectorConfig, candidates: &[String], cancel: &AtomicBool) -> RefreshOutcome {
    let stdout = match run(config, &["api", "snapshot"], cancel) {
        Ok(stdout) => stdout,
        Err(diagnostic) => return RefreshOutcome::Failure { diagnostic },
    };
    match decode_snapshot(&stdout) {
        Ok(mut observation) => {
            stamp_assignment_ages(&mut observation);
            RefreshOutcome::Success {
                observation,
                evidence: candidates
                    .iter()
                    .map(|pane_id| (pane_id.clone(), evidence_for(config, pane_id, cancel)))
                    .collect(),
            }
        }
        Err(error) => RefreshOutcome::Failure {
            diagnostic: format!("herdr snapshot could not be read: {error}"),
        },
    }
}

/// Measures every agent's assignment age against one read of the clock.
///
/// The decoder stays clockless and fixture-driven, so the age is stamped here,
/// where the observation is produced; one instant for the whole snapshot keeps
/// the rows of a single frame comparable.
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

/// Foreground evidence for one continuity candidate.
///
/// A pane whose query fails, times out or cannot be decoded is inconclusive: an
/// unreadable answer is not evidence of a replacement, and it never fails the
/// refresh that produced the candidate.
fn evidence_for(
    config: &CollectorConfig,
    pane_id: &str,
    cancel: &AtomicBool,
) -> ForegroundEvidence {
    let mut evidence = run(config, &["pane", "process-info", "--pane", pane_id], cancel)
        .ok()
        .and_then(|stdout| decode_process_info(&stdout).ok())
        .unwrap_or(ForegroundEvidence::Inconclusive);
    // Herdr names the command; this machine knows how long it has held the pane
    // and whether it has taken the terminal over. Asked here, where the process
    // is already in hand, so nothing else has to know about /proc.
    if let ForegroundEvidence::NonShell { pid, local, .. } = &mut evidence {
        *local = procfs::facts(*pid);
    }
    evidence
}

/// Runs one `herdr` subcommand, returning its stdout.
///
/// `Err` carries a user-facing diagnostic; the command has been killed and
/// reaped by then. Shared with [`crate::focus`], which runs the same CLI under
/// the same timeout and reaping rules.
pub(crate) fn run(
    config: &CollectorConfig,
    args: &[&str],
    cancel: &AtomicBool,
) -> Result<String, String> {
    let executable = &config.executable;
    let mut command = Command::new(executable);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", executable.display()))?;

    // Drain both pipes on their own threads: waiting for a command that fills
    // a pipe would deadlock, and the same threads keep its output bounded.
    let stdout = drain(child.stdout.take().expect("stdout is piped"), STDOUT_CAP);
    let stderr = drain(child.stderr.take().expect("stderr is piped"), STDERR_CAP);

    let status = wait_for_exit(&mut child, config, cancel);
    // Descendants may keep the output pipes open after the CLI exits. The
    // collector owns this isolated group, so end it before joining readers.
    #[cfg(unix)]
    unsafe {
        // SAFETY: process_group(0) assigned this child its own group; a negative
        // PID targets only that group, never Radar or the pane it observes.
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let stdout = stdout.join().expect("stdout drain thread panicked");
    let stderr = stderr.join().expect("stderr drain thread panicked");

    match status {
        Ok(status) if status.success() => Ok(String::from_utf8_lossy(&stdout).into_owned()),
        Ok(status) => Err(diagnostic(exit_diagnostic(status), &stderr)),
        Err(reason) => Err(diagnostic(reason, &stderr)),
    }
}

/// Waits for a command to exit, killing and reaping it on timeout or
/// cancellation. `Err` is the diagnostic for having killed it.
fn wait_for_exit(
    child: &mut Child,
    config: &CollectorConfig,
    cancel: &AtomicBool,
) -> Result<ExitStatus, String> {
    let timeout = config.command_timeout;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return Err(format!("herdr could not be waited for: {error}"));
            }
        }
        if cancel.load(Ordering::SeqCst) {
            kill_and_reap(child);
            return Err("collection cancelled".to_string());
        }
        if Instant::now() >= deadline {
            kill_and_reap(child);
            return Err(format!("herdr timed out after {timeout:?}"));
        }
        thread::sleep(WAIT_GRANULARITY);
    }
}

/// Kills a still-running command and reaps it, so a stalled command leaves no
/// process — not even a zombie — behind.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads one of a command's pipes to end of file on its own thread, keeping at
/// most `cap` bytes and discarding the rest.
///
/// Reading past the cap rather than stopping is deliberate: an unread pipe
/// fills at the OS buffer size and would block the command forever.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0u8; 8 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => return kept,
                Ok(read) => {
                    let room = cap - kept.len();
                    kept.extend_from_slice(&buffer[..read.min(room)]);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return kept,
            }
        }
    })
}

/// How a command ended, in terms a user can act on.
fn exit_diagnostic(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("herdr exited with status {code}"),
        None => format!("herdr was killed ({status})"),
    }
}

/// Quotes the first stderr line in a failure diagnostic: enough to identify the
/// problem without dumping a whole stream into the overview.
fn diagnostic(reason: String, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let Some(line) = stderr.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return reason;
    };
    let line: String = line.chars().take(DIAGNOSTIC_CHARS).collect();
    format!("{reason}: {line}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn defaults_match_the_documented_initial_settings() {
        let config = CollectorConfig::default();
        assert_eq!(config.executable, PathBuf::from("herdr"));
        assert_eq!(config.poll_interval, Duration::from_secs(1));
        assert_eq!(config.command_timeout, Duration::from_secs(5));
    }

    #[test]
    fn draining_keeps_the_cap_and_still_reads_to_end_of_file() {
        let oversize = vec![b'x'; STDOUT_CAP + 512];
        let kept = drain(Cursor::new(oversize), STDOUT_CAP)
            .join()
            .expect("drain thread");
        assert_eq!(kept.len(), STDOUT_CAP);

        let short = b"{\"result\":{}}".to_vec();
        let kept = drain(Cursor::new(short.clone()), STDOUT_CAP)
            .join()
            .expect("drain thread");
        assert_eq!(kept, short);
    }

    #[test]
    fn a_diagnostic_quotes_only_the_first_stderr_line() {
        let stderr = b"\n  herdr: no runtime is listening  \nmore detail\n";
        assert_eq!(
            diagnostic("herdr exited with status 3".to_string(), stderr),
            "herdr exited with status 3: herdr: no runtime is listening"
        );
        assert_eq!(
            diagnostic("herdr timed out after 5s".to_string(), b""),
            "herdr timed out after 5s"
        );

        let long = format!("{}\n", "e".repeat(DIAGNOSTIC_CHARS + 50));
        let quoted = diagnostic("failed".to_string(), long.as_bytes());
        assert_eq!(quoted.chars().count(), "failed: ".len() + DIAGNOSTIC_CHARS);
    }
}
