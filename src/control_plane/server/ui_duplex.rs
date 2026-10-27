//! One duplex attachment: reading, ordered actions, bounded auxiliary work, and
//! one writer that prefers replies without starving publications.

use tokio::io::{AsyncRead, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

use crate::content::{self, Segments};
use crate::control_plane::protocol::{self, Code, Refusal, Response};
use crate::control_plane::public_contracts::ui_session as dto;
use crate::control_plane::transport::{self, DuplexFuture, FRAME_TIMEOUT, Tasks, WRITE_TIMEOUT};
use crate::ui_session::actor::{Subscription, UiError, UiHandle};

/// Replies waiting for the writer.
const REPLIES: usize = 8;
/// Replies written back-to-back before a publication is offered the socket.
const REPLY_BURST: usize = 4;
/// Content and demand requests that may wait behind their answer.
const AUXILIARY: usize = 2;

type Read = Box<dyn AsyncRead + Send + Unpin>;

pub(super) fn open(
    handle: Option<&UiHandle>,
    request: crate::control_plane::protocol::Request,
    read: Read,
    write: OwnedWriteHalf,
) -> DuplexFuture {
    let handle = handle.cloned();
    Box::pin(async move {
        serve(handle, request, read, write).await;
    })
}

async fn serve(
    handle: Option<UiHandle>,
    request: protocol::Request,
    read: Read,
    mut write: OwnedWriteHalf,
) {
    let Some(handle) = handle else {
        return refuse(
            &mut write,
            &request.id,
            Code::BackendUnavailable,
            "this daemon has no UI session",
        )
        .await;
    };
    let Ok(open) = serde_json::from_value::<dto::AttachmentOpen>(request.params.clone()) else {
        return refuse(&mut write, &request.id, Code::BadParams, "bad opener").await;
    };
    let subscription = match handle
        .open_duplex(&open.attachment_id, open.role, open.interaction_id)
        .await
    {
        Ok(subscription) => subscription,
        Err(UiError::Busy) => {
            return refuse(
                &mut write,
                &request.id,
                Code::Busy,
                "the UI session is busy",
            )
            .await;
        }
        Err(error) => {
            return refuse(
                &mut write,
                &request.id,
                Code::Refused,
                &format!("{error:?}"),
            )
            .await;
        }
    };
    // The opener is the last JSON on this connection; everything after it is
    // framed MessagePack in both directions.
    let acknowledged = Response::ok(
        &request.id,
        serde_json::json!({"codec": "messagepack", "framing": "u32-be"}),
    );
    let Ok(line) = serde_json::to_vec(&acknowledged) else {
        return;
    };
    if write.write_all(&line).await.is_err() || write.write_all(b"\n").await.is_err() {
        return;
    }
    let session_only = open.role == dto::AttachmentRole::Presenter;
    duplex(
        handle,
        subscription,
        open.attachment_id,
        session_only,
        read,
        write,
    )
    .await;
}

async fn duplex(
    handle: UiHandle,
    subscription: Subscription,
    attachment: String,
    session_only: bool,
    read: Read,
    write: OwnedWriteHalf,
) {
    let (replies, mut outgoing) = mpsc::channel::<Outgoing>(REPLIES);
    let (actions, mut ordered) = mpsc::channel::<dto::DuplexRequest>(1);
    let (auxiliary, mut others) = mpsc::channel::<dto::DuplexRequest>(AUXILIARY);

    // Every task belongs to this connection and ends with it, including when the
    // serving future is dropped by daemon shutdown.
    let mut children = Tasks::default();
    let sender = replies.clone();
    let action_handle = handle.clone();
    let action_attachment = attachment.clone();
    let action_replies = replies.clone();
    children.spawn(async move {
        let mut reader = BufReader::new(read);
        loop {
            let Ok(bytes) = transport::read_frame_body(&mut reader, FRAME_TIMEOUT).await else {
                return;
            };
            let Ok(request) = content::decode::<dto::DuplexRequest>(&bytes) else {
                return;
            };
            let id = request.id;
            let slot = match request.request {
                dto::DuplexOperation::Action { .. } => &actions,
                _ => &auxiliary,
            };
            if slot.try_send(request).is_err()
                && send(
                    &sender,
                    id,
                    refused(Code::Busy, "a request of that kind is already in flight"),
                    Vec::new(),
                    None,
                )
                .await
                .is_err()
            {
                return;
            }
        }
    });

    children.spawn(async move {
        let mut write = write;
        let mut subscription = subscription;
        write_out(&mut write, &mut subscription, session_only, &mut outgoing).await;
        let _ = write.shutdown().await;
    });

    children.spawn(async move {
        while let Some(request) = ordered.recv().await {
            let dto::DuplexOperation::Action {
                action_id,
                gate,
                action,
            } = request.request
            else {
                continue;
            };
            let (done, written) = oneshot::channel();
            let reply = if action.is_ordinary() && gate.is_none() {
                refused(
                    Code::BadParams,
                    "ordinary UI input requires its displayed gate",
                )
            } else {
                match action_handle.enqueue_action(&action_attachment, action_id, gate, action) {
                    Ok(answer) => match answer.await {
                        Ok(outcome) => dto::DuplexReply::Action { outcome },
                        Err(_) => unavailable(),
                    },
                    Err(error) => refused(failure(&error), "the UI session refused the action"),
                }
            };
            if send(&action_replies, request.id, reply, Vec::new(), Some(done))
                .await
                .is_err()
            {
                return;
            }
            // The one action slot stays taken until its reply is on the wire.
            if written.await.is_err() {
                return;
            }
        }
    });

    let auxiliary_handle = handle.clone();
    let auxiliary_attachment = attachment.clone();
    let auxiliary_replies = replies.clone();
    children.spawn(async move {
        while let Some(request) = others.recv().await {
            let (reply, payload) = match request.request {
                dto::DuplexOperation::Content { hash } => {
                    match auxiliary_handle.enqueue_content(&hash) {
                        Ok(answer) => match answer.await {
                            Ok(Some(content)) => {
                                (dto::DuplexReply::Content { hash }, vec![content])
                            }
                            _ => (
                                dto::DuplexReply::Expired {
                                    head: auxiliary_handle.head(),
                                },
                                Vec::new(),
                            ),
                        },
                        Err(_) => (unavailable(), Vec::new()),
                    }
                }
                dto::DuplexOperation::Demand { processes } => {
                    match auxiliary_handle.enqueue_demand(&auxiliary_attachment, processes) {
                        Ok(answer) => match answer.await {
                            Ok(Ok(())) => (dto::DuplexReply::Demand, Vec::new()),
                            _ => (refused(Code::Refused, "the attachment is gone"), Vec::new()),
                        },
                        Err(_) => (unavailable(), Vec::new()),
                    }
                }
                dto::DuplexOperation::Action { .. } => continue,
            };
            if send(&auxiliary_replies, request.id, reply, payload, None)
                .await
                .is_err()
            {
                return;
            }
        }
    });

    children.wait().await;
    children.shutdown().await;
}

struct Outgoing {
    segments: Segments,
    written: Option<oneshot::Sender<()>>,
}

fn refused(code: Code, message: &str) -> dto::DuplexReply {
    dto::DuplexReply::Refused {
        code,
        message: message.into(),
    }
}

fn unavailable() -> dto::DuplexReply {
    refused(Code::BackendUnavailable, "the UI session stopped")
}

fn failure(error: &UiError) -> Code {
    match error {
        UiError::Busy => Code::Busy,
        _ => Code::BackendUnavailable,
    }
}

async fn send(
    replies: &mpsc::Sender<Outgoing>,
    id: u64,
    reply: dto::DuplexReply,
    payload: Vec<content::Content>,
    written: Option<oneshot::Sender<()>>,
) -> Result<(), ()> {
    let metadata = dto::DuplexMetadata::Reply { id, reply };
    let segments = content::content_frame(&metadata, &payload).map_err(|_| ())?;
    replies
        .send(Outgoing { segments, written })
        .await
        .map_err(|_| ())
}

/// Replies take turns with the newest publication: a burst of them, then the
/// publication the subscription currently holds, and never a second frame while
/// one is being written.
async fn write_out(
    write: &mut OwnedWriteHalf,
    subscription: &mut Subscription,
    session_only: bool,
    outgoing: &mut mpsc::Receiver<Outgoing>,
) {
    loop {
        let mut burst = 0;
        while burst < REPLY_BURST {
            let Ok(next) = outgoing.try_recv() else { break };
            if !transport::write_frame(write, &next.segments).await {
                return;
            }
            if let Some(written) = next.written {
                let _ = written.send(());
            }
            burst += 1;
        }
        tokio::select! {
            biased;
            publication = subscription.prepare_frame(session_only) => {
                let Some(frame) = publication else { return };
                let metadata = dto::DuplexMetadata::Publication {
                    head: frame.head.clone(),
                };
                let Ok(segments) = content::content_frame(&metadata, &frame.content) else {
                    return;
                };
                if !transport::write_frame(write, &segments).await {
                    return;
                }
                // Only a fully written head may become the client's known state.
                subscription.sent_frame(&frame.head);
            }
            reply = outgoing.recv() => {
                let Some(next) = reply else { return };
                if !transport::write_frame(write, &next.segments).await {
                    return;
                }
                if let Some(written) = next.written {
                    let _ = written.send(());
                }
            }
        }
    }
}

async fn refuse(write: &mut OwnedWriteHalf, id: &str, code: Code, message: &str) {
    let response = Response::refused(id, &Refusal::new(code, message));
    let Ok(mut line) = serde_json::to_vec(&response) else {
        return;
    };
    line.push(b'\n');
    let write = async {
        write.write_all(&line).await?;
        write.flush().await
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, write).await;
}
