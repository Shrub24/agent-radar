//! Operator-facing exceptions to a close's normal outcome.

use crate::control_plane::lifecycle::{ClosedExecution, EndedExecution, ExecutionEnd, ResourceEnd};

pub(super) fn describe(closed: &ClosedExecution) -> Option<String> {
    let mut parts: Vec<String> = closed
        .executions
        .iter()
        .filter_map(|ended| exception(ended, closed.executions.len() > 1))
        .collect();
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

/// One execution's exception to a normal outcome, named when the close covered
/// more than one: an operator who is told an exit was not verified has to know
/// which agent it was.
fn exception(ended: &EndedExecution, named: bool) -> Option<String> {
    let subject = match &ended.end {
        ExecutionEnd::Exited => return None,
        ExecutionEnd::Unverified(reason) => format!("exit not verified: {reason}"),
        ExecutionEnd::Unsignalled(reason) => format!("nothing was signalled: {reason}"),
    };
    Some(if named {
        format!("{} {subject}", ended.name())
    } else {
        subject
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::lifecycle::Completion;

    fn execution(subject: Option<&str>, end: ExecutionEnd) -> EndedExecution {
        EndedExecution {
            subject_id: subject.map(str::to_string),
            registration_id: "9d8c7b6a-5f4e-4d3c-9b2a-1809f7e6d5c4".into(),
            end,
            completion: None,
        }
    }

    #[test]
    fn normal_removal_is_quiet_with_or_without_a_completion() {
        let mut closed = ClosedExecution {
            executions: vec![execution(
                Some("c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"),
                ExecutionEnd::Exited,
            )],
            resource: Some(ResourceEnd::Removed),
            escalated: false,
        };
        assert_eq!(describe(&closed), None);
        closed.executions[0].completion = Some(Completion {
            code: Some(7),
            signal: None,
            observed_at: "2026-01-01T00:00:00.000Z".into(),
        });
        assert_eq!(describe(&closed), None);
    }

    #[test]
    fn an_uncertain_removal_does_not_claim_the_pane_was_kept() {
        let closed = ClosedExecution {
            executions: vec![execution(
                Some("c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"),
                ExecutionEnd::Unverified("scope kill failed".into()),
            )],
            resource: Some(ResourceEnd::Unverified("reply lost".into())),
            escalated: true,
        };
        assert_eq!(
            describe(&closed).as_deref(),
            Some(
                "exit not verified: scope kill failed · pane removal not verified: reply lost · scope escalation attempted"
            )
        );
    }

    #[test]
    fn a_survivor_of_a_branch_close_is_named() {
        let closed = ClosedExecution {
            executions: vec![
                execution(
                    Some("c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"),
                    ExecutionEnd::Exited,
                ),
                execution(
                    Some("0f1e2d3c-4b5a-4697-8877-665544332211"),
                    ExecutionEnd::Unverified("the process outlived the shutdown window".into()),
                ),
                execution(
                    Some("aabbccdd-eeff-4a00-8111-223344556677"),
                    ExecutionEnd::Exited,
                ),
            ],
            resource: Some(ResourceEnd::Removed),
            escalated: true,
        };
        assert_eq!(
            describe(&closed).as_deref(),
            Some(
                "0f1e2d3c exit not verified: the process outlived the shutdown window · scope escalation attempted"
            )
        );
    }

    #[test]
    fn a_place_that_hosted_nothing_reports_no_execution() {
        let closed = ClosedExecution {
            executions: Vec::new(),
            resource: Some(ResourceEnd::Removed),
            escalated: false,
        };
        assert_eq!(describe(&closed), None);
    }
}
