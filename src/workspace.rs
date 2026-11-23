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
    // A server with no session is offered a workspace picker rather than a
    // placeholder session nobody asked for: the inner pane waits for the first
    // session to exist, and the sidebar opens the picker. It starts zoomed and
    // expanded because no client is attached yet for a popup to draw on.
    let empty = String::from_utf8_lossy(&sessions.stdout).trim().is_empty();
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
            "-e",
            &format!("RADAR_WORKSPACE_EMPTY={}", u8::from(empty)),
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
        attach.extend(["sh", "-c", WAIT_THEN_ATTACH, "sh", &tmux_word, backend]);
        attach.extend(session);
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
        // The fit is also the window-resized hook, so the first fit here and every
        // later resize of the window follow the same policy.
        let fit = fit_sidebar("%0");
        run(&["set-hook", "-g", "window-resized", &fit])?;
        run(&["set-hook", "-g", "window-unzoomed", &fit])?;
        run(&["if-shell", "-F", "1", &fit])?;
        if empty {
            run(&["resize-pane", "-Z", "-t", "%0"])?;
        } else {
            run(&["select-pane", "-t", "%1"])?;
        }
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

/// Runs in the inner pane: waits until the agents' server has the session (any
/// session when none is named), then attaches to it. A fresh server has none
/// until the operator creates a workspace.
const WAIT_THEN_ATTACH: &str = r#"printf 'waiting for a workspace\n'; until "$1" -L "$2" has-session ${3:+-t "$3"} 2>/dev/null; do sleep 0.5; done; exec "$1" -L "$2" attach-session ${3:+-t "$3"}"#;

/// Whether this sidebar was started over a server with no workspace, so it opens
/// the workspace picker once.
pub fn opens_empty() -> bool {
    std::env::var("RADAR_WORKSPACE_EMPTY").is_ok_and(|value| value == "1")
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

/// The sidebar's share of the window, as a percentage, and the columns that share
/// is kept within.
const SIDEBAR_SHARE: u32 = 25;
const SIDEBAR_MIN: u32 = 32;
const SIDEBAR_MAX: u32 = 60;

/// The sidebar's width policy, as a tmux command list: a quarter of the window,
/// between the bounds. A hidden sidebar (`@radar_hidden`) stays a one-column rail,
/// and a zoomed one is left alone. tmux's resize takes a percentage but no clamp,
/// so each bound is a conditional command. The conditions read only the window and
/// the flags, so one command's resize cannot change which command applies next.
fn fit_sidebar(pane: &str) -> String {
    let hidden = "#{&&:#{@radar_hidden},#{==:#{window_zoomed_flag},0}}";
    let free = "#{&&:#{==:#{@radar_hidden},},#{==:#{window_zoomed_flag},0}}";
    let quarter = format!("#{{e|/:#{{e|*:#{{window_width}},{SIDEBAR_SHARE}}},100}}");
    format!(
        "if-shell -F -t {pane} '{hidden}' 'resize-pane -t {pane} -x 1' ; \
         if-shell -F -t {pane} '{free}' 'resize-pane -t {pane} -x {SIDEBAR_SHARE}%' ; \
         if-shell -F -t {pane} '#{{&&:{free},#{{e|<:{quarter},{SIDEBAR_MIN}}}}}' 'resize-pane -t {pane} -x {SIDEBAR_MIN}' ; \
         if-shell -F -t {pane} '#{{&&:{free},#{{e|>:{quarter},{SIDEBAR_MAX}}}}}' 'resize-pane -t {pane} -x {SIDEBAR_MAX}'"
    )
}

fn commands(tmux: &str, host: &str, pane: &str) -> SidebarConfig {
    let argv = |args: &[&str]| {
        Some(
            [tmux, "-L", host]
                .into_iter()
                .chain(args.iter().copied())
                .map(String::from)
                .collect(),
        )
    };
    // tmux runs a command list only as an `if-shell` branch, so a list given as one
    // argument goes through a condition that always holds.
    let list = |commands: &str| argv(&["if-shell", "-F", "1", commands]);
    // Zoom and unzoom state what they want rather than toggle, so the mode and the
    // window cannot disagree after the user zooms by hand.
    let zoomed = |then: &str, otherwise: &str| {
        argv(&[
            "if-shell",
            "-F",
            "-t",
            pane,
            "#{window_zoomed_flag}",
            then,
            otherwise,
        ])
    };
    let fit = fit_sidebar(pane);
    SidebarConfig {
        // The flag is what the fit reads, so a hidden sidebar is not refitted by a
        // resize of the window and is not the sidebar's width.
        hide_command: argv(&[
            "resize-pane",
            "-t",
            pane,
            "-x",
            "1",
            ";",
            "set-option",
            "-p",
            "-t",
            pane,
            "@radar_hidden",
            "1",
        ]),
        show_command: list(&format!("set-option -p -t {pane} -u @radar_hidden ; {fit}")),
        zoom_command: zoomed("", &format!("resize-pane -Z -t {pane}")),
        unzoom_command: zoomed(&format!("resize-pane -Z -t {pane} ; {fit}"), ""),
        // Unzooms only when zoomed, so a sidebar that was only narrowed keeps its width.
        focus_command: argv(&[
            "if-shell",
            "-F",
            "-t",
            pane,
            "#{window_zoomed_flag}",
            &format!("resize-pane -Z -t {pane} ; {fit} ; select-pane -t %1"),
            "select-pane -t %1",
        ]),
        launch_command: None,
    }
}

/// Launches a presenter addressed by interaction identity, never by a synthetic key.
pub fn presenter_command(radar: &str, interaction: &str) -> Option<Vec<String>> {
    let host = std::env::var("RADAR_WORKSPACE_SERVER").ok()?;
    let pane = std::env::var("RADAR_WORKSPACE_SIDEBAR").ok()?;
    let tmux = crate::service::program("RADAR_TMUX", "tmux");
    Some(presenter_popup(
        &tmux.to_string_lossy(),
        &host,
        &pane,
        radar,
        interaction,
    ))
}

fn presenter_popup(
    tmux: &str,
    host: &str,
    pane: &str,
    radar: &str,
    interaction: &str,
) -> Vec<String> {
    let mut argv = popup(tmux, host, pane, radar, 'A', None);
    argv.truncate(argv.len() - 3);
    argv.extend([
        radar.to_owned(),
        "present".into(),
        format!("--interaction={interaction}"),
    ]);
    argv
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
    fn a_presenter_popup_carries_only_its_interaction_not_a_key_or_row() {
        let argv = presenter_popup(
            "/bin/tmux",
            "outer",
            "%7",
            "/path with space/radar",
            "interaction-42",
        );
        assert_eq!(
            &argv[..7],
            [
                "/bin/tmux",
                "-L",
                "outer",
                "display-popup",
                "-B",
                "-E",
                "-t"
            ]
        );
        assert_eq!(
            &argv[argv.len() - 3..],
            [
                "/path with space/radar",
                "present",
                "--interaction=interaction-42"
            ]
        );
        assert!(
            !argv
                .iter()
                .any(|arg| arg.starts_with("--key") || arg.starts_with("--row"))
        );
    }

    /// A disposable tmux server laid out as `launch` lays out the workspace: the
    /// sidebar `%0` beside the agent pane `%1`, fitted and hooked. Without tmux the
    /// test says so and passes without checking anything.
    struct Workspace(String);

    impl Workspace {
        fn start(test: &str) -> Option<Self> {
            if Command::new("tmux").arg("-V").output().is_err() {
                eprintln!("skipping {test}: no tmux on PATH");
                return None;
            }
            let workspace = Self(format!("radar-ws-{test}-{}", std::process::id()));
            workspace.tmux(&[
                "new-session",
                "-d",
                "-s",
                "w",
                "-x",
                "160",
                "-y",
                "48",
                "sleep 600",
            ]);
            workspace.tmux(&["split-window", "-h", "-p", "75", "-t", "%0", "sleep 600"]);
            let fit = fit_sidebar("%0");
            workspace.tmux(&["set-hook", "-g", "window-resized", fit.as_str()]);
            workspace.tmux(&["if-shell", "-F", "1", fit.as_str()]);
            Some(workspace)
        }

        fn tmux(&self, args: &[&str]) -> String {
            let output = Command::new("tmux")
                .args(["-L", &self.0, "-f", "/dev/null"])
                .args(args)
                .env_remove("TMUX")
                .output()
                .expect("tmux runs");
            assert!(
                output.status.success(),
                "tmux {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        fn host_commands(&self) -> SidebarConfig {
            commands("tmux", &self.0, "%0")
        }

        /// Runs one of the host commands the sidebar would run.
        fn run(&self, command: &Option<Vec<String>>) {
            let command = command.as_ref().expect("the command is set");
            let status = Command::new(&command[0])
                .args(&command[1..])
                .env_remove("TMUX")
                .status()
                .expect("the host command runs");
            assert!(status.success(), "{command:?}");
        }

        fn width(&self, pane: &str) -> u32 {
            self.tmux(&["display", "-p", "-t", pane, "#{pane_width}"])
                .parse()
                .expect("a width")
        }

        fn window(&self, format: &str) -> String {
            self.tmux(&["display", "-p", "-t", "w", format])
        }

        fn resize_window(&self, columns: u32) {
            let columns = columns.to_string();
            self.tmux(&["resize-window", "-x", &columns, "-y", "48", "-t", "w"]);
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .args(["-L", &self.0, "kill-server"])
                .output();
        }
    }

    #[test]
    fn the_sidebar_holds_a_quarter_of_the_window_between_its_bounds() {
        let Some(workspace) = Workspace::start("quarter") else {
            return;
        };
        assert_eq!(workspace.width("%0"), 40, "a quarter of the launch size");
        for (columns, sidebar) in [(100, 32), (200, 50), (400, 60), (160, 40)] {
            workspace.resize_window(columns);
            assert_eq!(
                workspace.width("%0"),
                sidebar,
                "a window of {columns} columns"
            );
        }
    }

    #[test]
    fn a_hidden_sidebar_stays_a_rail_through_a_resize_and_shows_again() {
        let Some(workspace) = Workspace::start("rail") else {
            return;
        };
        let commands = workspace.host_commands();
        workspace.run(&commands.hide_command);
        assert_eq!(workspace.width("%0"), 1);
        workspace.resize_window(200);
        assert_eq!(
            workspace.width("%0"),
            1,
            "a resize does not unhide the rail"
        );
        workspace.run(&commands.show_command);
        assert_eq!(workspace.width("%0"), 50, "show restores the policy width");
    }

    #[test]
    fn an_expanded_sidebar_stays_expanded_through_a_resize_and_refits_when_it_returns() {
        let Some(workspace) = Workspace::start("expand") else {
            return;
        };
        let commands = workspace.host_commands();
        workspace.run(&commands.zoom_command);
        assert_eq!(workspace.window("#{window_zoomed_flag}"), "1");
        workspace.resize_window(120);
        assert_eq!(
            workspace.window("#{window_zoomed_flag}"),
            "1",
            "a resize does not end the expansion"
        );
        workspace.run(&commands.unzoom_command);
        assert_eq!(workspace.window("#{window_zoomed_flag}"), "0");
        assert_eq!(
            workspace.width("%0"),
            32,
            "a quarter of 120 columns, held at the bound"
        );
    }

    #[test]
    fn a_focus_hands_the_keyboard_to_the_agent_pane_and_unzooms_an_expanded_sidebar() {
        let Some(workspace) = Workspace::start("focus") else {
            return;
        };
        let commands = workspace.host_commands();
        let active = || workspace.window("#{pane_id}");
        workspace.tmux(&["select-pane", "-t", "%0"]);
        workspace.tmux(&["resize-pane", "-t", "%0", "-x", "45"]);
        workspace.run(&commands.focus_command);
        assert_eq!(active(), "%1");
        assert_eq!(
            workspace.width("%0"),
            45,
            "a focus does not refit a sidebar that was not expanded"
        );

        workspace.tmux(&["select-pane", "-t", "%0"]);
        workspace.tmux(&["resize-pane", "-Z", "-t", "%0"]);
        workspace.run(&commands.focus_command);
        assert_eq!(active(), "%1");
        assert_eq!(workspace.window("#{window_zoomed_flag}"), "0");
        assert_eq!(workspace.width("%0"), 40, "unzoomed onto the policy width");
    }
}
