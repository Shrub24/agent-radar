//! Test support for work the daemon applies off the caller's thread.
//!
//! A socket write returning says the bytes were accepted, not that the daemon
//! has ingested them: publishers are read and applied by the server, effects run
//! off the connection that asked for them, and a subscription is served after
//! its acceptor. A test that asserts on the result must wait for the settled
//! state; asserting straight after the write is a race that passes under a quiet
//! suite and fails under a loaded one, which is the worst kind.

use std::time::{Duration, Instant};

/// How long a test waits for asynchronously applied state before failing.
pub(crate) const SETTLE: Duration = Duration::from_secs(5);

/// How often a wait re-checks.
const POLL: Duration = Duration::from_millis(5);

/// Waits for `condition` to yield a value, failing the test with `what` if it
/// never does.
///
/// The condition returns the value it wants rather than a boolean, so the read
/// that observes the settled state is the read the test asserts on: there is no
/// window between "it happened" and "the assertion looks".
pub(crate) fn eventually<T>(what: &str, mut condition: impl FnMut() -> Option<T>) -> T {
    eventually_within(SETTLE, what, &mut condition)
}

/// [`eventually`] with an explicit bound, for a wait that legitimately takes
/// longer than a settle.
pub(crate) fn eventually_within<T>(
    timeout: Duration,
    what: &str,
    mut condition: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = condition() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {timeout:?}"
        );
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::panic;

    #[test]
    fn a_value_is_returned_as_soon_as_the_condition_holds() {
        let seen = Cell::new(0);
        let value = eventually("the third look", || {
            seen.set(seen.get() + 1);
            (seen.get() == 3).then_some(seen.get())
        });
        assert_eq!(value, 3, "the wait returns what the condition settled on");
        assert_eq!(seen.get(), 3, "and stops looking once it holds");
    }

    #[test]
    fn a_condition_that_never_holds_fails_naming_what_was_waited_for() {
        let failure = panic::catch_unwind(|| {
            eventually_within(Duration::from_millis(20), "the snapshot", || None::<()>)
        })
        .expect_err("a wait that never settles must fail");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains("the snapshot"),
            "the failure names what the test waited for: {message}"
        );
    }
}
