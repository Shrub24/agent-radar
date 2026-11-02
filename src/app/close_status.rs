//! Operator-facing exceptions to a close's normal outcome.

use crate::control_plane::lifecycle::{ClosedExecution, ExecutionEnd, ResourceEnd};

pub(super) fn describe(closed: &ClosedExecution) -> Option<String> {
    let mut parts = Vec::new();
    match &closed.execution {
        ExecutionEnd::Exited | ExecutionEnd::NotApplicable => {}
        ExecutionEnd::Unverified(reason) => parts.push(format!("exit not verified: {reason}")),
        ExecutionEnd::Unsignalled(reason) => parts.push(format!("nothing was signalled: {reason}")),
    }
    match &closed.resource {
        None | Some(ResourceEnd::Removed | ResourceEnd::Absent) => {}
        Some(ResourceEnd::Kept(reason)) => parts.push(format!("pane kept: {reason}")),
        Some(ResourceEnd::Unverified(reason)) => {
            parts.push(format!("pane removal not verified: {reason}"));
        }
    }
    if closed.escalated {
        parts.push("scope escalation attempted".into());
    }
    // Completion is diagnostic detail; a missing code never makes Close fail.
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::lifecycle::Completion;

    #[test]
    fn normal_removal_is_quiet_with_or_without_a_completion() {
        let mut closed = ClosedExecution {
            execution: ExecutionEnd::Exited,
            resource: Some(ResourceEnd::Removed),
            completion: None,
            escalated: false,
        };
        assert_eq!(describe(&closed), None);
        closed.completion = Some(Completion {
            code: Some(7),
            signal: None,
            observed_at: "2026-01-01T00:00:00.000Z".into(),
        });
        assert_eq!(describe(&closed), None);
    }

    #[test]
    fn an_uncertain_removal_does_not_claim_the_pane_was_kept() {
        let closed = ClosedExecution {
            execution: ExecutionEnd::Unverified("scope kill failed".into()),
            resource: Some(ResourceEnd::Unverified("reply lost".into())),
            completion: None,
            escalated: true,
        };
        assert_eq!(
            describe(&closed).as_deref(),
            Some(
                "exit not verified: scope kill failed · pane removal not verified: reply lost · scope escalation attempted"
            )
        );
    }
}
