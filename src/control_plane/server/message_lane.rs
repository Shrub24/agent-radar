use super::*;
use crate::control_plane::answer as answers;
use crate::control_plane::messages::{self, MessageStore};
use crate::control_plane::public_contracts::{
    AgentIdentity, Attention, Message, MessagePublishParams, PublishBody,
};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageInboxParams {
    pub(super) registration_id: String,
    #[serde(default)]
    pub(super) after: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageReadParams {
    pub(super) registration_id: String,
    pub(super) message_id: String,
}

/// The addressed message lane's JSON surface. Publishing, paging, retrieval and
/// receipt all derive the caller's subject from its verified execution; the
/// registration id is a lookup input, never an address.
pub(super) fn message_request(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let (registry, messages, store, changed) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.messages),
            core.store.clone(),
            core.messages_changed.clone(),
        )
    };
    match request.method.as_str() {
        "communication.publish" => {
            let params: MessagePublishParams = match decode_params(request) {
                Ok(value) => value,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            let producer = match message_identity(&registry, &params.sender_registration_id) {
                Ok(identity) => identity,
                Err(error) => return message_error(request, error),
            };
            let body = PublishBody::Handoff {
                correlation_id: params.correlation_id,
                summary: params.summary,
                content: params.content,
            };
            if let Err(error) = validate_producer_recipients(
                &registry,
                &producer,
                &body,
                std::slice::from_ref(&params.recipient_subject_id),
            ) {
                return message_error(request, error);
            }
            let message = Message {
                version: messages::MESSAGE_VERSION,
                id: request.id.clone(),
                producer,
                recipients: vec![params.recipient_subject_id],
                body,
                created_at_unix_ms: unix_millis().max(0) as u64,
            };
            match messages.publish(&message) {
                Ok((message, created)) => {
                    if created {
                        // Only acceptance and receipt change a mailbox, so only
                        // they wake a subscriber.
                        let _ = changed.send(());
                        append_message_event(
                            &store,
                            "communication.message_published",
                            &message.id,
                            message.recipients.first().cloned(),
                            &message.producer.registration_id,
                        );
                    }
                    respond(
                        &request.id,
                        Ok(answers::PublishedMessage { message, created }),
                    )
                }
                Err(error) => message_error(request, error),
            }
        }
        "communication.inbox" => {
            let params: MessageInboxParams = match decode_params(request) {
                Ok(value) => value,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            let identity = match message_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return message_error(request, error),
            };
            let limit = params.limit.unwrap_or(messages::MAX_PAGE);
            match messages.page(&identity.subject_id, params.after.as_deref(), limit) {
                Ok((page, next)) => {
                    let page = crate::control_plane::public_contracts::MessagePage {
                        message_ids: page.into_iter().map(|message| message.id).collect(),
                        next,
                    };
                    respond(&request.id, Ok(page))
                }
                Err(error) => message_error(request, error),
            }
        }
        "communication.get" => {
            let params: MessageReadParams = match decode_params(request) {
                Ok(value) => value,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            let identity = match message_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return message_error(request, error),
            };
            match messages.get(&params.message_id, &identity.subject_id) {
                Ok((message, receipt)) => {
                    respond(&request.id, Ok(answers::ReadMessage { message, receipt }))
                }
                Err(error) => message_error(request, error),
            }
        }
        "communication.ack" => {
            let params: MessageReadParams = match decode_params(request) {
                Ok(value) => value,
                Err(refusal) => return Response::refused(&request.id, &refusal),
            };
            let identity = match message_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return message_error(request, error),
            };
            let now_ms = unix_millis().max(0) as u64;
            match messages.receive(&params.message_id, &identity, now_ms) {
                Ok((receipt, created)) => {
                    if created {
                        let _ = changed.send(());
                        append_message_event(
                            &store,
                            "communication.message_received",
                            &receipt.message_id,
                            Some(identity.subject_id.clone()),
                            &identity.registration_id,
                        );
                    }
                    respond(
                        &request.id,
                        Ok(answers::AcknowledgedMessage { receipt, created }),
                    )
                }
                Err(error) => message_error(request, error),
            }
        }
        _ => Response::refused(
            &request.id,
            &Refusal::new(Code::UnknownMethod, "unknown communication method"),
        ),
    }
}

/// What a heartbeat reply can say about one registration's mailbox.
pub(super) enum AttentionHint {
    Waiting(Attention),
    /// The caller has no managed subject, so it has no mailbox.
    None,
    /// It has one, but the mailbox could not be read right now.
    Unavailable,
}

pub(super) fn attention_for(
    registry: &Registry,
    messages: &MessageStore,
    registration_id: &str,
) -> AttentionHint {
    let identity = match message_identity(registry, registration_id) {
        Ok(identity) => identity,
        Err(
            messages::Error::NotFound
            | messages::Error::Invalid(_)
            | messages::Error::WrongRecipient,
        ) => return AttentionHint::None,
        Err(_) => return AttentionHint::Unavailable,
    };
    match messages.attention(&identity.subject_id) {
        Ok(attention) => AttentionHint::Waiting(attention),
        Err(_) => AttentionHint::Unavailable,
    }
}

pub(super) fn message_identity(
    registry: &Registry,
    registration_id: &str,
) -> Result<AgentIdentity, messages::Error> {
    if !valid_identifier(registration_id) {
        return Err(messages::Error::Invalid(
            "registration_id is invalid".into(),
        ));
    }
    let context = registry
        .context(registration_id, unix_millis())
        .map_err(messages::Error::Internal)?
        .ok_or(messages::Error::NotFound)?;
    if context.context.freshness != crate::control_plane::registry::publication::Freshness::Fresh {
        return Err(messages::Error::Invalid(
            "sender identity context is not fresh".into(),
        ));
    }
    let session_id = context
        .context
        .session
        .ok_or_else(|| messages::Error::Invalid("sender has no current Pi session".into()))?;
    let attachment = registry::presence::attachment(registry, registration_id)
        .map_err(messages::Error::Internal)?
        .ok_or_else(|| {
            messages::Error::Invalid("message operations require a current granted claim".into())
        })?;
    let subject = subjects::read(registry, &attachment.claim_id)
        .map_err(messages::Error::Internal)?
        .filter(|claim| claim.session.as_deref() == Some(session_id.as_str()))
        .ok_or_else(|| {
            messages::Error::Invalid("the granted claim does not match the current session".into())
        })?;
    Ok(AgentIdentity {
        registration_id: registration_id.to_owned(),
        session_id,
        subject_id: subject.subject_id.clone(),
        claim_id: subject.claim_id,
    })
}

/// A body kind decides who may be addressed. Handoff stays parent-only: the
/// envelope's recipient set is not a general send permission.
pub(super) fn validate_producer_recipients(
    registry: &Registry,
    producer: &AgentIdentity,
    body: &PublishBody,
    recipients: &[String],
) -> Result<(), messages::Error> {
    let PublishBody::Handoff { .. } = body;
    let [recipient] = recipients else {
        return Err(messages::Error::Invalid(
            "a handoff names exactly one recipient".into(),
        ));
    };
    let parent = subjects::current_attachment(registry, &producer.subject_id)
        .map_err(messages::Error::Internal)?
        .ok_or(messages::Error::NotFound)?
        .managed
        .parent_subject_id;
    if !messages::is_parent_recipient(parent.as_deref(), recipient) {
        return Err(messages::Error::WrongRecipient);
    }
    subjects::current_attachment(registry, recipient)
        .map_err(messages::Error::Internal)?
        .ok_or(messages::Error::NotFound)?;
    Ok(())
}

pub(super) fn append_message_event(
    store: &Store,
    kind: &str,
    message_id: &str,
    subject_id: Option<String>,
    registration_id: &str,
) {
    let event = store::Event {
        subject_id,
        claim_id: None,
        lifecycle_event_id: None,
        seq: 0,
        kind: kind.to_owned(),
        registration_id: Some(registration_id.to_owned()),
        spawn: None,
        request_id: Some(message_id.to_owned()),
        at: store::now(),
    };
    if let Err(error) = store.append_event(event) {
        tracing::error!(%error, %message_id, "communication event append failed");
    }
}

pub(super) fn message_error(request: &Request, error: messages::Error) -> Response {
    let (code, message) = match error {
        messages::Error::Invalid(message) => (Code::BadParams, message),
        messages::Error::Conflict => (
            Code::Refused,
            "message id was already used for different content".into(),
        ),
        messages::Error::NotFound => (Code::NotFound, "message or identity was not found".into()),
        messages::Error::WrongRecipient => (
            Code::Refused,
            "current identity is not an addressed recipient".into(),
        ),
        messages::Error::Capacity => (
            Code::Busy,
            "communication store is at its configured bound".into(),
        ),
        messages::Error::Internal(message) => (Code::Internal, message),
    };
    Response::refused(&request.id, &Refusal::new(code, message))
}
