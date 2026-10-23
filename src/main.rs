use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_radar::control_plane::observation::Snapshot;
use agent_radar::managed_control as radar;
use agent_radar::snapshot_fetch::{Fetched, SnapshotFetcher};
use agent_radar::topology::ContainerControl;
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
    let Some(invocation) = cli.invocation else {
        let result = run();
        disable_mouse_capture();
        let restored = ratatui::try_restore();
        return result.and(restored);
    };
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
        Invocation::Run { agent, program } => {
            let socket = agent_radar::control_plane::socket_path();
            let kind = if agent {
                control_plane::lifecycle::ProcessKind::Agent
            } else {
                control_plane::lifecycle::ProcessKind::Command
            };
            control_plane::lifecycle::run_managed(&socket.to_string_lossy(), kind, &program)
        }
        Invocation::LaunchBootstrap { socket, program } => {
            control_plane::lifecycle::run_bootstrap(&socket, &program)
        }
        Invocation::LifecycleWake {
            socket,
            subject,
            server,
        } => control_plane::lifecycle::run_wake(&socket, &subject, &server),
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

fn run() -> io::Result<()> {
    let config = load_config();
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
    let mut app = App::new();
    if let Some(reason) = log_note {
        app.set_focus_message(Some(format!("no dashboard log: {reason}")));
    }
    let mut list = ListState::default();
    app.set_bus_diagnostic(task_diagnostic);
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

    loop {
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
                            Ok(events) => app.replace_bus_snapshot(events),
                            Err(problem) => app.set_bus_diagnostic(Some(problem)),
                        }
                    }
                    app.set_bus_diagnostic(source_diagnostic(&snapshot, "tasks"));
                    state = snapshot.observation.clone();
                    app.refresh(&state);
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
        // here is what puts a failure on the footer, and never waits on Herdr.
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
                        Action::NewWorkspace => {
                            let control = choose_project(&mut terminal, &config.tui, &mut app)?;
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
                        Action::NewWorkspace => {
                            let control = choose_project(&mut terminal, &config.tui, &mut app)?;
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
fn choose_project(
    terminal: &mut ratatui::DefaultTerminal,
    tui: &agent_radar::config::TuiConfig,
    app: &mut App,
) -> io::Result<Option<Confirmed>> {
    let cwd = std::env::current_dir()?;
    let found =
        agent_radar::project::candidates(tui.project_source.as_deref(), tui.project_filter, &cwd);
    disable_mouse_capture();
    ratatui::try_restore()?;
    let chosen = agent_radar::project::pick(&found.directories);
    *terminal = ratatui::try_init()?;
    enable_mouse_capture();
    terminal.clear()?;
    if let Some(note) = found.note {
        app.set_focus_message(Some(note));
    }
    Ok(chosen?.map(|directory| app.new_workspace(Some(&directory))))
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
