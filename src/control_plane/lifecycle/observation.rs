use super::{Registry, read_recorded, record_exit_locked, write};
use crate::control_plane::registry::ProcessVerifier;
use crate::model::ProcessIdentity;
use crate::process_control::{self, Acquired};
use serde::{Deserialize, Serialize};

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
    if subject.exited_at.is_some() {
        return Ok(true);
    }
    let Some(process) = subject.process else {
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
    if current.process.as_ref() != Some(&process) {
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
        let at = crate::control_plane::store::format_millis(now_ms);
        current.observation = Some(Observation {
            process,
            evidence,
            source,
            changed_at: at.clone(),
        });
        current.recorded_at = at;
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
