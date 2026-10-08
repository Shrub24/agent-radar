//! Birth-identity process verification, independent of channel freshness.

use crate::model::ProcessIdentity;

use super::{Registration, Registry};

/// Result of comparing a registered birth identity with local procfs evidence.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessVerification {
    /// The complete boot/PID/start tuple matches a readable local process.
    Verified,
    /// The process table confirms that this PID is not present.
    Absent,
    /// The PID exists, but boot identity or process start time differs.
    Mismatched,
    /// No identity was supplied, the platform lacks procfs, or evidence could
    /// not be read reliably. This is not evidence of absence.
    Unavailable,
}

/// Injectable source of local process birth identity evidence.
pub trait ProcessVerifier: Send + Sync {
    /// Return the boot identity and start ticks for this PID, `Ok(None)` only
    /// when the process is positively absent, and `Err` when procfs evidence is
    /// unavailable or permission denied.
    fn inspect(&self, pid: i32) -> Result<Option<ProcessIdentity>, String>;
}

/// Local procfs verifier for the current Linux host.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalProcfsVerifier;

impl ProcessVerifier for LocalProcfsVerifier {
    fn inspect(&self, pid: i32) -> Result<Option<ProcessIdentity>, String> {
        local_inspect(pid)
    }
}

#[cfg(target_os = "linux")]
fn local_inspect(pid: i32) -> Result<Option<ProcessIdentity>, String> {
    use std::fs;

    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| format!("read procfs boot identity: {error}"))?;
    let boot_id = boot.trim();
    if boot_id.is_empty() {
        return Err("procfs boot identity is empty".into());
    }
    let process_path = format!("/proc/{pid}");
    match fs::metadata(&process_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(format!("{process_path} is not a process directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect {process_path}: {error}")),
    }
    let stat_path = format!("{process_path}/stat");
    let stat =
        fs::read_to_string(&stat_path).map_err(|error| format!("read {stat_path}: {error}"))?;
    let (observed_pid, start_ticks) = crate::procfs::Stat::birth_identity(&stat)
        .ok_or_else(|| format!("parse {stat_path} birth identity"))?;
    if observed_pid != pid {
        return Err(format!(
            "{stat_path} names PID {observed_pid}, expected {pid}"
        ));
    }
    Ok(Some(ProcessIdentity {
        boot_id: boot_id.to_string(),
        pid: observed_pid,
        start_ticks,
    }))
}

#[cfg(not(target_os = "linux"))]
fn local_inspect(_pid: i32) -> Result<Option<ProcessIdentity>, String> {
    Err("local process birth identity is unavailable on this platform".into())
}

/// Compare the entire recorded identity against an injectable evidence source.
pub fn verify_identity(
    claimed: Option<&ProcessIdentity>,
    verifier: &dyn ProcessVerifier,
) -> ProcessVerification {
    let Some(claimed) = claimed else {
        return ProcessVerification::Unavailable;
    };
    match verifier.inspect(claimed.pid) {
        Ok(None) => ProcessVerification::Absent,
        Ok(Some(observed))
            if observed.pid == claimed.pid
                && observed.boot_id == claimed.boot_id
                && observed.start_ticks == claimed.start_ticks =>
        {
            ProcessVerification::Verified
        }
        Ok(Some(_)) => ProcessVerification::Mismatched,
        Err(_) => ProcessVerification::Unavailable,
    }
}

impl Registry {
    /// Verify one registration's optional process claim without holding the
    /// registry admission lock while procfs is read. Registration is cloned
    /// under the ordinary file-read boundary first; the injectable verifier then
    /// runs independently of all registry transitions.
    pub fn verify_process(
        &self,
        agent_id: &str,
        verifier: &dyn ProcessVerifier,
    ) -> Result<ProcessVerification, String> {
        let registration = self
            .get(agent_id)?
            .ok_or_else(|| format!("agent {agent_id} is not registered"))?;
        Ok(verify_registration(&registration, verifier))
    }
}

/// Verify an already-read immutable registration. Useful to batch registry reads
/// and keep external evidence collection entirely outside registry locks.
pub fn verify_registration(
    registration: &Registration,
    verifier: &dyn ProcessVerifier,
) -> ProcessVerification {
    verify_identity(registration.request.process.as_ref(), verifier)
}
