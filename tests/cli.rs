#![cfg(unix)]
//! The arguments Radar answers without a dashboard: version and help.
//!
//! Both print and exit, so they are driven as the binary an operator runs rather
//! than as a function call. The version comes from the crate, not a literal, so
//! the test fails if the flag is dropped or starts reporting another number.

use std::process::Command;

#[test]
fn the_version_flag_reports_this_crate_and_exits_cleanly() {
    for flag in ["--version", "-v"] {
        let output = Command::new(env!("CARGO_BIN_EXE_radar"))
            .arg(flag)
            .output()
            .expect("run radar with the version flag");
        assert_eq!(output.status.code(), Some(0), "{flag}");
        let printed = String::from_utf8(output.stdout).expect("utf-8 version output");
        assert_eq!(
            printed.trim(),
            format!(
                "radar {} (wire {})",
                env!("CARGO_PKG_VERSION"),
                agent_radar::control_plane::ops::PROTOCOL_REVISION
            ),
            "{flag} printed {printed:?}"
        );
        assert!(output.stderr.is_empty(), "{flag} wrote to stderr");
    }
}

#[test]
fn a_run_outside_a_pane_refuses_before_running_anything() {
    // The only thing this could execute is the scope wrapper, and a refusal
    // happens before that: the program never starts.
    let output = Command::new(env!("CARGO_BIN_EXE_radar"))
        .args(["run", "--", "sh", "-c", "exit 42"])
        .env_remove("TMUX_PANE")
        .output()
        .expect("run radar run outside a pane");
    assert_eq!(output.status.code(), Some(2));
    let printed = String::from_utf8(output.stderr).expect("utf-8 refusal");
    assert!(printed.contains("not a tmux pane"), "{printed}");
}

#[test]
fn a_run_with_no_command_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_radar"))
        .args(["run", "--"])
        .output()
        .expect("run radar run with no command");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_workspace_keeps_backend_ownership_and_cleans_up_only_its_private_host() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("radar-workspace-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let tmux = root.join("tmux");
    let log = root.join("calls");
    std::fs::write(
        &tmux,
        r#"#!/bin/sh
for word in "$@"; do printf '<%s>' "$word"; done >> "$RADAR_TEST_LOG"
printf '\n' >> "$RADAR_TEST_LOG"
if [ "$3" = has-session ] && [ "$RADAR_TEST_MISSING" = yes ]; then
    echo 'no existing backend session' >&2
    exit 1
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let run = |missing| {
        Command::new(env!("CARGO_BIN_EXE_radar"))
            .args(["workspace", "--session", "chosen session"])
            .env("RADAR_TMUX", &tmux)
            .env("RADAR_TEST_LOG", &log)
            .env("RADAR_TEST_MISSING", missing)
            .output()
            .unwrap()
    };
    let output = run("no");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = std::fs::read_to_string(&log).unwrap();
    let calls: Vec<_> = calls.lines().collect();
    assert!(calls[0].ends_with("<has-session><-t><chosen session>"));
    assert!(
        calls
            .iter()
            .skip(1)
            .all(|line| line.starts_with("<-L><radar-ui-")),
        "every mutation targets the private UI server: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|line| line.contains("<display-popup>") && line.contains("<85%>")),
        "popup belongs to the outer host"
    );
    assert!(
        calls.iter().any(|line| line.contains("<env><-u><TMUX>")
            && line.ends_with("<attach-session><-t><chosen session>")),
        "the agent area attaches to the requested existing session"
    );
    assert!(calls.last().unwrap().ends_with("<kill-server>"));

    std::fs::write(&log, "").unwrap();
    let output = run("yes");
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "a missing backend creates no host"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_usage_text_lists_the_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--help")
        .output()
        .expect("run radar with --help");
    assert_eq!(output.status.code(), Some(0));
    let printed = String::from_utf8(output.stdout).expect("utf-8 usage output");
    assert!(printed.contains("--version"), "{printed}");
}
