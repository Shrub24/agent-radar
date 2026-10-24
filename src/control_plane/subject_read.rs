//! Subject authority projected for consumers; process joins remain internal.

use super::lifecycle::{self, SubjectRecord};
use super::public_contracts::subject::{AttachmentState, ChannelFacts, Subject};
use super::registry::{Channel, ProcessVerification, Registry, presence};
use crate::model::ProcessIdentity;

pub(crate) fn read(
    registry: &Registry,
    record: SubjectRecord,
    now_ms: i64,
    verify: impl FnOnce(Option<ProcessIdentity>) -> ProcessVerification,
) -> Result<Subject, String> {
    let claim = record
        .current_claim
        .as_deref()
        .or(record.last_claim.as_deref())
        .map(|id| lifecycle::read(registry, id))
        .transpose()?
        .flatten();
    let mut result = Subject {
        subject_id: record.subject_id.clone(),
        session_id: record.session,
        managed: record.managed,
        state: AttachmentState::Detached,
        location: None,
        scoped: None,
        execution: None,
        assignment: crate::control_plane::registry::publication::assignment_facts(
            registry,
            &record.subject_id,
            now_ms,
        )?
        .map(|facts| ChannelFacts {
            writer: facts.writer,
            snapshot: facts.snapshot,
        }),
        facts: None,
    };
    let Some(claim) = claim else {
        return Ok(result);
    };
    let current = record.current_claim.as_deref() == Some(claim.claim_id.as_str());
    let process = &claim.process;
    result.scoped = Some(process.scope.is_some());
    let standing = presence::decision(registry, &claim.claim_id)?;
    result.state =
        if !current || process.exited_at.is_some() || process.ended_externally_at.is_some() {
            AttachmentState::Detached
        } else {
            match standing.as_ref().map(|decision| &decision.standing) {
                Some(presence::Standing::Reserved) => AttachmentState::Starting,
                Some(presence::Standing::Granted) => match verify(process.identity.clone()) {
                    ProcessVerification::Verified => AttachmentState::Live,
                    ProcessVerification::Absent | ProcessVerification::Mismatched => {
                        AttachmentState::Detached
                    }
                    ProcessVerification::Unavailable => AttachmentState::Unknown,
                },
                _ => AttachmentState::Unknown,
            }
        };
    if current {
        let registration = registry.get(&process.registration_id)?;
        if result.state == AttachmentState::Live {
            result.location = standing.and_then(|decision| decision.location).or_else(|| {
                registration
                    .as_ref()
                    .and_then(|r| r.request.location.clone())
            });
        }
        let channel = |kind| -> Result<Option<ChannelFacts>, String> {
            Ok(registry
                .published_facts(&process.registration_id, kind, now_ms)?
                .map(|facts| ChannelFacts {
                    writer: facts.writer,
                    snapshot: facts.snapshot,
                }))
        };
        result.execution = channel(Channel::Execution)?;
        result.facts = channel(Channel::Facts)?;
    }
    // Verification is external; a concurrent attachment change invalidates this
    // sample rather than letting it describe the replacement with old evidence.
    if lifecycle::read_subject_record(registry, &record.subject_id)?
        .is_none_or(|latest| latest.current_claim != record.current_claim)
    {
        result.state = AttachmentState::Unknown;
        result.location = None;
        result.execution = None;
        result.assignment = None;
        result.facts = None;
    }
    Ok(result)
}
