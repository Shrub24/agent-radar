//! The daemon side of the shared UI session: request handlers and the push
//! stream. The session itself lives in [`crate::ui_session`]; this adapts it to
//! the wire and owns nothing of its own.

use std::future::Future;
use std::pin::Pin;

use serde_json::json;

use super::{Request, Response, decode_params};
use crate::control_plane::protocol::{self, Code, Refusal};
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::transport::FrameStream;
use crate::ui_session::actor::{Subscription, UiError, UiHandle};

fn refusal(request: &Request, error: UiError) -> Response {
    let (code, message) = match error {
        UiError::Busy => (Code::Busy, "the UI session is busy".to_string()),
        UiError::Stopped => (Code::Busy, "the UI session has stopped".to_string()),
        UiError::Refused(reason) => (
            Code::Refused,
            serde_json::to_value(reason)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
        ),
    };
    Response::refused(&request.id, &Refusal::new(code, message))
}

/// Serves `ui.attach`, `ui.detach`, `ui.action` and `ui.content.get`.
pub(super) fn request(handle: Option<&UiHandle>, request: &Request) -> Response {
    let Some(handle) = handle else {
        return Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "this daemon has no UI session"),
        );
    };
    match request.method.as_str() {
        "ui.attach" => {
            let params: dto::AttachParams = match decode_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            match handle.attach_for(&params.attachment_id, params.role, params.interaction_id) {
                Ok(result) => Response::ok(&request.id, json!(result)),
                Err(error) => refusal(request, error),
            }
        }
        "ui.detach" => {
            let params: dto::DetachParams = match decode_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            handle.detach(&params.attachment_id);
            Response::ok(&request.id, json!(dto::DetachResult { released: true }))
        }
        "ui.action" => {
            let params: dto::ActionParams = match decode_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            if params.action.is_ordinary() && params.gate.is_none() {
                return Response::refused(
                    &request.id,
                    &Refusal::new(
                        Code::BadParams,
                        "ordinary UI input requires its displayed gate generation",
                    ),
                );
            }
            match handle.act(
                &params.attachment_id,
                params.action_id,
                params.gate,
                params.action,
            ) {
                Ok(outcome) => Response::ok(&request.id, json!(outcome)),
                Err(error) => refusal(request, error),
            }
        }
        "ui.content.get" => {
            let params: dto::ContentGetParams = match decode_params(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            match handle.content(&params.hash) {
                Ok(Some(rows)) => Response::ok(
                    &request.id,
                    json!(dto::ContentGetResult {
                        rows: (*rows).clone()
                    }),
                ),
                Ok(None) => Response::refused(
                    &request.id,
                    &Refusal::new(Code::NotFound, "content_expired"),
                ),
                Err(error) => refusal(request, error),
            }
        }
        other => Response::refused(
            &request.id,
            &Refusal::new(Code::UnknownMethod, format!("unknown method `{other}`")),
        ),
    }
}

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
                    let answer = json!({"result": { "request": super::public_record(record) }});
                    let status = outcome_status(&answer);
                    dto::CommandOutcome {
                        request_id: id,
                        status,
                        answer: Some(answer),
                    }
                }
                None => dto::CommandOutcome {
                    request_id: id,
                    status: dto::CommandStatus::SubmissionUncertain,
                    answer: None,
                },
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
            return dto::CommandOutcome {
                request_id: job.request_id,
                status: dto::CommandStatus::DispatchRefused,
                answer: None,
            };
        };
        let Some(backend) = super::locked(&core).backend.clone() else {
            return dto::CommandOutcome {
                request_id: job.request_id,
                status: dto::CommandStatus::Refused,
                answer: Some(json!({"error": {"code": "backend_unavailable",
                    "message": "this daemon has no runtime to move focus on"}})),
            };
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let (status, answer) = match backend.focus_outcome(&target, &cancel) {
            crate::runtime::FocusOutcome::Completed => (dto::CommandStatus::Completed, None),
            crate::runtime::FocusOutcome::Refused(message) => {
                // A server with nothing attached is answered with the place that
                // could not be shown: the frontend can open a terminal for it.
                let code = if message == crate::runtime::NO_CLIENT {
                    "no_client"
                } else {
                    "refused"
                };
                (
                    dto::CommandStatus::Refused,
                    Some(json!({"error": {"code": code, "message": message,
                        "target": target}})),
                )
            }
            crate::runtime::FocusOutcome::Unknown(message) => (
                dto::CommandStatus::Unknown,
                Some(json!({"error": {"code": "unknown", "message": message}})),
            ),
        };
        return dto::CommandOutcome {
            request_id: job.request_id,
            status,
            answer,
        };
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
                return dto::CommandOutcome {
                    request_id: job.request_id,
                    status: dto::CommandStatus::DispatchRefused,
                    answer: None,
                };
            }
        },
        None => {
            let dto::InteractionResult::Project { path } = &job.result else {
                return dto::CommandOutcome {
                    request_id: job.request_id,
                    status: dto::CommandStatus::Refused,
                    answer: None,
                };
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
        return dto::CommandOutcome {
            request_id: job.request_id,
            status: dto::CommandStatus::DispatchRefused,
            answer: None,
        };
    };
    let response = match super::ops::classify(&request.method) {
        Some(super::ops::Method::Operation(operation)) => {
            super::execute(operation, &request, &core, verification)
        }
        _ => {
            return dto::CommandOutcome {
                request_id: job.request_id,
                status: dto::CommandStatus::Refused,
                answer: None,
            };
        }
    };
    let answer = serde_json::to_value(response).expect("serialize domain response");
    let status = outcome_status(&answer);
    dto::CommandOutcome {
        request_id: job.request_id,
        status,
        answer: Some(answer),
    }
}

fn outcome_status(answer: &serde_json::Value) -> dto::CommandStatus {
    use dto::CommandStatus as S;
    match answer
        .pointer("/result/request/outcome")
        .and_then(serde_json::Value::as_str)
    {
        Some("completed") => S::Completed,
        Some("refused") => S::Refused,
        Some("unknown") => S::Unknown,
        _ if answer.pointer("/result/request").is_some() => S::Admitted,
        _ => match answer
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
        {
            Some("bad_params" | "refused" | "busy" | "backend_unavailable") => S::Refused,
            _ => S::SubmissionUncertain,
        },
    }
}

struct Frames {
    id: String,
    subscription: Subscription,
}

impl FrameStream for Frames {
    fn next<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let publication = self.subscription.next().await?;
            serde_json::to_vec(&Response::ok(&self.id, json!(publication))).ok()
        })
    }
}

/// Opens the push stream `ui.subscribe` asks for.
pub(super) fn open(
    handle: Option<&UiHandle>,
    line: &[u8],
) -> Option<Result<Box<dyn FrameStream>, Response>> {
    let request = protocol::decode_request(line).ok()?;
    if request.method != "ui.subscribe" {
        return None;
    }
    let Some(handle) = handle else {
        return Some(Err(Response::refused(
            &request.id,
            &Refusal::new(Code::BackendUnavailable, "this daemon has no UI session"),
        )));
    };
    let params: dto::SubscribeParams = match decode_params(&request) {
        Ok(params) => params,
        Err(response) => return Some(Err(response)),
    };
    Some(
        match handle.subscribe(&params.attachment_id, params.known_tree_hash) {
            Ok(subscription) => Ok(Box::new(Frames {
                id: request.id,
                subscription,
            })),
            Err(error) => Err(refusal(&request, error)),
        },
    )
}
