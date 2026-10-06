//! Bus consumer tests: what the details show for a publisher's tasks, which
//! row they attach to, and what the panel says when the bus is not running.
//!
//! These drive the same path the executable does — [`App::apply_bus_event`]
//! feeding the join that [`ui::render`] draws — over a real observation rather
//! than a hand-built row, so a change to either half of the join fails here.

use std::time::{SystemTime, UNIX_EPOCH};

use agent_radar::app::DetailPage;
use agent_radar::bus::{BusEvent, Task, TaskState};
use agent_radar::model::{
    AgentObservation, FleetObservation, HerdsmanFacts, Lineage, Location, Pane, RuntimeStatus,
    SessionUuid, Tab, Workspace,
};
use agent_radar::{App, ObservationState, RowId, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::widgets::ListState;

/// The session UUID the fixture pane publishes as its lineage.
const SESSION: &str = "11111111-1111-4111-8111-111111111111";
/// A second session UUID some tests connect with instead.
const OTHER_SESSION: &str = "22222222-2222-4222-8222-222222222222";

fn location(pane_id: &str) -> Location {
    Location {
        workspace_id: "wH".into(),
        tab_id: "wH:t1".into(),
        pane_id: pane_id.into(),
    }
}

/// A one-workspace fleet with one agent row per entry: its pane id, the lineage
/// UUID it publishes (`None` for a row that publishes none), and the background
/// tasks its pane tokens carry. The token list is what says what is unresolved
/// — the running count beside it is not a task set — so the count publishes
/// that many ids.
fn fleet(agents: &[(&str, Option<&str>, Option<u32>)]) -> ObservationState {
    let mut state = ObservationState::new();
    state.apply_success(FleetObservation {
        workspaces: vec![Workspace {
            workspace_id: "wH".into(),
            label: Some("home".into()),
            number: None,
        }],
        tabs: vec![Tab {
            tab_id: "wH:t1".into(),
            workspace_id: "wH".into(),
            label: None,
            number: None,
        }],
        panes: agents
            .iter()
            .map(|(pane_id, _, _)| Pane {
                location: location(pane_id),
                label: None,
                title: None,
            })
            .collect(),
        agents: agents
            .iter()
            .map(|(pane_id, session, background_running)| AgentObservation {
                location: location(pane_id),
                name: Some("pi".into()),
                label: Some(format!("worker {pane_id}")),
                status: Some(RuntimeStatus::Working),
                session: None,
                lineage: session.map(|uuid| Lineage {
                    session: SessionUuid::parse(uuid).expect("test UUID"),
                    parent: None,
                }),
                facts: HerdsmanFacts {
                    background_running: *background_running,
                    background_tasks: (0..background_running.unwrap_or(0))
                        .map(|index| format!("bg-{}", index + 1))
                        .collect(),
                    ..HerdsmanFacts::default()
                },
            })
            .collect(),
    });
    state
}

/// A task with only the two required fields; each test fills what it needs.
fn task(id: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        state,
        command: None,
        cwd: None,
        pid: None,
        started_at: None,
        last_output_at: None,
        output_bytes: None,
        exit_code: None,
    }
}

/// Connects `session` with its full list, as the listener would report it.
fn publish(app: &mut App, session: &str, tasks: Vec<Task>) {
    app.apply_bus_event(BusEvent::Connected {
        session: session.into(),
        pane: None,
    });
    app.apply_bus_event(BusEvent::Tasks {
        session: session.into(),
        tasks,
    });
}

fn app_for(state: &ObservationState) -> App {
    let mut app = App::new();
    app.refresh(state);
    app
}

fn select_agent(app: &mut App, pane_id: &str) {
    let rows = app.visible_rows();
    let index = rows
        .iter()
        .position(|row| row.id == &agent_radar::RowId::Agent(pane_id.into()))
        .expect("agent row is visible");
    app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
    assert_eq!(app.selected_index(), Some(index));
}

/// Selects a task row, which the tree lists only in the view that shows them.
fn select_task(app: &mut App, id: &str) {
    let rows = app.visible_rows();
    let index = rows
        .iter()
        .position(|row| matches!(&row.id, agent_radar::RowId::Task(task) if task.id == id))
        .expect("task row is visible");
    app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
    assert_eq!(app.selected_index(), Some(index));
}

/// The whole screen, one trimmed row per line.
///
/// Every detail page in turn, concatenated, with every disclosure it offers
/// open: a test about a fact does not have to know which page or which block
/// carries it, and a fact that goes missing from all of them still fails.
fn pages(state: &ObservationState, app: &mut App) -> String {
    let shown = app.detail_page();
    let mut out = Vec::new();
    for page in DetailPage::ALL {
        app.select_page(page);
        disclose_all(app);
        out.push(screen(state, app));
    }
    app.select_page(shown);
    out.join("\n")
}

/// Opens every block the page on screen offers, so a test about a fact behind a
/// disclosure reads it the way a reader who opened it does. Idempotent: a page
/// read twice is not a page toggled shut.
fn disclose_all(app: &mut App) {
    for key in app.disclosures() {
        if !app.disclosure_open(&key) {
            app.toggle_block(&key);
        }
    }
}

/// Wide enough that the details column reaches its maximum width, so a phrase
/// a test asserts sits on one row instead of being wrapped mid-phrase.
fn screen(state: &ObservationState, app: &App) -> String {
    drawn(state, app).0
}

/// A draw of the whole screen, and where it put the panel's markers, for the
/// tests about what a click on a disclosure lands on.
fn drawn(state: &ObservationState, app: &App) -> (String, agent_radar::Geometry) {
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).expect("infallible test backend");
    let mut geometry = agent_radar::Geometry::default();
    terminal
        .draw(|frame| {
            geometry = ui::render(frame, state, app, &mut ListState::default(), 0);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let screen = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (screen, geometry)
}

/// A draw of one screen size that records the layout the way the main loop does,
/// so a test can scroll the panel against what it just drew.
fn draw_at(
    state: &ObservationState,
    app: &mut App,
    width: u16,
    height: u16,
) -> (String, agent_radar::Geometry) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("infallible backend");
    let mut geometry = agent_radar::Geometry::default();
    terminal
        .draw(|frame| {
            geometry = ui::render(frame, state, app, &mut ListState::default(), 0);
        })
        .expect("draw");
    app.note_layout(geometry.clone());
    let buffer = terminal.backend().buffer();
    let screen = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (screen, geometry)
}

/// The glyph a drawn screen has at a position, if it has one there.
fn glyph_at(screen: &str, at: (u16, u16)) -> Option<char> {
    screen
        .lines()
        .nth(at.1 as usize)?
        .chars()
        .nth(at.0 as usize)
}

/// A left click at a position, through the same handler the executable uses.
fn click_at(app: &mut App, at: (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    });
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis() as u64
}

#[test]
fn tasks_join_by_the_exact_session_uuid() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    let now = now_unix_ms();
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some("nix build .#radar".into());
    running.cwd = Some("/home/x/proj".into());
    running.started_at = Some(now - 158_000);
    running.last_output_at = Some(now - 4_000);
    running.output_bytes = Some(18_244);
    running.exit_code = Some(0);
    publish(&mut app, SESSION, vec![running]);

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    for part in [
        "task: bg-1",
        "running",
        "2m38s",
        "output 4s ago",
        "18244 B",
        "exit 0",
        "command: nix build .#radar",
        "cwd: /home/x/proj",
    ] {
        assert!(screen.contains(part), "missing {part:?}:\n{screen}");
    }
}

#[test]
fn a_row_with_its_own_session_uuid_never_falls_back_to_the_pane() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    // The publisher names this very pane, but for a different session: the row
    // publishes a UUID of its own, so its tasks are not this publisher's.
    app.apply_bus_event(BusEvent::Connected {
        session: OTHER_SESSION.into(),
        pane: Some("wH:p1".into()),
    });
    app.apply_bus_event(BusEvent::Tasks {
        session: OTHER_SESSION.into(),
        tasks: vec![task("bg-1", TaskState::Running)],
    });

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    assert!(!screen.contains("task:"), "{screen}");
}

#[test]
fn a_row_without_a_session_uuid_joins_by_the_hello_pane() {
    let state = fleet(&[("wH:p1", None, None)]);
    let mut app = app_for(&state);
    app.apply_bus_event(BusEvent::Connected {
        session: SESSION.into(),
        pane: Some("wH:p1".into()),
    });
    app.apply_bus_event(BusEvent::Tasks {
        session: SESSION.into(),
        tasks: vec![task("bg-1", TaskState::Running)],
    });

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    assert!(screen.contains("task: bg-1"), "{screen}");
}

#[test]
fn a_session_matching_no_row_is_shown_nowhere() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        OTHER_SESSION,
        vec![task("bg-9", TaskState::Running)],
    );

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    assert!(!screen.contains("task:"), "{screen}");
    assert!(!screen.contains("bg-9"), "{screen}");
}

#[test]
fn absent_optional_fields_draw_no_placeholder() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    publish(&mut app, SESSION, vec![task("bg-9", TaskState::Flushing)]);

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    assert!(screen.contains("task: bg-9"), "{screen}");
    assert!(screen.contains("flushing"), "{screen}");
    for absent in ["command (", "cwd (", "exit ", " B", "ago"] {
        assert!(
            !screen.contains(absent),
            "placeholder {absent:?}:\n{screen}"
        );
    }
}

#[test]
fn every_published_state_word_is_drawn_as_published() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    let mut review = task("bg-3", TaskState::Review);
    review.exit_code = Some(130);
    publish(
        &mut app,
        SESSION,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Flushing),
            review,
            task("bg-4", TaskState::Unknown("paused".into())),
            task("bg-5", TaskState::Unknown("pa\tused".into())),
        ],
    );

    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);
    for part in [
        "task: bg-1 · running",
        "task: bg-2 · flushing",
        "task: bg-3 · review",
        "exit 130",
        "task: bg-4 · paused",
        // A word the publisher wrote with a tab in it reaches the screen as
        // text, never as a control character.
        "task: bg-5 · pa used",
    ] {
        assert!(screen.contains(part), "missing {part:?}:\n{screen}");
    }
    assert!(!screen.contains('\t'), "{screen}");
}

#[test]
fn an_over_long_command_is_bounded_and_control_sequences_never_reach_the_screen() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    let mut running = task("bg-1", TaskState::Running);
    // A tail marker after the bound proves the value was truncated rather than
    // lost, and the colours around it prove the sequences were stripped.
    running.command = Some(format!("\x1b[31mnix build {}\x1b[0m TAIL", "x".repeat(400)));
    running.cwd = Some(format!("/tmp/{}", "y".repeat(400)));
    publish(&mut app, SESSION, vec![running]);

    select_agent(&mut app, "wH:p1");
    // The published command and directory are Tasks facts, and the long text a
    // disclosure holds: the bound is measured on that one page with its block
    // open, not on a union that multiplies the count.
    app.select_page(DetailPage::Tasks);
    disclose_all(&mut app);
    let screen = screen(&state, &app);
    assert!(
        !screen.contains('\x1b'),
        "a control sequence reached the screen"
    );
    let drawn = screen.matches('x').count();
    assert!(
        (1..400).contains(&drawn) && drawn > 200,
        "the command is bounded, not drawn whole: {drawn} characters\n{screen}"
    );
    let directories = screen.matches('y').count();
    assert!(
        (1..400).contains(&directories) && directories > 200,
        "the working directory is bounded too: {directories}\n{screen}"
    );
    assert!(screen.contains('…'), "the bound is stated: {screen}");
    assert!(
        !screen.contains("TAIL"),
        "the tail past the bound is not drawn"
    );
    assert!(screen.contains("command: "), "{screen}");
    // The long word wraps onto its own row, so only the label is contiguous.
    assert!(screen.contains("cwd:"), "{screen}");
}

#[test]
fn an_empty_list_is_connected_and_distinct_from_no_connection() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(2))]);
    let mut app = app_for(&state);
    select_agent(&mut app, "wH:p1");

    // No publisher: the pane tokens are the baseline, and nothing claims a
    // connection.
    let baseline = pages(&state, &mut app);
    assert!(
        baseline.contains("awaiting: 2 background tasks"),
        "{baseline}"
    );
    assert!(!baseline.contains("connected"), "{baseline}");

    // A live connection that publishes nothing unresolved is not silence.
    publish(&mut app, SESSION, vec![]);
    let connected = pages(&state, &mut app);
    assert!(
        connected.contains("bus: connected — no unresolved tasks"),
        "{connected}"
    );
}

#[test]
fn a_disconnect_restores_the_token_baseline() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(2))]);
    let mut app = app_for(&state);
    publish(&mut app, SESSION, vec![task("bg-1", TaskState::Running)]);
    select_agent(&mut app, "wH:p1");
    assert!(pages(&state, &mut app).contains("task: bg-1"));

    app.apply_bus_event(BusEvent::Disconnected {
        session: SESSION.into(),
    });
    let screen = pages(&state, &mut app);
    assert!(!screen.contains("task:"), "{screen}");
    assert!(!screen.contains("connected"), "{screen}");
    assert!(screen.contains("awaiting: 2 background tasks"), "{screen}");
}

#[test]
fn the_token_count_is_compared_on_running_tasks_only() {
    let cases = [
        // The tokens report one; the bus lists two running.
        (
            Some(1),
            vec![
                task("bg-1", TaskState::Running),
                task("bg-2", TaskState::Running),
            ],
        ),
        // The tokens report none and nothing is running: only a `review` task
        // is outstanding, which the token does not count.
        (Some(0), vec![task("bg-3", TaskState::Review)]),
        // Agreed: the tokens count exactly the running tasks.
        (
            Some(2),
            vec![
                task("bg-1", TaskState::Running),
                task("bg-2", TaskState::Running),
            ],
        ),
    ];
    for (reported, tasks) in cases {
        let state = fleet(&[("wH:p1", Some(SESSION), reported)]);
        let mut app = app_for(&state);
        publish(&mut app, SESSION, tasks);
        select_agent(&mut app, "wH:p1");
        let screen = pages(&state, &mut app);
        // The list is shown whatever the tokens say.
        assert!(
            screen.contains("task: bg-"),
            "reported {reported:?}:\n{screen}"
        );
        if reported == Some(1) {
            assert!(
                screen.contains("tokens: report 1 running; the bus lists 2"),
                "{screen}"
            );
        } else {
            assert!(
                !screen.contains("tokens:"),
                "reported {reported:?}:\n{screen}"
            );
        }
    }
}

#[test]
fn bus_data_survives_a_refresh_and_is_rebuilt_from_the_bus() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(1))]);
    let mut app = app_for(&state);
    // The subject is task rows, hidden by default in agents view.
    app.toggle_tasks();
    // The pane's own id is the baseline, held without any connection.
    let rows: Vec<_> = app
        .visible_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect();
    let token_rows = rows.clone();

    publish(&mut app, SESSION, vec![task("bg-1", TaskState::Running)]);
    // The publisher's task is a row of the tree now, and a refresh rebuilds it
    // from the bus state rather than losing it: the bus is not a fact of the
    // observation, so an observation refresh may not clear it.
    app.refresh(&state);
    let after: Vec<_> = app
        .visible_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(
        after, token_rows,
        "the task identities are the same either way"
    );
    assert!(matches!(after.last(), Some(RowId::Task(_))), "{after:?}");

    select_agent(&mut app, "wH:p1");
    assert!(pages(&state, &mut app).contains("task: bg-1"));
}

#[test]
fn a_bus_diagnostic_is_stated_beside_source_freshness() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    app.set_bus_diagnostic(Some(
        "bus socket /run/user/1000/agent-radar/radar.sock: another Radar owns it".into(),
    ));
    select_agent(&mut app, "wH:p1");
    let screen = pages(&state, &mut app);

    assert!(screen.contains("Fleet · current"), "{screen}");
    assert!(screen.contains("bus off"), "{screen}");
    assert!(screen.contains("source: current"), "{screen}");
    // The diagnostic wraps at the panel's width, so its parts are asserted
    // separately.
    for part in [
        "bus: unavailable — bus socket",
        "/run/user/1000/agent-radar/radar.sock",
        "another Radar owns",
    ] {
        assert!(screen.contains(part), "missing {part:?}:\n{screen}");
    }
    assert!(!screen.contains("UNAVAILABLE"), "{screen}");
    assert!(!screen.contains("source: unavailable"), "{screen}");
}

/// A running task with the long text a disclosure holds.
fn task_with_text(id: &str, command: &str, cwd: &str) -> Task {
    let mut task = task(id, TaskState::Running);
    task.command = Some(command.into());
    task.cwd = Some(cwd.into());
    task
}

#[test]
fn a_task_processes_page_names_its_published_pid_and_borrows_no_metrics() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(1))]);
    let mut app = app_for(&state);
    app.toggle_tasks();
    let mut running = task("bg-1", TaskState::Running);
    running.pid = Some(4242);
    publish(&mut app, SESSION, vec![running]);
    select_task(&mut app, "bg-1");
    app.select_page(DetailPage::Processes);

    let processes = screen(&state, &app);
    // The publisher's PID is named, with where it came from.
    assert!(processes.contains("pid: 4242"), "{processes}");
    assert!(processes.contains("source: bus"), "{processes}");
    // It arrives without the identity of the process it names, so nothing was
    // measured for it: the page says why instead of borrowing the row's or the
    // owner's metrics.
    assert!(processes.contains("metrics: unavailable"), "{processes}");
    assert!(processes.contains("birth identity"), "{processes}");
    assert!(!processes.contains("birth:"), "{processes}");
    assert!(!processes.contains("cpu:"), "{processes}");
    assert!(!processes.contains("rss:"), "{processes}");
    assert!(!processes.contains("descendant"), "{processes}");
}

#[test]
fn a_disclosure_target_follows_wrapped_rows_and_the_scroll() {
    // A label long enough that the page's own title wraps over two rows, so a
    // scroll that counted lines would move the rows below it by more than the
    // row it was asked for.
    let pane = "wH:p1-with-a-long-identifier-tail";
    let state = fleet(&[(pane, Some(SESSION), Some(1))]);
    let mut app = app_for(&state);
    app.toggle_tasks();
    // A command long enough that the panel collapses it: the block carries a
    // marker, and its command wraps over several rows once it is opened.
    let command = format!("nix build .#radar {}", "--verbose ".repeat(12));
    publish(
        &mut app,
        SESSION,
        vec![task_with_text("bg-1", &command, "/home/x/proj")],
    );
    select_agent(&mut app, pane);
    app.select_page(DetailPage::Tasks);

    // Closed, the block is one row, and its marker is drawn on it. Opening it is
    // the click on that row, and nothing else.
    let (screen, geometry) = draw_at(&state, &mut app, 120, 10);
    let (key, marker) = geometry
        .disclosure_markers
        .first()
        .expect("the task block has a marker")
        .clone();
    assert_eq!(
        glyph_at(&screen, (marker.x, marker.y)),
        Some('\u{25b8}'),
        "the target is on the glyph:\n{screen}"
    );
    click_at(&mut app, (marker.x, marker.y));
    assert!(app.disclosure_open(&key), "the glyph's row opens its block");

    // Opened, its command wraps, so the page occupies more rows than the panel
    // has and has to scroll.
    let (opened, geometry) = draw_at(&state, &mut app, 120, 10);
    let content = geometry.details.expect("the panel is drawn");
    assert!(
        geometry.details_rows > geometry.details_viewport as usize,
        "the page is taller than the panel: {geometry:?}"
    );
    let block = geometry
        .disclosure_markers
        .iter()
        .find(|(marker_key, _)| marker_key == &key)
        .expect("the open block has a marker")
        .1;
    assert_eq!(
        glyph_at(&opened, (block.x, block.y)),
        Some('\u{25be}'),
        "the open block's target is on its glyph:\n{opened}"
    );

    // Scrolling by one row moves the page by exactly one drawn row, wrapped
    // lines included: every row is where the row below it was.
    app.scroll_page(1);
    assert_eq!(app.details_scroll(), 1);
    let (scrolled, geometry) = draw_at(&state, &mut app, 120, 10);
    let content_now = geometry.details.expect("the panel is drawn");
    assert_eq!(content_now, content, "the panel kept its size");
    let before: Vec<&str> = opened.lines().collect();
    let after: Vec<&str> = scrolled.lines().collect();
    // The page's own rows: below the panel's frame and its row of page tabs. Only
    // the panel's columns move, so the fleet's are left out of the comparison.
    let first = geometry.detail_tabs[0].expect("the pages are tabbed").y + 1;
    let panel_row = |screen: &[&str], row: u16| -> String {
        screen[row as usize]
            .chars()
            .skip(content.x as usize)
            .take(content.width as usize)
            .collect()
    };
    for row in first..content.bottom() - 2 {
        assert_eq!(
            panel_row(&after, row),
            panel_row(&before, row + 1),
            "row {row} is the row below it after one row of scroll:\n{scrolled}"
        );
    }
    let moved = geometry
        .disclosure_markers
        .iter()
        .find(|(marker_key, _)| marker_key == &key)
        .expect("the open block is still on screen")
        .1;
    assert_eq!(moved.y, block.y - 1, "by the row it was asked for");
    assert_eq!(glyph_at(&scrolled, (moved.x, moved.y)), Some('\u{25be}'));

    // The row the marker used to occupy answers to nothing, and the row it is on
    // now closes exactly the block it labels.
    click_at(&mut app, (block.x, block.y));
    assert!(
        app.disclosure_open(&key),
        "a position the marker left does not toggle its block"
    );
    click_at(&mut app, (moved.x, moved.y));
    assert!(!app.disclosure_open(&key), "the glyph's row closes it");

    // The terminal narrows until the panel stacks under the fleet, and the page
    // wraps further still: the target is on the glyph there too.
    let (stacked, geometry) = draw_at(&state, &mut app, 46, 24);
    let closed = geometry
        .disclosure_markers
        .iter()
        .find(|(marker_key, _)| marker_key == &key)
        .expect("the marker is drawn on a stacked panel")
        .1;
    assert!(
        is_inside(
            geometry.details.expect("the panel is drawn"),
            (closed.x, closed.y)
        ),
        "the marker is drawn in the panel it belongs to: {closed:?}"
    );
    assert_eq!(
        glyph_at(&stacked, (closed.x, closed.y)),
        Some('\u{25b8}'),
        "the target follows the glyph after a resize:\n{stacked}"
    );
    click_at(&mut app, (closed.x, closed.y));
    assert!(app.disclosure_open(&key), "the stacked marker opens it");
}

/// Whether a position is inside a drawn area.
fn is_inside(area: ratatui::layout::Rect, at: (u16, u16)) -> bool {
    at.0 >= area.x && at.0 < area.right() && at.1 >= area.y && at.1 < area.bottom()
}

#[test]
fn a_task_block_hides_only_its_command_and_directory() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    let mut running = task_with_text("bg-1", "nix build .#radar", "/home/x/proj");
    running.started_at = Some(now_unix_ms() - 158_000);
    running.output_bytes = Some(18_244);
    running.exit_code = Some(0);
    publish(&mut app, SESSION, vec![running]);
    select_agent(&mut app, "wH:p1");
    app.select_page(DetailPage::Tasks);

    // Closed: the facts a reader compares down the list stay drawn — identity,
    // state and the measures beside them — and the text that wraps over the
    // panel is the part behind the marker.
    let (collapsed, geometry) = drawn(&state, &app);
    assert!(collapsed.contains("▸ task: bg-1 · running"), "{collapsed}");
    assert!(collapsed.contains("2m38s"), "{collapsed}");
    assert!(collapsed.contains("18244 B"), "{collapsed}");
    assert!(collapsed.contains("exit 0"), "{collapsed}");
    assert!(!collapsed.contains("command:"), "{collapsed}");
    assert!(!collapsed.contains("cwd:"), "{collapsed}");
    let markers = geometry.disclosure_markers;
    assert_eq!(markers.len(), 1, "one block to open: {markers:?}");
    let details = geometry.details.expect("the panel is drawn");
    let (key, marker) = &markers[0];
    assert!(
        marker.x >= details.x
            && marker.right() <= details.right()
            && marker.y >= details.y
            && marker.y < details.bottom(),
        "the marker is drawn in the panel it belongs to: {marker:?} in {details:?}"
    );

    // Opened, and only through the interface a reader has.
    assert!(!app.disclosure_open(key));
    app.toggle_block(key);
    assert!(app.disclosure_open(key));
    let (opened, _) = drawn(&state, &app);
    assert!(opened.contains("▾ task: bg-1 · running"), "{opened}");
    assert!(opened.contains("command: nix build .#radar"), "{opened}");
    assert!(opened.contains("cwd: /home/x/proj"), "{opened}");
    assert!(opened.contains("exit 0"), "{opened}");
}

#[test]
fn a_task_with_nothing_long_to_hide_draws_no_marker() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    publish(&mut app, SESSION, vec![task("bg-1", TaskState::Running)]);
    select_agent(&mut app, "wH:p1");
    app.select_page(DetailPage::Tasks);

    // The task line is drawn as it always was, and the marker is not: a glyph
    // that opens nothing is a target the pointer would answer with a block that
    // is not there.
    let (screen, geometry) = drawn(&state, &app);
    assert!(screen.contains("task: bg-1 · running"), "{screen}");
    assert!(app.disclosures().is_empty(), "nothing to open");
    assert!(
        geometry.disclosure_markers.is_empty(),
        "a marker with nothing behind it: {:?}\n{screen}",
        geometry.disclosure_markers
    );
}

#[test]
fn an_opened_task_block_survives_its_task_and_leaves_with_it() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        SESSION,
        vec![task_with_text("bg-1", "nix build", "/home/x")],
    );
    select_agent(&mut app, "wH:p1");
    app.select_page(DetailPage::Tasks);
    let block = app.disclosures().pop().expect("the task has text to open");
    app.toggle_block(&block);
    assert!(app.disclosure_open(&block));
    assert!(screen(&state, &app).contains("command: nix build"));

    // The publisher publishes its list again with the same task in it, and the
    // next poll refreshes against the same observation: the task is the same
    // one, so the block the reader opened is still theirs.
    app.apply_bus_event(BusEvent::Tasks {
        session: SESSION.into(),
        tasks: vec![task_with_text("bg-1", "nix build", "/home/x")],
    });
    app.refresh(&state);
    assert_eq!(app.disclosures(), vec![block.clone()]);
    assert!(app.disclosure_open(&block));

    // A list that no longer carries the task takes its block with it, and the
    // task that replaced it starts closed.
    publish(
        &mut app,
        SESSION,
        vec![task_with_text("bg-2", "cargo test", "/home/y")],
    );
    assert!(
        !app.disclosure_open(&block),
        "a task that left left its expansion behind"
    );
    let replaced = app.disclosures();
    assert_eq!(replaced.len(), 1, "{replaced:?}");
    assert!(!app.disclosure_open(&replaced[0]));
    let screen = screen(&state, &app);
    assert!(screen.contains("task: bg-2"), "{screen}");
    assert!(!screen.contains("command: cargo test"), "{screen}");
}
