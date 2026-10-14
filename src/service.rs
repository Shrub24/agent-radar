//! The user units that own the control daemon and its tmux server, and the
//! commands that start and stop them.
//!
//! Systemd is the only authority that starts or stops either unit. The daemon
//! asks for its tmux server through [`ensure_tmux_server`]; the CLI wraps
//! `systemctl --user` for everything else. Nothing here signals a process
//! directly.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// The socket unit that holds the control socket path and auto-starts the daemon.
pub const SOCKET_UNIT: &str = "radar-control.socket";
/// The daemon's service unit, started by the socket on the first connection.
pub const DAEMON_UNIT: &str = "radar-daemon.service";

/// How long a started tmux server may take to answer on its socket.
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(5);

/// The template instance unit that runs the tmux server `server`.
pub fn tmux_unit(server: &str) -> String {
    format!("radar-tmux@{server}.service")
}

/// Runs `systemctl --user` with `arguments`.
pub fn systemctl(arguments: &[&str]) -> Result<Output, String> {
    Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .map_err(|error| format!("systemctl --user: {error}"))
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
    Command::new("tmux")
        .args(["-L", server, "show-options", "-g"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Ensures the tmux server `server` is running under its own unit, and that
/// Radar is the one serving it.
///
/// A server that answers while its unit is not active was not started by Radar,
/// so it is refused and nothing is started. Otherwise the unit is started and
/// the server must answer within [`SERVER_START_TIMEOUT`].
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
    let started = systemctl(&["start", &unit])?;
    if !started.status.success() {
        return Err(format!(
            "starting {unit}: {}",
            String::from_utf8_lossy(&started.stderr).trim()
        ));
    }
    let deadline = Instant::now() + SERVER_START_TIMEOUT;
    while !server_answers(server) {
        if Instant::now() >= deadline {
            return Err(format!(
                "{unit} started but tmux server `{server}` does not answer on its socket"
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// The panes `server` holds, live or retained, or `None` when no server answers.
pub fn server_panes(server: &str) -> Result<Option<Vec<String>>, String> {
    if !server_answers(server) {
        return Ok(None);
    }
    let listing = Command::new("tmux")
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
        return Err(format!(
            "tmux server `{server}`: {}",
            String::from_utf8_lossy(&listing.stderr).trim()
        ));
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
    let stopped = systemctl(&["stop", &unit])?;
    if stopped.status.success() {
        Ok(format!("stopped {unit}"))
    } else {
        Err(format!(
            "stopping {unit}: {}",
            String::from_utf8_lossy(&stopped.stderr).trim()
        ))
    }
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
    let started = systemctl(&["start", SOCKET_UNIT])?;
    if started.status.success() {
        Ok(())
    } else {
        Err(format!(
            "starting {SOCKET_UNIT}: {}",
            String::from_utf8_lossy(&started.stderr).trim()
        ))
    }
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
fn explain_states(socket: &str, daemon: &str, problem: &str) -> String {
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
    let result = match action {
        "start" => start_control_socket().map(|()| "started the control socket".to_string()),
        "stop" => {
            // The socket goes first: with it stopped nothing can reconnect the
            // service between the two steps.
            systemctl(&["stop", SOCKET_UNIT])
                .and_then(|_| systemctl(&["stop", DAEMON_UNIT]))
                .map(|_| "stopped the control socket and daemon".to_string())
        }
        "restart" => systemctl(&["restart", DAEMON_UNIT]).map(|_| "restarted the daemon".into()),
        "status" => Ok(format!(
            "{SOCKET_UNIT}: {}\n{DAEMON_UNIT}: {}",
            unit_state(SOCKET_UNIT),
            unit_state(DAEMON_UNIT)
        )),
        other => Err(format!("unknown daemon action `{other}`")),
    };
    settle(result)
}

/// Runs one `radar server` action for `server` and returns the lines to print and
/// the exit code.
pub fn server_command(action: &str, server: &str) -> (String, i32) {
    let unit = tmux_unit(server);
    let result = match action {
        "start" => systemctl(&["start", &unit]).map(|_| format!("started {unit}")),
        "stop" => stop_tmux_server(server),
        "restart" => refuse_while_busy(server, "restarted")
            .and_then(|()| systemctl(&["restart", &unit]))
            .map(|_| format!("restarted {unit}")),
        "status" => {
            let panes = match server_panes(server) {
                Ok(Some(panes)) => format!("{} pane(s)", panes.len()),
                Ok(None) => "no server answering".to_string(),
                Err(problem) => problem,
            };
            Ok(format!("{unit}: {}\n{panes}", unit_state(&unit)))
        }
        other => Err(format!("unknown server action `{other}`")),
    };
    settle(result)
}

/// The unit's state as systemd names it, or why it could not be asked.
fn unit_state(unit: &str) -> String {
    match systemctl(&["show", "--property=ActiveState", "--value", unit]) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
        Err(problem) => problem,
    }
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
    use super::*;

    #[test]
    fn the_tmux_unit_is_named_for_its_server() {
        assert_eq!(tmux_unit("radar"), "radar-tmux@radar.service");
    }

    #[test]
    fn a_stopped_socket_and_a_failed_unit_are_named_differently() {
        assert!(explain_states("inactive", "inactive", "refused").starts_with("daemon stopped"));
        assert!(
            explain_states("active", "failed", "refused").starts_with("daemon failed to start")
        );
        assert!(
            explain_states("failed", "inactive", "refused").starts_with("daemon failed to start")
        );
        // A state nothing could be read for keeps the connection's own reason.
        assert_eq!(explain_states("unknown", "unknown", "refused"), "refused");
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
        let _ = Command::new("tmux")
            .args(["-L", &server, "kill-server"])
            .output();
        let problem = refused.expect_err("a pane refuses the stop");
        assert!(problem.contains("holds 1 pane"), "{problem}");
        assert!(problem.contains("not stopped"), "{problem}");
    }

    #[test]
    fn an_unknown_action_is_refused_with_exit_one() {
        assert_eq!(daemon_command("explode").1, 1);
        assert_eq!(server_command("explode", "radar").1, 1);
    }
}
