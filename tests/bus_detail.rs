//! Bus consumer tests: what the details show for a publisher's tasks, which
//! row they attach to, and what the panel says when the bus is not running.
//!
//! These drive the same path the executable does — [`App::apply_bus_event`]
//! feeding the join that [`ui::render`] draws — over a real observation rather
//! than a hand-built row, so a change to either half of the join fails here.

use std::time::{SystemTime, UNIX_EPOCH};

use agent_radar::bus::{BusEvent, Task, TaskState};
use agent_radar::model::{
    AgentObservation, FleetObservation, HerdsmanFacts, Lineage, Location, Pane, RuntimeStatus,
    SessionUuid, Tab, Workspace,
};
use agent_radar::{App, ObservationState, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
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

/// The whole screen, one trimmed row per line.
///
/// Wide enough that the details column reaches its maximum width, so a phrase
/// a test asserts sits on one row instead of being wrapped mid-phrase.
fn screen(state: &ObservationState, app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).expect("infallible test backend");
    terminal
        .draw(|frame| {
            ui::render(frame, state, app, &mut ListState::default(), 0);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let screen = screen(&state, &app);
    for part in [
        "task: bg-1",
        "running",
        "2m38s",
        "output 4s ago",
        "18244 B",
        "exit 0",
        "command (bg-1): nix build .#radar",
        "cwd (bg-1): /home/x/proj",
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
    let screen = screen(&state, &app);
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
    let screen = screen(&state, &app);
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
    let screen = screen(&state, &app);
    assert!(!screen.contains("task:"), "{screen}");
    assert!(!screen.contains("bg-9"), "{screen}");
}

#[test]
fn absent_optional_fields_draw_no_placeholder() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    publish(&mut app, SESSION, vec![task("bg-9", TaskState::Flushing)]);

    select_agent(&mut app, "wH:p1");
    let screen = screen(&state, &app);
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
    let screen = screen(&state, &app);
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
    assert!(screen.contains("command (bg-1): "), "{screen}");
    // The long word wraps onto its own row, so only the label is contiguous.
    assert!(screen.contains("cwd (bg-1):"), "{screen}");
}

#[test]
fn an_empty_list_is_connected_and_distinct_from_no_connection() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(2))]);
    let mut app = app_for(&state);
    select_agent(&mut app, "wH:p1");

    // No publisher: the pane tokens are the baseline, and nothing claims a
    // connection.
    let baseline = screen(&state, &app);
    assert!(
        baseline.contains("awaiting: 2 background tasks"),
        "{baseline}"
    );
    assert!(!baseline.contains("connected"), "{baseline}");

    // A live connection that publishes nothing unresolved is not silence.
    publish(&mut app, SESSION, vec![]);
    let connected = screen(&state, &app);
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
    assert!(screen(&state, &app).contains("task: bg-1"));

    app.apply_bus_event(BusEvent::Disconnected {
        session: SESSION.into(),
    });
    let screen = screen(&state, &app);
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
        let screen = screen(&state, &app);
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
fn bus_data_survives_a_refresh_and_changes_no_tree_row() {
    let state = fleet(&[("wH:p1", Some(SESSION), Some(1))]);
    let mut app = app_for(&state);
    let rows: Vec<_> = app
        .visible_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect();

    publish(&mut app, SESSION, vec![task("bg-1", TaskState::Running)]);
    // A refresh reconciles the observation; the bus is not a fact of it.
    app.refresh(&state);
    let after: Vec<_> = app
        .visible_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(rows, after);

    select_agent(&mut app, "wH:p1");
    assert!(screen(&state, &app).contains("task: bg-1"));
}

#[test]
fn a_bus_diagnostic_is_stated_beside_source_freshness() {
    let state = fleet(&[("wH:p1", Some(SESSION), None)]);
    let mut app = app_for(&state);
    app.set_bus_diagnostic(Some(
        "bus socket /run/user/1000/agent-radar/radar.sock: another Radar owns it".into(),
    ));
    select_agent(&mut app, "wH:p1");
    let screen = screen(&state, &app);

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
