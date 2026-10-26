//! The user units that own the control daemon and its tmux server, and the
//! commands that start and stop them.
//!
//! Systemd is the only authority that starts or stops either unit. The daemon
//! asks for its tmux server through [`ensure_tmux_server`]; the CLI wraps
//! `systemctl --user` for everything else. Nothing here signals a process
//! directly.

use std::process::{Command, Output};

/// The socket unit that holds the control socket path and auto-starts the daemon.
pub const SOCKET_UNIT: &str = "radar-control.socket";
/// The daemon's service unit, started by the socket on the first connection.
pub const DAEMON_UNIT: &str = "radar-daemon.service";

/// The template instance unit that runs the tmux server `server`.
pub fn tmux_unit(server: &str) -> String {
    format!("radar-tmux@{server}.service")
}

/// Where tmux keeps a named server's socket.
///
/// tmux reads `TMUX_TMPDIR` and falls back to the temporary directory, so this
/// follows the same order: a throwaway server a test started with its own
/// `TMUX_TMPDIR` is looked for where that test put it.
pub fn server_socket(server: &str) -> std::path::PathBuf {
    let base = std::env::var_os("TMUX_TMPDIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("tmux-{}", rustix::process::getuid().as_raw()))
        .join(server)
}

/// Stops a throwaway server and removes the socket it leaves behind.
///
/// tmux does not unlink its socket when the server exits, so every name a test
/// or a launcher ever started keeps a file in the socket directory. This is
/// only for servers nothing else owns: Radar's own server is the unit's.
pub fn discard_server(server: &str) {
    let _ = Command::new(program("RADAR_TMUX", "tmux"))
        .args(["-L", server, "kill-server"])
        .output();
    let _ = std::fs::remove_file(server_socket(server));
}

/// Runs `systemctl --user` with `arguments`.
/// The executable the daemon runs for a tool.
///
/// A unit module names its tools by absolute path through `variable`, so the
/// daemon needs no `PATH` of its own. That matters because tmux hands a session
/// the creating client's `PATH`: a daemon given a minimal one would give every
/// pane the same, instead of the user's own.
pub fn program(variable: &str, default: &str) -> std::ffi::OsString {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.into())
}

pub fn systemctl(arguments: &[&str]) -> Result<Output, String> {
    Command::new(program("RADAR_SYSTEMCTL", "systemctl"))
        .arg("--user")
        .args(arguments)
        .output()
        .map_err(|error| format!("systemctl --user: {error}"))
}

/// Runs one unit action that must succeed, naming the unit if it does not.
///
/// A launched process that returns is not a completed action: systemd refusing a
/// start, stop or restart still leaves a zero-length stdout and a non-zero status.
fn unit_action(verb: &str, unit: &str) -> Result<(), String> {
    let output = systemctl(&[verb, unit]).map_err(|problem| format!("{unit}: {problem}"))?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let reason = if reason.is_empty() {
        format!("systemd exited with {}", output.status)
    } else {
        reason
    };
    Err(format!("{unit}: {reason}"))
}

/// Whether `unit` is active now.
pub fn unit_active(unit: &str) -> bool {
    systemctl(&["is-active", "--quiet", unit]).is_ok_and(|output| output.status.success())
}

/// Whether a tmux server answers on `server`'s socket.
///
/// `show-options` reads global state and never starts a server, so asking does
/// not change what is running.
pub fn server_answers(server: &str) -> bool {
    Command::new(program("RADAR_TMUX", "tmux"))
        .args(["-L", server, "show-options", "-g"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Ensures the tmux server `server` is running under its own unit, and that
/// Radar is the one serving it.
///
/// A server that answers while its unit is not active was not started by Radar,
/// so it is refused and nothing is started. Otherwise the unit is started and
/// the unit's `ExecStartPost` waits for the server to answer before success.
pub fn ensure_tmux_server(server: &str) -> Result<(), String> {
    let unit = tmux_unit(server);
    if unit_active(&unit) {
        return if server_answers(server) {
            Ok(())
        } else {
            Err(format!(
                "{unit} is active but tmux server `{server}` does not answer on its socket"
            ))
        };
    }
    if server_answers(server) {
        return Err(format!(
            "tmux server `{server}` answers on its socket but {unit} did not start it; \
             Radar serves only the server its own unit owns and refuses this one"
        ));
    }
    unit_action("start", &unit).map_err(|problem| format!("starting {problem}"))
}

/// The panes `server` holds, live or retained, or `None` when no server answers.
pub fn server_panes(server: &str) -> Result<Option<Vec<String>>, String> {
    if !server_answers(server) {
        return Ok(None);
    }
    let listing = Command::new(program("RADAR_TMUX", "tmux"))
        .args([
            "-L",
            server,
            "list-panes",
            "-a",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index} #{pane_id}",
        ])
        .output()
        .map_err(|error| format!("tmux: {error}"))?;
    if !listing.status.success() {
        let reason = String::from_utf8_lossy(&listing.stderr).trim().to_string();
        // A server kept alive with no session has no target to list. That is no
        // panes, not a failed read: tmux says so and exits non-zero.
        if reason.contains("no current target") {
            return Ok(Some(Vec::new()));
        }
        return Err(format!("tmux server `{server}`: {reason}"));
    }
    Ok(Some(
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

/// Stops the tmux server's unit, but only when the server holds no pane.
///
/// Managed resume and restart cannot yet hand a live process to a new server, so
/// a server with panes is refused rather than ended under them. The refusal names
/// the panes and sends no signal.
pub fn stop_tmux_server(server: &str) -> Result<String, String> {
    refuse_while_busy(server, "stopped")?;
    let unit = tmux_unit(server);
    unit_action("stop", &unit)
        .map(|()| format!("stopped {unit}"))
        .map_err(|problem| format!("stopping {problem}"))
}

/// Stops the socket before the daemon so reset cannot race a new client request.
pub fn stop_for_reset() -> Result<(), String> {
    unit_action("stop", SOCKET_UNIT).map_err(|problem| format!("stopping {problem}"))?;
    unit_action("stop", DAEMON_UNIT).map_err(|problem| format!("stopping {problem}"))
}

/// Starts the daemon service behind its control socket after a reset.
pub fn start_after_reset() -> Result<(), String> {
    unit_action("start", SOCKET_UNIT).map_err(|problem| format!("starting {problem}"))?;
    unit_action("start", DAEMON_UNIT).map_err(|problem| format!("starting {problem}"))
}

/// Reset is the one operator action that intentionally discards tmux topology;
/// unlike the ordinary server restart it does not refuse while panes exist.
pub fn restart_for_reset(server: &str) -> Result<(), String> {
    let unit = tmux_unit(server);
    // The unit's ExecStartPost owns the readiness check, so systemd's restart
    // result is authoritative here.
    unit_action("restart", &unit).map_err(|problem| format!("restarting {problem}"))
}

/// The gate every server stop and restart passes before it touches the unit: no
/// pane, live or retained, and no durable operation pending. Nothing is signalled
/// on refusal. The socket is consulted only while it is listening, since a
/// connection would start the daemon.
fn refuse_while_busy(server: &str, verb: &str) -> Result<(), String> {
    if let Some(panes) = server_panes(server)?
        && !panes.is_empty()
    {
        return Err(format!(
            "tmux server `{server}` holds {} pane(s), so it is not {verb}:\n  {}",
            panes.len(),
            panes.join("\n  ")
        ));
    }
    if unit_active(SOCKET_UNIT) {
        let pending = crate::control_plane::client::unfinished_requests()?;
        if !pending.is_empty() {
            return Err(format!(
                "tmux server `{server}` has {} durable operation(s) pending, so it is not {verb}:\n  {}",
                pending.len(),
                pending.join("\n  ")
            ));
        }
    }
    Ok(())
}

/// Starts the control socket, which lets the daemon start on the next
/// connection. Starting an already listening socket is a no-op.
pub fn start_control_socket() -> Result<(), String> {
    unit_action("start", SOCKET_UNIT).map_err(|problem| format!("starting {problem}"))
}

/// Names why the daemon cannot be reached, from systemd's view of its units.
///
/// A stopped socket means the operator stopped it, and a failed unit means it did
/// not start. Anything else, including a systemd that cannot be asked, keeps the
/// reason the connection gave, so nothing is reported as a state it is not.
pub fn explain_unreachable(problem: &str) -> String {
    explain_states(&unit_state(SOCKET_UNIT), &unit_state(DAEMON_UNIT), problem)
}

/// The rule behind [`explain_unreachable`], over the two unit states it reads.
///
/// A unit whose state could not be read explains nothing, so the connection's own
/// reason is kept rather than being replaced by a guess.
fn explain_states(
    socket: &Result<String, String>,
    daemon: &Result<String, String>,
    problem: &str,
) -> String {
    let socket = socket.as_deref().unwrap_or("unknown");
    let daemon = daemon.as_deref().unwrap_or("unknown");
    if socket == "failed" || daemon == "failed" {
        format!("daemon failed to start: see `journalctl --user -u {DAEMON_UNIT}` ({problem})")
    } else if matches!(socket, "inactive" | "deactivating") {
        "daemon stopped: run `radar daemon start`".to_string()
    } else {
        problem.to_string()
    }
}

/// Runs one `radar daemon` action and returns the lines to print and the exit
/// code.
pub fn daemon_command(action: &str) -> (String, i32) {
    match action {
        "start" => {
            settle(start_control_socket().map(|()| "started the control socket".to_string()))
        }
        "stop" => {
            // The socket goes first: with it stopped nothing can reconnect the
            // service between the two steps. Each stop is checked, so a refused
            // socket stop never reports the daemon as stopped too.
            let stopped = unit_action("stop", SOCKET_UNIT)
                .map_err(|problem| format!("stopping {problem}"))
                .and_then(|()| {
                    unit_action("stop", DAEMON_UNIT).map_err(|problem| {
                        format!("the control socket stopped, stopping {problem}")
                    })
                });
            settle(stopped.map(|()| "stopped the control socket and daemon".to_string()))
        }
        "restart" => settle(
            unit_action("restart", DAEMON_UNIT)
                .map(|()| "restarted the daemon".to_string())
                .map_err(|problem| format!("restarting {problem}")),
        ),
        "status" => status_report(&[
            (SOCKET_UNIT, unit_state(SOCKET_UNIT)),
            (DAEMON_UNIT, unit_state(DAEMON_UNIT)),
        ]),
        other => settle(Err(format!("unknown daemon action `{other}`"))),
    }
}

/// Runs one `radar server` action for `server` and returns the lines to print and
/// the exit code.
pub fn server_command(action: &str, server: &str) -> (String, i32) {
    let unit = tmux_unit(server);
    match action {
        "start" => settle(
            unit_action("start", &unit)
                .map(|()| format!("started {unit}"))
                .map_err(|problem| format!("starting {problem}")),
        ),
        "stop" => settle(stop_tmux_server(server)),
        "restart" => settle(
            refuse_while_busy(server, "restarted")
                .and_then(|()| unit_action("restart", &unit))
                .map(|()| format!("restarted {unit}"))
                .map_err(|problem| format!("restarting {problem}")),
        ),
        "status" => status_report(&[
            (&unit, unit_state(&unit)),
            (
                "panes",
                server_panes(server).map(|panes| match panes {
                    Some(panes) => format!("{} pane(s)", panes.len()),
                    None => "no server answering".to_string(),
                }),
            ),
        ]),
        other => settle(Err(format!("unknown server action `{other}`"))),
    }
}

/// The unit's state as systemd names it, or why it could not be read.
///
/// A failed read is an `Err`, not a state: reporting the diagnostic where a state
/// belongs would present an unknown unit as a known one.
fn unit_state(unit: &str) -> Result<String, String> {
    let output = systemctl(&["show", "--property=ActiveState", "--value", unit])
        .map_err(|problem| format!("{unit}: {problem}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let reason = if reason.is_empty() {
            format!("systemd exited with {}", output.status)
        } else {
            reason
        };
        return Err(format!("{unit}: {reason}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// One line per subject, and an exit code that matches what could be read.
///
/// A subject whose evidence could not be read is still named, so the operator
/// sees the rest of the picture, but the command fails: a lookup that did not
/// happen is never a successful report.
fn status_report(rows: &[(&str, Result<String, String>)]) -> (String, i32) {
    let mut lines = Vec::with_capacity(rows.len());
    let mut read_all = true;
    for (subject, state) in rows {
        match state {
            Ok(state) => lines.push(format!("{subject}: {state}")),
            Err(problem) => {
                read_all = false;
                lines.push(format!("{subject}: unknown ({problem})"));
            }
        }
    }
    (lines.join("\n"), if read_all { 0 } else { 1 })
}

/// Turns an action's outcome into the lines to print and the exit code. A
/// command that ran but reported failure is an error, never a silent success.
fn settle(result: Result<String, String>) -> (String, i32) {
    match result {
        Ok(message) => (message, 0),
        Err(problem) => (problem, 1),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_tool_is_named_by_its_variable_and_otherwise_by_its_bare_name() {
        // Distinct names, so no other test's environment is involved.
        assert_eq!(
            program("RADAR_TEST_TOOL_UNSET", "tmux"),
            std::ffi::OsString::from("tmux")
        );
        // SAFETY: only this test reads or writes these two names.
        unsafe {
            std::env::set_var("RADAR_TEST_TOOL_SET", "/nix/store/x/bin/tmux");
            std::env::set_var("RADAR_TEST_TOOL_EMPTY", "");
        }
        assert_eq!(
            program("RADAR_TEST_TOOL_SET", "tmux"),
            std::ffi::OsString::from("/nix/store/x/bin/tmux")
        );
        assert_eq!(
            program("RADAR_TEST_TOOL_EMPTY", "tmux"),
            std::ffi::OsString::from("tmux")
        );
    }

    use super::*;

    #[test]
    fn the_tmux_unit_is_named_for_its_server() {
        assert_eq!(tmux_unit("radar"), "radar-tmux@radar.service");
    }

    fn state(state: &str) -> Result<String, String> {
        Ok(state.to_string())
    }

    #[test]
    fn a_stopped_socket_and_a_failed_unit_are_named_differently() {
        assert!(
            explain_states(&state("inactive"), &state("inactive"), "refused")
                .starts_with("daemon stopped")
        );
        assert!(
            explain_states(&state("active"), &state("failed"), "refused")
                .starts_with("daemon failed to start")
        );
        assert!(
            explain_states(&state("failed"), &state("inactive"), "refused")
                .starts_with("daemon failed to start")
        );
        // A state nothing could be read for keeps the connection's own reason,
        // since an unread unit is not evidence of either state.
        let unread = Err("socket: systemctl --user: timed out".to_string());
        assert_eq!(
            explain_states(&unread, &state("unknown"), "refused"),
            "refused"
        );
    }

    #[test]
    fn a_status_report_is_a_failure_when_any_subject_could_not_be_read() {
        let (lines, code) = status_report(&[
            ("radar-control.socket", state("active")),
            ("radar-daemon.service", state("inactive")),
        ]);
        assert_eq!(
            lines,
            "radar-control.socket: active\nradar-daemon.service: inactive"
        );
        assert_eq!(code, 0);

        let (lines, code) = status_report(&[
            ("radar-control.socket", state("active")),
            (
                "radar-daemon.service",
                Err("radar-daemon.service: not found".to_string()),
            ),
        ]);
        assert_eq!(
            lines,
            "radar-control.socket: active\n\
             radar-daemon.service: unknown (radar-daemon.service: not found)"
        );
        assert_eq!(
            code, 1,
            "an unread unit was reported as a successful status"
        );
    }

    #[test]
    fn a_refused_unit_operation_names_the_unit_and_the_action() {
        // A systemctl that always refuses: the message must name what was asked
        // for, because the operator's next step depends on which part failed.
        let refusing = format!(
            "{}/radar-refusing-systemctl",
            std::env::temp_dir().display()
        );
        std::fs::write(
            &refusing,
            "#!/bin/sh\necho 'Job failed. See journal.' >&2\nexit 1\n",
        )
        .expect("write a refusing systemctl");
        let mut permissions = std::fs::metadata(&refusing)
            .expect("stat the script")
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&refusing, permissions).expect("make it executable");
        // SAFETY: the variable is read only by the call under test, which runs in
        // this test's own process before any other test can observe it.
        unsafe { std::env::set_var("RADAR_SYSTEMCTL", &refusing) };
        let restart = daemon_command("restart");
        let start = server_command("start", "radar");
        unsafe { std::env::remove_var("RADAR_SYSTEMCTL") };
        let _ = std::fs::remove_file(&refusing);

        assert_eq!(restart.1, 1, "{}", restart.0);
        assert!(restart.0.contains("restarting"), "{}", restart.0);
        assert!(restart.0.contains("Job failed"), "{}", restart.0);
        assert_eq!(start.1, 1, "{}", start.0);
        assert!(start.0.contains("radar-tmux@radar.service"), "{}", start.0);
    }

    #[test]
    fn a_server_with_a_pane_is_refused_for_stop_without_touching_its_unit() {
        if Command::new("tmux").arg("-V").output().is_err() {
            return; // tmux is required for this check; the gate runs it where tmux exists.
        }
        // A server this test starts itself is foreign to Radar's unit, which is the
        // point: the refusal happens before any unit is asked to stop.
        let server = format!("radar-stop-gate-{}", std::process::id());
        let started = Command::new("tmux")
            .args([
                "-L",
                &server,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "probe",
            ])
            .output()
            .expect("tmux runs");
        assert!(
            started.status.success(),
            "tmux could not start a probe server"
        );
        let refused = stop_tmux_server(&server);
        discard_server(&server);
        let problem = refused.expect_err("a pane refuses the stop");
        assert!(problem.contains("holds 1 pane"), "{problem}");
        assert!(problem.contains("not stopped"), "{problem}");
    }

    #[test]
    fn a_server_with_no_session_holds_no_panes_rather_than_failing() {
        if Command::new("tmux").arg("-V").output().is_err() {
            return; // tmux is required for this check; the gate runs it where tmux exists.
        }
        let server = format!("radar-empty-probe-{}", std::process::id());
        let run = |arguments: &[&str]| {
            Command::new("tmux")
                .args(["-L", &server])
                .args(arguments)
                .output()
                .expect("tmux runs")
        };
        run(&["-f", "/dev/null", "new-session", "-d", "-s", "probe"]);
        // Kept alive after its last session closes, which is how Radar's own unit
        // is configured: an empty server is a state this must report, not refuse.
        run(&["set-option", "-g", "exit-empty", "off"]);
        run(&["kill-session", "-t", "probe"]);
        let panes = server_panes(&server);
        discard_server(&server);
        assert_eq!(
            panes.expect("an empty server is readable"),
            Some(Vec::new()),
            "a running server with no session holds no panes"
        );
    }

    #[test]
    fn an_unknown_action_is_refused_with_exit_one() {
        assert_eq!(daemon_command("explode").1, 1);
        assert_eq!(server_command("explode", "radar").1, 1);
    }
}
