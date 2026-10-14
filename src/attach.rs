//! Opening a terminal attached to the tmux server a focus could not reach.
//!
//! A tmux focus needs a client to show it to. With none attached the daemon
//! refuses (see [`crate::runtime::NO_CLIENT`]) and changes nothing, and this is
//! what the dashboard then does: it opens a terminal that attaches to the
//! target's server, because only the dashboard's own environment knows which
//! terminal the user runs and holds the desktop session it needs. The daemon
//! never starts one.

use std::ffi::OsString;
use std::process::{Command, Stdio};

/// The program and leading arguments that run the rest of the line in a new
/// terminal window.
///
/// A configured prefix is used as written, because the flag that precedes the
/// command is not the same for every terminal (`-e`, `--`, `start --`). Without
/// one, `$TERMINAL` is run with `-e`, the convention most terminals share.
pub fn terminal_prefix(
    configured: Option<&[String]>,
    environment: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(prefix) = configured {
        return Ok(prefix.to_vec());
    }
    match environment.map(str::trim).filter(|value| !value.is_empty()) {
        Some(terminal) => Ok(vec![terminal.to_string(), "-e".to_string()]),
        None => Err("no terminal to attach with: set `[tui] terminal` or $TERMINAL".to_string()),
    }
}

/// The server and tmux's own id for the place a handle names, from the handle
/// `tmux:<server>:<id>` the tmux backend gives every place.
pub fn parse_handle(handle: &str) -> Option<(&str, &str)> {
    let rest = handle.strip_prefix("tmux:")?;
    let (server, id) = rest.split_once(':')?;
    (!server.is_empty() && !id.is_empty()).then_some((server, id))
}

/// The command that opens a terminal attached to `id` on `server`.
///
/// tmux resolves a pane, window or session id given to `attach-session -t`, so
/// the target needs no lookup first.
pub fn attach_command(
    prefix: &[String],
    tmux: &OsString,
    server: &str,
    id: &str,
) -> Option<(OsString, Vec<OsString>)> {
    let (program, leading) = prefix.split_first()?;
    let mut arguments: Vec<OsString> = leading.iter().map(OsString::from).collect();
    arguments.extend([
        tmux.clone(),
        OsString::from("-L"),
        OsString::from(server),
        OsString::from("attach-session"),
        OsString::from("-t"),
        OsString::from(id),
    ]);
    Some((OsString::from(program), arguments))
}

/// Opens the terminal for a focus target and waits for it to close.
///
/// This blocks until the terminal exits, so it runs on its own thread. A terminal
/// that fails to start, or exits at once with an error, is reported.
pub fn open_terminal(prefix: &[String], handle: &str) -> Result<(), String> {
    let (server, id) =
        parse_handle(handle).ok_or_else(|| format!("{handle} is not a tmux place"))?;
    let tmux = crate::service::program("RADAR_TMUX", "tmux");
    let (program, arguments) = attach_command(prefix, &tmux, server, id)
        .ok_or_else(|| "the configured terminal is empty".to_string())?;
    let mut child = Command::new(&program)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", program.to_string_lossy()))?;
    let started = std::time::Instant::now();
    let status = child
        .wait()
        .map_err(|error| format!("{}: {error}", program.to_string_lossy()))?;
    // A terminal that stays open for a while was used; one that fails at once did
    // not start, and the user is told rather than left waiting.
    if !status.success() && started.elapsed() < std::time::Duration::from_secs(3) {
        return Err(format!(
            "{} exited at once with {status}: check `[tui] terminal`",
            program.to_string_lossy()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn a_configured_prefix_wins_over_the_environment() {
        let configured = words(&["wezterm", "start", "--"]);
        assert_eq!(
            terminal_prefix(Some(&configured), Some("foot")).unwrap(),
            configured
        );
    }

    #[test]
    fn the_environment_terminal_runs_with_dash_e() {
        assert_eq!(
            terminal_prefix(None, Some(" foot ")).unwrap(),
            words(&["foot", "-e"])
        );
    }

    #[test]
    fn no_terminal_is_an_error_that_names_both_settings() {
        for environment in [None, Some(""), Some("  ")] {
            let error = terminal_prefix(None, environment).unwrap_err();
            assert!(error.contains("[tui] terminal") && error.contains("$TERMINAL"));
        }
    }

    #[test]
    fn a_handle_names_its_server_and_tmux_id() {
        assert_eq!(parse_handle("tmux:radar:%3"), Some(("radar", "%3")));
        assert_eq!(parse_handle("tmux:radar:$0"), Some(("radar", "$0")));
        assert_eq!(parse_handle("wA:p1"), None);
        assert_eq!(parse_handle("tmux::%3"), None);
        assert_eq!(parse_handle("tmux:radar:"), None);
    }

    #[test]
    fn the_attach_command_runs_after_the_prefix_and_names_the_server() {
        let (program, arguments) = attach_command(
            &words(&["wezterm", "start", "--"]),
            &OsString::from("/nix/store/x/bin/tmux"),
            "radar",
            "%3",
        )
        .expect("a command");
        assert_eq!(program, OsString::from("wezterm"));
        let arguments: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            words(&[
                "start",
                "--",
                "/nix/store/x/bin/tmux",
                "-L",
                "radar",
                "attach-session",
                "-t",
                "%3"
            ])
        );
        assert!(attach_command(&[], &OsString::from("tmux"), "radar", "%3").is_none());
    }
}
