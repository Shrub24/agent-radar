//! The daemon's own registry, read where it is owned.
//!
//! A display used to read these records over the control socket, one page at a
//! time. The daemon's observation sampler reads them in process instead, so one
//! snapshot carries the agents, spawn edges, containers and lifecycle subjects
//! the `*.list` requests serve. The wire shapes are built here and shared with
//! those requests, so a snapshot cannot disagree with what a client reads.

use serde_json::{Value, json};

use crate::control_plane::containers;
use crate::control_plane::lifecycle as subjects;
use crate::control_plane::registry::{
    Channel, LocationEvidence, Registration, Registry, SpawnEdge,
};
use crate::model::FleetObservation;
use crate::registry_observation::{LifecycleSubject, RegistryAgent, RegistryRead};

/// One page of a read that runs to the end of its pages.
///
/// The registry truncates a page to `MAX_LISTED`, so this is the largest page
/// one read can return, and a shorter page is the last one.
const PAGE: usize = crate::control_plane::registry::MAX_LISTED;

/// Every agent, spawn edge, container and lifecycle subject this daemon holds.
///
/// `inventory` is the fleet the same refresh collected: a spawn edge's recorded
/// location is confirmed against it rather than against a second listing.
pub(crate) fn read(
    registry: &Registry,
    inventory: Option<&FleetObservation>,
    now_ms: i64,
) -> Result<RegistryRead, String> {
    Ok(RegistryRead {
        agents: agents(registry, now_ms)?,
        topology: topology(registry, inventory)?,
        lifecycle: lifecycle(registry)?,
        containers: containers_of(registry)?,
    })
}

/// One `agent.list` entry: the registration, its claimed process, and the
/// channels it currently publishes.
///
/// `verification` is the registry's verdict on the claimed process, and is
/// `Unavailable` where nothing verified it — a claim is never a verification.
pub(crate) fn agent_entry(
    registry: &Registry,
    registration: &Registration,
    verification: crate::control_plane::registry::ProcessVerification,
    now_ms: i64,
) -> Result<Value, String> {
    let execution = registry.published_facts(&registration.agent_id, Channel::Execution, now_ms)?;
    let assignment =
        registry.published_facts(&registration.agent_id, Channel::Assignment, now_ms)?;
    let facts = registry.published_facts(&registration.agent_id, Channel::Facts, now_ms)?;
    let context = registry.context(&registration.agent_id, now_ms)?;
    Ok(json!({
        "registration": registration.public(),
        "process": process_evidence(registration, verification),
        "execution": execution,
        "assignment": assignment,
        "facts": facts,
        "context": context,
    }))
}

/// The process evidence served beside a registration.
pub(crate) fn process_evidence(
    registration: &Registration,
    verification: crate::control_plane::registry::ProcessVerification,
) -> ProcessEvidence {
    ProcessEvidence {
        claimed: registration.request.process.is_some(),
        claim: registration.request.process.clone(),
        verification,
    }
}

/// What a registration claimed about its process, and what the daemon verified.
///
/// The claim is same-UID process evidence for matching a pane, not a credential.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ProcessEvidence {
    claimed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim: Option<crate::model::ProcessIdentity>,
    verification: crate::control_plane::registry::ProcessVerification,
}

/// What the fleet says about a spawn edge's recorded location.
///
/// An inventory that was not read at all leaves the location unconfirmed rather
/// than absent: nothing here invents a missing pane.
pub(crate) fn location_evidence(
    inventory: Option<&FleetObservation>,
    edge: &SpawnEdge,
) -> LocationEvidence {
    let recorded = edge
        .location
        .as_ref()
        .and_then(|location| location.pane.as_deref());
    match (inventory, recorded) {
        (Some(inventory), Some(pane)) if inventory.pane(pane).is_some() => {
            LocationEvidence::Present
        }
        (Some(_), Some(_)) => LocationEvidence::Absent,
        _ => LocationEvidence::Unavailable,
    }
}

/// Every registration, read to the end of its pages.
///
/// Nothing here verifies a claimed process: the sampler reads what the daemon
/// holds, and a client's own read is where a verdict is produced.
fn agents(registry: &Registry, now_ms: i64) -> Result<Vec<RegistryAgent>, String> {
    let mut agents = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = registry.list_after(after.as_deref(), PAGE)?;
        let last = page
            .last()
            .map(|registration| registration.agent_id.clone());
        for registration in &page {
            let entry = agent_entry(
                registry,
                registration,
                crate::control_plane::registry::ProcessVerification::Unavailable,
                now_ms,
            )?;
            agents.push(serde_json::from_value(entry).map_err(|error| {
                format!("an agent record this daemon built is unreadable: {error}")
            })?);
        }
        if page.len() < PAGE {
            return Ok(agents);
        }
        after = Some(next_cursor(after, last, "agent")?);
    }
}

/// Every spawn edge, read to the end of its pages.
fn topology(
    registry: &Registry,
    inventory: Option<&FleetObservation>,
) -> Result<Vec<crate::control_plane::registry::PublicSpawnTopology>, String> {
    let mut topology = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = registry.spawn_edges_after(after.as_deref(), PAGE)?;
        let last = page.last().map(|edge| edge.request_id.clone());
        topology.extend(
            page.iter()
                .map(|edge| edge.topology(location_evidence(inventory, edge))),
        );
        if page.len() < PAGE {
            return Ok(topology);
        }
        after = Some(next_cursor(after, last, "spawn edge")?);
    }
}

/// Every managed lifecycle subject, read to the end of its pages.
///
/// A subject's exit is observed by the daemon's recovery sweep, which owns that
/// work; this reads the state that sweep maintains.
fn lifecycle(registry: &Registry) -> Result<Vec<LifecycleSubject>, String> {
    let mut subjects_seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = subjects::list_logical_after(registry, after.as_deref(), PAGE)?;
        let last = page.last().map(logical_id);
        for subject in &page {
            let Some(current) = subjects::read_logical(registry, &logical_id(subject))? else {
                continue;
            };
            let view = subjects::public_view(&current);
            subjects_seen.push(serde_json::from_value(view).map_err(|error| {
                format!("a lifecycle record this daemon built is unreadable: {error}")
            })?);
        }
        if page.len() < PAGE {
            return Ok(subjects_seen);
        }
        after = Some(next_cursor(after, last, "lifecycle subject")?);
    }
}

/// The stable logical id a lifecycle read pages by.
fn logical_id(subject: &crate::control_plane::lifecycle::Subject) -> String {
    subject
        .logical_subject_id
        .clone()
        .unwrap_or_else(|| subject.subject_id.clone())
}

/// Every container the daemon records, read to the end of its pages.
fn containers_of(
    registry: &Registry,
) -> Result<Vec<crate::control_plane::containers::PublicContainer>, String> {
    let mut containers_seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = containers::list_after(registry, after.as_deref(), PAGE)?;
        let last = page.last().map(|container| container.id.clone());
        containers_seen.extend(page.iter().map(|container| container.public()));
        if page.len() < PAGE {
            return Ok(containers_seen);
        }
        after = Some(next_cursor(after, last, "container")?);
    }
}

/// The cursor after `last`, refusing a page that repeated the cursor it was
/// given: a read that makes no progress would otherwise never end.
fn next_cursor(after: Option<String>, last: Option<String>, what: &str) -> Result<String, String> {
    match last {
        Some(last) if Some(&last) != after.as_ref() => Ok(last),
        _ => Err(format!("the registry repeated a {what} page cursor")),
    }
}

#[cfg(test)]
mod tests {
    use super::next_cursor;

    #[test]
    fn a_page_that_repeats_its_cursor_is_refused_rather_than_read_again() {
        assert_eq!(
            next_cursor(Some("a".into()), Some("b".into()), "agent"),
            Ok("b".into())
        );
        assert!(next_cursor(Some("a".into()), Some("a".into()), "agent").is_err());
        // A page that returned nothing cannot advance a read either.
        assert!(next_cursor(Some("a".into()), None, "agent").is_err());
    }
}
