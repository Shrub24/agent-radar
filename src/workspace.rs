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
    // Check the existing backend before creating any presentation state. Never
    // start a shell, agent or backend server as a side effect of opening a view.
    let mut check = Command::new(&tmux);
    check.args(["-L", backend, "has-session"]);
    if let Some(session) = session {
        check.args(["-t", session]);
    }
    let output = check.output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
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
            ["set-option", "-g", "allow-passthrough", "on"],
        ] {
            run(&options)?;
        }
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
        attach.extend([&tmux_word, "-L", backend, "attach-session"]);
        if let Some(session) = session {
            attach.extend(["-t", session]);
        }
        run(&attach)?;
        run(&[
            "bind-key",
            "-n",
            "F9",
            "display-popup",
            "-E",
            "-w",
            "85%",
            "-h",
            "85%",
            &radar,
        ])?;
        run(&["bind-key", "-n", "F10", "select-pane", "-t", "%0"])?;
        run(&["bind-key", "-n", "F12", "select-pane", "-t", "%1"])?;
        run(&["bind-key", "-n", "F11", "resize-pane", "-Z", "-t", "%0"])?;
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
    result.and(cleanup)
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
    SidebarConfig {
        hide_command: resize(&["-x", "1"]),
        show_command: resize(&["-x", "32"]),
        zoom_command: resize(&["-Z"]),
        unzoom_command: resize(&["-Z"]),
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
        assert_eq!(commands.zoom_command, commands.unzoom_command);
    }
}
