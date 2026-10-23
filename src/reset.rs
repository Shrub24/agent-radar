use std::{fs, path::Path};

use crate::{
    control_plane::{
        LocalProcfsVerifier,
        lifecycle::{self, ProcessKind, ProcessRecord},
        registry::Registry,
        server,
    },
    managed_scope::{Scope, VerifiedScope},
    model::SessionUuid,
    service,
};

const RESET_STOP_TIMEOUT: &str = "reset stop";
/// Print the planned reset without changing state unless `yes` is set.
pub fn run(server_name: &str, hard: bool, yes: bool) -> i32 {
    let root = server::state_dir();
    match preview(&root, hard) {
        Ok(plan) => print!("{plan}"),
        Err(error) => {
            eprintln!("radar reset: {error}");
            return 1;
        }
    }
    if !yes {
        println!("No changes made. Re-run with `--yes` to reset.");
        return 0;
    }

    if let Err(error) = service::stop_for_reset() {
        eprintln!("radar reset: {}", restore_daemon(&error));
        return 1;
    }

    let registry = match Registry::open(&root) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!(
                "radar reset: {}",
                restore_daemon(&format!("cannot read saved process state: {error}"))
            );
            return 1;
        }
    };
    if let Err(error) = stop_scopes(&registry) {
        eprintln!("radar reset: {}", restore_daemon(&error));
        return 1;
    }
    if let Err(error) = clear_state(&root, hard, &registry) {
        eprintln!(
            "radar reset: state cleanup failed and the daemon remains stopped; rerun reset to finish: {error}"
        );
        return 1;
    }
    drop(registry);
    println!("Radar state reset.");

    if let Err(error) = service::restart_for_reset(server_name) {
        eprintln!(
            "radar reset: state reset, tmux restart failed, and the daemon remains stopped: {error}"
        );
        return 1;
    }
    println!("Restarted Radar tmux server `{server_name}`.");

    if let Err(error) = service::start_after_reset() {
        eprintln!("radar reset: state and tmux reset, but daemon restart failed: {error}");
        return 1;
    }
    println!("Restarted Radar daemon.");
    0
}

fn restore_daemon(problem: &str) -> String {
    match service::start_after_reset() {
        Ok(()) => format!("{problem}; daemon restarted without changing state"),
        Err(error) => format!("{problem}; daemon restoration also failed: {error}"),
    }
}

fn preview(root: &Path, hard: bool) -> Result<String, String> {
    if fs::symlink_metadata(root).is_ok() {
        ensure_real_directory(root)?;
    }
    let mut text = format!(
        "Radar reset ({})\n",
        if hard {
            "hard"
        } else {
            "preserve durable subjects"
        }
    );
    let mut counted = 0;
    for category in [
        "agents",
        "claims",
        "lifecycle",
        "spawns",
        "requests",
        "closes",
        "publications",
        "events",
        "communications",
    ] {
        let mut count = count_files(&root.join(category))?;
        if category == "events" {
            count += count_files(&root.join("events.json"))?;
            count += count_files(&root.join("events.dirty"))?;
        }
        counted += count;
        let action = match (hard, category) {
            (false, "lifecycle") => {
                "purge claims/process records; keep subject IDs and session associations"
            }
            (false, "communications") => "keep subject-addressed handoffs and inboxes",
            (false, _) => "purge",
            (true, _) => "purge",
        };
        text.push_str(&format!("  {category}: {count} file(s) — {action}\n"));
    }
    let other = count_files(root)?.saturating_sub(counted);
    if other > 0 {
        text.push_str(&format!("  other state: {other} file(s) — purge\n"));
    }
    text.push_str("  managed scopes: stop gracefully; reset refuses if any survive\n");
    text.push_str("  tmux: restart Radar's served server; panes/layout are lost and arbitrary pane processes may receive normal tmux shutdown signals\n");
    text.push_str("  daemon: stop before cleanup, start after tmux restart\n");
    Ok(text)
}

fn count_files(path: &Path) -> Result<usize, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("{}: refusing to follow a symlink", path.display()));
    }
    if !metadata.is_dir() {
        return Ok(1);
    }
    let mut count = 0;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for item in
            fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let item = item.map_err(|error| format!("{}: {error}", directory.display()))?;
            let file_type = item.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() {
                pending.push(item.path());
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

fn stop_scopes(registry: &Registry) -> Result<(), String> {
    let output = service::systemctl(&[
        "list-units",
        "--type=scope",
        "--all",
        "--plain",
        "--no-legend",
        "radar-*-main.scope",
    ])
    .map_err(|error| format!("cannot inspect managed scopes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect managed scopes: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut scopes = Vec::<VerifiedScope>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        let Some(unit) = columns.first().copied() else {
            continue;
        };
        let state = columns
            .get(2)
            .copied()
            .ok_or_else(|| format!("cannot read active state for managed scope `{unit}`"))?;
        if state == "inactive" {
            continue;
        }
        if !matches!(state, "active" | "activating" | "deactivating" | "failed") {
            return Err(format!(
                "managed scope `{unit}` has unknown state `{state}`"
            ));
        }
        let registration_id = registration_from_unit(unit)
            .ok_or_else(|| format!("active scope `{unit}` has an unrecognized name"))?;
        let process = lifecycle::read_process(registry, &registration_id)?
            .ok_or_else(|| format!("active scope `{unit}` has no Radar process record"))?;
        verify_recorded_scope(&process, &registration_id)?;
        let scope = process.scope.as_ref().ok_or_else(|| {
            format!("active scope `{unit}` is not recorded as a Radar scope; refusing reset")
        })?;
        let verified = match process.kind {
            ProcessKind::Command => scope.verify_unit(&registration_id),
            ProcessKind::Agent => {
                let identity = process.identity.as_ref().ok_or_else(|| {
                    format!("active agent scope `{unit}` has no captured process identity")
                })?;
                scope.verify(&registration_id, identity, &LocalProcfsVerifier)
            }
        };
        scopes.push(
            verified
                .map_err(|error| format!("active scope `{unit}` cannot be verified: {error}"))?,
        );
    }
    let refs = scopes.iter().collect::<Vec<_>>();
    let stop_error = crate::managed_scope::stop(&refs).err();
    let mut survivors = Vec::new();
    for scope in &scopes {
        match scope.remaining() {
            Ok(names) if !names.is_empty() => {
                survivors.push(format!("{}: {}", scope.unit, names.join(", ")));
            }
            Ok(_) => {}
            Err(error) => survivors.push(format!("{}: unknown ({error})", scope.unit)),
        }
    }
    if let Some(error) = stop_error {
        let suffix = if survivors.is_empty() {
            String::new()
        } else {
            format!("; survivors: {}", survivors.join("; "))
        };
        return Err(format!("{RESET_STOP_TIMEOUT}: {error}{suffix}"));
    }
    if !survivors.is_empty() {
        return Err(format!(
            "managed processes survived reset stop: {}",
            survivors.join("; ")
        ));
    }
    // The daemon is stopped, so no launch is in flight and every empty slice is dead,
    // including those whose records an earlier reset already cleared.
    crate::managed_scope::sweep(None)
        .map(|_| ())
        .map_err(|error| format!("managed scope cleanup: {error}"))
}

fn verify_recorded_scope(process: &ProcessRecord, registration_id: &str) -> Result<(), String> {
    let expected = Scope::for_registration(registration_id)?;
    if process.registration_id != registration_id || process.scope.as_ref() != Some(&expected) {
        return Err(format!(
            "process record for `{registration_id}` does not match its managed scope"
        ));
    }
    Ok(())
}

fn registration_from_unit(unit: &str) -> Option<String> {
    let hex = unit.strip_prefix("radar-")?.strip_suffix("-main.scope")?;
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
    .to_ascii_lowercase();
    SessionUuid::parse(&id).map(|_| id)
}

fn clear_state(root: &Path, hard: bool, registry: &Registry) -> Result<(), String> {
    if !entry_exists(root)? {
        fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
    }
    ensure_real_directory(root)?;
    if !hard {
        lifecycle::clear_subject_attachments(registry)?;
    }

    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !hard
            && matches!(
                entry.file_name().to_str(),
                Some("lifecycle" | "communications")
            )
        {
            continue;
        }
        remove_path(&entry.path())?;
    }
    if !hard {
        prune_lifecycle(root)?;
    }
    Ok(())
}

fn prune_lifecycle(root: &Path) -> Result<(), String> {
    let lifecycle = root.join("lifecycle");
    if !entry_exists(&lifecycle)? {
        return Ok(());
    }
    ensure_real_directory(&lifecycle)?;
    for entry in fs::read_dir(&lifecycle).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() != "identity" {
            remove_path(&entry.path())?;
        }
    }
    let identity = lifecycle.join("identity");
    if entry_exists(&identity)? {
        ensure_real_directory(&identity)?;
        for entry in fs::read_dir(&identity).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                remove_path(&entry.path())?;
            }
        }
    }
    Ok(())
}

fn entry_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn ensure_real_directory(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} is not a real directory", path.display()));
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))
    } else if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|error| format!("{}: {error}", path.display()))
    } else {
        Err(format!("{}: unsupported filesystem entry", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::lifecycle::{ManagedMetadata, SubjectRecord};
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    const SUBJECT_ID: &str = "00000000-0000-4000-8000-000000000001";
    const PARENT_SUBJECT_ID: &str = "00000000-0000-4000-8000-000000000002";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "radar-reset-{}",
                crate::control_plane::store::random_uuid()
            ));
            fs::create_dir_all(&path).expect("scratch");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn subject(id: &str) -> SubjectRecord {
        SubjectRecord {
            version: 1,
            subject_id: id.to_string(),
            managed: ManagedMetadata {
                parent_subject_id: Some(PARENT_SUBJECT_ID.into()),
                ..ManagedMetadata::default()
            },
            session: Some("session".into()),
            current_claim: Some("claim".into()),
            last_claim: Some("claim".into()),
            recorded_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn fixture(root: &Path) {
        let identity = root.join("lifecycle/identity");
        fs::create_dir_all(&identity).unwrap();
        write_json(
            &identity.join(format!("{SUBJECT_ID}.json")),
            &subject(SUBJECT_ID),
        );
        fs::create_dir_all(root.join("agents")).unwrap();
        fs::write(root.join("agents/registration.json"), b"{}").unwrap();
        fs::create_dir_all(root.join("lifecycle/private")).unwrap();
        fs::write(root.join("lifecycle/private/claim.json"), b"{}").unwrap();
        fs::create_dir_all(root.join("spawns")).unwrap();
        fs::write(root.join("spawns/edge.json"), b"{}").unwrap();
        fs::create_dir_all(root.join("communications")).unwrap();
        for directory in [
            "lifecycle",
            "lifecycle/identity",
            "lifecycle/private",
            "agents",
            "spawns",
            "communications",
        ] {
            fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn preview_describes_the_reset_without_mutating_state() {
        let scratch = Scratch::new();
        fixture(&scratch.0);
        let before = fs::read(scratch.0.join("agents/registration.json")).unwrap();
        let report = preview(&scratch.0, false).unwrap();
        assert!(report.contains("agents: 1 file(s) — purge"));
        assert!(
            report.contains(
                "communications: 0 file(s) — keep subject-addressed handoffs and inboxes"
            )
        );
        assert_eq!(
            fs::read(scratch.0.join("agents/registration.json")).unwrap(),
            before
        );
    }

    #[test]
    fn default_reset_keeps_durable_identity_and_handoffs_without_attachment_authority() {
        let scratch = Scratch::new();
        fixture(&scratch.0);
        let registry = Registry::open(&scratch.0).expect("registry");
        let communications = scratch.0.join("communications");
        let handoff = communications.join("handoff");
        fs::write(&handoff, b"durable communication").unwrap();

        clear_state(&scratch.0, false, &registry).unwrap();
        clear_state(&scratch.0, false, &registry).unwrap();

        let path = scratch
            .0
            .join(format!("lifecycle/identity/{SUBJECT_ID}.json"));
        let record: SubjectRecord = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(record.subject_id, SUBJECT_ID);
        assert_eq!(
            record.managed.parent_subject_id.as_deref(),
            Some(PARENT_SUBJECT_ID)
        );
        assert_eq!(fs::read(handoff).unwrap(), b"durable communication");
        assert!(!scratch.0.join("agents").exists());
        assert!(!scratch.0.join("lifecycle/private").exists());
        assert!(!scratch.0.join("spawns").exists());
        drop(registry);
        let registry = Registry::open(&scratch.0).expect("registry reopens after reset");
        assert!(
            lifecycle::current_attachment(&registry, SUBJECT_ID)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn hard_reset_removes_subjects_and_communications() {
        let scratch = Scratch::new();
        fixture(&scratch.0);
        let registry = Registry::open(&scratch.0).expect("registry");
        clear_state(&scratch.0, true, &registry).unwrap();
        clear_state(&scratch.0, true, &registry).unwrap();
        assert!(!scratch.0.join("lifecycle").exists());
        assert!(!scratch.0.join("communications").exists());
        assert!(fs::read_dir(&scratch.0).unwrap().next().is_none());
    }
}
