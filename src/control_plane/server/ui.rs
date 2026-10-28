//! Executes the effects the shared UI session plans. The session itself lives in
//! [`crate::ui_session`] and the connection in `ui_duplex`; nothing of its own
//! state lives here.

use serde_json::json;

use super::Request;
use crate::control_plane::Code;
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::answer::Answer;
use crate::control_plane::store::{RequestOutcome, RequestRecord};

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
                Some(record) => {
                    let status = record_status(&record);
                    let answer = json!({"result": { "request": record.public() }});
                    dto::CommandOutcome::answering(&id, status, answer)
                }
                None => dto::CommandOutcome::new(&id, dto::CommandStatus::SubmissionUncertain),
            };
        }
    };
    // A focus plan names a runtime location, not a request record: the daemon
    // moves the client serving that location, which is the one place the target
    // is known without asking the frontend.
    if let Some(target) = job.plan.as_ref().and_then(|plan| match &plan.effect {
        crate::ui_session::targets::Effect::Focus(target) => Some(target.clone()),
        crate::ui_session::targets::Effect::Domain { .. } => None,
    }) {
        let Some(core) = core.upgrade() else {
            return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::DispatchRefused);
        };
        let Some(backend) = super::locked(&core).backend.clone() else {
            return dto::CommandOutcome::error(
                &job.request_id,
                dto::CommandStatus::Refused,
                "backend_unavailable",
                "this daemon has no runtime to move focus on",
            );
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        match backend.focus_outcome(&target, &cancel) {
            crate::runtime::FocusOutcome::Completed => {
                return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Completed);
            }
            crate::runtime::FocusOutcome::Refused(message) => {
                // A server with nothing attached is answered with the place that
                // could not be shown: the frontend can open a terminal for it.
                let code = if message == crate::runtime::NO_CLIENT {
                    "no_client"
                } else {
                    "refused"
                };
                return dto::CommandOutcome::answering(
                    &job.request_id,
                    dto::CommandStatus::Refused,
                    json!({"error": {"code": code, "message": message, "target": target}}),
                );
            }
            crate::runtime::FocusOutcome::Unknown(message) => {
                return dto::CommandOutcome::error(
                    &job.request_id,
                    dto::CommandStatus::Unknown,
                    "unknown",
                    &message,
                );
            }
        }
    }
    // A frozen plan carries the request a client would have sent, under the
    // identity it was frozen with; a project answer names the directory the
    // daemon creates a workspace in.
    let (method, params) = match &job.plan {
        Some(plan) => match &plan.effect {
            crate::ui_session::targets::Effect::Domain { method, params } => {
                (method.to_string(), params.clone())
            }
            // A focus never reaches this branch: it is served above.
            crate::ui_session::targets::Effect::Focus(_) => {
                return dto::CommandOutcome::new(
                    &job.request_id,
                    dto::CommandStatus::DispatchRefused,
                );
            }
        },
        None => {
            let dto::InteractionResult::Project { path } = &job.result else {
                return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Refused);
            };
            (
                "topology.create".to_string(),
                json!({
                    "request": { "kind": "workspace", "directory": path, "focus": false },
                    "label": crate::project::project_name(std::path::Path::new(path)),
                    "requester": "radar-ui",
                }),
            )
        }
    };
    let request = Request {
        id: job.request_id.clone(),
        method,
        params,
    };
    let Some(core) = core.upgrade() else {
        return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::DispatchRefused);
    };
    let response = match super::ops::classify(&request.method) {
        Some(super::ops::Method::Operation(operation)) => {
            super::execute(operation, &request, &core, verification)
        }
        _ => return dto::CommandOutcome::new(&job.request_id, dto::CommandStatus::Refused),
    };
    let status = response_status(&response);
    let answer = serde_json::to_value(response).expect("serialize domain response");
    dto::CommandOutcome::answering(&job.request_id, status, answer)
}

/// What a record establishes for the operator: its verdict once settled, and
/// otherwise only that the daemon has taken it.
fn record_status(record: &RequestRecord) -> dto::CommandStatus {
    use dto::CommandStatus as S;
    match record.settled() {
        Some(RequestOutcome::Completed) => S::Completed,
        Some(RequestOutcome::Refused) => S::Refused,
        Some(RequestOutcome::Unknown) => S::Unknown,
        None => S::Admitted,
    }
}

/// What a domain response establishes. A refusal before admission is definite
/// only for the codes that say nothing was recorded; any other error leaves the
/// outcome unestablished.
fn response_status(response: &super::Response) -> dto::CommandStatus {
    use dto::CommandStatus as S;
    match response {
        super::Response::Result { result, .. } => {
            use serde::Deserialize;
            match Answer::deserialize(result) {
                Ok(answer) => record_status(&answer.request),
                Err(_) => S::SubmissionUncertain,
            }
        }
        super::Response::Error { error, .. } => match error.code() {
            Some(Code::BadParams | Code::Refused | Code::Busy | Code::BackendUnavailable) => {
                S::Refused
            }
            _ => S::SubmissionUncertain,
        },
    }
}
