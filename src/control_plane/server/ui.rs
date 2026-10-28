//! Executes the effects the shared UI session plans. The session itself lives in
//! [`crate::ui_session`] and the connection in `ui_duplex`; nothing of its own
//! state lives here.

use crate::control_plane::Code;
use crate::control_plane::answer::Reply;
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::store::RequestOutcome;

/// Reuses domain admission directly, not a self-RPC or a second creation path.
pub(super) fn execute(
    effect: crate::ui_session::actor::Effect,
    core: &std::sync::Weak<std::sync::Mutex<super::Core>>,
    verification: &std::sync::Arc<super::VerificationBudget>,
) -> dto::CommandOutcome {
    let job = match effect {
        crate::ui_session::actor::Effect::Dispatch(job) => *job,
        crate::ui_session::actor::Effect::Reconcile(id) => {
            let record = core
                .upgrade()
                .and_then(|core| super::locked(&core).store.get(&id).ok().flatten());
            return match record {
                Some(record) => dto::CommandOutcome::recorded(
                    &id,
                    record_status(record.settled()),
                    record.public(),
                ),
                None => dto::CommandOutcome::new(&id, dto::CommandStatus::SubmissionUncertain),
            };
        }
    };
    // A focus plan names a runtime location, not a request record: the daemon
    // moves the client serving that location, which is the one place the target
    // is known without asking the frontend.
    if let Some(target) = job.plan.as_ref().and_then(|plan| match &plan.effect {
        crate::ui_session::targets::Effect::Focus(target) => Some(target.clone()),
        crate::ui_session::targets::Effect::Domain(_) => None,
    }) {
        let Some(core) = core.upgrade() else {
            return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::DispatchRefused);
        };
        let Some(backend) = super::locked(&core).backend.clone() else {
            return dto::CommandOutcome::error(
                &job.request_id,
                dto::CommandStatus::Refused,
                Code::BackendUnavailable,
                "this daemon has no runtime to move focus on",
            );
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        match backend.focus_outcome(&target, &cancel) {
            crate::runtime::FocusOutcome::Completed => {
                return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Completed);
            }
            crate::runtime::FocusOutcome::NoClient => {
                return dto::CommandOutcome::error(
                    &job.request_id,
                    dto::CommandStatus::Refused,
                    Code::NoClient,
                    crate::runtime::NO_CLIENT,
                )
                .about(target);
            }
            crate::runtime::FocusOutcome::Refused(message) => {
                // A server with nothing attached is answered with the place that
                // could not be shown: the frontend can open a terminal for it.
                return dto::CommandOutcome::error(
                    &job.request_id,
                    dto::CommandStatus::Refused,
                    Code::Refused,
                    &message,
                )
                .about(target);
            }
            crate::runtime::FocusOutcome::Unknown(message) => {
                return dto::CommandOutcome::error(
                    &job.request_id,
                    dto::CommandStatus::Unknown,
                    Code::Unknown,
                    &message,
                );
            }
        }
    }
    use crate::control_plane::command::{Action, Command};
    let command = match &job.plan {
        Some(plan) => match &plan.effect {
            crate::ui_session::targets::Effect::Domain(command) => *command.clone(),
            crate::ui_session::targets::Effect::Focus(_) => unreachable!("focus is handled above"),
        },
        None => {
            let dto::InteractionResult::Project { path } = &job.result else {
                return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Refused);
            };
            Command::new(
                "radar-ui",
                Action::Create {
                    request: crate::runtime::CreateRequest::Workspace {
                        directory: Some(path.clone()),
                        focus: Some(false),
                    },
                    label: Some(crate::project::project_name(std::path::Path::new(path))),
                    recipe: None,
                },
            )
        }
    };
    let Some(core) = core.upgrade() else {
        return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::DispatchRefused);
    };
    let reply = super::execute_command(&job.request_id, command, &core, verification);
    let status = reply_status(&reply);
    match reply {
        Ok(answer) => dto::CommandOutcome::answering(
            &job.request_id,
            status,
            dto::CommandAnswer::Domain {
                id: Some(job.request_id.clone()),
                result: Box::new(answer),
            },
        ),
        Err(refusal) => dto::CommandOutcome::answering(
            &job.request_id,
            status,
            dto::CommandAnswer::Error {
                id: Some(job.request_id.clone()),
                error: dto::OutcomeError {
                    code: refusal.code,
                    message: refusal.message,
                    target: None,
                },
            },
        ),
    }
}

/// What a record establishes for the operator: its verdict once settled, and
/// otherwise only that the daemon has taken it.
fn record_status(outcome: Option<RequestOutcome>) -> dto::CommandStatus {
    use dto::CommandStatus as S;
    match outcome {
        Some(RequestOutcome::Completed) => S::Completed,
        Some(RequestOutcome::Refused) => S::Refused,
        Some(RequestOutcome::Unknown) => S::Unknown,
        None => S::Admitted,
    }
}

/// What a domain reply establishes. A refusal before admission is definite only
/// for the codes that say nothing was recorded; any other error leaves the
/// outcome unestablished.
fn reply_status(reply: &Reply) -> dto::CommandStatus {
    use dto::CommandStatus as S;
    match reply {
        Ok(answer) => record_status(answer.request.settled()),
        Err(refusal) => match refusal.code {
            Code::BadParams | Code::Refused | Code::Busy | Code::BackendUnavailable => S::Refused,
            _ => S::SubmissionUncertain,
        },
    }
}
