use super::{Registry, read_recorded, record_exit_locked, write};
use crate::control::timestamp_millis;
use crate::control_plane::registry::{ProcessVerifier, RegistryLocation};
use crate::model::ProcessIdentity;
use crate::process_control::{self, Acquired};
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
    subject_id: String,
}
impl Drop for Collection<'_> {
    fn drop(&mut self) {
        self.registry
            .observing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.subject_id);
    }
}

pub fn observe_exit_from(
    registry: &Registry,
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    source: Observer,
) -> Result<bool, String> {
    let subject = read_recorded(registry, subject_id)?;
    if subject.process.exited_at.is_some() {
        return Ok(true);
    }
    let Some(process) = subject.process.identity else {
        return Ok(false);
    };
    let mut collecting = registry.observing.lock().unwrap_or_else(|e| e.into_inner());
    if !collecting.insert(subject_id.to_owned()) {
        // This is a coalesced hint, not evidence that the process is running.
        return Ok(false);
    }
    drop(collecting);
    let _collection = Collection {
        registry,
        subject_id: subject_id.to_owned(),
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
    let mut current = read_recorded(registry, subject_id)?;
    if current.process.identity.as_ref() != Some(&process) {
        return Err("process binding changed during reconciliation".into());
    }
    if current.process.exited_at.is_some() {
        return Ok(true);
    }
    if current
        .process
        .observation
        .as_ref()
        .is_none_or(|previous| previous.evidence != evidence)
    {
        let at = crate::control_plane::store::format_millis(now_ms);
        current.process.observation = Some(Observation {
            process,
            evidence,
            source,
            changed_at: at.clone(),
        });
        current.process.recorded_at = at;
        write(registry, &current)?;
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
    subject_id: &str,
    location: &RegistryLocation,
    evidence: PaneEvidence,
    now_ms: i64,
) -> Result<bool, String> {
    let subject = read_recorded(registry, subject_id)?;
    if terminal(&subject) || subject.process.bound_registration_id.is_none() {
        return Ok(terminal(&subject));
    }
    let at = crate::control_plane::store::format_millis(now_ms);
    // Physical evidence is gathered outside the shared transition lock, so an
    // independent subject stays admissible while this one is reconciled.
    if evidence == PaneEvidence::Unavailable {
        return Ok(false);
    }
    let _transition = registry.transition();
    let mut current = read_recorded(registry, subject_id)?;
    if terminal(&current) {
        return Ok(true);
    }
    if current.process.bound_registration_id.is_none() {
        return Ok(false);
    }
    match evidence {
        PaneEvidence::Unavailable => Ok(false),
        PaneEvidence::Present => {
            // A contradicting observation clears pending confirmation.
            if current.process.pane_absence.take().is_some() {
                current.process.recorded_at = at;
                write(registry, &current)?;
            }
            Ok(false)
        }
        PaneEvidence::Absent => {
            let pending = match current.process.pane_absence.take() {
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
                current.process.pane_absence = None;
                current.process.ended_externally_at = Some(at.clone());
                current.process.ended_evidence = Some(EndedEvidence {
                    reason: EndedReason::PaneLost,
                    source: location.backend.clone(),
                    location: location.clone(),
                    confirmations: pending.confirmations,
                    observed_at: at.clone(),
                });
                current.process.recorded_at = at;
                write(registry, &current)?;
                return Ok(true);
            }
            current.process.pane_absence = Some(pending);
            current.process.recorded_at = at;
            write(registry, &current)?;
            Ok(false)
        }
    }
}

fn terminal(subject: &super::Claim) -> bool {
    subject.process.exited_at.is_some() || subject.process.ended_externally_at.is_some()
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
