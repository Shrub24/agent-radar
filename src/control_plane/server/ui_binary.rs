//! Binary TUI adapter; domain operations still enter their existing admission path.

use std::future::Future;
use std::pin::Pin;

use crate::content;
use crate::control_plane::transport::FrameStream;
use crate::ui_session::actor::{Subscription, UiHandle};
use crate::ui_session::publication::{Reply, Request};

pub(super) struct BinaryFrames {
    pub handle: UiHandle,
    pub subscription: Option<Subscription>,
    pub session_only: bool,
}

impl FrameStream for BinaryFrames {
    fn accepts_requests(&self) -> bool {
        self.subscription.is_none()
    }

    fn busy(&self) -> Option<content::Segments> {
        content::Segments::encoded(&Reply::Refused {
            code: "busy".into(),
            message: "the control lane is at capacity".into(),
        })
        .ok()
    }

    fn next<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Option<content::Segments>> + Send + 'a>> {
        Box::pin(async move {
            match &mut self.subscription {
                Some(subscription) => {
                    let frame = subscription.next_frame(self.session_only).await?;
                    content::content_frame(&frame.head, &frame.content).ok()
                }
                None => std::future::pending().await,
            }
        })
    }

    fn respond<'a>(
        &'a mut self,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<content::Segments>> + Send + 'a>> {
        Box::pin(async move {
            if self.subscription.is_some() {
                return None;
            }
            let request: Request = content::decode(&bytes).ok()?;
            let handle = self.handle.clone();
            let reply = tokio::task::spawn_blocking(move || answer(&handle, request))
                .await
                .ok()?;
            content::Segments::encoded(&reply).ok()
        })
    }
}

fn answer(handle: &UiHandle, request: Request) -> Reply {
    let result = match request {
        Request::Attach {
            attachment_id,
            role,
            interaction_id,
        } => handle
            .attach_for(&attachment_id, role, interaction_id)
            .map(|_| Reply::Attached),
        Request::Action {
            attachment_id,
            action_id,
            gate,
            action,
        } => {
            if action.is_ordinary() && gate.is_none() {
                return Reply::Refused {
                    code: "bad_params".into(),
                    message: "ordinary UI input requires its displayed gate".into(),
                };
            }
            handle
                .act(&attachment_id, action_id, gate, action)
                .map(|outcome| Reply::Action { outcome })
        }
        Request::Detach { attachment_id } => {
            handle.detach(&attachment_id);
            Ok(Reply::Attached)
        }
        Request::Content { hash } => handle.encoded_content(&hash).map(|content| match content {
            Some(content) => Reply::Content { content },
            None => Reply::Expired {
                head: handle.head(),
            },
        }),
        Request::Demand {
            attachment_id,
            processes,
        } => handle
            .demand(&attachment_id, processes)
            .map(|()| Reply::Demand),
    };
    result.unwrap_or_else(|error| Reply::Refused {
        code: match error {
            crate::ui_session::actor::UiError::Busy => "busy",
            crate::ui_session::actor::UiError::Stopped => "backend_unavailable",
            crate::ui_session::actor::UiError::Refused(_) => "refused",
        }
        .into(),
        message: format!("{error:?}"),
    })
}
