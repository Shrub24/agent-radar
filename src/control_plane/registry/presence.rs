//! Durable session-load decisions. Presence retries never create claim opportunities.

use super::publication::write_atomic;
use super::{Registry, RegistryLocation, read_record_file, validate_registration_id};
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub source: String,
    pub incarnation: String,
    pub process: crate::model::ProcessIdentity,
    pub session_id: String,
    pub claim_id: String,
    #[serde(default)]
    pub registration_id: Option<String>,
    #[serde(default)]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub location: Option<RegistryLocation>,
    #[serde(default)]
    pub spawn_token: Option<String>,
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
    pub registration_id: String,
    pub claim_id: String,
    pub process: crate::model::ProcessIdentity,
}

/// Process identity, not pane/session or a caller-selected ID, resolves registration.
pub fn registration(
    registry: &Registry,
    report: &Report,
    now_ms: i64,
) -> Result<super::Registration, String> {
    if let Some(id) = &report.registration_id {
        if let Some(existing) = registry.get(id)? {
            if existing.request.process.as_ref() != Some(&report.process)
                || existing.request.source != report.source
                || existing.request.incarnation != report.incarnation
            {
                return Err("registration belongs to another process".into());
            }
            return Ok(existing);
        }
        return Err(
            "acknowledged registration is missing; authoritative recovery is unresolved".into(),
        );
    }
    let mut after = None;
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
        after = page.last().map(|r| r.registration_id.clone());
    }
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
            spawn_token: report.spawn_token.clone(),
        },
        now_ms,
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Pending,
    Granted,
    Duplicate,
    Released,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub registration_id: String,
    pub claim_id: String,
    pub session_id: String,
    pub standing: Standing,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub location: Option<RegistryLocation>,
}

/// The small public receipt; session/location context stays inside Radar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub registration_id: String,
    pub claim_id: String,
    pub standing: Standing,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_id: Option<String>,
}

impl From<&Decision> for Answer {
    fn from(claim: &Decision) -> Self {
        Self {
            registration_id: claim.registration_id.clone(),
            claim_id: claim.claim_id.clone(),
            standing: claim.standing.clone(),
            subject_id: claim.subject_id.clone(),
        }
    }
}

fn read(registry: &Registry, name: &str) -> Result<Option<Decision>, String> {
    let path = registry.root.join("claims").join(name);
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let bytes = read_record_file(&path, MAX_BYTES)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("claim {}: {e}", path.display()))
}

fn write(registry: &Registry, name: &str, claim: &Decision) -> Result<(), String> {
    let directory = registry.root.join("claims");
    super::prepare_directory(&directory)?;
    let bytes = serde_json::to_vec(claim).map_err(|e| e.to_string())?;
    write_atomic(&directory, &directory.join(name), &bytes, MAX_BYTES)
}

/// One process's current authority over a subject: the pair a reader needs, and
/// nothing about how it was established. Resolved once per request so no leaf
/// re-reads the claims, and copied inward as a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub subject_id: String,
    pub claim_id: String,
}

/// The attachment a registration currently holds, if any. A pending, duplicate
/// or released claim is not authority.
pub fn attachment(
    registry: &Registry,
    registration_id: &str,
) -> Result<Option<Attachment>, String> {
    let Some(claim) = current(registry, registration_id)? else {
        return Ok(None);
    };
    let Some(subject_id) = claim.subject_id else {
        return Ok(None);
    };
    if claim.standing != Standing::Granted
        || crate::control_plane::lifecycle::read_subject_record(registry, &subject_id)?
            .is_none_or(|record| record.current_claim.as_deref() != Some(claim.claim_id.as_str()))
    {
        return Ok(None);
    }
    Ok(Some(Attachment {
        claim_id: claim.claim_id,
        subject_id,
    }))
}

pub fn current(registry: &Registry, registration_id: &str) -> Result<Option<Decision>, String> {
    validate_registration_id(registration_id)?;
    match read(registry, &format!("current-{registration_id}.json"))? {
        Some(pointer) => read(registry, &format!("{}.json", pointer.claim_id)),
        None => Ok(None),
    }
}

/// Caller serializes presence decisions, including verification and binding.
pub fn begin(
    registry: &Registry,
    registration_id: &str,
    claim_id: &str,
    session_id: &str,
    location: Option<RegistryLocation>,
) -> Result<Decision, String> {
    for id in [registration_id, claim_id, session_id] {
        validate_registration_id(id)?;
    }
    let name = format!("{claim_id}.json");
    if let Some(mut claim) = read(registry, &name)? {
        if claim.registration_id != registration_id || claim.session_id != session_id {
            return Err("claim ID already identifies a different process or session".into());
        }
        if current(registry, registration_id)?
            .is_some_and(|c| c.claim_id != claim_id && c.standing != Standing::Released)
        {
            claim.standing = Standing::Released;
            claim.subject_id = None;
            write(registry, &name, &claim)?;
            return Ok(claim);
        }
        if location.is_some() {
            claim.location = location;
        }
        save(registry, &claim)?;
        return Ok(claim);
    }
    if let Some(mut previous) = current(registry, registration_id)? {
        crate::control_plane::lifecycle::release_attachment(
            registry,
            &previous.claim_id,
            previous.subject_id.as_deref(),
        )?;
        previous.standing = Standing::Released;
        previous.subject_id = None;
        write(registry, &format!("{}.json", previous.claim_id), &previous)?;
        write(
            registry,
            &format!("current-{registration_id}.json"),
            &previous,
        )?;
    }
    let claim = Decision {
        registration_id: registration_id.into(),
        claim_id: claim_id.into(),
        session_id: session_id.into(),
        standing: Standing::Pending,
        subject_id: None,
        location,
    };
    save(registry, &claim)?;
    Ok(claim)
}

/// Decision first, current projection second. Reassertion heals an interrupted projection.
pub fn save(registry: &Registry, claim: &Decision) -> Result<(), String> {
    write(registry, &format!("{}.json", claim.claim_id), claim)?;
    write(
        registry,
        &format!("current-{}.json", claim.registration_id),
        claim,
    )
}

pub fn release(
    registry: &Registry,
    registration_id: &str,
    claim_id: &str,
) -> Result<Decision, String> {
    validate_registration_id(registration_id)?;
    validate_registration_id(claim_id)?;
    let mut claim = read(registry, &format!("{claim_id}.json"))?.ok_or("claim is not recorded")?;
    if claim.registration_id != registration_id {
        return Err("claim belongs to another registration".into());
    }
    crate::control_plane::lifecycle::release_attachment(
        registry,
        claim_id,
        claim.subject_id.as_deref(),
    )?;
    claim.standing = Standing::Released;
    claim.subject_id = None;
    write(registry, &format!("{claim_id}.json"), &claim)?;
    if current(registry, registration_id)?.is_some_and(|c| c.claim_id == claim_id) {
        write(registry, &format!("current-{registration_id}.json"), &claim)?;
    }
    Ok(claim)
}
