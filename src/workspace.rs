//! Experimental workspace host. The outer tmux owns presentation only; agent
//! sessions remain in the daemon's existing server.

use std::io;
use std::process::Command;

use crate::config::SidebarConfig;

pub fn launch(backend: &str, session: Option<&str>) -> io::Result<()> {
    let tmux = crate::service::program("RADAR_TMUX", "tmux");
    let radar = std::env::current_exe()?;
    let radar = radar.to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let host = format!("radar-ui-{}-{nonce}", std::process::id());
    let run = |arguments: &[&str]| -> io::Result<()> {
        let output = Command::new(&tmux)
            .args(["-L", &host, "-f", "/dev/null"])
            .args(arguments)
            .env_remove("TMUX")
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    };
    // The agents' server is the daemon's unit: never start it from here, so a
    // missing one is reported, not replaced by a stray server of the same name.
    let sessions = Command::new(&tmux)
        .args(["-L", backend, "list-sessions", "-F", "#{session_name}"])
        .output()?;
    if !sessions.status.success() {
        return Err(io::Error::other(format!(
            "no tmux server '{backend}' is running; start it with `radar server start`"
        )));
    }
    // Attach to the named session, or the first one, creating it only when the
    // server has none, so opening a workspace works from a fresh server.
    let first = String::from_utf8_lossy(&sessions.stdout)
        .lines()
        .next()
        .map(str::to_string);
    let session = session
        .map(str::to_string)
        .or(first)
        .unwrap_or_else(|| "main".into());
    let session = session.as_str();
    let result = (|| {
        run(&[
            "new-session",
            "-d",
            "-s",
            "workspace",
            "-x",
            "160",
            "-y",
            "48",
            "-e",
            &format!("RADAR_WORKSPACE_SERVER={host}"),
            "-e",
            "RADAR_WORKSPACE_SIDEBAR=%0",
            &radar,
            "sidebar",
        ])?;
        for options in [
            ["set-option", "-g", "status", "off"],
            ["set-option", "-g", "prefix", "None"],
            ["set-option", "-g", "mouse", "on"],
            ["set-option", "-g", "set-clipboard", "on"],
            ["set-option", "-g", "extended-keys", "on"],
            ["set-option", "-g", "extended-keys-format", "csi-u"],
            ["set-option", "-g", "allow-passthrough", "on"],
            // An inert host: it adds no latency and says nothing. Every key
            // reaches the pane under it except the few bound below.
            ["set-option", "-g", "escape-time", "0"],
            ["set-option", "-g", "focus-events", "on"],
            ["set-option", "-g", "prefix2", "None"],
            ["set-option", "-g", "bell-action", "none"],
            ["set-option", "-g", "visual-bell", "off"],
            ["set-option", "-g", "visual-activity", "off"],
            ["set-option", "-g", "visual-silence", "off"],
            ["set-option", "-g", "display-time", "1"],
        ] {
            run(&options)?;
        }
        // The host's own client is the terminal (Kitty, `xterm-kitty`), which
        // tmux does not credit with extended keys by itself. Without the
        // feature it never asks the terminal for them and Shift+Enter arrives
        // as a plain Enter.
        run(&["set-option", "-as", "terminal-features", "xterm*:extkeys"])?;
        run(&["unbind-key", "-a", "-T", "prefix"])?;
        let mut attach = vec![
            "split-window",
            "-h",
            "-p",
            "75",
            "-t",
            "%0",
            "env",
            "-u",
            "TMUX",
        ];
        let tmux_word = tmux.to_string_lossy();
        attach.extend([
            &tmux_word,
            "-L",
            backend,
            "new-session",
            "-A",
            "-s",
            session,
        ]);
        run(&attach)?;
        for binding in bindings() {
            run(&binding)?;
        }
        // A workspace nobody is attached to has nothing left to show. Set once a
        // client has attached: tmux destroys an unattached session on its next
        // loop, so setting it earlier removes the workspace before it opens.
        run(&[
            "set-hook",
            "-g",
            "client-attached",
            "set-option -t workspace destroy-unattached on",
        ])?;
        run(&["select-pane", "-t", "%1"])?;
        let status = Command::new(&tmux)
            .args(["-L", &host, "attach-session", "-t", "workspace"])
            .env_remove("TMUX")
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("workspace attach: {status}")))
        }
    })();
    // This is a private, per-launch UI server. Killing it closes only the
    // sidebar and the inner attach client, not the inner server or its agents.
    let cleanup = run(&["kill-server"]);
    // tmux leaves its socket behind when the server exits, so the file goes with
    // it: one launch, one name, and nothing left in the socket directory.
    let _ = std::fs::remove_file(crate::service::server_socket(&host));
    result.and(cleanup)
}

/// The host's own key bindings.
///
/// A binding's command is a single argument. tmux parses a `;` given as its own
/// argument as a command separator rather than as part of the binding, so the
/// line becomes two commands: the binding takes the first half and the rest runs
/// immediately. That is how `send-keys -t %0 Z` fired on every workspace launch
/// and flipped the sidebar to the full dashboard before anyone pressed a key.
fn bindings() -> [[&'static str; 4]; 4] {
    [
        ["bind-key", "-n", "F8", "detach-client"],
        ["bind-key", "-n", "F10", "select-pane -t %0"],
        // Expand sends the sidebar its own toggle, so the mode and the zoom change
        // together, from either pane.
        [
            "bind-key",
            "-n",
            "F11",
            "select-pane -t %0 ; send-keys -t %0 Z",
        ],
        // Returning to the agents while expanded collapses the sidebar first, so
        // the window and the sidebar's mode change together.
        [
            "bind-key",
            "-n",
            "F12",
            "if-shell -F \"#{window_zoomed_flag}\" \"send-keys -t %0 Z ; select-pane -t %1\" \"select-pane -t %1\"",
        ],
    ]
}

/// Host commands for a sidebar created by this launcher, not a terminal split.
pub fn sidebar_commands() -> Option<SidebarConfig> {
    let host = std::env::var("RADAR_WORKSPACE_SERVER").ok()?;
    let pane = std::env::var("RADAR_WORKSPACE_SIDEBAR").ok()?;
    Some(commands(
        &crate::service::program("RADAR_TMUX", "tmux").to_string_lossy(),
        &host,
        &pane,
    ))
}

fn commands(tmux: &str, host: &str, pane: &str) -> SidebarConfig {
    let resize = |args: &[&str]| {
        Some(
            [
                vec![
                    tmux.to_string(),
                    "-L".into(),
                    host.into(),
                    "resize-pane".into(),
                    "-t".into(),
                    pane.into(),
                ],
                args.iter().map(|arg| arg.to_string()).collect(),
            ]
            .concat(),
        )
    };
    // Zoom and unzoom state what they want rather than toggle, so the mode and the
    // window cannot disagree after the user zooms by hand.
    let zoomed = |then: &str, otherwise: &str| {
        Some(
            [
                tmux,
                "-L",
                host,
                "if-shell",
                "-F",
                "-t",
                pane,
                "#{window_zoomed_flag}",
                then,
                otherwise,
            ]
            .map(String::from)
            .to_vec(),
        )
    };
    let leave = format!("resize-pane -Z -t {pane}");
    SidebarConfig {
        hide_command: resize(&["-x", "1"]),
        show_command: resize(&["-x", "32"]),
        zoom_command: zoomed("", &leave),
        unzoom_command: zoomed(&leave, ""),
        launch_command: None,
    }
}

/// The tmux command that floats `radar modal` over the whole workspace: the
/// outer client, so the sidebar and the agent area both lie behind it. A picker
/// is a list and gets the larger block; a confirmation is a few lines.
pub fn popup_command(radar: &str, key: char, row: Option<&str>) -> Option<Vec<String>> {
    let host = std::env::var("RADAR_WORKSPACE_SERVER").ok()?;
    let pane = std::env::var("RADAR_WORKSPACE_SIDEBAR").ok()?;
    let tmux = crate::service::program("RADAR_TMUX", "tmux");
    Some(popup(
        &tmux.to_string_lossy(),
        &host,
        &pane,
        radar,
        key,
        row,
    ))
}

fn popup(
    tmux: &str,
    host: &str,
    pane: &str,
    radar: &str,
    key: char,
    row: Option<&str>,
) -> Vec<String> {
    let (width, height) = if matches!(key, '?' | 'A') {
        ("76", "20")
    } else {
        ("68", "16")
    };
    let mut command = [
        "-L",
        host,
        "display-popup",
        "-B",
        "-E",
        "-t",
        pane,
        "-w",
        width,
        "-h",
        height,
    ]
    .map(String::from)
    .to_vec();
    command.insert(0, tmux.to_string());
    command.extend([radar.to_string(), "modal".into(), format!("--key={key}")]);
    command.extend(row.map(|row| format!("--row={row}")));
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modal_floats_over_the_outer_client_carrying_the_exact_row() {
        let row = r#"{"Agent":"%5"}"#;
        let command = popup(
            "/bin/tmux",
            "radar-ui-view",
            "%0",
            "/bin/radar",
            'D',
            Some(row),
        );
        assert_eq!(
            &command[..3],
            ["/bin/tmux", "-L", "radar-ui-view"],
            "the outer server, not the agents'"
        );
        assert!(command.contains(&"display-popup".into()) && command.contains(&"-E".into()));
        assert_eq!(
            &command[command.len() - 4..],
            ["/bin/radar", "modal", "--key=D", &format!("--row={row}")]
        );
        let picker = popup("/bin/tmux", "h", "%0", "/bin/radar", '?', None);
        assert!(picker.contains(&"76".into()), "a list gets the wider block");
        assert!(
            !picker.iter().any(|word| word.starts_with("--row")),
            "no row was selected"
        );
    }

    #[test]
    fn a_host_binding_keeps_its_whole_command_as_one_argument() {
        for binding in bindings() {
            assert_eq!(binding[0], "bind-key");
            assert_eq!(binding[1], "-n");
            assert!(
                !binding.contains(&";"),
                "{} is split into two commands, so tmux runs the second one at once",
                binding[2]
            );
        }

        let f11 = bindings()
            .into_iter()
            .find(|binding| binding[2] == "F11")
            .expect("F11 focuses the sidebar and expands it");
        assert_eq!(
            f11[3], "select-pane -t %0 ; send-keys -t %0 Z",
            "the focus and the toggle run in that order when the key is pressed"
        );

        let f12 = bindings()
            .into_iter()
            .find(|binding| binding[2] == "F12")
            .expect("F12 returns to the agents");
        assert!(
            f12[3].contains("-F \"#{window_zoomed_flag}\""),
            "an unquoted format is expanded when the key is bound, leaving if-shell no argument"
        );
    }

    #[test]
    fn host_commands_target_the_outer_server_and_sidebar_not_the_agent_backend() {
        let commands = commands("/bin/tmux", "radar-ui-view", "%0");
        assert_eq!(
            commands.hide_command.unwrap(),
            [
                "/bin/tmux",
                "-L",
                "radar-ui-view",
                "resize-pane",
                "-t",
                "%0",
                "-x",
                "1"
            ]
        );
        assert_eq!(commands.show_command.unwrap().last().unwrap(), "32");
        let zoom = commands.zoom_command.unwrap();
        let unzoom = commands.unzoom_command.unwrap();
        assert_ne!(zoom, unzoom, "each says which state it wants");
        assert_eq!(
            zoom.last().unwrap(),
            "resize-pane -Z -t %0",
            "zoom acts only when not zoomed"
        );
        assert_eq!(
            unzoom[unzoom.len() - 2],
            "resize-pane -Z -t %0",
            "unzoom acts only when zoomed"
        );
    }
}
