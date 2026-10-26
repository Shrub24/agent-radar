use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_radar::control_plane::observation::Snapshot;
use agent_radar::managed_control as radar;
use agent_radar::snapshot_fetch::{Fetched, SnapshotFetcher};
use agent_radar::topology::ContainerControl;
use agent_radar::tree::RowId;
use agent_radar::{
    Action, App, Closer, Config, Confirmed, Focuser, Geometry, ObservationState, PaneView,
    control_plane, theme, ui,
};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use ratatui::widgets::ListState;

fn main() -> io::Result<()> {
    let cli = <agent_radar::cli::Cli as clap::Parser>::parse();
    if cli.print_config {
        print!("{}", load_config().to_document());
        return Ok(());
    }
    let dashboard = match &cli.invocation {
        None => Some((false, None)),
        Some(agent_radar::cli::Invocation::Sidebar) => Some((true, None)),
        Some(agent_radar::cli::Invocation::Modal { key, row }) => {
            let row = match row.as_deref().map(serde_json::from_str) {
                None => None,
                Some(Ok(row)) => Some(row),
                Some(Err(error)) => {
                    eprintln!("radar modal: the row is not an identity: {error}");
                    std::process::exit(2);
                }
            };
            Some((false, Some((KeyCode::Char(*key), row))))
        }
        Some(_) => None,
    };
    if let Some((sidebar, modal)) = dashboard {
        let result = run(sidebar, modal);
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
                &load_config().daemon.tmux_server,
                session.as_deref(),
            ) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("radar workspace: {error}");
                    1
                }
            }
        }
        Invocation::Sidebar | Invocation::Modal { .. } => {
            unreachable!("these are dashboards, not printing commands")
        }
        Invocation::Server(action) => {
            let (verb, name) = action.parts();
            let name = match name {
                Some(name) => name.to_string(),
                None => load_config().daemon.tmux_server.clone(),
            };
            let (message, code) = agent_radar::service::server_command(verb, &name);
            println!("{message}");
            code
        }
        Invocation::Lifecycle(action) => run_lifecycle_command(action),
        Invocation::Reset { hard, yes } => {
            let server = load_config().daemon.tmux_server;
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

/// Which host command zooms the sidebar to the window or back: the width that
/// decides when it is drawn as the whole dashboard.
fn sidebar_zoom<'a>(config: &'a Config, app: &App) -> (&'static str, &'a Option<Vec<String>>) {
    let full = app.is_sidebar() && !app.sidebar_compact();
    if full {
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
) {
    if pending.is_some() {
        app.set_focus_message(Some("a sidebar host command is still running".into()));
        return;
    }
    let Some(command) = command else {
        app.set_focus_message(Some(format!("set [sidebar] {key}")));
        return;
    };
    let result = std::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match result {
        Ok(child) => *pending = Some(child),
        Err(error) => app.set_focus_message(Some(format!("sidebar {key}: {error}"))),
    }
}

fn poll_sidebar_command(pending: &mut Option<std::process::Child>, app: &mut App) -> bool {
    let Some(child) = pending.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(None) => return false,
        Ok(Some(status)) if status.success() => {}
        Ok(Some(status)) => {
            app.set_focus_message(Some(format!("sidebar host command exited {status}")))
        }
        Err(error) => app.set_focus_message(Some(format!("sidebar host command: {error}"))),
    }
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
        assert!(poll_sidebar_command(&mut pending, &mut app));
        assert!(pending.is_none());
        assert!(app.focus_message().unwrap().contains("exited"));
    }

    #[test]
    fn the_width_chooses_hide_show_zoom_or_unzoom() {
        let config = Config::default();
        let mut app = App::sidebar();
        app.note_width(30);
        assert_eq!(sidebar_resize(&config, &app).0, "hide_command");
        app.note_width(10);
        assert_eq!(sidebar_resize(&config, &app).0, "show_command");

        app.note_width(140);
        assert_eq!(sidebar_zoom(&config, &app).0, "unzoom_command");
        assert!(
            !app.sidebar_compact(),
            "a zoomed sidebar draws the dashboard"
        );
        app.note_width(30);
        assert!(app.sidebar_compact());
        assert_eq!(sidebar_zoom(&config, &app).0, "zoom_command");
        assert!(!App::new().sidebar_compact(), "the dashboard never is");
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
fn source_diagnostic(snapshot: &Snapshot, name: &str) -> Option<String> {
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

fn run(sidebar: bool, modal: Option<(KeyCode, Option<RowId>)>) -> io::Result<()> {
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
    let (runtime, container_controls, lifecycle_controls, observation, task_diagnostic) = (
        selected.provider,
        selected.containers,
        selected.lifecycle,
        selected.observation,
        selected.task_diagnostic,
    );
    // One runtime, shared: the focuser and the closer read and act on the same
    // adapter through the same seam. The dashboard collects nothing itself: the
    // daemon's sampler owns that, and this reads its snapshot.
    let mut focuser = Focuser::new(Arc::clone(&runtime));
    let mut closer = Closer::new(Arc::clone(&runtime));
    // Managed lifecycle actions target daemon-owned subjects and run off the UI thread.
    let mut managed = radar::LifecycleControl::new(lifecycle_controls);
    // A confirmed container control goes to the daemon, one at a time and off the
    // UI thread. A run without a daemon has no container to act on and says so.
    let mut containers = ContainerControl::new(container_controls);
    let mut state = ObservationState::new();
    let mut app = if modal.is_some() {
        App::modal()
    } else if sidebar {
        App::sidebar()
    } else {
        App::new()
    };
    app.set_modal_host(in_workspace);
    // The popup opens its modal once the first snapshot has arrived, because the
    // row it was started for is selected by identity in that snapshot.
    let mut pending_modal = modal;
    let mut queued_key = None;
    if let Some(reason) = log_note {
        app.set_focus_message(Some(format!("no dashboard log: {reason}")));
    }
    let mut list = ListState::default();
    app.set_task_diagnostic(task_diagnostic);
    // The daemon samples for every display; this fetcher is how the dashboard
    // reads what it published, and it is absent only from a daemon that does not
    // serve the observation contract, which the diagnostic screen refuses.
    let fetcher = observation.map(SnapshotFetcher::new);
    // The animation frame, and the clock it is derived from: a mark has to move
    // at the same rate whether or not anything else made the loop redraw.
    let started = Instant::now();
    let mut phase: usize = 0;
    let mut dirty = true;
    // A focus with no client attached opens a terminal and asks again, because a
    // client has to exist before a focus can show anything.
    let mut attach = Attach::default();
    let mut sidebar_command = None;

    loop {
        dirty |= poll_sidebar_command(&mut sidebar_command, &mut app);
        // Foreground evidence is swept while a pane view is showing panes, and
        // while finished sessions are listed: telling a session that has gone
        // from a pane in use again needs the foreground either way.
        let sweeping = app.pane_view() != PaneView::Hidden || app.shows_finished();
        if let Some(fetcher) = &fetcher {
            fetcher.wants(sweeping);
            match fetcher.poll() {
                Some(Fetched::Snapshot(snapshot)) => {
                    // Task sessions arrive as one source of the same revision, so
                    // the dashboard no longer reads them separately.
                    if let Some(tasks) = snapshot.tasks.as_ref() {
                        match control_plane::client::tasks_from_snapshot(tasks) {
                            Ok(events) => app.replace_task_snapshot(events),
                            Err(problem) => app.set_task_diagnostic(Some(problem)),
                        }
                    }
                    app.set_task_diagnostic(source_diagnostic(&snapshot, "tasks"));
                    state = snapshot.observation.clone();
                    app.refresh(&state);
                    if let Some((code, row)) = pending_modal.take() {
                        app.show_everything();
                        match row {
                            // A row that is gone is refused, never replaced: the
                            // action was chosen for that one.
                            Some(row) if !app.select_exact(&row) => app.set_focus_message(Some(
                                "that row is gone; nothing was changed".into(),
                            )),
                            _ => {
                                queued_key =
                                    Some(crossterm::event::KeyEvent::new(code, KeyModifiers::NONE))
                            }
                        }
                    }
                    dirty = true;
                }
                // The previous view stands, and the reason it is not moving is
                // on the footer rather than in a half-updated screen.
                Some(Fetched::Stale(reason)) => {
                    app.set_focus_message(Some(reason));
                    dirty = true;
                }
                None => {}
            }
        }
        // A focus request is answered on its own thread; applying the outcome
        // here is what puts a failure on the footer, and never waits on the backend.
        if let Some(outcome) = focuser.poll() {
            let message = attach.answered(&outcome, &config.tui);
            app.set_focus_message(message.or_else(|| outcome.diagnostic().map(str::to_string)));
            dirty = true;
        }
        if let Some(message) = attach.poll(&mut focuser) {
            app.set_focus_message(Some(message));
            dirty = true;
        }
        // A confirmed close is answered on its own thread too: input, collection
        // and quitting stay responsive while it waits on the runtime.
        if let Some(outcome) = closer.poll() {
            app.set_focus_message(Some(outcome.unwrap_or_else(|message| message)));
            dirty = true;
        }
        // A container control is answered on its own thread too, and its outcome
        // keeps the request id it was dispatched under so a lost answer leaves an
        // operation to read back.
        if let Some(finished) = containers.poll() {
            app.apply_container_outcome(finished);
            dirty = true;
        }
        // A daemon lifecycle operation is read back and displayed by request id.
        if let Some(finished) = managed.poll() {
            app.apply_subject_outcome(finished);
            dirty = true;
        }

        // A message that has been up long enough goes away by itself: the
        // footer is the hint line again without the user having to press a key.
        if app.expire_focus_message(Instant::now()) {
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
            // A fetch that is waiting for a newer revision is answered off this
            // thread; polling a little more often keeps a published revision
            // from sitting unapplied behind the idle interval.
            Duration::from_millis(50)
        };
        // The popup is done once its modal is answered, its work has finished and
        // the answer has been read.
        if app.is_modal_only()
            && pending_modal.is_none()
            && queued_key.is_none()
            && !app.picker_open()
            && app.confirmation().is_none()
            && app.focus_message().is_none()
            && !closer.is_busy()
            && !managed.is_busy()
            && !containers.is_busy()
        {
            break;
        }
        let event = match queued_key.take() {
            Some(key) => Event::Key(key),
            None => {
                if !event::poll(timeout)? {
                    continue;
                }
                event::read()?
            }
        };
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if (key.code == KeyCode::Char('q')
                    && !app.is_filter_editing()
                    && !app.picker_open()
                    && app.confirmation().is_none())
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    break;
                }
                // A palette choice runs the key it names, so a command and its
                // hotkey share one implementation.
                let mut pressing = vec![key];
                while let Some(key) = pressing.pop() {
                    let Some(action) = app.handle_key(key) else {
                        break;
                    };
                    if let Action::Replay(code) = action {
                        pressing.push(crossterm::event::KeyEvent::new(code, KeyModifiers::NONE));
                        continue;
                    }
                    match action {
                        Action::Replay(_) => {}
                        Action::Focus(target) => {
                            attach.ask(&target);
                            focuser.start(target);
                        }
                        Action::BeginAction(operation) => dispatch(
                            app.begin_action(operation, &state),
                            &mut app,
                            &mut closer,
                            &mut managed,
                            &mut containers,
                        ),
                        Action::ToggleSidebar => {
                            let (key, command) = sidebar_resize(&config, &app);
                            run_host_command(key, command, &mut app, &mut sidebar_command)
                        }
                        Action::ToggleZoom => {
                            let (key, command) = sidebar_zoom(&config, &app);
                            run_host_command(key, command, &mut app, &mut sidebar_command)
                        }
                        Action::LaunchSidebar => run_host_command(
                            "launch_command",
                            &config.sidebar.launch_command,
                            &mut app,
                            &mut sidebar_command,
                        ),
                        Action::Modal { key, row } => {
                            let row = row.and_then(|row| serde_json::to_string(&row).ok());
                            let radar = std::env::current_exe()
                                .map(|path| path.to_string_lossy().into_owned())
                                .unwrap_or_else(|_| "radar".into());
                            let command = match key {
                                KeyCode::Char(key) => agent_radar::workspace::popup_command(
                                    &radar,
                                    key,
                                    row.as_deref(),
                                ),
                                _ => None,
                            };
                            run_host_command("modal", &command, &mut app, &mut sidebar_command)
                        }
                        Action::LaunchConfigured => {
                            let shell = std::env::var("SHELL").ok();
                            let path = std::env::var_os("PATH");
                            let confirmed = app.launch_configured(
                                &state,
                                &config.tui.launch_command,
                                shell.as_deref(),
                                path.as_deref(),
                            );
                            dispatch(
                                confirmed,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                        Action::NewWorkspace => open_workspace_picker(&config.tui, &mut app)?,
                        Action::NewWorkspaceIn(directory) => {
                            let control = Some(app.new_workspace(Some(&directory)));
                            dispatch(
                                control,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                        Action::ConfirmAction => {
                            let confirmed = app.confirm(&state);
                            dispatch(
                                confirmed,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                    }
                }
                dirty = true;
            }
            Event::Mouse(mouse) => {
                if let Some(action) = app.handle_mouse(mouse) {
                    match action {
                        // Only the palette chooses commands, and it takes keys.
                        Action::Replay(_) => {}
                        Action::Focus(target) => {
                            attach.ask(&target);
                            focuser.start(target);
                        }
                        Action::BeginAction(operation) => dispatch(
                            app.begin_action(operation, &state),
                            &mut app,
                            &mut closer,
                            &mut managed,
                            &mut containers,
                        ),
                        Action::ToggleSidebar => {
                            let (key, command) = sidebar_resize(&config, &app);
                            run_host_command(key, command, &mut app, &mut sidebar_command)
                        }
                        Action::ToggleZoom => {
                            let (key, command) = sidebar_zoom(&config, &app);
                            run_host_command(key, command, &mut app, &mut sidebar_command)
                        }
                        Action::LaunchSidebar => run_host_command(
                            "launch_command",
                            &config.sidebar.launch_command,
                            &mut app,
                            &mut sidebar_command,
                        ),
                        // Only a key opens a popup; a click reaches no action here.
                        Action::Modal { .. } => {}
                        Action::LaunchConfigured => {
                            let shell = std::env::var("SHELL").ok();
                            let path = std::env::var_os("PATH");
                            let confirmed = app.launch_configured(
                                &state,
                                &config.tui.launch_command,
                                shell.as_deref(),
                                path.as_deref(),
                            );
                            dispatch(
                                confirmed,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                        Action::NewWorkspace => open_workspace_picker(&config.tui, &mut app)?,
                        Action::NewWorkspaceIn(directory) => {
                            let control = Some(app.new_workspace(Some(&directory)));
                            dispatch(
                                control,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                        Action::ConfirmAction => {
                            let confirmed = app.confirm(&state);
                            dispatch(
                                confirmed,
                                &mut app,
                                &mut closer,
                                &mut managed,
                                &mut containers,
                            );
                        }
                    }
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
    focuser.shutdown();
    closer.shutdown();
    managed.shutdown();
    containers.shutdown();
    Ok(())
}

/// Carries out one confirmed action.
///
/// Unmanaged closes use the runtime; managed lifecycle and topology controls use
/// daemon request IDs. A possibly accepted operation is read back by that ID,
/// never repeated through a different control path.
fn dispatch(
    confirmed: Option<Confirmed>,
    app: &mut App,
    closer: &mut Closer,
    managed: &mut radar::LifecycleControl,
    containers: &mut ContainerControl,
) {
    match confirmed {
        Some(Confirmed::Direct(request)) => closer.start(request),
        Some(Confirmed::Subject {
            operation,
            request_id,
            subject_id,
            claim_id,
            child_close,
            close_request_id,
        }) => {
            let operation = match operation {
                agent_radar::app::Operation::Restart => radar::Operation::Restart,
                agent_radar::app::Operation::Delete => radar::Operation::Cancel,
                _ => {
                    app.set_focus_message(Some("unsupported managed lifecycle action".into()));
                    return;
                }
            };
            if let Err(message) = managed.start(radar::Control {
                operation,
                request_id,
                subject_id,
                claim_id,
                child_close,
                close_request_id,
                enrollment: None,
            }) {
                app.set_focus_message(Some(message));
            }
        }
        Some(Confirmed::Container(control)) => {
            if let Err(message) = containers.start(control) {
                app.set_focus_message(Some(message));
            }
        }
        None => {}
    }
}

/// Lets the operator pick the directory a new workspace starts in, and returns the
/// creation for it. The picker draws its own screen, so the dashboard gives the
/// terminal back first and takes it again after, whatever the picker answered.
/// Cancelling creates nothing.
/// Opens the workspace chooser over the directories a new workspace may start
/// in. It draws inside the dashboard rather than taking the terminal over, so the
/// fleet stays visible and `Esc` costs nothing.
fn open_workspace_picker(tui: &agent_radar::config::TuiConfig, app: &mut App) -> io::Result<()> {
    let cwd = std::env::current_dir()?;
    let found =
        agent_radar::project::candidates(tui.project_source.as_deref(), tui.project_filter, &cwd);
    if let Some(note) = found.note {
        app.set_focus_message(Some(note));
    }
    app.open_workspace_picker(found.directories);
    Ok(())
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
    /// Set once a terminal was opened for `asked`, with when to look again.
    waiting: Option<(Instant, u32)>,
    /// What the terminal thread says when it ends.
    ended: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

/// How often, and how many times, a focus is asked again once a terminal is open.
const ATTACH_RETRY: Duration = Duration::from_millis(250);
const ATTACH_TRIES: u32 = 24;

impl Attach {
    /// A new focus request replaces whatever was waiting.
    fn ask(&mut self, target: &agent_radar::Target) {
        self.asked = Some(target.clone());
        self.waiting = None;
    }

    /// Reads a focus answer. With no client attached it opens a terminal once and
    /// then asks again until the server has one; any other answer ends the wait.
    fn answered(
        &mut self,
        outcome: &agent_radar::FocusOutcome,
        tui: &agent_radar::config::TuiConfig,
    ) -> Option<String> {
        let no_client = matches!(
            outcome,
            agent_radar::FocusOutcome::Refused(message)
                if message == agent_radar::runtime::NO_CLIENT
        );
        let Some(target) = self.asked.clone().filter(|_| no_client) else {
            self.waiting = None;
            return None;
        };
        if let Some((_, tries)) = self.waiting {
            if tries >= ATTACH_TRIES {
                self.waiting = None;
                return Some("no terminal attached to the tmux server in time".to_string());
            }
            self.waiting = Some((Instant::now() + ATTACH_RETRY, tries + 1));
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
        self.waiting = Some((Instant::now() + ATTACH_RETRY, 0));
        Some("no tmux client attached: opening a terminal".to_string())
    }

    /// Asks the focus again when it is time, and reports a terminal that failed.
    fn poll(&mut self, focuser: &mut Focuser) -> Option<String> {
        if let Some(Err(message)) = self.ended.as_ref().and_then(|ended| ended.try_recv().ok()) {
            self.waiting = None;
            self.ended = None;
            return Some(message);
        }
        let (at, _) = self.waiting?;
        if Instant::now() >= at
            && let Some(target) = self.asked.clone()
        {
            focuser.start(target);
            // Not asked again until its answer sets the next time.
            self.waiting = self
                .waiting
                .map(|(_, tries)| (Instant::now() + Duration::from_secs(3600), tries));
        }
        None
    }
}
