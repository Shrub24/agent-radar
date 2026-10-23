//! Physical process grouping; unit state is never an application outcome.

use crate::{
    control_plane::registry::ProcessVerifier,
    model::{ProcessIdentity, SessionUuid},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub unit: String,
    pub slice: String,
}

/// A scope whose membership has been established: the unit exists, and the
/// recorded process is inside it. Only this can be stopped or inspected.
#[derive(Clone, Debug)]
pub struct VerifiedScope {
    pub unit: String,
    path: String,
}

impl Scope {
    pub fn for_registration(id: &str) -> Result<Self, String> {
        let id = SessionUuid::parse(id).ok_or("registration must be a canonical UUID")?;
        let hex = id.as_str().replace('-', "");
        Ok(Self {
            unit: format!("radar-{hex}-main.scope"),
            slice: format!("radar-{hex}.slice"),
        })
    }

    pub fn wrap(&self, executable: &str, argv: &[String]) -> Result<(String, Vec<String>), String> {
        // The launch seam carries an absolute executable, so a configured name is
        // resolved on `PATH` here and a launcher that cannot be found refuses
        // rather than letting the command run some other way.
        Ok(self.wrap_with(&resolve_launcher()?, executable, argv))
    }

    /// The argv builder, with the launcher already resolved: the flags and the
    /// literal argv are what this states, so it needs nothing installed.
    fn wrap_with(
        &self,
        launcher: &str,
        executable: &str,
        argv: &[String],
    ) -> (String, Vec<String>) {
        let mut args = vec![
            "--user".into(),
            "--scope".into(),
            "--expand-environment=no".into(),
            "--quiet".into(),
            format!("--unit={}", self.unit),
            format!("--slice={}", self.slice),
            format!("--description=Radar process {}", self.unit),
            "--property=SendSIGKILL=no".into(),
            "--property=TimeoutStopSec=2s".into(),
            "--".into(),
            executable.into(),
        ];
        args.extend_from_slice(argv);
        (launcher.to_string(), args)
    }

    /// A name alone never establishes an association with a running process.
    pub fn verify_unit(&self, id: &str) -> Result<VerifiedScope, String> {
        if self != &Self::for_registration(id)? {
            return Err("scope does not identify its registration".into());
        }
        let out = Command::new(crate::service::program("RADAR_SYSTEMCTL", "systemctl"))
            .args([
                "--user",
                "show",
                &self.unit,
                "--property=ControlGroup",
                "--value",
            ])
            .output()
            .map_err(|e| format!("systemd user manager unavailable: {e}"))?;
        if !out.status.success() {
            return Err("managed scope unavailable; restart under radar run".into());
        }
        let path = String::from_utf8(out.stdout)
            .map_err(|_| "invalid scope response")?
            .trim()
            .to_string();
        let suffix = format!("/radar.slice/{}/{}", self.slice, self.unit);
        if !path.starts_with('/')
            || !path.ends_with(&suffix)
            || path.split('/').any(|p| p == "." || p == "..")
        {
            return Err("managed scope membership is unresolved".into());
        }
        Ok(VerifiedScope {
            unit: self.unit.clone(),
            path,
        })
    }

    pub fn verify(
        &self,
        id: &str,
        process: &ProcessIdentity,
        verifier: &dyn ProcessVerifier,
    ) -> Result<VerifiedScope, String> {
        let verified = self.verify_unit(id)?;
        let path = &verified.path;
        if verifier.inspect(process.pid)?.as_ref() != Some(process) {
            return Err("scope member birth identity changed".into());
        }
        let membership = fs::read_to_string(format!("/proc/{}/cgroup", process.pid))
            .map_err(|e| e.to_string())?;
        if !membership.lines().any(|line| {
            line.strip_prefix("0::").is_some_and(|p| {
                p == path
                    || p.strip_prefix(path)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        }) {
            return Err("recorded process is outside its managed scope".into());
        }
        if verifier.inspect(process.pid)?.as_ref() != Some(process) {
            return Err("scope member birth identity changed".into());
        }
        Ok(verified)
    }
}

/// `systemd-run` as an absolute path: configured, or found on `PATH`.
fn resolve_launcher() -> Result<String, String> {
    let configured = crate::service::program("RADAR_SYSTEMD_RUN", "systemd-run");
    resolve_launcher_in(&configured, std::env::var_os("PATH").as_deref())
}

fn resolve_launcher_in(
    configured: &std::ffi::OsStr,
    search: Option<&std::ffi::OsStr>,
) -> Result<String, String> {
    let path = std::path::PathBuf::from(configured);
    if path.is_absolute() {
        return path
            .to_str()
            .map(str::to_string)
            .ok_or_else(|| "`RADAR_SYSTEMD_RUN` is not valid text".to_string());
    }
    search
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(&path))
        .find(|candidate| candidate.is_file())
        .and_then(|found| found.to_str().map(str::to_string))
        .ok_or_else(|| {
            format!(
                "`{}` is not installed; set `RADAR_SYSTEMD_RUN` to the scope launcher",
                configured.to_string_lossy()
            )
        })
}

/// One manager transaction starts all stop jobs together. Timeout and TERM-only
/// semantics are properties of the scopes, not a per-process polling loop.
pub fn stop(scopes: &[&VerifiedScope]) -> Result<(), String> {
    if scopes.is_empty() {
        return Ok(());
    }
    let out = Command::new(crate::service::program("RADAR_SYSTEMCTL", "systemctl"))
        .args(["--user", "stop"])
        .args(scopes.iter().map(|s| &s.unit))
        .output()
        .map_err(|e| format!("scope stop: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "scope stop: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

impl VerifiedScope {
    pub fn remaining(&self) -> Result<Vec<String>, String> {
        let root = Path::new("/sys/fs/cgroup").join(self.path.trim_start_matches('/'));
        let mut pending = vec![root];
        let mut pids = BTreeSet::new();
        while let Some(path) = pending.pop() {
            let contents = match fs::read_to_string(path.join("cgroup.procs")) {
                Ok(text) => text,
                // A previously verified cgroup cannot be removed while populated.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("scope survivors are unknown: {e}")),
            };
            for line in contents.lines() {
                let pid = line.parse::<i32>().map_err(|_| "invalid scope member")?;
                if pid <= 0 {
                    return Err("invalid scope member".into());
                }
                pids.insert(pid);
            }
            for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                    pending.push(entry.path());
                }
            }
        }
        Ok(pids
            .into_iter()
            .map(|pid| {
                fs::read_to_string(format!("/proc/{pid}/comm"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "process".into())
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launcher_that_cannot_be_found_refuses_before_anything_runs() {
        let absent = resolve_launcher_in("systemd-run".as_ref(), None);
        assert!(absent.is_err(), "no PATH names a launcher: {absent:?}");
        let elsewhere = resolve_launcher_in("systemd-run".as_ref(), Some("/nonexistent".as_ref()));
        assert!(elsewhere.is_err(), "{elsewhere:?}");
        // A configured absolute path is used as given, so a missing one fails at
        // exec rather than silently running the command unscoped.
        assert_eq!(
            resolve_launcher_in("/run/current-system/sw/bin/systemd-run".as_ref(), None),
            Ok("/run/current-system/sw/bin/systemd-run".to_string())
        );
    }

    #[test]
    fn launch_names_only_the_process_and_preserves_literal_argv() {
        let id = "59f5b964-2999-4187-ad65-8cc1b95ef814";
        let scope = Scope::for_registration(id).unwrap();
        assert_eq!(
            scope.unit,
            "radar-59f5b96429994187ad658cc1b95ef814-main.scope"
        );
        assert_eq!(scope.slice, "radar-59f5b96429994187ad658cc1b95ef814.slice");
        assert_eq!(scope, Scope::for_registration(id).unwrap());
        let (program, argv) = scope.wrap_with(
            "/run/current-system/sw/bin/systemd-run",
            "/private/radar",
            &[
                "launch-bootstrap".into(),
                "--".into(),
                "literal $HOME\n*".into(),
            ],
        );
        assert!(
            program.starts_with('/'),
            "the launcher is an absolute path: {program}"
        );
        let separator = argv.iter().position(|a| a == "--").unwrap();
        for flag in [
            "--user",
            "--scope",
            "--expand-environment=no",
            "--quiet",
            "--property=SendSIGKILL=no",
            "--property=TimeoutStopSec=2s",
        ] {
            assert!(
                argv[..separator].iter().any(|a| a == flag),
                "missing {flag}"
            );
        }
        assert_eq!(
            &argv[separator + 1..],
            [
                "/private/radar",
                "launch-bootstrap",
                "--",
                "literal $HOME\n*"
            ]
        );
        assert!(
            !argv[..separator]
                .iter()
                .any(|a| a.contains("private") || a.contains("$HOME"))
        );
        assert!(
            argv[..separator]
                .iter()
                .any(|a| a.starts_with("--description=Radar process radar-"))
        );
    }
}
