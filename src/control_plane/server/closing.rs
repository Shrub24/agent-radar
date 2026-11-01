//! Branches that are being closed right now.
//!
//! A close is not instantaneous: it signals every execution it found, waits out one
//! bounded window, captures what it can, and only then removes the place. In that
//! window a launch into the same branch would be admitted, create work, and find its
//! place removed moments later — a race that leaves the operation record and the
//! topology disagreeing about what exists.
//!
//! So admission refuses a launch into a place a close holds. The hold is memory
//! only, and that is deliberate: it describes what this process is doing at this
//! instant, not a durable fact about the branch. A daemon that restarts mid-close
//! holds nothing, which is correct — the durable operation record is what says the
//! close's outcome is still owed, and recovery is where that is answered.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The places the closes in this process currently hold, by the ids a launch names.
#[derive(Default)]
pub(super) struct Closing {
    places: Mutex<BTreeSet<String>>,
}

impl Closing {
    fn lock(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.places.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a launch naming this place would land inside a branch being closed.
    pub(super) fn holds(&self, place: &str) -> bool {
        self.lock().contains(place)
    }

    /// Holds these places until the returned guard is dropped.
    ///
    /// A place another close already holds stays held by that close: this guard
    /// removes only what it added, so two overlapping closes cannot release each
    /// other's hold.
    pub(super) fn begin(self: &Arc<Self>, places: impl IntoIterator<Item = String>) -> Held {
        let mut added = Vec::new();
        {
            let mut held = self.lock();
            for place in places {
                if held.insert(place.clone()) {
                    added.push(place);
                }
            }
        }
        Held {
            added,
            closing: Arc::clone(self),
        }
    }
}

/// One close's hold, released when the close is done.
pub(super) struct Held {
    added: Vec<String>,
    closing: Arc<Closing>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut held = self.closing.lock();
        for place in &self.added {
            held.remove(place);
        }
    }
}

/// Refuses a launch that would land inside a branch a close is removing.
///
/// The launch has not been admitted yet, so refusing here costs nothing and leaves
/// no record: the caller state is exactly what it was, and the operator sees why.
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
    fn a_hold_is_released_when_the_close_is_done() {
        let closing = Arc::new(Closing::default());
        {
            let _held = closing.begin(["%1".to_string(), "%2".to_string()]);
            assert!(closing.holds("%1"));
            assert!(closing.holds("%2"));
        }
        assert!(!closing.holds("%1"), "a finished close holds nothing");
        assert!(!closing.holds("%2"));
    }

    #[test]
    fn overlapping_closes_hold_their_own_places_only() {
        let closing = Arc::new(Closing::default());
        let first = closing.begin(["%1".to_string()]);
        let second = closing.begin(["%1".to_string(), "%2".to_string()]);
        // The first close keeps its hold: the second only added to it, so dropping
        // the second cannot release a branch the first is still removing.
        drop(second);
        assert!(closing.holds("%1"));
        assert!(!closing.holds("%2"));
        drop(first);
        assert!(!closing.holds("%1"));
    }
}
