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
            match handle.attach(&params.attachment_id, params.role) {
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
            match handle.act(&params.attachment_id, params.action_id, params.action) {
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
