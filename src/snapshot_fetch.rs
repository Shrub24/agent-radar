//! The dashboard's snapshot fetcher.
//!
//! The dashboard no longer collects. It reads the daemon's revisioned snapshot
//! and keeps only its own view: the tree build, folds, selection and filter. A
//! read that fails or names a revision the daemon no longer holds leaves the
//! previous view standing and visibly stale, rather than mixing two revisions
//! into one screen.
//!
//! The fetch runs on its own thread so input and quitting stay responsive, and
//! it names the revision it already holds so the daemon can hold the read open
//! for a newer one instead of the dashboard polling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::control_plane::client::{DaemonRuntime, ObservationRead};
use crate::control_plane::observation::Snapshot;

/// How long one fetch waits for a newer revision before asking again.
const WAIT_MS: u64 = 1000;

/// How long to wait after a failed read before trying again.
const RETRY: Duration = Duration::from_millis(500);

/// What a fetch produced, for the dashboard to apply.
pub enum Fetched {
    /// A complete revision. It is applied whole or not at all.
    Snapshot(Box<Snapshot>),
    /// The read failed. The view stands as it was, with this reason.
    Stale(String),
}

/// What the dashboard is currently showing, so the sampler knows what to
/// collect. Only the process table is optional.
#[derive(Default)]
struct Wanted {
    processes: bool,
}

/// Reads the daemon's shared observation snapshot on its own thread.
pub struct SnapshotFetcher {
    cancel: Arc<AtomicBool>,
    wanted: Arc<Mutex<Wanted>>,
    slot: Arc<Mutex<Option<Fetched>>>,
    worker: Option<JoinHandle<()>>,
}

impl SnapshotFetcher {
    /// Starts fetching from `daemon`.
    pub fn new(daemon: Arc<DaemonRuntime>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let wanted = Arc::new(Mutex::new(Wanted::default()));
        let slot: Arc<Mutex<Option<Fetched>>> = Arc::new(Mutex::new(None));
        let worker = {
            let cancel = Arc::clone(&cancel);
            let wanted = Arc::clone(&wanted);
            let slot = Arc::clone(&slot);
            thread::Builder::new()
                .name("radar-dashboard-fetch".to_string())
                .spawn(move || fetch_loop(daemon, cancel, wanted, slot))
                .ok()
        };
        Self {
            cancel,
            wanted,
            slot,
            worker,
        }
    }

    /// Records what the dashboard is showing, which is what the daemon samples
    /// for. Cheap enough to call every frame.
    pub fn wants(&self, processes: bool) {
        let mut wanted = locked(&self.wanted);
        wanted.processes = processes;
    }

    /// Takes a finished fetch, if one is waiting. Never blocks.
    pub fn poll(&self) -> Option<Fetched> {
        locked(&self.slot).take()
    }
}

impl Drop for SnapshotFetcher {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One fetch, repeated until the dashboard stops.
///
/// The revision the fetcher holds is its own: a snapshot the daemon no longer
/// keeps comes back as `snapshot_expired`, and the next read starts from the
/// current revision instead of asking for a page of a revision that is gone.
fn fetch_loop(
    daemon: Arc<DaemonRuntime>,
    cancel: Arc<AtomicBool>,
    wanted: Arc<Mutex<Wanted>>,
    slot: Arc<Mutex<Option<Fetched>>>,
) {
    let mut epoch: Option<String> = None;
    let mut revision: Option<u64> = None;
    while !cancel.load(Ordering::SeqCst) {
        let processes = locked(&wanted).processes;
        let mut classes = vec!["fleet", "registry", "tasks"];
        if processes {
            classes.push("processes");
        }
        match daemon.observation_get(epoch.as_deref(), revision, WAIT_MS, &classes, &cancel) {
            Ok(ObservationRead::Snapshot(snapshot)) => {
                epoch = Some(snapshot.epoch.clone());
                revision = Some(snapshot.revision);
                *locked(&slot) = Some(Fetched::Snapshot(snapshot));
            }
            // The daemon holds the revision this fetcher has and published
            // nothing newer within the wait. Asking again is the whole loop.
            Ok(ObservationRead::NotModified { .. }) => {}
            Ok(ObservationRead::Expired {
                epoch: held,
                revision: at,
            }) => {
                epoch = Some(held);
                revision = Some(at);
            }
            Ok(ObservationRead::TooLarge) => {
                *locked(&slot) = Some(Fetched::Stale(
                    "the daemon's observation snapshot is larger than one response".to_string(),
                ));
                sleep(RETRY, &cancel);
            }
            Err(problem) => {
                *locked(&slot) = Some(Fetched::Stale(problem));
                sleep(RETRY, &cancel);
            }
        }
    }
}

fn sleep(for_: Duration, cancel: &AtomicBool) {
    let deadline = std::time::Instant::now() + for_;
    while std::time::Instant::now() < deadline {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}
