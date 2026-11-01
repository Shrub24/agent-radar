//! What a close established, in the words an operator needs.
//!
//! The record publishes the parts rather than a sentence, and this is the one place
//! they become a line: the execution's end, what became of the pane, the program's
//! completion and whether the shutdown had to be escalated. Each part keeps its own
//! meaning, so a view never reports an unverified exit as an exit, and never reports
//! a pane it kept as a failure.

use crate::control_plane::lifecycle::{ClosedExecution, ExecutionEnd, ResourceEnd};

/// One line for a close's report, or `None` when the report says only that the
/// close is done — a removal that went as asked is not news.
pub fn describe(closed: &ClosedExecution) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match &closed.execution {
        ExecutionEnd::Exited => {}
        ExecutionEnd::Unverified(reason) => {
            parts.push(format!("exit not verified: {reason}"));
        }
        ExecutionEnd::Unsignalled(reason) => {
            parts.push(format!("nothing was signalled: {reason}"));
        }
    }
    match &closed.resource {
        // The resource going away is what was asked for. Only an answer that
        // differs from that is worth a line.
        None | Some(ResourceEnd::Removed) | Some(ResourceEnd::Absent) => {}
        Some(ResourceEnd::Kept(reason)) | Some(ResourceEnd::Unverified(reason)) => {
            parts.push(format!("pane left alone: {reason}"));
        }
    }
    if closed.escalated {
        parts.push("killed after the grace".into());
    }
    if let Some(completion) = &closed.completion {
        parts.push(match (completion.code, completion.signal) {
            (Some(code), _) => format!("program exited {code}"),
            (None, Some(signal)) => format!("program died on signal {signal}"),
            (None, None) => "program exit has no code".into(),
        });
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::lifecycle::{Completion, ExecutionEnd, ResourceEnd};

    fn closed(
        execution: ExecutionEnd,
        resource: Option<ResourceEnd>,
        completion: Option<Completion>,
        escalated: bool,
    ) -> ClosedExecution {
        ClosedExecution {
            execution,
            resource,
            completion,
            escalated,
        }
    }

    #[test]
    fn a_removal_that_went_as_asked_says_nothing() {
        assert_eq!(
            describe(&closed(
                ExecutionEnd::Exited,
                Some(ResourceEnd::Removed),
                None,
                false
            )),
            None,
            "a plain close is not news"
        );
    }

    #[test]
    fn an_unverified_exit_is_named_rather_than_claimed() {
        let line = describe(&closed(
            ExecutionEnd::Unverified("the scope did not empty".into()),
            Some(ResourceEnd::Removed),
            None,
            false,
        ))
        .expect("a line");
        assert!(line.contains("exit not verified"), "{line}");
        assert!(line.contains("the scope did not empty"), "{line}");
        assert!(
            !line.contains("exited"),
            "an unverified exit is no exit: {line}"
        );
    }

    #[test]
    fn a_kept_pane_and_a_completion_are_their_own_facts() {
        let line = describe(&closed(
            ExecutionEnd::Exited,
            Some(ResourceEnd::Kept("no pane of this daemon's".into())),
            Some(Completion {
                code: Some(130),
                signal: None,
                observed_at: "2026-01-01T00:00:00.000Z".into(),
            }),
            true,
        ))
        .expect("a line");
        assert!(line.contains("pane left alone"), "{line}");
        assert!(line.contains("program exited 130"), "{line}");
        assert!(line.contains("killed after the grace"), "{line}");
    }
}
