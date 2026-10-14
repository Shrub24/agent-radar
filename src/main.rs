use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_radar::bus::Listener;
use agent_radar::managed_control as radar;
use agent_radar::topology::ContainerControl;
use agent_radar::{
    Action, App, Closer, Collector, CollectorConfig, Config, Confirmed, Focuser, Geometry,
    ObservationState, PaneView, control_plane, theme, ui,
};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use ratatui::widgets::ListState;

const USAGE: &str = "\
radar — read-only fleet overview for local coding agents

usage: radar [--print-config] [--help]
       radar daemon [start|stop|restart|status|--help]
       radar server <start|stop|restart|status> [NAME]

  --print-config   print the configuration in use, to copy and edit
  --help           this message
  daemon           control the daemon's user units; with no action, serve in
                   the foreground (what the service unit runs)
  server           control the tmux server a daemon serves, by name

Configuration is read from `$RADAR_CONFIG`, else
`$XDG_CONFIG_HOME/radar/config.toml`, else `~/.config/radar/config.toml`.
Radar draws without one.
";

const DAEMON_USAGE: &str = "\
radar daemon — the control plane Radar talks to

usage: radar daemon [start|stop|restart|status]

With no action, serves one JSON request and one JSON response per line on the
control socket, in the foreground, until signalled. The user units run it that
way: under systemd the socket unit passes its listener, and the daemon serves it
without unlinking the path. Standalone, it binds `$RADAR_CONTROL_SOCKET`, else
`$XDG_RUNTIME_DIR/agent-radar/control.sock`, else `/tmp/agent-radar-<uid>/control.sock`.

`start` starts the control socket, which lets the daemon run on the next
connection. `stop` stops the socket, then the daemon; the tmux server keeps
running. `restart` restarts the daemon. `status` reports both units.

The physical backend is tmux on the `radar` server, set by `daemon.backend` and
`daemon.tmux_server` in the configuration. `$RADAR_CONTROL_BACKEND` and
`$RADAR_TMUX_SERVER` override them. A name that is no backend, or a server the
daemon cannot serve, refuses startup rather than falling back to another backend.
";

fn main() -> io::Result<()> {
    if let Some(code) = handle_arguments() {
        std::process::exit(code);
    }
    let result = run();
    disable_mouse_capture();
    let restored = ratatui::try_restore();
    result.and(restored)
}

/// Handles the arguments that print something and exit. `None` means there is a
/// dashboard to run.
fn handle_arguments() -> Option<i32> {
    let argument = std::env::args().nth(1);
    match argument.as_deref() {
        None => None,
        Some("--print-config") => {
            print!("{}", load_config().to_document());
            Some(0)
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            Some(0)
        }
        Some(control_plane::lifecycle::BOOTSTRAP_COMMAND) => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            Some(control_plane::lifecycle::run_bootstrap(&arguments))
        }
        Some(control_plane::lifecycle::WAKE_COMMAND) => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            Some(control_plane::lifecycle::run_wake(&arguments))
        }
        Some("daemon") => Some(match std::env::args().nth(2).as_deref() {
            Some("--help" | "-h") => {
                print!("{DAEMON_USAGE}");
                0
            }
            Some(action @ ("start" | "stop" | "restart" | "status")) => {
                let (message, code) = agent_radar::service::daemon_command(action);
                println!("{message}");
                code
            }
            Some(other) => {
                eprintln!("radar daemon: unknown argument `{other}`\n\n{DAEMON_USAGE}");
                2
            }
            None => serve_daemon(),
        }),
        Some("server") => Some(run_server_command()),
        Some(other) => {
            eprintln!("radar: unknown argument `{other}`\n\n{USAGE}");
            Some(2)
        }
    }
}

/// `radar server <action> [NAME]`: controls the tmux server a daemon serves. The
/// name defaults to the configured server.
fn run_server_command() -> i32 {
    let mut arguments = std::env::args().skip(2);
    let action = arguments.next();
    let name = arguments
        .next()
        .unwrap_or_else(|| load_config().daemon.tmux_server);
    match action.as_deref() {
        Some(action @ ("start" | "stop" | "restart" | "status")) => {
            let (message, code) = agent_radar::service::server_command(action, &name);
            println!("{message}");
            code
        }
        _ => {
            eprintln!("radar server: expected start, stop, restart or status\n\n{USAGE}");
            2
        }
    }
}

/// Serves the control plane until the process is signalled, and reports the
/// socket it is on before it starts waiting.
///
/// Radar itself never starts this process: the daemon is the operator's to run,
/// and a TUI that spawned a long-lived server on someone's behalf would be a
/// surprise on their machine. The exit code is the shell's to read, so a daemon
/// that could not bind says so and stops rather than serving nothing quietly.
fn serve_daemon() -> i32 {
    if let Err(diagnostic) = agent_radar::install_signal_handlers() {
        eprintln!("radar daemon: {diagnostic}");
        return 1;
    }
    let mut daemon = match agent_radar::Daemon::bind() {
        Ok(daemon) => daemon,
        Err(diagnostic) => {
            eprintln!("radar daemon: {diagnostic}");
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

fn run() -> io::Result<()> {
    let config = load_config();
    theme::install(config.clone());
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
    let (runtime, container_controls, lifecycle_controls) =
        (selected.provider, selected.containers, selected.lifecycle);
    // One runtime, shared: the collector, the focuser and the closer read and act
    // on the same adapter through the same seam.
    let mut collector = Collector::new(CollectorConfig::default(), Arc::clone(&runtime));
    let mut focuser = Focuser::new(Arc::clone(&runtime));
    let mut closer = Closer::new(Arc::clone(&runtime));
    // Managed lifecycle actions target daemon-owned subjects and run off the UI thread.
    let mut managed = radar::LifecycleControl::new(lifecycle_controls);
    // A confirmed container control goes to the daemon, one at a time and off the
    // UI thread. A run without a daemon has no container to act on and says so.
    let mut containers = ContainerControl::new(container_controls);
    let mut state = ObservationState::new();
    let mut app = App::new();
    let mut list = ListState::default();
    // The bus is the only inbound surface Radar has and nothing depends on it:
    // a listener that could not bind leaves the fleet observed as usual and is
    // reported as a diagnostic of its own.
    let mut listener = match Listener::bind() {
        Ok(listener) => Some(listener),
        Err(diagnostic) => {
            app.set_bus_diagnostic(Some(diagnostic));
            None
        }
    };
    // The animation frame, and the clock it is derived from: a mark has to move
    // at the same rate whether or not anything else made the loop redraw.
    let started = Instant::now();
    let mut phase: usize = 0;
    let mut dirty = true;

    loop {
        // Foreground evidence is swept while a pane view is showing panes, and
        // while finished sessions are listed: telling a session that has gone
        // from a pane in use again needs the foreground either way.
        let sweeping = app.pane_view() != PaneView::Hidden || app.shows_finished();
        if collector.tick(&mut state, sweeping) {
            app.refresh(&state);
            dirty = true;
        }
        // Bus events are produced by the listener's own threads; draining them
        // is what makes them visible, and any of them changes what is drawn.
        if let Some(listener) = &listener {
            for event in listener.drain() {
                app.apply_bus_event(event);
                dirty = true;
            }
        }
        // A focus request is answered on its own thread; applying the outcome
        // here is what puts a failure on the footer, and never waits on Herdr.
        if let Some(outcome) = focuser.poll() {
            app.set_focus_message(outcome.diagnostic().map(str::to_string));
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
            terminal.draw(|frame| {
                geometry = ui::render(frame, &state, &app, &mut list, phase);
            })?;
            app.note_layout(geometry);
            dirty = false;
        }
        let timeout = if animating {
            Duration::from_millis(1000 / fps as u64)
        } else if collector.is_refreshing() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if (key.code == KeyCode::Char('q')
                    && !app.is_filter_editing()
                    && app.confirmation().is_none())
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    break;
                }
                if let Some(action) = app.handle_key(key) {
                    match action {
                        Action::Focus(target) => focuser.start(target),
                        Action::BeginAction(operation) => app.begin_action(operation, &state),
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
                        Action::Focus(target) => focuser.start(target),
                        Action::BeginAction(operation) => app.begin_action(operation, &state),
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
    if let Some(listener) = &mut listener {
        listener.stop();
    }
    focuser.shutdown();
    closer.shutdown();
    managed.shutdown();
    containers.shutdown();
    collector.shutdown();
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
            child_close,
            close_request_id,
        }) => {
            let operation = match operation {
                agent_radar::app::Operation::Restart => radar::Operation::Restart,
                agent_radar::app::Operation::ClosePane => radar::Operation::Cancel,
                _ => {
                    app.set_focus_message(Some("unsupported managed lifecycle action".into()));
                    return;
                }
            };
            if let Err(message) = managed.start(radar::Control {
                operation,
                request_id,
                subject_id,
                child_close,
                close_request_id,
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
