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
fn the_usage_text_lists_the_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_radar"))
        .arg("--help")
        .output()
        .expect("run radar with --help");
    assert_eq!(output.status.code(), Some(0));
    let printed = String::from_utf8(output.stdout).expect("utf-8 usage output");
    assert!(printed.contains("--version"), "{printed}");
}
