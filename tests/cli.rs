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
fn the_usage_text_lists_the_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--help")
        .output()
        .expect("run radar with --help");
    assert_eq!(output.status.code(), Some(0));
    let printed = String::from_utf8(output.stdout).expect("utf-8 usage output");
    assert!(printed.contains("--version"), "{printed}");
}
