//! Physical process grouping; unit state is never an application outcome.

use crate::{
    control_plane::registry::ProcessVerifier,
    model::{ProcessIdentity, SessionUuid},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, ffi::OsStr, fs, path::Path, process::Command};

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

/// What one cleanup pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Slices stopped because nothing was left inside them.
    pub slices: usize,
    /// Dead units whose failed state was cleared.
    pub cleared: usize,
}

/// The unified hierarchy every scope lives under.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Stops every `radar-*.slice` whose cgroup is empty, then clears dead `radar-*` units.
///
/// A slice stop ends everything inside it, so emptiness is read from `cgroup.events`
/// and never inferred from a recorded exit: a main process can be gone while a
/// descendant keeps the group alive. An empty slice holds no authority, so this is
/// safe to run without a record to justify it, which is what lets it clear slices
/// left by a state reset or by an earlier run whose exit nobody observed.
///
/// `seen` carries the slices found empty by the previous pass. A launch creates its
/// slice before its scope joins it, so a running daemon stops a slice only once it
/// has been empty on two passes, never on first sight. `None` stops on first sight,
/// for a caller that knows no launch can be in flight.
pub fn sweep(seen: Option<&mut BTreeSet<String>>) -> Result<Swept, String> {
    sweep_in(
        &crate::service::program("RADAR_SYSTEMCTL", "systemctl"),
        Path::new(CGROUP_ROOT),
        seen,
    )
}

fn sweep_in(
    systemctl: &OsStr,
    cgroup_root: &Path,
    seen: Option<&mut BTreeSet<String>>,
) -> Result<Swept, String> {
    let empty = empty_slices(systemctl, cgroup_root)?;
    let empty: Vec<String> = match seen {
        Some(seen) => {
            let stale = empty.intersection(seen).cloned().collect();
            *seen = empty;
            stale
        }
        None => empty.into_iter().collect(),
    };
    if !empty.is_empty() {
        transact(systemctl, "stop", &empty)?;
    }
    let dead = list_units(systemctl, "radar-*", &["--state=failed"])?;
    if !dead.is_empty() {
        transact(systemctl, "reset-failed", &dead)?;
    }
    Ok(Swept {
        slices: empty.len(),
        cleared: dead.len(),
    })
}

fn list_units(systemctl: &OsStr, pattern: &str, extra: &[&str]) -> Result<Vec<String>, String> {
    let out = Command::new(systemctl)
        .args([
            "--user",
            "list-units",
            pattern,
            "--all",
            "--plain",
            "--no-legend",
        ])
        .args(extra)
        .output()
        .map_err(|e| format!("unit listing: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "unit listing: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|unit| unit.starts_with("radar-"))
        .map(str::to_owned)
        .collect())
}

/// The `radar-*.slice` units whose groups hold nothing, from one manager call
/// however many slices exist. An unresolvable or unreadable group reads as populated,
/// so a stop is never sent on a guess.
fn empty_slices(systemctl: &OsStr, root: &Path) -> Result<BTreeSet<String>, String> {
    let slices = list_units(systemctl, "radar-*.slice", &[])?;
    if slices.is_empty() {
        return Ok(BTreeSet::new());
    }
    let out = Command::new(systemctl)
        .args(["--user", "show", "-p", "Id", "-p", "ControlGroup"])
        .args(&slices)
        .output()
        .map_err(|e| format!("scope membership: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "scope membership: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut empty = BTreeSet::new();
    // One block per unit, separated by a blank line.
    for block in String::from_utf8_lossy(&out.stdout).split("\n\n") {
        let field = |name: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
        };
        let (Some(id), Some(path)) = (field("Id="), field("ControlGroup=")) else {
            continue;
        };
        if !path.starts_with('/') || path.split('/').any(|part| part == "." || part == "..") {
            continue;
        }
        let events = root
            .join(path.trim_start_matches('/'))
            .join("cgroup.events");
        // `populated` covers descendants, so a nested group with a live process
        // keeps its slice.
        if fs::read_to_string(events)
            .is_ok_and(|text| text.lines().any(|line| line.trim() == "populated 0"))
        {
            empty.insert(id.to_owned());
        }
    }
    Ok(empty)
}

fn transact(systemctl: &OsStr, verb: &str, units: &[String]) -> Result<(), String> {
    let out = Command::new(systemctl)
        .args(["--user", verb])
        .args(units)
        .output()
        .map_err(|e| format!("unit {verb}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "unit {verb}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_slice_with_nothing_left_in_it_is_stopped() {
        let dir = std::env::temp_dir().join(format!("radar-sweep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let groups = dir.join("cgroup");
        for (name, populated) in [("one", "0"), ("two", "1")] {
            let group = groups.join(format!("radar.slice/radar-{name}.slice"));
            fs::create_dir_all(&group).unwrap();
            fs::write(
                group.join("cgroup.events"),
                format!("populated {populated}\nfrozen 0\n"),
            )
            .unwrap();
        }
        let record = dir.join("calls");
        let systemctl = dir.join("systemctl");
        fs::write(
            &systemctl,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> '{record}'
case "$*" in
  *"list-units radar-*.slice"*) printf 'radar-one.slice loaded inactive dead - -\nradar-two.slice loaded active running - -\n' ;;
  *"--state=failed"*) printf 'radar-dead.scope loaded failed failed - -\n' ;;
  *show*)
    printf 'ControlGroup=/radar.slice/radar-one.slice\nId=radar-one.slice\n\nControlGroup=/radar.slice/radar-two.slice\nId=radar-two.slice\n' ;;
esac
"#,
                record = record.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();

        let mut seen = BTreeSet::new();
        let first = sweep_in(systemctl.as_os_str(), &groups, Some(&mut seen)).expect("first pass");
        assert_eq!(
            first.slices, 0,
            "an empty slice is not stopped on first sight"
        );
        let swept = sweep_in(systemctl.as_os_str(), &groups, Some(&mut seen)).expect("sweep");

        assert_eq!(
            swept,
            Swept {
                slices: 1,
                cleared: 1
            }
        );
        let calls = fs::read_to_string(&record).unwrap();
        assert!(calls.contains("--user stop radar-one.slice"), "{calls}");
        assert!(
            !calls
                .lines()
                .any(|line| line.starts_with("--user stop") && line.contains("radar-two")),
            "a populated slice must never be stopped: {calls}"
        );
        assert!(
            calls.contains("--user reset-failed radar-dead.scope"),
            "{calls}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

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
