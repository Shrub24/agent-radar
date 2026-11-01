//! In-flight branch holds. Durable operation records, not these holds, own recovery.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Default)]
pub(super) struct Closing {
    places: Mutex<BTreeMap<String, usize>>,
}

impl Closing {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, usize>> {
        self.places.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn holds(&self, place: &str) -> bool {
        self.lock().contains_key(place)
    }

    pub(super) fn begin(self: &Arc<Self>, places: impl IntoIterator<Item = String>) -> Held {
        let places: BTreeSet<_> = places.into_iter().collect();
        {
            let mut held = self.lock();
            for place in &places {
                *held.entry(place.clone()).or_default() += 1;
            }
        }
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
