use super::*;

/// Serves the daemon's shared observation snapshot.
///
/// A read records nothing: it is a projection of what the sampler has already
/// published, and publishing a revision appends nothing to the durable journal.
/// The revision is epoch-qualified, because a restarted daemon counting from one
/// again must not look like the revision a caller already holds.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HandoffInboxParams {
    pub(super) registration_id: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HandoffReadParams {
    pub(super) registration_id: String,
    pub(super) handoff_id: String,
}

pub(super) fn communication_request(core: &Arc<Mutex<Core>>, request: &Request) -> Response {
    let (registry, communications, store) = {
        let core = locked(core);
        (
            Arc::clone(&core.registry),
            Arc::clone(&core.communications),
            core.store.clone(),
        )
    };
    match request.method.as_str() {
        "communication.publish" => {
            let params: crate::control_plane::public_contracts::HandoffPublishParams =
                match decode_params(request) {
                    Ok(value) => value,
                    Err(response) => return response,
                };
            let sender = match communication_identity(&registry, &params.sender_registration_id) {
                Ok(identity) => identity,
                Err(error) => return communication_error(request, error),
            };
            if let Err(error) =
                validate_handoff_recipient(&registry, &sender, &params.recipient_subject_id)
            {
                return communication_error(request, error);
            }
            let handoff = crate::control_plane::public_contracts::Handoff {
                version: communication::HANDOFF_VERSION,
                id: request.id.clone(),
                sender,
                recipient_subject_id: params.recipient_subject_id.clone(),
                correlation_id: params.correlation_id,
                summary: params.summary,
                content: params.content,
                published_at_unix_ms: unix_millis().max(0) as u64,
            };
            match communications.publish(&handoff) {
                Ok((handoff, created)) => {
                    if created {
                        let subject_id = Some(handoff.recipient_subject_id.clone());
                        append_communication_event(
                            &store,
                            "communication.handoff_published",
                            &handoff.id,
                            subject_id,
                            &handoff.sender.registration_id,
                        );
                    }
                    Response::ok(
                        &request.id,
                        serde_json::json!({"handoff": handoff, "created": created}),
                    )
                }
                Err(error) => communication_error(request, error),
            }
        }
        "communication.inbox" => {
            let params: HandoffInboxParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let identity = match communication_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return communication_error(request, error),
            };
            match communications.pending(&identity) {
                Ok(handoffs) => {
                    let inbox = crate::control_plane::public_contracts::HandoffInbox {
                        handoff_ids: handoffs.into_iter().map(|handoff| handoff.id).collect(),
                    };
                    Response::ok(
                        &request.id,
                        serde_json::to_value(inbox)
                            .expect("handoff inbox DTO is JSON serializable"),
                    )
                }
                Err(error) => communication_error(request, error),
            }
        }
        "communication.get" => {
            let params: HandoffReadParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let identity = match communication_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return communication_error(request, error),
            };
            match communications.get(&params.handoff_id, &identity) {
                Ok((handoff, receipt)) => Response::ok(
                    &request.id,
                    serde_json::json!({"handoff": handoff, "receipt": receipt}),
                ),
                Err(error) => communication_error(request, error),
            }
        }
        "communication.ack" => {
            let params: HandoffReadParams = match decode_params(request) {
                Ok(value) => value,
                Err(response) => return response,
            };
            let identity = match communication_identity(&registry, &params.registration_id) {
                Ok(identity) => identity,
                Err(error) => return communication_error(request, error),
            };
            let now_ms = unix_millis().max(0) as u64;
            match communications.acknowledge(&params.handoff_id, &identity, now_ms) {
                Ok((receipt, created)) => {
                    if created {
                        append_communication_event(
                            &store,
                            "communication.handoff_acknowledged",
                            &receipt.handoff_id,
                            Some(identity.subject_id.clone()),
                            &identity.registration_id,
                        );
                    }
                    Response::ok(
                        &request.id,
                        serde_json::json!({"receipt": receipt, "created": created}),
                    )
                }
                Err(error) => communication_error(request, error),
            }
        }
        _ => Response::refused(
            &request.id,
            &Refusal::new(Code::UnknownMethod, "unknown communication method"),
        ),
    }
}

pub(super) fn communication_identity(
    registry: &Registry,
    registration_id: &str,
) -> Result<crate::control_plane::public_contracts::HandoffIdentity, communication::Error> {
    if !valid_identifier(registration_id) {
        return Err(communication::Error::Invalid(
            "registration_id is invalid".into(),
        ));
    }
    let context = registry
        .context(registration_id, unix_millis())
        .map_err(communication::Error::Internal)?
        .ok_or(communication::Error::NotFound)?;
    if context.context.freshness != crate::control_plane::registry::publication::Freshness::Fresh {
        return Err(communication::Error::Invalid(
            "sender identity context is not fresh".into(),
        ));
    }
    let session_id = context
        .context
        .session
        .ok_or_else(|| communication::Error::Invalid("sender has no current Pi session".into()))?;
    let attachment = registry::presence::attachment(registry, registration_id)
        .map_err(communication::Error::Internal)?
        .ok_or_else(|| {
            communication::Error::Invalid(
                "handoff operations require a current granted claim".into(),
            )
        })?;
    let subject = subjects::read(registry, &attachment.claim_id)
        .map_err(communication::Error::Internal)?
        .filter(|claim| claim.session.as_deref() == Some(session_id.as_str()))
        .ok_or_else(|| {
            communication::Error::Invalid(
                "the granted claim does not match the current session".into(),
            )
        })?;
    if subject.identity_unresolved {
        return Err(communication::Error::Invalid(
            "Radar launch identity is unresolved".into(),
        ));
    }
    Ok(crate::control_plane::public_contracts::HandoffIdentity {
        registration_id: registration_id.to_owned(),
        session_id,
        subject_id: subject
            .subject_id
            .clone()
            .unwrap_or_else(|| subject.claim_id.clone()),
        claim_id: subject.claim_id,
    })
}

pub(super) fn validate_handoff_recipient(
    registry: &Registry,
    sender: &crate::control_plane::public_contracts::HandoffIdentity,
    recipient_subject_id: &str,
) -> Result<(), communication::Error> {
    let sender_subject_id = sender.subject_id.as_str();
    let sender_subject = subjects::read_logical(registry, sender_subject_id)
        .map_err(communication::Error::Internal)?
        .ok_or(communication::Error::NotFound)?;
    if sender_subject.identity_unresolved {
        return Err(communication::Error::Invalid(
            "sender logical identity is unresolved".into(),
        ));
    }
    let recipient_subject = subjects::read_logical(registry, recipient_subject_id)
        .map_err(communication::Error::Internal)?
        .ok_or(communication::Error::NotFound)?;
    if recipient_subject.identity_unresolved {
        return Err(communication::Error::Invalid(
            "recipient logical identity is unresolved".into(),
        ));
    }
    if communication::is_parent_recipient(
        sender_subject.managed.parent_subject_id.as_deref(),
        recipient_subject_id,
    ) {
        Ok(())
    } else {
        Err(communication::Error::WrongRecipient)
    }
}

pub(super) fn append_communication_event(
    store: &Store,
    kind: &str,
    handoff_id: &str,
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
        request_id: Some(handoff_id.to_owned()),
        at: store::now(),
    };
    if let Err(error) = store.append_event(event) {
        tracing::error!(%error, %handoff_id, "communication pointer event append failed");
    }
}

pub(super) fn communication_error(request: &Request, error: communication::Error) -> Response {
    let (code, message) = match error {
        communication::Error::Invalid(message) => (Code::BadParams, message),
        communication::Error::Conflict => (
            Code::Refused,
            "handoff id was already used for different content".into(),
        ),
        communication::Error::NotFound => {
            (Code::NotFound, "handoff or identity was not found".into())
        }
        communication::Error::WrongRecipient => (
            Code::Refused,
            "current identity is not the addressed recipient".into(),
        ),
        communication::Error::Capacity => (
            Code::Busy,
            "communication store is at its configured bound".into(),
        ),
        communication::Error::Internal(message) => (Code::Internal, message),
    };
    Response::Error {
        id: request.id.clone(),
        error: protocol::ErrorBody {
            code: code.as_str().to_owned(),
            message,
        },
    }
}
