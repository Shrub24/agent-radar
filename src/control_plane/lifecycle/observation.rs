use super::{Registry, record_exit_locked};
use crate::control_plane::events::{Source, Transition};
use crate::control_plane::registry::{ProcessVerifier, RegistryLocation};
use crate::identity::RegistrationId;
use crate::model::ProcessIdentity;
use crate::process_control::{self, Acquired};
use crate::timestamp::timestamp_millis;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long a bound pane must stay absent, and how many fresh observations must
/// confirm it, before Radar ends the execution externally.
///
/// One listing that omits a pane is not evidence that its process stopped, so a
/// weak absence only starts a confirmation window. The window and the count are
/// both required: a burst of passes inside one window is not confirmation either.
pub const ABSENCE_WINDOW: Duration = Duration::from_secs(6);
pub const ABSENCE_CONFIRMATIONS: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Observer {
    Read,
    Sweep,
    TmuxHook,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Running,
    Absent,
    Replaced,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub process: ProcessIdentity,
    pub evidence: Evidence,
    pub source: Observer,
    /// Last evidence change, not a claim that every later sweep succeeded.
    pub changed_at: String,
}

/// What one fresh inventory read said about a bound pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEvidence {
    /// The backend reports the recorded pane right now.
    Present,
    /// The backend answered and the recorded pane is not in its report. Weak on
    /// its own: it needs confirmation before it changes durable state.
    Absent,
    /// No fresh read: no backend, a failed read, a stale read or a stopping
    /// daemon. Never evidence of absence, and never confirmation.
    Unavailable,
}

/// An absence of a bound pane under confirmation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PaneAbsence {
    pub location: RegistryLocation,
    /// When the first fresh absence was seen, which starts the window.
    pub first_seen_at: String,
    pub last_seen_at: String,
    /// Fresh, consecutive absences seen in this window.
    pub confirmations: u32,
}

/// Why an execution ended without a verified process exit.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndedReason {
    /// The pane the launch was created in is gone and the process exit could not
    /// be verified. A pane's absence is not a process's death, so this names what
    /// was actually observed.
    PaneLost,
}

/// The evidence summary an ended-externally transition carries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EndedEvidence {
    pub reason: EndedReason,
    /// The backend whose inventory was the evidence, as it named itself.
    pub source: String,
    pub location: RegistryLocation,
    pub confirmations: u32,
    pub observed_at: String,
}

struct Collection<'a> {
    registry: &'a Registry,
    registration_id: RegistrationId,
}
impl Drop for Collection<'_> {
    fn drop(&mut self) {
        self.registry
            .observing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.registration_id);
    }
}

pub fn observe_exit_from(
    registry: &Registry,
    registration_id: &RegistrationId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    source: Observer,
) -> Result<bool, String> {
    let Some(execution) = super::read_process(registry, registration_id)? else {
        return Ok(false);
    };
    if execution.exited_at.is_some() {
        return Ok(true);
    }
    let Some(process) = execution.identity else {
        return Ok(false);
    };
    let mut collecting = registry.observing.lock().unwrap_or_else(|e| e.into_inner());
    if !collecting.insert(registration_id.clone()) {
        // This is a coalesced hint, not evidence that the process is running.
        return Ok(false);
    }
    drop(collecting);
    let _collection = Collection {
        registry,
        registration_id: registration_id.clone(),
    };
    // Physical I/O is outside the shared transition lock. Independent subjects
    // remain admissible even while one verifier is waiting on its source.
    let acquired = process_control::acquire(&process, verifier);
    let evidence = match &acquired {
        Ok(Acquired::Process(_)) => Evidence::Running,
        Ok(Acquired::Absent) => Evidence::Absent,
        Ok(Acquired::Replaced) => Evidence::Replaced,
        Err(_) => Evidence::Unavailable,
    };
    let _transition = registry.transition();
    let Some(mut current) = super::read_process(registry, registration_id)? else {
        return Ok(false);
    };
    if current.identity.as_ref() != Some(&process) {
        return Err("process binding changed during reconciliation".into());
    }
    if current.exited_at.is_some() {
        return Ok(true);
    }
    if current
        .observation
        .as_ref()
        .is_none_or(|previous| previous.evidence != evidence)
    {
        super::record_process_transition(
            registry,
            registration_id,
            Source::ProcessObservation,
            Transition::Observed(Observation {
                process,
                evidence,
                source,
                changed_at: crate::control_plane::store::format_millis(now_ms),
            }),
            now_ms,
        )?;
    }
    match acquired {
        Ok(Acquired::Process(_)) => Ok(false),
        Ok(Acquired::Absent | Acquired::Replaced) => {
            record_exit_locked(registry, &mut current, now_ms)?;
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

/// Records what a fresh inventory read said about one bound pane.
///
/// Reports whether the execution is now ended. Definitive process evidence stays
/// the only thing that records a verified exit: this path ends an execution
/// externally, naming the pane loss it actually observed, and only after the
/// absence is confirmed inside its window. An unavailable read does nothing, so a
/// failed, stale or incomplete scan never counts as absence.
pub fn observe_pane(
    registry: &Registry,
    registration_id: &RegistrationId,
    location: &RegistryLocation,
    evidence: PaneEvidence,
    now_ms: i64,
) -> Result<bool, String> {
    let Some(subject) = super::read_process(registry, registration_id)? else {
        return Ok(false);
    };
    if terminal(&subject) || subject.bound_registration_id.is_none() {
        return Ok(terminal(&subject));
    }
    let at = crate::control_plane::store::format_millis(now_ms);
    // Physical evidence is gathered outside the shared transition lock, so an
    // independent subject stays admissible while this one is reconciled.
    if evidence == PaneEvidence::Unavailable {
        return Ok(false);
    }
    let _transition = registry.transition();
    let Some(mut current) = super::read_process(registry, registration_id)? else {
        return Ok(false);
    };
    if terminal(&current) {
        return Ok(true);
    }
    if current.bound_registration_id.is_none() {
        return Ok(false);
    }
    let record = |transition: Transition| {
        super::record_process_transition(
            registry,
            registration_id,
            Source::ProcessObservation,
            transition,
            now_ms,
        )
    };
    match evidence {
        PaneEvidence::Unavailable => Ok(false),
        PaneEvidence::Present => {
            // A contradicting observation clears pending confirmation.
            if current.pane_absence.take().is_some() {
                record(Transition::AbsencePending(None))?;
            }
            Ok(false)
        }
        PaneEvidence::Absent => {
            let pending = match current.pane_absence.take() {
                Some(pending) if pending.location == *location => PaneAbsence {
                    location: location.clone(),
                    first_seen_at: pending.first_seen_at,
                    last_seen_at: at.clone(),
                    confirmations: pending.confirmations.saturating_add(1),
                },
                // A different pane, or no window yet, starts one now.
                _ => PaneAbsence {
                    location: location.clone(),
                    first_seen_at: at.clone(),
                    last_seen_at: at.clone(),
                    confirmations: 1,
                },
            };
            if confirmed(&pending, now_ms) {
                record(Transition::AbsencePending(None))?;
                record(Transition::EndedExternally(EndedEvidence {
                    reason: EndedReason::PaneLost,
                    source: location.backend.clone(),
                    location: location.clone(),
                    confirmations: pending.confirmations,
                    observed_at: at.clone(),
                }))?;
                return Ok(true);
            }
            record(Transition::AbsencePending(Some(pending)))?;
            Ok(false)
        }
    }
}

fn terminal(execution: &super::ProcessRecord) -> bool {
    execution.exited_at.is_some() || execution.ended_externally_at.is_some()
}

/// Whether the window has elapsed with enough fresh confirmations. An unreadable
/// start time is not a licence to end an execution.
fn confirmed(pending: &PaneAbsence, now_ms: i64) -> bool {
    if pending.confirmations < ABSENCE_CONFIRMATIONS {
        return false;
    }
    match timestamp_millis(&pending.first_seen_at) {
        Some(first) => now_ms.saturating_sub(first) >= ABSENCE_WINDOW.as_millis() as i64,
        None => false,
    }
}
