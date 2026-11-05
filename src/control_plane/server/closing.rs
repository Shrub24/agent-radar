//! In-flight branch holds. Durable operation records, not these holds, own recovery.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// Told the held places each time the set changes, so a display can show a branch
/// as closing rather than only gone.
type Watcher = Arc<dyn Fn(Vec<String>) + Send + Sync>;

#[derive(Default)]
pub(super) struct Closing {
    places: Mutex<BTreeMap<String, usize>>,
    watcher: OnceLock<Watcher>,
}

impl Closing {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, usize>> {
        self.places.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn watch(&self, watcher: Watcher) {
        let _ = self.watcher.set(watcher);
    }

    /// Called with the lock held, so snapshots reach the watcher in the order the
    /// set changed: a stale one can never overwrite a newer.
    fn changed(&self, held: &BTreeMap<String, usize>) {
        if let Some(watcher) = self.watcher.get() {
            watcher(held.keys().cloned().collect());
        }
    }

    pub(super) fn holds(&self, place: &str) -> bool {
        self.lock().contains_key(place)
    }

    pub(super) fn begin(self: &Arc<Self>, places: impl IntoIterator<Item = String>) -> Held {
        let places: BTreeSet<_> = places.into_iter().collect();
        let mut held = self.lock();
        for place in &places {
            *held.entry(place.clone()).or_default() += 1;
        }
        self.changed(&held);
        drop(held);
        Held {
            places,
            closing: Arc::clone(self),
        }
    }
}

pub(super) struct Held {
    places: BTreeSet<String>,
    closing: Arc<Closing>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut held = self.closing.lock();
        for place in &self.places {
            let count = held.get_mut(place).expect("a guard owns its hold");
            *count -= 1;
            if *count == 0 {
                held.remove(place);
            }
        }
        self.closing.changed(&held);
    }
}

pub(super) fn refuse_if_closing(
    core: &Arc<Mutex<super::Core>>,
    place: &str,
) -> Result<(), crate::control_plane::protocol::Refusal> {
    let closing = Arc::clone(&super::locked(core).closing);
    if !closing.holds(place) {
        return Ok(());
    }
    Err(crate::control_plane::protocol::Refusal::new(
        crate::control_plane::protocol::Code::Busy,
        format!("{place} is inside a branch that is being closed"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watcher_hears_the_held_places_when_a_hold_starts_and_ends() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let closing = Arc::new(Closing::default());
        let sink = Arc::clone(&heard);
        closing.watch(Arc::new(move |places| sink.lock().unwrap().push(places)));
        let first = closing.begin(["%1".into()]);
        let second = closing.begin(["%1".into(), "%2".into()]);
        drop(first);
        drop(second);
        assert_eq!(
            *heard.lock().unwrap(),
            vec![
                vec!["%1".to_string()],
                vec!["%1".to_string(), "%2".to_string()],
                vec!["%1".to_string(), "%2".to_string()],
                Vec::<String>::new(),
            ]
        );
    }

    #[test]
    fn a_slow_delivery_never_lands_after_a_newer_set() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let last = Arc::new(Mutex::new(vec!["unset".to_string()]));
        let calls = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(Closing::default());
        let (sink, count) = (Arc::clone(&last), Arc::clone(&calls));
        closing.watch(Arc::new(move |places| {
            // The third delivery lingers; the one after it must still end up last.
            if count.fetch_add(1, Ordering::SeqCst) == 2 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            *sink.lock().unwrap() = places;
        }));
        let first = closing.begin(["%1".into()]);
        let second = closing.begin(["%1".into(), "%2".into()]);
        std::thread::scope(|scope| {
            scope.spawn(move || drop(first));
            std::thread::sleep(std::time::Duration::from_millis(20));
            drop(second);
        });
        assert!(
            last.lock().unwrap().is_empty(),
            "{:?}",
            last.lock().unwrap()
        );
    }

    #[test]
    fn overlapping_closes_keep_the_place_held_until_both_finish() {
        for first_finishes_first in [true, false] {
            let closing = Arc::new(Closing::default());
            let first = closing.begin(["%1".into(), "%1".into()]);
            let second = closing.begin(["%1".into(), "%2".into()]);
            if first_finishes_first {
                drop(first);
                assert!(closing.holds("%1"));
                assert!(closing.holds("%2"));
                drop(second);
            } else {
                drop(second);
                assert!(closing.holds("%1"));
                assert!(!closing.holds("%2"));
                drop(first);
            }
            assert!(!closing.holds("%1"));
            assert!(!closing.holds("%2"));
        }
    }
}
