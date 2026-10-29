//! One attention subscription: a recipient's mailbox state, pushed while the
//! connection holds a current grant. No message body travels here, so a slow
//! reader costs one connection, and a missed update is recovered from the next
//! baseline or heartbeat.

use std::sync::Arc;
use std::sync::Mutex;

use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::broadcast;

use super::*;
use crate::content;
use crate::control_plane::messages::MessageStore;
use crate::control_plane::protocol::{Code, Refusal, Response};
use crate::control_plane::public_contracts::Attention;
use crate::control_plane::transport::{DuplexFuture, WRITE_TIMEOUT};

const MAX_ATTENTION_FRAME: usize = 4 * 1024;
/// How often a quiet subscription re-checks that its grant still stands. Nothing
/// else about a quiet subscriber expires.
const REVALIDATE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchOpen {
    registration_id: String,
}

pub(super) fn open(
    core: &Arc<Mutex<Core>>,
    request: crate::control_plane::protocol::Request,
    write: OwnedWriteHalf,
) -> DuplexFuture {
    let Ok(open) = serde_json::from_value::<WatchOpen>(request.params.clone()) else {
        return Box::pin(async move {
            let mut write = write;
            refuse(
                &mut write,
                &request.id,
                Code::BadParams,
                "the attention opener names a registration",
            )
            .await;
        });
    };
    let (registry, messages, changed) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.messages),
            core.messages_changed.subscribe(),
        )
    };
    Box::pin(async move {
        serve(registry, messages, changed, open, request.id, write).await;
    })
}

async fn serve(
    registry: Arc<Registry>,
    messages: Arc<MessageStore>,
    mut changed: broadcast::Receiver<()>,
    open: WatchOpen,
    id: String,
    mut write: OwnedWriteHalf,
) {
    // The baseline is established before anything else can arrive: the watch is
    // already subscribed, and the first read reflects the mailbox as it is now.
    let mut last = match attention(registry.clone(), messages.clone(), &open).await {
        Hint::Waiting(attention) => attention,
        Hint::None => {
            return refuse(
                &mut write,
                &id,
                Code::Refused,
                "this registration has no managed recipient subject",
            )
            .await;
        }
        Hint::Unavailable => {
            return refuse(
                &mut write,
                &id,
                Code::Internal,
                "the recipient mailbox could not be read",
            )
            .await;
        }
    };
    let acknowledgement = Response::ok(
        &id,
        serde_json::json!({"codec": "messagepack", "framing": "u32-be"}),
    );
    let Ok(mut line) = serde_json::to_vec(&acknowledgement) else {
        return;
    };
    line.push(b'\n');
    if write.write_all(&line).await.is_err() {
        return;
    }
    if !push(&mut write, &last).await {
        return;
    }
    let mut revalidate = tokio::time::interval(REVALIDATE);
    revalidate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            // A lagged wake is only a missed hint; the read below is authoritative.
            received = changed.recv() => if matches!(
                received,
                Err(broadcast::error::RecvError::Closed)
            ) {
                return;
            },
            _ = revalidate.tick() => {}
        }
        match attention(registry.clone(), messages.clone(), &open).await {
            Hint::Waiting(attention) if attention.revision != last.revision => {
                last = attention;
                if !push(&mut write, &last).await {
                    return;
                }
            }
            Hint::Waiting(_) => {}
            // The grant is gone, so no further recipient information is exposed.
            Hint::None | Hint::Unavailable => return,
        }
    }
}

enum Hint {
    Waiting(Attention),
    None,
    Unavailable,
}

/// Resolving the registration reads registry records, so it runs off the
/// transport's worker.
async fn attention(registry: Arc<Registry>, messages: Arc<MessageStore>, open: &WatchOpen) -> Hint {
    let registration = open.registration_id.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let registry = registry;
        match super::message_lane::message_identity(&registry, &registration) {
            Ok(identity) => Ok(Some(messages.attention(&identity.subject_id))),
            Err(crate::control_plane::messages::Error::NotFound)
            | Err(crate::control_plane::messages::Error::Invalid(_))
            | Err(crate::control_plane::messages::Error::WrongRecipient) => Ok(None),
            Err(_) => Err(()),
        }
    })
    .await;
    match resolved {
        Ok(Ok(Some(attention))) => Hint::Waiting(attention),
        Ok(Ok(None)) => Hint::None,
        _ => Hint::Unavailable,
    }
}

async fn push(write: &mut OwnedWriteHalf, attention: &Attention) -> bool {
    let Ok(frame) = content::frame(attention) else {
        return false;
    };
    if frame.len() > MAX_ATTENTION_FRAME + 4 {
        return false;
    }
    let written = async {
        write.write_all(&frame).await?;
        write.flush().await
    };
    tokio::time::timeout(WRITE_TIMEOUT, written)
        .await
        .map(|result| result.is_ok())
        .unwrap_or(false)
}

async fn refuse(write: &mut OwnedWriteHalf, id: &str, code: Code, message: &str) {
    let response = Response::refused(id, &Refusal::new(code, message));
    let Ok(mut line) = serde_json::to_vec(&response) else {
        return;
    };
    line.push(b'\n');
    let written = async {
        write.write_all(&line).await?;
        write.flush().await
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, written).await;
}
