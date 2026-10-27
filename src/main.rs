use std::io;
use std::time::{Duration, Instant};

use agent_radar::control_plane::public_contracts::ui_session::AttachmentRole;
use agent_radar::ui_session::client::{UiClient, Wake};
use agent_radar::ui_session::publication::Observation;
use agent_radar::{
    Action, App, Config, Geometry, ObservationState, PaneView, control_plane, theme, ui,
};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use futures_util::StreamExt;
use ratatui::widgets::ListState;

fn main() -> io::Result<()> {
    let cli = <agent_radar::cli::Cli as clap::Parser>::parse();
    if cli.print_config {
        print!("{}", load_config().to_document());
        return Ok(());
    }
    if let Some(agent_radar::cli::Invocation::Present { interaction }) = &cli.invocation {
        let result = run_presenter(interaction);
        disable_mouse_capture();
        return result.and(ratatui::try_restore());
    }
    let sidebar = match &cli.invocation {
        None => Some(false),
        Some(agent_radar::cli::Invocation::Sidebar) => Some(true),
        Some(_) => None,
    };
    if let Some(sidebar) = sidebar {
        let result = run(sidebar);
        disable_mouse_capture();
        let restored = ratatui::try_restore();
        return result.and(restored);
    }
    let invocation = cli.invocation.expect("a non-dashboard invocation");
    std::process::exit(invoke(invocation))
}

/// Carries out one invocation that prints its answer and exits with a code.
fn invoke(invocation: agent_radar::cli::Invocation) -> i32 {
    use agent_radar::cli::Invocation;
    match invocation {
        Invocation::Daemon { action } => match action {
            Some(action) => {
                let (message, code) = agent_radar::service::daemon_command(action.as_str());
                println!("{message}");
                code
            }
            None => serve_daemon(),
        },
        Invocation::Workspace { session } => {
            match agent_radar::workspace::launch(
                &load_config().server.tmux_server,
                session.as_deref(),
            ) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("radar workspace: {error}");
                    1
                }
            }
        }
        Invocation::Sidebar | Invocation::Present { .. } => {
            unreachable!("these are dashboards, not printing commands")
        }
        Invocation::Server(action) => {
            let (verb, name) = action.parts();
            let name = match name {
                Some(name) => name.to_string(),
                None => load_config().server.tmux_server.clone(),
            };
            let (message, code) = agent_radar::service::server_command(verb, &name);
            println!("{message}");
            code
        }
        Invocation::Lifecycle(action) => run_lifecycle_command(action),
        Invocation::Reset { hard, yes } => {
            let server = load_config().server.tmux_server;
            agent_radar::reset::run(&server, hard, yes)
        }
        Invocation::Completions { shell } => {
            // Rendered first: a reader that stops early (`radar completions fish
            // | head`) closes the pipe, and a half-written script is the reader's
            // choice rather than an error worth a panic.
            let mut command = <agent_radar::cli::Cli as clap::CommandFactory>::command();
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut command, "radar", &mut script);
            match std::io::Write::write_all(&mut std::io::stdout(), &script) {
                Ok(()) => 0,
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
                Err(error) => {
                    eprintln!("radar completions: {error}");
                    1
                }
            }
        }
        Invocation::Run {
            launch: true,
            socket: Some(socket),
            ..
        } => control_plane::lifecycle::run_launch(&socket),
        Invocation::Run { agent, program, .. } => {
            let socket = agent_radar::control_plane::socket_path();
            let kind = if agent {
                control_plane::lifecycle::ProcessKind::Agent
            } else {
                control_plane::lifecycle::ProcessKind::Command
            };
            control_plane::lifecycle::run_managed(&socket.to_string_lossy(), kind, &program)
        }
        Invocation::LifecycleWake {
            socket,
            subject,
            server,
        } => control_plane::lifecycle::run_wake(&socket, &subject, &server),
    }
}

/// Runs the host command that hides the sidebar, or unhides it while the rail is
/// drawn, or zooms it to the window. The terminal owns the split, so a missing or
/// failing command is only reported.
/// Which host command hides or unhides the sidebar now: the terminal's own width
/// decides, because it owns the split and the rail is what a narrow one draws.
fn sidebar_resize<'a>(config: &'a Config, app: &App) -> (&'static str, &'a Option<Vec<String>>) {
    if app.is_sidebar() && app.sidebar_railed() {
        ("show_command", &config.sidebar.show_command)
    } else {
        ("hide_command", &config.sidebar.hide_command)
    }
}

/// Which host command gives the sidebar the window or takes it back: the mode
/// chooses, and the width follows it.
fn sidebar_zoom<'a>(config: &'a Config, app: &App) -> (&'static str, &'a Option<Vec<String>>) {
    if app.is_expanded() {
        ("unzoom_command", &config.sidebar.unzoom_command)
    } else {
        ("zoom_command", &config.sidebar.zoom_command)
    }
}

fn run_host_command(
    key: &str,
    command: &Option<Vec<String>>,
    app: &mut App,
    pending: &mut Option<std::process::Child>,
) -> bool {
    if pending.is_some() {
        app.set_focus_message(Some("a sidebar host command is still running".into()));
        return false;
    }
    let Some(command) = command else {
        app.set_focus_message(Some(format!("set [sidebar] {key}")));
        return false;
    };
    let result = std::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match result {
        Ok(child) => {
            *pending = Some(child);
            true
        }
        Err(error) => {
            app.set_focus_message(Some(format!("sidebar {key}: {error}")));
            false
        }
    }
}

/// Reaps a finished host command. A pending expand or return takes effect only
/// when the host reports success: the mode follows the window, not the attempt.
fn poll_sidebar_command(
    pending: &mut Option<std::process::Child>,
    flip: &mut bool,
    app: &mut App,
) -> bool {
    let Some(child) = pending.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(None) => return false,
        Ok(Some(status)) if status.success() => {
            if std::mem::take(flip) {
                app.toggle_expanded();
            }
        }
        Ok(Some(status)) => {
            app.set_focus_message(Some(format!("sidebar host command exited {status}")))
        }
        Err(error) => app.set_focus_message(Some(format!("sidebar host command: {error}"))),
    }
    *flip = false;
    *pending = None;
    true
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;

    #[test]
    fn host_commands_report_failure_without_blocking_the_dashboard() {
        let mut app = App::sidebar();
        let mut pending = None;
        let mut config = Config::default();
        run_host_command(
            "hide_command",
            &config.sidebar.hide_command,
            &mut app,
            &mut pending,
        );
        assert!(pending.is_none());
        assert!(
            app.focus_message()
                .unwrap()
                .contains("[sidebar] hide_command")
        );

        let command = vec!["sh".into(), "-c".into(), "exit 7".into()];
        config.sidebar.hide_command = Some(command.clone());
        config.sidebar.zoom_command = Some(command);
        run_host_command(
            "hide_command",
            &config.sidebar.hide_command,
            &mut app,
            &mut pending,
        );
        assert!(pending.is_some(), "the UI receives a child, not its exit");
        run_host_command(
            "hide_command",
            &config.sidebar.hide_command,
            &mut app,
            &mut pending,
        );
        assert!(app.focus_message().unwrap().contains("still running"));
        pending.as_mut().unwrap().wait().unwrap();
        assert!(poll_sidebar_command(&mut pending, &mut false, &mut app));
        assert!(pending.is_none());
        assert!(app.focus_message().unwrap().contains("exited"));
    }

    #[test]
    fn the_width_chooses_hide_or_show_and_the_mode_chooses_zoom_or_unzoom() {
        let config = Config::default();
        let mut app = App::sidebar();
        app.note_width(30);
        assert_eq!(sidebar_resize(&config, &app).0, "hide_command");
        app.note_width(10);
        assert_eq!(sidebar_resize(&config, &app).0, "show_command");

        assert_eq!(sidebar_zoom(&config, &app).0, "zoom_command");
        app.toggle_expanded();
        app.note_width(30);
        assert_eq!(
            sidebar_zoom(&config, &app).0,
            "unzoom_command",
            "the mode, not the width, says it is expanded"
        );
        assert!(
            !app.sidebar_compact(),
            "an expanded sidebar is the dashboard"
        );
        app.note_width(10);
        assert!(!app.sidebar_railed(), "and cannot be the rail");
        assert!(!App::new().sidebar_compact(), "the dashboard never is");
    }

    #[test]
    fn a_host_command_that_cannot_run_leaves_the_mode_alone() {
        let mut app = App::sidebar();
        let mut pending = None;
        assert!(!run_host_command(
            "zoom_command",
            &None,
            &mut app,
            &mut pending
        ));
        assert!(!app.is_expanded());
        assert!(app.focus_message().unwrap().contains("zoom_command"));
    }
}

/// `radar lifecycle`: the operator's end of unresolved-operation recovery.
///
/// `reconcile` re-reads current evidence and reports what it could establish and
/// what it could not, with a reason. `abandon` ends one unresolved record's
/// recovery obligation, releasing only the administrative block it holds; the
/// uncertainty stays recorded and no effect is dispatched.
fn run_lifecycle_command(action: agent_radar::cli::LifecycleAction) -> i32 {
    use agent_radar::cli::LifecycleAction;
    let runtime = match agent_radar::control_plane::connect() {
        Ok(selected) => selected,
        Err(error) => {
            eprintln!("radar lifecycle: {error}");
            return 1;
        }
    };
    let Some(lifecycle) = runtime.lifecycle else {
        eprintln!("radar lifecycle: the daemon serves no lifecycle contract");
        return 1;
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match action {
        LifecycleAction::Reconcile => {
            let answer = match agent_radar::control_plane::client::reconcile(&cancel) {
                Ok(answer) => answer,
                Err(error) => {
                    eprintln!("radar lifecycle reconcile: {error}");
                    return 1;
                }
            };
            println!("{answer}");
            0
        }
        LifecycleAction::Abandon { request_id: held } => {
            let request_id = agent_radar::control_plane::random_uuid();
            match lifecycle.abandon_record(&request_id, &held, &cancel) {
                Ok(answer) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "request_id": held,
                            "outcome": answer.outcome(),
                            "message": answer.message(),
                        })
                    );
                    0
                }
                Err(error) => {
                    eprintln!("radar lifecycle abandon: {error}");
                    1
                }
            }
        }
    }
}

/// Exit status for state this build cannot read. It is `EX_CONFIG` from
/// `sysexits.h`, and the unit's `RestartPreventExitStatus` names it so a broken
/// journal produces one explained failure instead of a restart loop.
const UNUSABLE_STATE_EXIT: i32 = 78;

/// Serves the control plane until the process is signalled, and reports the
/// socket it is on before it starts waiting.
///
/// Radar itself never starts this process: the daemon is the operator's to run,
/// and a TUI that spawned a long-lived server on someone's behalf would be a
/// surprise on their machine. The exit code is the shell's to read, so a daemon
/// that could not bind says so and stops rather than serving nothing quietly.
fn serve_daemon() -> i32 {
    agent_radar::log::init_daemon();
    if let Err(diagnostic) = agent_radar::install_signal_handlers() {
        eprintln!("radar daemon: {diagnostic}");
        return 1;
    }
    let mut daemon = match agent_radar::Daemon::bind() {
        Ok(daemon) => daemon,
        Err(diagnostic) => {
            eprintln!("radar daemon: {diagnostic}");
            if agent_radar::control_plane::events::is_unreadable_state(&diagnostic) {
                eprintln!(
                    "radar daemon: this state is not repairable by a restart; run \
                     `radar reset --hard --yes` to discard it"
                );
                return UNUSABLE_STATE_EXIT;
            }
            return 1;
        }
    };
    println!(
        "radar daemon: serving {} with records under {}",
        daemon.path().display(),
        daemon.state_dir().display()
    );
    match daemon.run() {
        Ok(()) => 0,
        Err(diagnostic) => {
            eprintln!("radar daemon: {diagnostic}");
            1
        }
    }
}

/// The configuration for this run.
///
/// A file that cannot be used is reported rather than ignored — a mistyped
/// colour would otherwise leave the user editing a document with no effect —
/// but it does not stop the dashboard: the built-in colours still work.
fn load_config() -> Config {
    match Config::load() {
        Ok(config) => config,
        Err(diagnostic) => {
            eprintln!("radar: {diagnostic}");
            eprintln!("radar: drawing with the built-in colours");
            Config::default()
        }
    }
}

/// Waits for the daemon with the diagnostic on screen. Until it answers, the screen
/// says why it cannot be reached and how to start it, and the connection is retried
/// every two seconds. There is no other source to draw from, so the dashboard never
/// starts without the daemon. `None` means the operator quit while waiting.
///
/// Starting the socket is what brings a stopped daemon back: an explicit stop lasts
/// only until the next `radar` run, which starts it again here.
fn await_daemon(
    terminal: &mut ratatui::DefaultTerminal,
) -> io::Result<Option<control_plane::Selected>> {
    let mut reason = match agent_radar::service::start_control_socket() {
        Ok(()) => "connecting to the control daemon".to_string(),
        Err(problem) => problem,
    };
    let mut next_attempt = Instant::now();
    loop {
        if Instant::now() >= next_attempt {
            next_attempt = Instant::now() + Duration::from_secs(2);
            match control_plane::connect() {
                Ok(selected) => return Ok(Some(selected)),
                Err(problem) => reason = agent_radar::service::explain_unreachable(&problem),
            }
        }
        terminal.draw(|frame| draw_daemon_diagnostic(frame, &reason))?;
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            return Ok(None);
        }
    }
}

/// The screen drawn while the daemon cannot be reached: the reason, and what to do.
/// The diagnostic one source of a snapshot carries, if it has one.
///
/// A source that is stale or unavailable names why, and the footer shows that
/// rather than a fleet-wide verdict the source did not make.
fn source_diagnostic(snapshot: &Observation, name: &str) -> Option<String> {
    snapshot
        .sources
        .iter()
        .find(|source| source.name == name)
        .and_then(|source| match &source.state {
            agent_radar::observation::SourceFreshness::Stale { diagnostic }
            | agent_radar::observation::SourceFreshness::Unavailable { diagnostic } => {
                Some(diagnostic.clone())
            }
            _ => None,
        })
}

fn draw_daemon_diagnostic(frame: &mut ratatui::Frame, reason: &str) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Text};
    use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
    let text = Text::from(vec![
        Line::styled(
            "Radar cannot reach its control daemon.",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(reason.to_string()),
        Line::raw(""),
        Line::raw("Start it with `radar daemon start`, or see `radar daemon status`."),
        Line::raw("Radar keeps retrying. Press q to quit."),
    ]);
    let block = Block::default().borders(Borders::ALL).title(" radar ");
    frame.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        frame.area(),
    );
}

fn run_presenter(interaction: &str) -> io::Result<()> {
    use agent_radar::control_plane::public_contracts::ui_session::{InteractionResult, UiAction};
    use agent_radar::picker::{Item, Picker, Step};
    theme::install(load_config());
    // Apply the same socket-path trust check as the dashboard before attaching.
    let selected = control_plane::client::connect().map_err(io::Error::other)?;
    let daemon = selected
        .observation
        .ok_or_else(|| io::Error::other("daemon has no observation service"))?;
    let wake = Wake::default();
    let mut terminal = ratatui::try_init()?;
    let runtime = session_runtime()?;
    runtime.block_on(async {
    let mut client = UiClient::presenter(
        &tokio::runtime::Handle::current(),
        daemon.socket().to_path_buf(),
        interaction.into(),
        wake.clone(),
    );
    let result = async {
    let mut events = EventStream::new();
    let mut picker = None::<Picker<String>>;
    let mut confirm = None::<agent_radar::ui_session::confirm::Confirm>;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let mut submitting = false;
    let mut attached = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(mirror) = client.poll() {
            let Some(b) = mirror
                .state
                .blocking
                .as_ref()
                .filter(|b| b.interaction_id == interaction)
            else {
                return Ok(());
            };
            if b.presenter.as_deref() == Some(&mirror.attachment_id) {
                attached = true;
                if b.kind == agent_radar::control_plane::public_contracts::ui_session::InteractionKind::Confirm
                    && confirm.is_none()
                    && !submitting
                {
                    confirm = Some(agent_radar::ui_session::confirm::Confirm::new(
                        b.interaction_id.clone(),
                        "Confirm",
                        b.prompt.clone().unwrap_or_default(),
                        b.losses.clone(),
                    ));
                    picker = None;
                } else if picker.is_none() && !submitting && confirm.is_none() {
                    let items = b
                        .choices
                        .iter()
                        .map(|p| Item::new(p.clone(), None::<String>, p.clone()))
                        .collect();
                    picker = Some(Picker::new(
                        "Workspace",
                        b.note
                            .as_ref()
                            .map(|note| format!("{note} · enter open · esc cancel"))
                            .unwrap_or_else(|| "enter open · esc cancel".into()),
                        items,
                    ));
                }
            }
        }
        if attached && !client.connected() {
            return Err(io::Error::other(
                "presenter connection lost; result may have been accepted",
            ));
        }
        if !attached && Instant::now() >= deadline {
            return Err(io::Error::other("interaction is closed or already claimed"));
        }
        terminal.draw(|frame| {
            if let Some(confirm) = &confirm {
                let mut lines = vec![
                    Line::from(Span::styled(
                        format!("{}  {}", confirm.title, confirm.prompt),
                        Style::new().add_modifier(Modifier::BOLD),
                    )),
                    Line::from("This may lose:"),
                ];
                for loss in &confirm.losses {
                    lines.push(Line::from(format!("  {loss}")));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{}[ Confirm ]{}",
                    if confirm.confirm_selected {
                        ""
                    } else {
                        "[ Cancel ]   "
                    },
                    if confirm.confirm_selected {
                        "   [ Cancel ]"
                    } else {
                        ""
                    },
                )));
                lines.push(Line::from("tab switch · enter answer · esc cancel"));
                frame.render_widget(ratatui::widgets::Paragraph::new(lines), frame.area());
            } else if let Some(picker) = &picker {
                ui::picker::draw(frame, picker, true);
            } else {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("Waiting for interaction acknowledgement…"),
                    frame.area(),
                );
            }
        })?;
        let event = tokio::select! {
            event = events.next() => Some(event),
            () = wake.woken() => None,
            () = tokio::time::sleep(Duration::from_secs(1)) => None,
        };
        match &event {
            Some(None) => return Ok(()),
            Some(Some(Err(error))) => return Err(io::Error::other(error.to_string())),
            _ => {}
        }
        if let Some(Some(Ok(Event::Key(key)))) = event
            && key.kind != KeyEventKind::Release
            && !submitting
        {
            if let Some(pending) = confirm.as_mut()
                && let Some(answer) = pending.handle_key(key)
            {
                let action = match answer {
                    agent_radar::ui_session::confirm::Answer::Confirm => {
                        UiAction::ResolveInteraction {
                            interaction_id: interaction.into(),
                            result: InteractionResult::Confirm { accepted: true },
                        }
                    }
                    agent_radar::ui_session::confirm::Answer::Cancel => {
                        UiAction::CancelInteraction {
                            interaction_id: interaction.into(),
                        }
                    }
                };
                client.submit(action);
                submitting = true;
                confirm = None;
                continue;
            }
            let step = if key.code == KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                Step::Close
            } else if let Some(picker) = &mut picker {
                picker.handle_key(key)
            } else {
                continue;
            };
            let action = match step {
                Step::Continue => continue,
                Step::Close => UiAction::CancelInteraction {
                    interaction_id: interaction.into(),
                },
                Step::Choose(path) => UiAction::ResolveInteraction {
                    interaction_id: interaction.into(),
                    result: InteractionResult::Project { path },
                },
            };
            client.submit(action);
            submitting = true;
            picker = None;
        }
    }
    }.await;
    client.shutdown().await;
    result
    })
}

fn run(sidebar: bool) -> io::Result<()> {
    let mut config = load_config();
    let workspace_host = sidebar
        .then(agent_radar::workspace::sidebar_commands)
        .flatten();
    let in_workspace = workspace_host.is_some();
    if let Some(commands) = workspace_host {
        config.sidebar = commands;
    }
    theme::install(config.clone());
    // The dashboard owns the terminal, so its diagnostics go to a file.
    let log_note = agent_radar::log::init_dashboard(&agent_radar::log::dashboard_log_path()).err();
    let mut terminal = ratatui::try_init()?;
    enable_mouse_capture();
    // The panic path restores the terminal through the hook `try_init` installed;
    // mouse capture is Radar's own state, so it is released ahead of that hook.
    let restore_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        disable_mouse_capture();
        restore_hook(info);
    }));
    let Some(selected) = await_daemon(&mut terminal)? else {
        return Ok(());
    };
    let (observation, task_diagnostic) = (selected.observation, selected.task_diagnostic);
    // One runtime, shared: the focuser and the session's dispatcher read and act
    // on the same adapter. The dashboard collects nothing itself: the daemon's
    // sampler owns that, and this reads its snapshot.
    // Managed lifecycle actions target daemon-owned subjects and run off the UI thread.
    // A confirmed container control goes to the daemon, one at a time and off the
    // UI thread. A run without a daemon has no container to act on and says so.
    let mut state = ObservationState::new();
    let mut app = if sidebar { App::sidebar() } else { App::new() };
    // Which presentation this process was started in. A sidebar that draws the
    // full dashboard and a dashboard that was asked for look the same on screen,
    // so the log states the mode, the argv that chose it and the width it was
    // given, and the two can be told apart afterwards.
    tracing::info!(
        mode = if app.is_sidebar() { "sidebar" } else { "dashboard" },
        expanded = app.is_expanded(),
        workspace_host = in_workspace,
        argv = ?std::env::args().collect::<Vec<_>>(),
        "dashboard started"
    );
    if let Some(reason) = log_note {
        app.set_focus_message(Some(format!("no dashboard log: {reason}")));
    }
    let mut list = ListState::default();
    app.set_task_diagnostic(task_diagnostic);
    // The daemon samples for every display; this fetcher is how the dashboard
    // reads what it published, and it is absent only from a daemon that does not
    // serve the observation contract, which the diagnostic screen refuses.
    // Every producer wakes the one event loop: terminal input, the daemon's
    // snapshot and the shared UI session.
    let wake = Wake::default();
    let session_socket = observation
        .as_ref()
        .map(|daemon| daemon.socket().to_path_buf());
    let mut displayed_observation: Option<std::sync::Arc<Observation>> = None;
    let runtime = session_runtime()?;
    runtime.block_on(async {
        if let Some(socket) = session_socket {
            let role = if sidebar {
                AttachmentRole::Sidebar
            } else {
                AttachmentRole::Dashboard
            };
            app.attach_session(UiClient::start(
                &tokio::runtime::Handle::current(),
                socket,
                role,
                wake.clone(),
            ));
        }
        let result = async {
            let mut events = EventStream::new();
            // The animation frame, and the clock it is derived from: a mark has to move
            // at the same rate whether or not anything else made the loop redraw.
            let started = Instant::now();
            let mut phase: usize = 0;
            let mut dirty = true;
            // A focus with no client attached opens a terminal and asks again, because a
            // client has to exist before a focus can show anything.
            let mut attach = Attach::default();
            let mut sidebar_command = None;
            let mut flip_on_success = false;
            let mut presentation = agent_radar::ui_session::presentation::Presentation::default();

            loop {
                dirty |= app.apply_session_update();
                let detached = sidebar && in_workspace && app.sidebar_compact();
                dirty |= presentation.poll(&mut app, &config.projects, detached, &wake);
                dirty |= poll_sidebar_command(&mut sidebar_command, &mut flip_on_success, &mut app);
                // Foreground evidence is swept while a pane view is showing panes, and
                // while finished sessions are listed: telling a session that has gone
                // from a pane in use again needs the foreground either way.
                let sweeping = app.pane_view() != PaneView::Hidden || app.shows_history();
                app.wants_processes(sweeping);
                if let Some(observation) = app.session_observation()
                    && displayed_observation
                        .as_ref()
                        .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &observation))
                {
                    app.set_task_diagnostic(source_diagnostic(&observation, "tasks"));
                    state = observation.observation.clone();
                    displayed_observation = Some(observation);
                    dirty = true;
                }
                // The session answers a focus. A server with no client attached is the one
                // answer this process acts on: it opens a terminal for the place it named
                // and asks again, so the retry stays one session command.
                if let Some((row, target)) = app.take_focus_recovery() {
                    attach.ask(&target, row);
                    if let Some(message) = attach.answered(true, &config.tui) {
                        app.set_focus_message(Some(message));
                    }
                    dirty = true;
                }
                if let Some(message) = attach.poll(&mut app) {
                    app.set_focus_message(Some(message));
                    dirty = true;
                }
                // Animation runs on the clock and only while something is working, so
                // a quiet dashboard redraws when it changed and nothing moves on its
                // own; a busy one redraws at the configured rate.
                let fps = theme::config().appearance.fps.max(1);
                let animating = app.animates();
                if animating {
                    let next = (started.elapsed().as_millis() as usize * fps as usize) / 1000;
                    if next != phase {
                        phase = next;
                        dirty = true;
                    }
                } else {
                    phase = 0;
                }
                if dirty {
                    let mut geometry = Geometry::default();
                    app.note_width(terminal.size()?.width);
                    terminal.draw(|frame| {
                        geometry = ui::render(frame, &state, &app, &mut list, phase);
                    })?;
                    app.note_layout(geometry);
                    dirty = false;
                }
                let timeout = if animating {
                    Duration::from_millis(1000 / fps as u64)
                } else {
                    // Snapshots, session updates and input wake the loop themselves.
                    // What remains is work that only answers on its own thread, and
                    // clocks: messages that expire, an attach that retries.
                    let working = attach.waiting.is_some() || sidebar_command.is_some();
                    Duration::from_millis(if working { 50 } else { 250 })
                };
                let event = {
                    let next = tokio::select! {
                        event = events.next() => Some(event),
                        () = wake.woken() => None,
                        () = tokio::time::sleep(timeout) => None,
                    };
                    match next {
                        // Woken by a producer or by the clock: loop to apply it.
                        None => continue,
                        Some(Some(event)) => event?,
                        // The terminal's input ended; nothing more can be asked of it.
                        Some(None) => break,
                    }
                };
                match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if (key.code == KeyCode::Char('q')
                            && !app.is_filter_editing()
                            && !app.picker_open()
                            && app.session_confirm().is_none())
                            || (key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL))
                        {
                            break;
                        }
                        if let Some(action) = app.handle_key(key)
                            && let Some(flip) =
                                perform(action, &mut app, &config, &state, &mut sidebar_command)
                        {
                            flip_on_success = flip;
                        }
                        dirty = true;
                    }
                    Event::Mouse(mouse) => {
                        if let Some(action) = app.handle_mouse(mouse) {
                            // The pointer has no mode to flip: the host state it shows is
                            // the one the keys report.
                            let _ =
                                perform(action, &mut app, &config, &state, &mut sidebar_command);
                        }
                        if let Some(offset) = app.take_scroll() {
                            *list.offset_mut() = offset;
                        }
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
            Ok(())
        }
        .await;
        app.shutdown_session().await;
        result
    })
}

/// What the dashboard asked the host to do.
///
/// Keys and the pointer produce the same actions, so they go through one
/// executor: two matches would drift, and only one of them would learn a new
/// command.
/// One runtime for a window and the attachment it reads: the client's tasks live
/// inside the loop that owns the terminal.
fn session_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn perform(
    action: Action,
    app: &mut App,
    config: &Config,
    state: &ObservationState,
    sidebar_command: &mut Option<std::process::Child>,
) -> Option<bool> {
    match action {
        Action::ToggleSidebar => {
            let (key, command) = sidebar_resize(config, app);
            let _ = run_host_command(key, command, app, sidebar_command);
        }
        // The mode follows the host: it flips when the command reports success, so
        // a missing or failing one leaves it.
        Action::ToggleZoom => {
            let (key, command) = sidebar_zoom(config, app);
            return Some(run_host_command(key, command, app, sidebar_command));
        }
        Action::LaunchSidebar => {
            let _ = run_host_command(
                "launch_command",
                &config.sidebar.launch_command,
                app,
                sidebar_command,
            );
        }
        Action::LaunchConfigured => {
            // The shell, PATH and command are the invoking terminal's; the place
            // and directory are the session's.
            let spec = agent_radar::control_plane::public_contracts::ui_session::LaunchSpec {
                command: config.agents.launch_command.clone(),
                shell: std::env::var("SHELL").unwrap_or_default(),
                path: std::env::var_os("PATH").map(|path| path.to_string_lossy().into_owned()),
            };
            if let Some(row) = app.command_row() {
                app.submit_session(
                    agent_radar::control_plane::public_contracts::ui_session::UiAction::PrepareLaunch {
                        row,
                        spec,
                    },
                );
            }
        }
        // The session froze the effect, so the answer names the interaction rather
        // than re-deriving a target.
        Action::ConfirmAction => {
            app.answer_session_confirm(agent_radar::ui_session::confirm::Answer::Confirm);
        }
        Action::PresenceStatus => app.show_presence_status(state),
    }
    None
}

/// Turns mouse capture on. A wheel turn and a click only arrive while it is on,
/// and what it costs is the terminal's own text selection: with capture on,
/// dragging selects nothing until the terminal's bypass key is held.
fn enable_mouse_capture() {
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
}

/// Releases mouse capture. Every exit path calls this: quitting, an error from
/// the loop, and the panic hook above.
fn disable_mouse_capture() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
}

/// Where a focus stands while a terminal is being opened for it.
#[derive(Default)]
struct Attach {
    /// The place being focused, from the request the user made.
    asked: Option<agent_radar::Target>,
    /// The row that asked for it, so the retry goes through the session rather
    /// than a second implementation of the same decision.
    row: Option<agent_radar::control_plane::public_contracts::ui_session::UiRowId>,
    /// Set once a terminal was opened for `asked`: what the recovery is doing.
    waiting: Option<Waiting>,
    /// What the terminal thread says when it ends.
    ended: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

/// Where a focus recovery stands, once a terminal has been opened for it.
#[derive(Clone, Copy)]
enum Waiting {
    /// It is time to ask again when this elapses.
    Retry { at: Instant, tries: u32 },
    /// The focus was just asked again; nothing happens until its answer arrives.
    Reply { tries: u32 },
}

/// How often, and how many times, a focus is asked again once a terminal is open.
const ATTACH_RETRY: Duration = Duration::from_millis(250);
const ATTACH_TRIES: u32 = 24;

impl Attach {
    /// A new focus request replaces whatever was waiting.
    fn ask(
        &mut self,
        target: &agent_radar::Target,
        row: agent_radar::control_plane::public_contracts::ui_session::UiRowId,
    ) {
        self.asked = Some(target.clone());
        self.row = Some(row);
        self.waiting = None;
    }

    /// Reads a focus answer. With no client attached it opens a terminal once and
    /// then asks again until the server has one; any other answer ends the wait.
    fn answered(
        &mut self,
        no_client: bool,
        tui: &agent_radar::config::TuiConfig,
    ) -> Option<String> {
        let Some(target) = self.asked.clone().filter(|_| no_client) else {
            self.waiting = None;
            return None;
        };
        if let Some(asked_again) = self.waiting {
            let tries = match asked_again {
                Waiting::Retry { tries, .. } | Waiting::Reply { tries } => tries,
            };
            if tries >= ATTACH_TRIES {
                self.waiting = None;
                return Some("no terminal attached to the tmux server in time".to_string());
            }
            self.waiting = Some(Waiting::Retry {
                at: Instant::now() + ATTACH_RETRY,
                tries: tries + 1,
            });
            return None;
        }
        let environment = std::env::var("TERMINAL").ok();
        let prefix = match agent_radar::attach::terminal_prefix(
            tui.terminal.as_deref(),
            environment.as_deref(),
        ) {
            Ok(prefix) => prefix,
            Err(message) => return Some(message),
        };
        let handle = match &target {
            agent_radar::Target::Pane(handle) | agent_radar::Target::Workspace(handle) => {
                handle.clone()
            }
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        self.ended = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(agent_radar::attach::open_terminal(&prefix, &handle));
        });
        self.waiting = Some(Waiting::Retry {
            at: Instant::now() + ATTACH_RETRY,
            tries: 0,
        });
        Some("no tmux client attached: opening a terminal".to_string())
    }

    /// Asks the focus again when it is time, and reports a terminal that failed.
    fn poll(&mut self, app: &mut App) -> Option<String> {
        if let Some(Err(message)) = self.ended.as_ref().and_then(|ended| ended.try_recv().ok()) {
            self.waiting = None;
            self.ended = None;
            return Some(message);
        }
        let Waiting::Retry { at, tries } = self.waiting? else {
            // Waiting on the answer to the focus just asked again.
            return None;
        };
        if Instant::now() >= at
            && let Some(row) = self.row.clone()
        {
            app.submit_focus_row(row);
            self.waiting = Some(Waiting::Reply { tries });
        }
        None
    }
}
