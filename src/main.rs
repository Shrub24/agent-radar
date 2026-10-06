use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_radar::bus::Listener;
use agent_radar::{
    Action, App, Closer, Collector, CollectorConfig, Config, Confirmed, Focuser, Geometry,
    HerdrConfig, HerdrRuntime, ManagedActions, ObservationState, PaneView, theme, ui,
};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use ratatui::widgets::ListState;

const USAGE: &str = "\
radar — read-only fleet overview for local coding agents

usage: radar [--print-config] [--help]

  --print-config   print the configuration in use, to copy and edit
  --help           this message

Configuration is read from `$RADAR_CONFIG`, else
`$XDG_CONFIG_HOME/radar/config.toml`, else `~/.config/radar/config.toml`.
Radar draws without one.
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
        Some(other) => {
            eprintln!("radar: unknown argument `{other}`\n\n{USAGE}");
            Some(2)
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

fn run() -> io::Result<()> {
    theme::install(load_config());
    let mut terminal = ratatui::try_init()?;
    enable_mouse_capture();
    // The panic path restores the terminal through the hook `try_init` installed;
    // mouse capture is Radar's own state, so it is released ahead of that hook.
    let restore_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        disable_mouse_capture();
        restore_hook(info);
    }));
    // One Herdr adapter, shared: the collector and the focuser read and act on
    // the same runtime through the same seam.
    let runtime = Arc::new(HerdrRuntime::new(HerdrConfig::default()));
    let mut collector = Collector::new(CollectorConfig::default(), Arc::clone(&runtime));
    let mut focuser = Focuser::new(Arc::clone(&runtime));
    let mut closer = Closer::new(Arc::clone(&runtime));
    // Managed close/restart goes through the owner's control directories, never
    // the mux. The worker reads nothing until an operator confirms an action;
    // an absent or untrusted root is the owner's transport being unavailable.
    let mut managed = ManagedActions::new(control_root());
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
            app.set_focus_message(outcome.err());
            dirty = true;
        }
        // A confirmed close is answered on its own thread too: input, collection
        // and quitting stay responsive while it waits on the runtime.
        if let Some(outcome) = closer.poll() {
            app.set_focus_message(Some(outcome.unwrap_or_else(|message| message)));
            dirty = true;
        }
        // Owner-routed outcomes arrive from the worker thread the same way: the
        // UI only reads what is already waiting, and never blocks on a file.
        while let Some(update) = managed.poll() {
            app.apply_managed_update(update);
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
                        Action::ConfirmAction => match app.confirm(&state) {
                            Some(Confirmed::Direct(request)) => closer.start(request),
                            Some(Confirmed::Managed(request)) => {
                                if let Err(message) = managed.start(request) {
                                    app.set_focus_message(Some(message));
                                }
                            }
                            None => {}
                        },
                    }
                }
                dirty = true;
            }
            Event::Mouse(mouse) => {
                if let Some(action) = app.handle_mouse(mouse) {
                    match action {
                        Action::Focus(target) => focuser.start(target),
                        Action::BeginAction(operation) => app.begin_action(operation, &state),
                        Action::ConfirmAction => match app.confirm(&state) {
                            Some(Confirmed::Direct(request)) => closer.start(request),
                            Some(Confirmed::Managed(request)) => {
                                if let Err(message) = managed.start(request) {
                                    app.set_focus_message(Some(message));
                                }
                            }
                            None => {}
                        },
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
    collector.shutdown();
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

/// The owner-control root (`~/.pi/agent/pi-herdsman/control`). Radar only ever
/// reads it and never creates it, so a missing name is just an unavailable
/// transport rather than something to make.
fn control_root() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join(".pi")
            .join("agent")
            .join("pi-herdsman")
            .join("control"),
        None => PathBuf::new(),
    }
}
