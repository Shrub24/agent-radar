//! Claims: the one durable record of an attachment.
//!
//! A claim is a proposed or established attachment of a registration to a subject.
//! Its standing is a recorded transition on the claim's own lifecycle stream, so
//! ordering, replay and a subject's current, reserved and previous grant all come
//! from one authority. Nothing here keeps a second copy of that authority: a
//! subject record holds its durable identity and metadata, and the claim says
//! which execution is attached to it.

use super::{Registry, validate_registration_id};
use crate::control_plane::events::AttachmentFacts;
pub use crate::control_plane::events::Standing;
use crate::control_plane::lifecycle;
use crate::identity::{ClaimId, RegistrationId, SubjectId};
use serde::{Deserialize, Serialize};

/// One presence report. Identity is the process's own, never a caller's choice.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub source: String,
    pub incarnation: String,
    pub process: crate::model::ProcessIdentity,
    pub session_id: String,
    /// The claim Radar returned for this attachment, echoed back on heartbeats and
    /// reconnects. A first report names none: Radar resolves its own reservation or
    /// mints the claim, so the client never owns an attachment identity.
    #[serde(default)]
    pub claim_id: Option<crate::identity::ClaimId>,
    #[serde(default)]
    pub registration_id: Option<crate::identity::RegistrationId>,
    #[serde(default)]
    pub subject_id: Option<crate::identity::SubjectId>,
    #[serde(default)]
    pub location: Option<super::RegistryLocation>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub registration_id: crate::identity::RegistrationId,
    pub claim_id: crate::identity::ClaimId,
    pub process: crate::model::ProcessIdentity,
}

/// Process identity, not pane/session or a caller-selected ID, resolves registration.
pub fn registration(
    registry: &Registry,
    report: &Report,
    now_ms: i64,
) -> Result<super::Registration, String> {
    let register = || {
        registry.register(
            &super::RegistrationRequest {
                source: report.source.clone(),
                incarnation: report.incarnation.clone(),
                process: Some(report.process.clone()),
                session: None,
                location: None,
                owner: report.owner.clone(),
                run: report.run.clone(),
                label: report.label.clone(),
                launch: None,
                spawn_token: report.registration_id.as_ref().map(ToString::to_string),
            },
            now_ms,
        )
    };
    if let Some(id) = &report.registration_id {
        if let Some(existing) = registry.get(id)? {
            if existing.request.process.as_ref() != Some(&report.process)
                || existing.request.source != report.source
                || existing.request.incarnation != report.incarnation
            {
                return Err("registration belongs to another process".into());
            }
            let _transition = registry.transition();
            super::spawn::heal_binding(registry, &existing.request, now_ms)?;
            return Ok(existing);
        }
        if lifecycle::read_process(registry, id)?
            .is_some_and(|process| process.kind == lifecycle::ProcessKind::Command)
        {
            return Err("a managed command is not an agent and cannot be attached".into());
        }
        if super::spawn::launched_registration(registry, Some(id.as_str()))?
            .is_none_or(|admitted| &admitted != id)
        {
            return Err("registration was not admitted by a managed launch".into());
        }
        // Binding a pre-minted registration uses the same captured-birth fence
        // as registration admission. An unknown ID refuses; never adopt instead.
        return register();
    }
    let mut after: Option<String> = None;
    loop {
        let page = registry.list_after(after.as_deref(), 256)?;
        if let Some(existing) = page.iter().find(|r| {
            r.request.process.as_ref() == Some(&report.process) && r.request.source == report.source
        }) {
            if existing.request.incarnation != report.incarnation {
                return Err("process is already registered with another incarnation".into());
            }
            return Ok(existing.clone());
        }
        if page.len() < 256 {
            break;
        }
        after = page.last().map(|r| r.registration_id.to_string());
    }
    register()
}

/// What one claim says about itself, as a caller reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub registration_id: RegistrationId,
    pub claim_id: ClaimId,
    pub session_id: Option<String>,
    pub standing: Standing,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_id: Option<SubjectId>,
}

impl Decision {
    fn of(claim_id: &ClaimId, facts: &AttachmentFacts) -> Self {
        Self {
            registration_id: facts.registration_id.clone(),
            claim_id: claim_id.clone(),
            session_id: facts.session.clone(),
            standing: facts.standing,
            subject_id: facts.subject_id.clone(),
        }
    }
}

/// The small public receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub registration_id: crate::identity::RegistrationId,
    pub claim_id: crate::identity::ClaimId,
    pub standing: Standing,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_id: Option<crate::identity::SubjectId>,
}

impl From<&Decision> for Answer {
    fn from(claim: &Decision) -> Self {
        Self {
            registration_id: claim.registration_id.clone(),
            claim_id: claim.claim_id.clone(),
            standing: claim.standing,
            subject_id: claim.subject_id.clone(),
        }
    }
}

/// One process's current authority over a subject: the pair a reader needs, and
/// nothing about how it was established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub subject_id: SubjectId,
    pub claim_id: ClaimId,
}

/// This claim, as recorded.
pub fn decision(registry: &Registry, claim_id: &ClaimId) -> Result<Option<Decision>, String> {
    Ok(registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .facts(claim_id)
        .map(|facts| Decision::of(claim_id, &facts)))
}

/// The newest claim one registration made, whatever its standing.
pub fn current(
    registry: &Registry,
    registration_id: &RegistrationId,
) -> Result<Option<Decision>, String> {
    let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    let Some(claim) = log.claim_of_registration(registration_id) else {
        return Ok(None);
    };
    Ok(log.facts(&claim).map(|facts| Decision::of(&claim, &facts)))
}

/// The authority a registration holds now. A pending, reserved, duplicate or
/// released claim is not authority, and a granted one is only authority while it
/// is still its subject's grant.
pub fn attachment(
    registry: &Registry,
    registration_id: &RegistrationId,
) -> Result<Option<Attachment>, String> {
    let Some(claim) = current(registry, registration_id)? else {
        return Ok(None);
    };
    let Some(subject_id) = claim.subject_id else {
        return Ok(None);
    };
    if !claim.standing.grants_authority()
        || lifecycle::granted_claim(registry, &subject_id)?.as_ref() != Some(&claim.claim_id)
    {
        return Ok(None);
    }
    Ok(Some(Attachment {
        claim_id: claim.claim_id,
        subject_id,
    }))
}

/// Starts or reasserts a presence attempt. Callers serialize presence decisions;
/// nothing here takes the transition lock, so a launch reservation that already
/// holds it cannot deadlock.
///
/// A report that names its claim resumes that attachment; one that names none
/// resolves the registration's reservation or discovers its live claim, so a lost
/// reply costs a retried read rather than a second attachment.
pub fn begin(
    registry: &Registry,
    registration_id: &RegistrationId,
    claim_id: Option<&ClaimId>,
    session_id: &str,
    now_ms: i64,
) -> Result<Decision, String> {
    validate_registration_id(session_id)?;
    if let Some(claim_id) = claim_id {
        let existing = decision(registry, claim_id)?.ok_or("claim is not recorded")?;
        if &existing.registration_id != registration_id
            || existing
                .session_id
                .as_deref()
                .is_some_and(|recorded| recorded != session_id)
        {
            return Err("claim ID already identifies a different process or session".into());
        }
        if existing.standing.is_terminal() {
            return Err("a released claim cannot be reopened".into());
        }
        return Ok(existing);
    }
    if let Some(reserved) = lifecycle::reservation_for(registry, registration_id)? {
        lifecycle::claim_presence(registry, &reserved, session_id, now_ms)?;
        return decision(registry, &reserved)?
            .ok_or_else(|| "the reserved claim was not readable".into());
    }
    if let Some(existing) = current(registry, registration_id)?
        && !existing.standing.is_terminal()
    {
        if existing
            .session_id
            .as_deref()
            .is_some_and(|recorded| recorded != session_id)
        {
            return Err(
                "the registration already holds a claim for another session; release it first"
                    .into(),
            );
        }
        if !existing.standing.grants_authority() {
            lifecycle::claim_presence(registry, &existing.claim_id, session_id, now_ms)?;
        }
        return decision(registry, &existing.claim_id)?.ok_or_else(|| "claim disappeared".into());
    }
    let execution = lifecycle::adopted_execution(registry, registration_id, now_ms)?;
    let claim_id = ClaimId::random();
    lifecycle::admit_claim(
        registry,
        &claim_id,
        execution,
        session_id,
        Standing::Pending,
        now_ms,
    )?;
    decision(registry, &claim_id)?.ok_or_else(|| "the presence claim was not recorded".into())
}

/// Records what a caller decided about a claim it already began.
pub fn record(registry: &Registry, decision: &Decision, now_ms: i64) -> Result<(), String> {
    lifecycle::record_claim_standing(registry, decision, now_ms)
}

/// Holds a subject for a launch that has not presented. Not authority: it excludes
/// competing attachments until this registration presents or is withdrawn.
pub fn reserve(
    registry: &Registry,
    registration_id: &RegistrationId,
    claim_id: &ClaimId,
    subject_id: &SubjectId,
    session_id: Option<String>,
    now_ms: i64,
) -> Result<(), String> {
    if let Some(existing) = decision(registry, claim_id)? {
        if &existing.registration_id != registration_id {
            return Err("claim ID already identifies a different process".into());
        }
        lifecycle::reserve_claim(registry, claim_id, subject_id, session_id, now_ms)?;
        return Ok(());
    }
    let execution = lifecycle::read_process(registry, registration_id)?
        .ok_or("a reservation is recorded for a launched execution")?;
    lifecycle::admit_claim(
        registry,
        claim_id,
        execution,
        session_id.as_deref().unwrap_or_default(),
        Standing::Reserved,
        now_ms,
    )
}

/// Ends this claim's authority, keeping it as the record of what it was.
pub fn release(
    registry: &Registry,
    registration_id: &RegistrationId,
    claim_id: &ClaimId,
) -> Result<Decision, String> {
    let existing = decision(registry, claim_id)?.ok_or("claim is not recorded")?;
    if &existing.registration_id != registration_id {
        return Err("claim belongs to another registration".into());
    }
    lifecycle::release_claim(registry, claim_id)?;
    decision(registry, claim_id)?.ok_or_else(|| "claim disappeared while releasing".into())
}
