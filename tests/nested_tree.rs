//! The normalized task projection: which facts a task row is built from, what
//! identity it carries, and how the rows and the selection move when a
//! publisher's list changes.
//!
//! These drive the real path — [`FleetTree::build`] plus
//! [`FleetTree::attach_tasks`] through [`App::refresh`] and
//! [`App::apply_bus_event`] — so the join, the child rows, the parent badge and
//! the detail summary are exercised together rather than asserted apart.

use agent_radar::Target;
use agent_radar::app::DetailPage;
use agent_radar::bus::{BusEvent, Task, TaskState};
use agent_radar::config::Config;
use agent_radar::control_plane::registry::PublicSpawnTopology;
use agent_radar::focus;
use agent_radar::model::{
    AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Lineage, Location, Pane,
    RuntimeStatus, SessionIdentity, SessionUuid, Tab, Workspace,
};
use agent_radar::registry_observation::{self, RegistryAgent, RegistryRead, RegistryView};
use agent_radar::runtime::BackendIdentity;
use agent_radar::tree::{RowKind, TaskId, TaskProjection, TaskRow, TaskSource, TreeNode};
use agent_radar::{Action, App, Geometry, ObservationState, Operation, PaneView, RowId, theme, ui};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use serde_json::{Value, json};

/// A session UUID the tests publish under, from one hex digit: the literal
/// shape a publisher sends, without the file carrying a real-looking id.
fn session(digit: char) -> &'static str {
    let group = |count: usize| digit.to_string().repeat(count);
    Box::leak(
        format!(
            "{}-{}-4{}-8{}-{}",
            group(8),
            group(4),
            group(3),
            group(3),
            group(12)
        )
        .into_boxed_str(),
    )
}

/// One agent row in a test fleet.
struct Agent<'a> {
    pane_id: &'a str,
    session: Option<&'a str>,
    parent: Option<&'a str>,
    /// The pane's unresolved task entries, as its own tokens publish them.
    tokens: &'a [&'a str],
    running: Option<u32>,
}

impl<'a> Agent<'a> {
    fn new(pane_id: &'a str) -> Self {
        Self {
            pane_id,
            session: None,
            parent: None,
            tokens: &[],
            running: None,
        }
    }

    fn session(mut self, session: &'a str) -> Self {
        self.session = Some(session);
        self
    }

    fn parent(mut self, parent: &'a str) -> Self {
        self.parent = Some(parent);
        self
    }

    fn tokens(mut self, tokens: &'a [&'a str]) -> Self {
        self.tokens = tokens;
        self
    }

    fn running(mut self, running: u32) -> Self {
        self.running = Some(running);
        self
    }
}

/// A one-workspace observation: every agent in one tab, with the lineage it
/// publishes and the task entries its pane tokens carry, and the panes the
/// inventory reports beside them.
fn observation(agents: &[Agent<'_>], panes: &[&str]) -> FleetObservation {
    FleetObservation {
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
        workspaces: vec![Workspace {
            workspace_id: "wH".into(),
            label: Some("home".into()),
            number: None,
            session_path: None,
        }],
        tabs: vec![Tab {
            tab_id: "wH:t1".into(),
            workspace_id: "wH".into(),
            label: None,
            number: None,
        }],
        panes: panes
            .iter()
            .map(|pane_id| Pane {
                location: location(pane_id),
                label: None,
                title: None,
                current_path: None,
            })
            .collect(),
        agents: agents
            .iter()
            .map(|agent| AgentObservation {
                location: location(agent.pane_id),
                name: Some("pi".into()),
                label: Some(format!("worker {}", agent.pane_id)),
                status: Some(RuntimeStatus::Working),
                session: agent
                    .session
                    .and_then(SessionUuid::parse)
                    .map(SessionIdentity::Uuid),
                lineage: agent.session.map(|uuid| Lineage {
                    session: SessionUuid::parse(uuid).expect("test UUID"),
                    parent: agent.parent.and_then(SessionUuid::parse),
                }),
                facts: HerdsmanFacts {
                    background_running: agent.running,
                    background_tasks: agent
                        .tokens
                        .iter()
                        .map(|entry| (*entry).to_string())
                        .collect(),
                    ..HerdsmanFacts::default()
                },
                registry: agent.session.map(|session| {
                    let registration_id = format!("test-{}", agent.pane_id);
                    let mut registered = registration(
                        &registration_id,
                        json!({
                            "backend": "test",
                            "workspace": "wH",
                            "tab": "wH:t1",
                            "pane": agent.pane_id
                        }),
                        false,
                        Vec::new(),
                    );
                    registered.context = Some(
                        serde_json::from_value(json!({
                            "version": 1,
                            "registration_id": registration_id,
                            "writer": {
                                "source": "herdsman",
                                "incarnation": REGISTRY_INCARNATION,
                                "generation": 1,
                                "sequence": 1
                            },
                            "context": {
                                "source": "herdsman",
                                "incarnation": REGISTRY_INCARNATION,
                                "generation": 1,
                                "sequence": 1,
                                "lease_ms": 30_000,
                                "received_at": "2026-01-01T00:00:00.000Z",
                                "expires_at": "2026-01-01T00:00:30.000Z",
                                "restored": false,
                                "freshness": "fresh",
                                "session": session
                            }
                        }))
                        .expect("fresh registry context fixture"),
                    );
                    RegistryView {
                        agent: registered,
                        binding: registry_observation::Binding::Joined,
                        spawn: None,
                        managed: None,
                    }
                }),
            })
            .collect(),
    }
}

/// A one-workspace fleet of agents, each pane reported by an agent.
fn fleet(agents: &[Agent<'_>]) -> ObservationState {
    let panes: Vec<&str> = agents.iter().map(|agent| agent.pane_id).collect();
    let mut state = ObservationState::new();
    state.apply_success(observation(agents, &panes));
    state
}

/// A fleet whose agent on `pane_id` has stopped being reported while its pane
/// stayed: its facts are last-observed from here on.
fn retained(pane_id: &str, tokens: &[&str]) -> ObservationState {
    let mut state = ObservationState::new();
    state.apply_success(observation(
        &[Agent::new(pane_id).tokens(tokens)],
        &[pane_id],
    ));
    state.apply_success(observation(&[], &[pane_id]));
    state.apply_evidence(pane_id, ForegroundEvidence::Shell);
    state
}

fn location(pane_id: &str) -> Location {
    Location {
        workspace_id: "wH".into(),
        tab_id: "wH:t1".into(),
        pane_id: pane_id.into(),
    }
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
fn publish(app: &mut App, session: &str, pane: Option<&str>, tasks: Vec<Task>) {
    app.apply_bus_event(BusEvent::Connected {
        session: session.into(),
        pane: pane.map(str::to_string),
    });
    publish_list(app, session, tasks);
}

/// A replacement on a live connection: one full list per change, no `hello`.
fn publish_list(app: &mut App, session: &str, tasks: Vec<Task>) {
    app.apply_bus_event(BusEvent::Tasks {
        session: session.into(),
        tasks,
    });
}

/// The projection a row holds, for the assertions about the source rather than
/// about the rows.
fn projection(app: &App, pane_id: &str) -> TaskProjection {
    agent_node(&app.tree().roots, pane_id)
        .map(|node| match &node.row.kind {
            RowKind::Agent(agent) => agent.tasks.clone(),
            _ => TaskProjection::default(),
        })
        .unwrap_or_default()
}

fn app_for(state: &ObservationState) -> App {
    // Installed before any draw in this binary: the configuration is global to
    // the process, so every test installs the same one before it can render.
    install_test_theme();
    let mut app = App::new();
    app.refresh(state);
    app
}

/// The app with background-task rows explicitly listed. The agents view hides
/// them by default, so the tests whose subject is a task row's identity,
/// rendering, selection or focus use this rather than relying on the old
/// always-visible default.
fn app_with_tasks(state: &ObservationState) -> App {
    let mut app = app_for(state);
    app.toggle_tasks();
    app
}

/// The node of the agent row on `pane_id`.
fn agent_node<'a>(nodes: &'a [TreeNode], pane_id: &str) -> Option<&'a TreeNode> {
    for node in nodes {
        if node.row.id == RowId::Agent(pane_id.to_string()) {
            return Some(node);
        }
        if let Some(found) = agent_node(&node.children, pane_id) {
            return Some(found);
        }
    }
    None
}

/// The task rows the tree hangs under the agent on `pane_id`.
fn tasks(app: &App, pane_id: &str) -> Vec<TaskRow> {
    projection(app, pane_id).tasks
}

fn ids(rows: &[TaskRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.id.as_str()).collect()
}

fn phases(rows: &[TaskRow]) -> Vec<Option<&str>> {
    rows.iter().map(|row| row.phase.as_deref()).collect()
}

/// Selects the row with this identity, which must be visible.
fn select(app: &mut App, id: &RowId) {
    let index = {
        let rows = app.visible_rows();
        rows.iter()
            .position(|row| row.id == id)
            .unwrap_or_else(|| panic!("{id:?} is not visible"))
    };
    app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
    assert_eq!(app.selected_index(), Some(index));
}

fn task_id(owner: &str, session: Option<&str>, id: &str) -> RowId {
    RowId::Task(TaskId {
        owner: owner.into(),
        session: session.map(str::to_string),
        id: id.into(),
    })
}

/// The whole screen, one trimmed row per line.
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

/// Every detail page in turn, concatenated, with every disclosure the page
/// offers open: a test about a fact does not have to know which page or which
/// block carries it, and a fact that goes missing from all of them still fails.
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

#[test]
fn a_connected_list_joins_by_exact_session_and_keeps_published_phases() {
    let state = fleet(&[Agent::new("wH:p1")
        .session(session('1'))
        .tokens(&["bg-9:review"])]);
    let mut app = app_for(&state);
    let mut rich = task("bg-1", TaskState::Running);
    rich.command = Some("nix build .#radar".into());
    rich.pid = Some(48_213);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            rich,
            task("bg-2", TaskState::Review),
            task("bg-3", TaskState::Unknown("paused".into())),
        ],
    );

    let rows = tasks(&app, "wH:p1");
    // Published order, and every phase word kept as published.
    assert_eq!(ids(&rows), ["bg-1", "bg-2", "bg-3"]);
    assert_eq!(
        phases(&rows),
        [Some("running"), Some("review"), Some("paused")]
    );
    assert!(rows.iter().all(|row| row.source == TaskSource::Bus));
    assert!(rows.iter().all(|row| !row.last_observed));
    assert_eq!(rows[0].id.session.as_deref(), Some(session('1')));
    assert_eq!(
        rows[0]
            .published
            .as_ref()
            .expect("a bus task carries its published record")
            .command
            .as_deref(),
        Some("nix build .#radar")
    );
    // A matched list is authoritative: the pane's own ids are not unioned in.
    assert!(!ids(&rows).contains(&"bg-9"));

    // The badge counts the same rows, not the pane's tokens.
    select(&mut app, &RowId::Agent("wH:p1".into()));
    let screen = pages(&state, &mut app);
    assert!(screen.contains("3 bg"), "{screen}");
    assert!(screen.contains("task: bg-1"), "{screen}");
    assert!(!screen.contains("bg-9"), "{screen}");
}

#[test]
fn without_a_publisher_the_rows_are_the_panes_own_ids_and_phases() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1')).tokens(&[
        "bg-2:running",
        "bg-1:review",
        "bg-1",
        "wH:bg-9:quiescing",
        "bg-7",
    ])]);
    let mut app = app_for(&state);
    let rows = tasks(&app, "wH:p1");

    // Published order, each id once, an id that contains a colon intact, and a
    // phase word Radar does not know kept as published.
    assert_eq!(ids(&rows), ["bg-2", "bg-1", "wH:bg-9", "bg-7"]);
    assert_eq!(
        phases(&rows),
        [Some("running"), Some("review"), Some("quiescing"), None]
    );
    assert!(rows.iter().all(|row| row.source == TaskSource::Tokens));
    assert!(rows.iter().all(|row| row.published.is_none()));
    assert_eq!(rows[0].id.session.as_deref(), Some(session('1')));
    assert_eq!(rows[0].basis(), "tokens");
    // Nothing is invented for a row the tokens alone describe.
    assert!(!rows[0].is_running() || rows[0].phase.as_deref() == Some("running"));

    select(&mut app, &RowId::Agent("wH:p1".into()));
    let screen = pages(&state, &mut app);
    assert!(screen.contains("4 bg"), "{screen}");
    assert!(
        screen.contains("background: bg-2:running, bg-1:review"),
        "{screen}"
    );
}

#[test]
fn a_connected_empty_list_wins_over_the_token_ids() {
    let state = fleet(&[Agent::new("wH:p1")
        .session(session('1'))
        .tokens(&["bg-1:review", "bg-2:running"])
        .running(1)]);
    let mut app = app_for(&state);
    // The baseline is the pane's own report...
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-1", "bg-2"]);
    // ...and a connected publisher that reports nothing unresolved replaces it
    // entirely, because the two are not the same fact.
    publish(&mut app, session('1'), None, Vec::new());
    assert!(tasks(&app, "wH:p1").is_empty());
    assert_eq!(projection(&app, "wH:p1").source, Some(TaskSource::Bus));

    select(&mut app, &RowId::Agent("wH:p1".into()));
    let screen = pages(&state, &mut app);
    assert!(
        screen.contains("bus: connected — no unresolved tasks"),
        "{screen}"
    );
    assert!(!screen.contains("bg-1"), "{screen}");
    assert!(!screen.contains("2 bg"), "{screen}");
    // The token count is still the pane's own, and is compared on the same
    // projected list: nothing is running, so the disagreement is stated.
    assert!(screen.contains("the bus lists 0"), "{screen}");
}

#[test]
fn a_disconnect_returns_the_row_to_its_token_ids() {
    let state = fleet(&[Agent::new("wH:p1")
        .session(session('1'))
        .tokens(&["bg-7:review"])]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-1"]);

    app.apply_bus_event(BusEvent::Disconnected {
        session: session('1').into(),
    });
    let rows = tasks(&app, "wH:p1");
    assert_eq!(ids(&rows), ["bg-7"]);
    assert_eq!(rows[0].source, TaskSource::Tokens);
    assert_eq!(rows[0].phase.as_deref(), Some("review"));
    // A gone publisher permits the pane's own report again — and proves nothing
    // about the tasks having ended.
    select(&mut app, &RowId::Agent("wH:p1".into()));
    let screen = pages(&state, &mut app);
    assert!(screen.contains("background: bg-7:review"), "{screen}");
    assert!(!screen.contains("connected"), "{screen}");
}

#[test]
fn publisher_hello_pane_is_never_used_to_join_tasks() {
    // A row without a published session UUID cannot receive a publisher's
    // task list, even when the legacy hello names its pane.
    let state = fleet(&[Agent::new("wH:p1")]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        Some("wH:p1"),
        vec![task("bg-1", TaskState::Running)],
    );
    assert!(tasks(&app, "wH:p1").is_empty());
    assert_eq!(projection(&app, "wH:p1").source, None);

    // A second publisher naming the same pane is still not a join candidate.
    publish(
        &mut app,
        session('2'),
        Some("wH:p1"),
        vec![task("bg-2", TaskState::Running)],
    );
    assert!(tasks(&app, "wH:p1").is_empty());
    assert_eq!(projection(&app, "wH:p1").source, None);
}

#[test]
fn an_exact_session_is_the_only_join_for_a_row_that_publishes_one() {
    let state = fleet(&[Agent::new("wH:p1").session(session('2')).tokens(&["bg-5"])]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        Some("wH:p1"),
        vec![task("bg-1", TaskState::Running)],
    );
    // The publisher names this very pane, but for a session the row does not
    // publish: the pane fallback is not a second chance for it.
    let rows = tasks(&app, "wH:p1");
    assert_eq!(ids(&rows), ["bg-5"]);
    assert_eq!(rows[0].source, TaskSource::Tokens);
}

#[test]
fn a_session_matching_no_row_is_attached_to_no_agent() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('2'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    assert!(tasks(&app, "wH:p1").is_empty());
    assert_eq!(projection(&app, "wH:p1").source, None);
}

#[test]
fn the_same_id_under_two_sessions_is_two_identities() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2").session(session('2')),
    ]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Review)],
    );
    publish(
        &mut app,
        session('2'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );

    let first = tasks(&app, "wH:p1");
    let second = tasks(&app, "wH:p2");
    assert_eq!(first[0].id.session.as_deref(), Some(session('1')));
    assert_eq!(second[0].id.session.as_deref(), Some(session('2')));
    assert_ne!(first[0].id, second[0].id);

    // Selection holds an identity, and the identity it holds survives a
    // replacement of the same session's list.
    let selected = task_id("wH:p1", Some(session('1')), "bg-1");
    select(&mut app, &selected);
    publish_list(
        &mut app,
        session('1'),
        vec![task("bg-1", TaskState::Review)],
    );
    assert_eq!(app.selected_row().map(|row| row.id.clone()), Some(selected));
}

#[test]
fn a_changed_session_does_not_inherit_a_same_numbered_task() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-1"));

    // `/new` in the same pane: a new session UUID on its own connection, and
    // the same task id. It is a different task, so the old selection does not
    // transfer — it falls back to the agent the task hung under.
    app.refresh(&fleet(&[Agent::new("wH:p1").session(session('3'))]));
    publish(
        &mut app,
        session('3'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-1"]);
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(RowId::Agent("wH:p1".into()))
    );
}

#[test]
fn task_rows_follow_the_agents_own_children() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2")
            .session(session('2'))
            .parent(session('1')),
    ]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Review),
        ],
    );

    let node = agent_node(&app.tree().roots, "wH:p1").expect("the owner row");
    let children: Vec<RowId> = node
        .children
        .iter()
        .map(|child| child.row.id.clone())
        .collect();
    assert_eq!(
        children,
        vec![
            RowId::Agent("wH:p2".into()),
            task_id("wH:p1", Some(session('1')), "bg-1"),
            task_id("wH:p1", Some(session('1')), "bg-2"),
        ]
    );
}

#[test]
fn a_bus_event_changes_the_rows_without_a_refresh() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    assert!(tasks(&app, "wH:p1").is_empty());

    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-1"]);
    // A replacement list is applied as it arrives; nothing waits for a poll.
    publish_list(
        &mut app,
        session('1'),
        vec![task("bg-2", TaskState::Review)],
    );
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-2"]);
    // And an explicit empty list clears the rows on the live connection.
    publish_list(&mut app, session('1'), Vec::new());
    assert!(tasks(&app, "wH:p1").is_empty());
}

#[test]
fn selection_and_folds_survive_a_bus_replacement() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Running),
        ],
    );
    let selected = task_id("wH:p1", Some(session('1')), "bg-2");
    select(&mut app, &selected);

    // The same identity published again keeps the selection where it is, and
    // the phase it carries is the new one.
    publish_list(
        &mut app,
        session('1'),
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Review),
        ],
    );
    assert_eq!(app.selected_row().map(|row| row.id.clone()), Some(selected));
    assert_eq!(tasks(&app, "wH:p1")[1].phase.as_deref(), Some("review"));

    // A folded agent keeps its fold and hides its task rows, whatever the bus
    // publishes next.
    select(&mut app, &RowId::Agent("wH:p1".into()));
    app.toggle_fold();
    assert!(app.is_collapsed(&RowId::Agent("wH:p1".into())));
    publish_list(
        &mut app,
        session('1'),
        vec![task("bg-3", TaskState::Running)],
    );
    assert!(app.is_collapsed(&RowId::Agent("wH:p1".into())));
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-3"]);
    assert!(
        !app.visible_rows()
            .iter()
            .any(|row| matches!(row.id, RowId::Task(_))),
        "a folded branch draws no task rows"
    );
}

#[test]
fn a_removed_selected_task_falls_back_to_its_agent() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Running),
        ],
    );
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-1"));

    // The publisher resolves that task: its row goes, and the selection lands
    // on the agent row it hung under rather than on whatever took its place.
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-2", TaskState::Running)],
    );
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(RowId::Agent("wH:p1".into()))
    );

    // And with the agent row gone too, selection is never left dangling: the
    // workspace row is what is left to hold it.
    app.refresh(&fleet(&[]));
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(RowId::Workspace("wH".into()))
    );
}

#[test]
fn retained_token_facts_are_labelled_and_never_animate() {
    install_test_theme();

    // The agent stopped being reported while its pane stayed: its facts are
    // last-observed, so a `running` word is not a live process.
    let state = retained("wH:p1", &["bg-1:running"]);
    let app = app_with_tasks(&state);
    let rows = tasks(&app, "wH:p1");
    assert_eq!(ids(&rows), ["bg-1"]);
    assert!(rows[0].last_observed);
    assert_eq!(rows[0].basis(), "last-observed");
    assert!(!rows[0].is_running());
    assert!(!app.animates(), "a last-observed task never moves");
    // And the row says so: a still mark and the basis word, never a frame.
    let lines = lines_at(&state, &app, 120, 12, 0);
    let row = row_of(&lines, "bg-1");
    assert!(
        row.contains(&format!(
            "{} bg-1 · running · last-observed",
            theme::pane_mark()
        )),
        "{row:?}"
    );

    // A connected publisher on a row is current facts, and a running task is
    // what the command animation is for.
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    assert!(tasks(&app, "wH:p1")[0].is_running());
    assert!(app.animates(), "a live running task animates");
}

/// Herdr metadata for `pane_id`: a Pi-owned record, managed by its owner, that
/// the backend reports as containment evidence only.
fn owner_metadata(pane_id: &str) -> AgentObservation {
    AgentObservation {
        location: location(pane_id),
        name: Some("pi".into()),
        label: Some("owner task".into()),
        status: Some(RuntimeStatus::Working),
        session: None,
        lineage: None,
        facts: HerdsmanFacts {
            managed_metadata: true,
            ..HerdsmanFacts::default()
        },
        registry: None,
    }
}

/// A fleet whose only agent record is Herdr metadata, not a presented agent.
fn metadata_only(metadata: Vec<AgentObservation>, panes: &[&str]) -> FleetObservation {
    let mut observation = observation(&[], panes);
    observation.metadata_agents = metadata;
    observation
}

/// Metadata that appears and then vanishes while its pane survives must never
/// become an agent row, retained or current. The pane it was recorded against
/// still refuses a direct close: Radar cannot verify the occupant and has no owner
/// to route the stop to, so the refusal names that rather than guessing.
#[test]
fn vanished_metadata_shows_no_row_and_its_pane_refuses_a_direct_close() {
    let mut state = ObservationState::new();
    state.apply_success(metadata_only(vec![owner_metadata("wH:p1")], &["wH:p1"]));
    assert!(agent_node(&app_for(&state).tree().roots, "wH:p1").is_none());

    state.apply_success(metadata_only(Vec::new(), &["wH:p1"]));
    assert!(
        state.retained().is_empty(),
        "metadata reached display continuity"
    );
    assert!(state.retained_containment().contains_key("wH:p1"));

    let mut app = app_for(&state);
    assert!(agent_node(&app.tree().roots, "wH:p1").is_none());
    // Every pane shows, so the pane row is the close target.
    app.cycle_panes();
    app.cycle_panes();
    assert_eq!(app.pane_view(), PaneView::All);
    select(&mut app, &RowId::Pane("wH:p1".into()));
    // Nothing is reported running, so no question is drawn; the close itself is
    // refused because the pane's occupant cannot be verified as anyone's.
    assert!(app.begin_action(Operation::Delete, &state).is_none());
    assert!(app.confirmation().is_none(), "nothing was asked");
    assert!(
        app.focus_message()
            .is_some_and(|message| message.contains("managed by its owner")),
        "{:?}",
        app.focus_message()
    );
}

/// Installs the configuration the presentation tests draw with, once per test
/// binary: the animation set is global to the process, so every test that needs
/// it installs the same thing rather than racing a different config in.
fn install_test_theme() {
    theme::install(
        Config::parse(
            "[appearance]\nworking = \"none\"\ncommand = \"pulse\"\n\n[processes]\nnix = \"~\"\n",
        )
        .expect("the test config parses"),
    );
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The whole screen at a chosen size and clock step, one trimmed row per line.
fn lines_at(
    state: &ObservationState,
    app: &App,
    width: u16,
    height: u16,
    tick: usize,
) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("infallible test backend");
    terminal
        .draw(|frame| {
            ui::render(frame, state, app, &mut ListState::default(), tick);
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
        .collect()
}

/// One row's own text: the rendered line's tree panel without its margins and
/// borders, so an assertion reads from the tree's content edge, where the
/// branch prefix starts. The details panel beside it is cut away first, since
/// its title can repeat the row's text.
fn row_of(lines: &[String], needle: &str) -> String {
    // The details panel's left edge as a character column, read from the top
    // border the two panels share; rows below repeat that geometry.
    let boundary = lines.first().and_then(|top| {
        let mut corners = top.match_indices('┌');
        corners.next()?;
        let (byte, _) = corners.next()?;
        Some(top[..byte].chars().count())
    });
    for line in lines {
        let tree: String = match boundary {
            Some(boundary) => line.chars().take(boundary).collect(),
            None => line.clone(),
        };
        let Some(left) = tree.find('│') else {
            continue;
        };
        let row = tree[left + '│'.len_utf8()..].trim_end_matches('│');
        if row.contains(needle) {
            return row.to_string();
        }
    }
    panic!("no row with {needle:?}:\n{}", lines.join("\n"))
}

/// Unix milliseconds now, for the age a published start time implies.
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis() as u64
}

/// A three-agent fleet where the worker nests: the owner, its worker, and one
/// flat sibling so a branch both continues and ends.
fn nested_fleet() -> ObservationState {
    fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2")
            .session(session('2'))
            .parent(session('1')),
        Agent::new("wH:p3").session(session('3')),
    ])
}

#[test]
fn connectors_describe_the_visible_branch_not_the_raw_tree() {
    let state = nested_fleet();
    let app = app_for(&state);
    let lines = lines_at(&state, &app, 120, 12, 0);

    // A workspace root draws no connector of its own.
    assert!(row_of(&lines, "▾ home").starts_with("▾ "));
    // The owner has a following sibling and a nested worker: a branch connector
    // and a disclosure marker in the column the row metadata names.
    let owner = row_of(&lines, "worker wH:p1");
    assert!(owner.starts_with("├─▾ "), "{owner:?}");
    let rows = app.visible_rows();
    let owner_row = rows
        .iter()
        .find(|row| row.id == &RowId::Agent("wH:p1".into()))
        .expect("the owner row is visible");
    assert_eq!(owner_row.marker_column(), 2);
    assert_eq!(owner.chars().nth(2), Some('▾'), "{owner:?}");

    // The worker is the owner's only child, so it uses the last-child connector
    // and carries the owner's continuation line because a sibling follows it.
    let worker = row_of(&lines, "worker wH:p2");
    assert!(worker.starts_with("│ └─"), "{worker:?}");
    let worker_row = rows
        .iter()
        .find(|row| row.id == &RowId::Agent("wH:p2".into()))
        .expect("the worker row is visible");
    assert_eq!(worker_row.marker_column(), 4);

    // The last sibling ends its branch: no continuation behind it.
    assert!(row_of(&lines, "worker wH:p3").starts_with("└─  "));
}

#[test]
fn a_filtered_view_reconnects_its_surviving_siblings() {
    let state = nested_fleet();
    let mut app = app_for(&state);
    app.handle_key(key(KeyCode::Char('/')));
    for c in "worker wH:p2".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));

    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(!lines.iter().any(|line| line.contains("worker wH:p3")));
    // The owner is now the last surviving child, so no vertical line is drawn
    // for a sibling the filter hid.
    assert!(row_of(&lines, "worker wH:p1").starts_with("└─▾ "));
    assert!(row_of(&lines, "worker wH:p2").starts_with("  └─"));

    // Clearing the filter restores the continuation it replaced.
    app.handle_key(key(KeyCode::Esc));
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(row_of(&lines, "worker wH:p2").starts_with("│ └─"));
}

#[test]
fn folding_a_branch_hides_its_rows_and_leaves_no_continuation() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2").session(session('2')),
        Agent::new("wH:p4")
            .session(session('4'))
            .parent(session('2')),
        Agent::new("wH:p3").session(session('3')),
    ]);
    let mut app = app_for(&state);
    // Before folding, the worker's child carries the continuation line.
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(row_of(&lines, "worker wH:p4").starts_with("│ └─"));

    select(&mut app, &RowId::Agent("wH:p2".into()));
    app.toggle_fold();
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(!lines.iter().any(|line| line.contains("worker wH:p4")));
    let worker = row_of(&lines, "worker wH:p2");
    assert!(worker.starts_with("├─▸ "), "{worker:?}");
    // The row that hid a child leaves nothing vertical behind it.
    assert!(!worker.contains('│'), "{worker:?}");
}

#[test]
fn a_narrow_terminal_fits_nested_connectors_and_sanitized_task_text() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2").session(session('2')),
    ]);
    let mut app = app_with_tasks(&state);
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some(format!("\x1b[31mnix build {}\x1b[0m", "x".repeat(200)));
    publish(&mut app, session('1'), None, vec![running]);

    let lines = lines_at(&state, &app, 40, 24, 0);
    for line in &lines {
        assert!(
            !line.contains('\x1b'),
            "a control sequence reached the screen"
        );
        assert!(
            line.chars().count() <= 40,
            "row overflows its panel: {line:?}"
        );
    }
    // The task hangs under the owner, whose sibling follows it: the connector
    // and the continuation are both drawn in the narrow panel.
    let task_row = row_of(&lines, "nix build");
    assert!(task_row.starts_with("│ └─"), "{task_row:?}");
    // The command is bounded rather than drawn whole or as control text.
    assert!(task_row.contains('…'), "the bound is stated: {task_row:?}");
    assert!(!task_row.contains("x".repeat(50).as_str()), "{task_row:?}");
}

#[test]
fn a_task_row_shows_its_command_phase_age_and_program_mark() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    let now = now_unix_ms();
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some("nix build .#radar".into());
    running.started_at = Some(now - 65_000);
    publish(
        &mut app,
        session('1'),
        None,
        vec![running, task("bg-2", TaskState::Review)],
    );

    let frames = theme::command_frames().expect("the command marks move");
    assert_ne!(theme::frame(frames, 0), theme::frame(frames, 1));
    let first = lines_at(&state, &app, 120, 12, 0);
    let second = lines_at(&state, &app, 120, 12, 1);

    // The published command, the configured program mark, the phase as
    // published and the age the start time implies.
    let task_row = row_of(&first, "nix build .#radar");
    assert!(
        task_row.contains(&format!(
            "{} ~ nix build .#radar · running · 1m05s",
            theme::frame(frames, 0)
        )),
        "{task_row:?}"
    );
    // A process reported alive moves in the configured command frames.
    assert_ne!(
        row_of(&first, "nix build .#radar"),
        row_of(&second, "nix build .#radar"),
        "the running task mark advances with the clock"
    );

    // A task with no published command, start or process draws none of them.
    let review_row = row_of(&first, "bg-2");
    assert!(review_row.contains("bg-2 · review"), "{review_row:?}");
    assert!(!review_row.contains('~'), "{review_row:?}");
    assert!(!review_row.contains("ago"), "{review_row:?}");
}

#[test]
fn a_selected_task_shows_its_published_facts_and_the_parent_agrees() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1')).running(1)]);
    let mut app = app_with_tasks(&state);
    let now = now_unix_ms();
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some("nix build .#radar".into());
    running.cwd = Some("/home/x/proj".into());
    running.pid = Some(48_213);
    running.started_at = Some(now - 120_000);
    running.last_output_at = Some(now - 5_000);
    running.output_bytes = Some(9_412);
    running.exit_code = Some(0);
    publish(&mut app, session('1'), None, vec![running]);

    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-1"));
    let panel = pages(&state, &mut app);
    for part in [
        "kind: background task",
        "task: bg-1",
        "phase: running",
        "source: bus",
        "command: nix build .#radar",
        "cwd: /home/x/proj",
        "pid: 48213",
        "started: 2m00s ago",
        "last output: 5s ago",
        "output: 9412 B",
        "exit: 0",
        "owner: wH:p1",
    ] {
        assert!(panel.contains(part), "missing {part:?}:\n{panel}");
    }

    // The parent's panel states the same facts from the same projection.
    select(&mut app, &RowId::Agent("wH:p1".into()));
    let parent = pages(&state, &mut app);
    for part in [
        "task: bg-1 · running",
        "command: nix build .#radar",
        "cwd: /home/x/proj",
        "exit 0",
        "1 bg",
    ] {
        assert!(parent.contains(part), "missing {part:?}:\n{parent}");
    }
}

#[test]
fn a_task_with_no_published_facts_draws_no_placeholder() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-7", TaskState::Flushing)],
    );
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-7"));
    let screen = screen(&state, &app);
    for part in [
        "task: bg-7",
        "phase: flushing",
        "source: bus",
        "owner: wH:p1",
    ] {
        assert!(screen.contains(part), "missing {part:?}:\n{screen}");
    }
    for absent in [
        "command:",
        "cwd:",
        "pid:",
        "started:",
        "last output:",
        "output:",
        "exit:",
    ] {
        assert!(
            !screen.contains(absent),
            "placeholder {absent:?}:\n{screen}"
        );
    }
}

/// Where a draw would have put things, for the pointer tests: the panels this
/// view draws, wide enough that the details sit beside the tree.
fn geometry_of(app: &App) -> Geometry {
    Geometry {
        tree_panel: Rect::new(0, 0, 60, 20),
        tree_content: Rect::new(1, 1, 58, 18),
        details: app.shows_details().then(|| Rect::new(60, 0, 40, 20)),
        details_rows: 100,
        details_viewport: 15,
        detail_tabs: [
            Some(Rect::new(62, 1, 10, 1)),
            Some(Rect::new(72, 1, 12, 1)),
            Some(Rect::new(84, 1, 8, 1)),
            Some(Rect::new(92, 1, 8, 1)),
        ],
        offset: 0,
        disclosure_markers: Vec::new(),
        process_rows: Vec::new(),
        process_folds: Vec::new(),
        details_content: Rect::new(61, 2, 38, 17),
        confirm_cancel: None,
        confirm_confirm: None,
    }
}

/// A left click at a terminal cell, through the real mouse path.
fn click_at(app: &mut App, at: (u16, u16)) -> Option<Action> {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    })
}

fn visible_ids(app: &App) -> Vec<RowId> {
    app.visible_rows()
        .iter()
        .map(|row| row.id.clone())
        .collect()
}

#[test]
fn a_token_only_task_without_a_phase_draws_no_phase_field() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1')).tokens(&["bg-1"])]);
    let mut app = app_with_tasks(&state);
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-1"));
    let panel = screen(&state, &app);
    for part in ["task: bg-1", "source: tokens", "owner: wH:p1"] {
        assert!(panel.contains(part), "missing {part:?}:\n{panel}");
    }
    // The pane published an id with no phase, so there is nothing to state —
    // and an absent fact draws nothing rather than a placeholder.
    assert!(!panel.contains("phase:"), "no phase placeholder:\n{panel}");

    // A phase word is still drawn as published, familiar or not.
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-9", TaskState::Unknown("paused".into()))],
    );
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-9"));
    let panel = screen(&state, &app);
    assert!(panel.contains("phase: paused"), "{panel}");
}

#[test]
fn enter_on_a_task_focuses_its_owners_pane() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    // The row exists because of the bus alone: no second observation refresh.
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    select(&mut app, &task_id("wH:p1", Some(session('1')), "bg-1"));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        Some(Action::Focus(Target::Pane("wH:p1".into())))
    );
    assert_eq!(app.focus_message(), None);
}

#[test]
fn a_second_click_on_a_task_focuses_its_owners_pane() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    let geometry = geometry_of(&app);
    app.note_layout(geometry.clone());
    let task = task_id("wH:p1", Some(session('1')), "bg-1");
    let index = visible_ids(&app)
        .iter()
        .position(|id| *id == task)
        .expect("the bus-created task is drawn");
    let at = (
        geometry.tree_content.x + 8,
        geometry.tree_content.y + index as u16,
    );

    assert!(click_at(&mut app, at).is_none(), "the first click selects");
    assert_eq!(app.selected_row().map(|row| row.id.clone()), Some(task));
    assert_eq!(
        click_at(&mut app, at),
        Some(Action::Focus(Target::Pane("wH:p1".into())))
    );
}

#[test]
fn a_disclosure_click_folds_a_nested_branch_and_never_focuses() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2")
            .session(session('2'))
            .parent(session('1')),
        Agent::new("wH:p3")
            .session(session('3'))
            .parent(session('2')),
    ]);
    let mut app = app_for(&state);
    let geometry = geometry_of(&app);
    app.note_layout(geometry.clone());
    // Drawn rows: home, p1, p2, p3. p2 is nested, so its disclosure sits in the
    // second two-cell column of the prefix.
    let marker = (geometry.tree_content.x + 4, geometry.tree_content.y + 2);
    let before = app.selected_row().map(|row| row.id.clone());

    assert!(
        click_at(&mut app, marker).is_none(),
        "a marker click sends nothing to Herdr"
    );
    assert!(app.is_collapsed(&RowId::Agent("wH:p2".into())));
    assert!(
        !visible_ids(&app).contains(&RowId::Agent("wH:p3".into())),
        "the folded branch hides its child"
    );
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        before,
        "a marker click folds without selecting"
    );

    assert!(
        click_at(&mut app, marker).is_none(),
        "the same cell unfolds it"
    );
    assert!(!app.is_collapsed(&RowId::Agent("wH:p2".into())));

    // Two cells past the marker: select, then the second click focuses.
    let beside = (geometry.tree_content.x + 6, geometry.tree_content.y + 2);
    assert!(click_at(&mut app, beside).is_none());
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(RowId::Agent("wH:p2".into()))
    );
    assert_eq!(
        click_at(&mut app, beside),
        Some(Action::Focus(Target::Pane("wH:p2".into())))
    );
    assert!(!app.is_collapsed(&RowId::Agent("wH:p2".into())));
}

#[test]
fn a_task_leaf_has_no_disclosure_cell() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    let geometry = geometry_of(&app);
    app.note_layout(geometry.clone());
    let task = task_id("wH:p1", Some(session('1')), "bg-1");
    let index = visible_ids(&app)
        .iter()
        .position(|id| *id == task)
        .expect("the task is drawn");
    // The task's prefix ends in a blank leaf column; a click there is an
    // ordinary select, never a fold.
    let at = (
        geometry.tree_content.x + 4,
        geometry.tree_content.y + index as u16,
    );
    assert!(click_at(&mut app, at).is_none());
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(task.clone())
    );
    assert!(!app.is_collapsed(&task));
}

#[test]
fn a_scrolled_tree_maps_clicks_to_the_rows_drawn_there() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2").session(session('2')),
        Agent::new("wH:p3").session(session('3')),
    ]);
    let mut app = app_for(&state);
    let geometry = Geometry {
        offset: 2,
        ..geometry_of(&app)
    };
    app.note_layout(geometry.clone());
    // Rows: home, p1, p2, p3. With the offset applied, the first drawn line is
    // the third row, not the first.
    let drawn = visible_ids(&app)[2].clone();
    assert_eq!(drawn, RowId::Agent("wH:p2".into()));
    let at = (geometry.tree_content.x + 10, geometry.tree_content.y);
    assert!(click_at(&mut app, at).is_none());
    assert_eq!(app.selected_row().map(|row| row.id.clone()), Some(drawn));
}

#[test]
fn a_disclosure_click_does_nothing_while_a_filter_is_active() {
    let state = fleet(&[
        Agent::new("wH:p1").session(session('1')),
        Agent::new("wH:p2")
            .session(session('2'))
            .parent(session('1')),
    ]);
    let mut app = app_for(&state);
    app.handle_key(key(KeyCode::Char('/')));
    for c in "worker wH:p2".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));

    let geometry = geometry_of(&app);
    app.note_layout(geometry.clone());
    // The filtered view draws home, p1, p2 whatever the fold state, so a marker
    // click there could not take effect and is ignored.
    let marker = (geometry.tree_content.x + 2, geometry.tree_content.y + 1);
    assert!(click_at(&mut app, marker).is_none());
    assert!(!app.is_collapsed(&RowId::Agent("wH:p1".into())));
    assert!(visible_ids(&app).contains(&RowId::Agent("wH:p2".into())));
}

#[test]
fn a_task_command_filter_keeps_ancestry_and_restores_the_pre_filter_fold() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some("nix build .#radar".into());
    publish(&mut app, session('1'), None, vec![running]);

    select(&mut app, &RowId::Agent("wH:p1".into()));
    app.toggle_fold();
    assert!(app.is_collapsed(&RowId::Agent("wH:p1".into())));

    app.handle_key(key(KeyCode::Char('/')));
    for c in "nix build".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    // The task's command is what matched; its owner and workspace stay visible
    // to place it, even though the branch was folded.
    assert_eq!(
        visible_ids(&app),
        vec![
            RowId::Workspace("wH".into()),
            RowId::Agent("wH:p1".into()),
            task_id("wH:p1", Some(session('1')), "bg-1"),
        ]
    );

    // Clearing the filter brings the fold back.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.is_collapsed(&RowId::Agent("wH:p1".into())));
    assert!(
        !visible_ids(&app)
            .iter()
            .any(|id| matches!(id, RowId::Task(_))),
        "the folded branch draws no task rows again"
    );
}

#[test]
fn agent_jumps_ignore_task_rows() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    select(&mut app, &RowId::Workspace("wH".into()));
    // `w` is an agent-state jump: a running task row is not a working agent.
    app.handle_key(key(KeyCode::Char('w')));
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(RowId::Agent("wH:p1".into()))
    );
}

#[test]
fn task_rows_start_hidden_in_agents_view_and_shown_in_the_process_views() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Flushing),
            task("bg-3", TaskState::Review),
            task("bg-4", TaskState::Unknown("paused".into())),
        ],
    );
    let task_ids = |app: &App| {
        visible_ids(app)
            .into_iter()
            .filter(|id| matches!(id, RowId::Task(_)))
            .collect::<Vec<_>>()
    };

    // Agents view is the fleet: the children are hidden and the parent's
    // projection — badge and details — still holds every unresolved task.
    assert_eq!(app.pane_view(), PaneView::Hidden);
    assert!(!app.shows_tasks());
    assert!(task_ids(&app).is_empty());
    assert_eq!(ids(&tasks(&app, "wH:p1")), ["bg-1", "bg-2", "bg-3", "bg-4"]);

    // Running view is the operator's view of outstanding work: every unresolved
    // phase is listed, not only a phase whose process is still alive.
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::Running);
    assert!(app.shows_tasks());
    assert_eq!(
        task_ids(&app),
        vec![
            task_id("wH:p1", Some(session('1')), "bg-1"),
            task_id("wH:p1", Some(session('1')), "bg-2"),
            task_id("wH:p1", Some(session('1')), "bg-3"),
            task_id("wH:p1", Some(session('1')), "bg-4"),
        ]
    );

    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::All);
    assert!(app.shows_tasks());
    assert_eq!(task_ids(&app).len(), 4);
}

#[test]
fn a_token_only_task_is_hidden_in_agents_view_too() {
    // The fallback rows the pane's own tokens name follow the same view choice
    // as a publisher's, so a phase word with no process behind it is not leaked.
    let state = fleet(&[Agent::new("wH:p1")
        .session(session('1'))
        .tokens(&["bg-1:review"])]);
    let app = app_for(&state);
    assert!(!app.shows_tasks());
    assert!(
        !visible_ids(&app)
            .iter()
            .any(|id| matches!(id, RowId::Task(_)))
    );
}

#[test]
fn b_toggles_task_rows_in_the_current_view_and_each_view_keeps_its_choice() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    let has_task = |app: &App| {
        visible_ids(app)
            .iter()
            .any(|id| matches!(id, RowId::Task(_)))
    };

    // Agents view starts hidden; `b` shows the children without moving views.
    app.handle_key(key(KeyCode::Char('b')));
    assert_eq!(app.pane_view(), PaneView::Hidden);
    assert!(app.shows_tasks());
    assert!(has_task(&app));

    // Running view has its own default, and `b` hides only running.
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::Running);
    assert!(app.shows_tasks());
    app.handle_key(key(KeyCode::Char('b')));
    assert!(!has_task(&app));

    // All view is untouched by that: it still starts shown.
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::All);
    assert!(app.shows_tasks());

    // Cycling all the way round keeps each view's choice: agents still shows
    // what `b` asked it to, and running still hides what `b` asked it to.
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::Hidden);
    assert!(app.shows_tasks());
    app.handle_key(key(KeyCode::Char('p')));
    assert_eq!(app.pane_view(), PaneView::Running);
    assert!(!app.shows_tasks());
    assert!(!has_task(&app));
}

#[test]
fn a_hidden_task_leaves_no_rows_filter_matches_or_pointer_targets() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    let mut running = task("bg-1", TaskState::Running);
    running.command = Some("nix build .#radar".into());
    publish(&mut app, session('1'), None, vec![running]);
    let task = task_id("wH:p1", Some(session('1')), "bg-1");
    select(&mut app, &task);

    app.handle_key(key(KeyCode::Char('b')));
    let owner = RowId::Agent("wH:p1".into());
    // The hidden task is not a row: the selection falls back to its owner, and
    // the owner — whose only children were tasks — has no disclosure to fold.
    assert_eq!(
        app.selected_row().map(|row| row.id.clone()),
        Some(owner.clone())
    );
    let owner_row = app
        .visible_rows()
        .into_iter()
        .find(|row| row.id == &owner)
        .expect("the owner row is visible");
    assert!(!owner_row.has_children, "no disclosure for hidden children");

    // The line the task row occupied is no longer a pointer target.
    let geometry = geometry_of(&app);
    app.note_layout(geometry.clone());
    let index = visible_ids(&app)
        .iter()
        .position(|id| *id == owner)
        .expect("owner drawn");
    let below = (
        geometry.tree_content.x + 6,
        geometry.tree_content.y + index as u16 + 1,
    );
    assert!(click_at(&mut app, below).is_none());
    assert_eq!(app.selected_row().map(|row| row.id.clone()), Some(owner));

    // Filtering does not implicitly enable the hidden rows, even when the
    // task's own command is what was typed.
    app.handle_key(key(KeyCode::Char('/')));
    for c in "nix build".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.shows_tasks());
    assert!(
        visible_ids(&app).is_empty(),
        "the hidden task matched nothing"
    );
}

#[test]
fn parent_facts_stay_current_while_task_rows_are_hidden() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![
            task("bg-1", TaskState::Running),
            task("bg-2", TaskState::Review),
        ],
    );
    // Hidden, but the badge reads the current projection rather than a row list.
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(row_of(&lines, "worker wH:p1").contains("2 bg"));

    // A replacement list arrives while the rows are hidden; revealing later
    // shows that list, never the one that was hidden.
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-3", TaskState::Review)],
    );
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(row_of(&lines, "worker wH:p1").contains("1 bg"));
    assert!(!lines.iter().any(|line| line.contains("bg-1")));

    app.handle_key(key(KeyCode::Char('b')));
    let lines = lines_at(&state, &app, 120, 12, 0);
    assert!(row_of(&lines, "bg-3").contains("review"));
    assert!(!lines.iter().any(|line| line.contains("bg-1")));
}

#[test]
fn revealing_task_rows_does_not_unfold_an_agent() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_with_tasks(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );
    let owner = RowId::Agent("wH:p1".into());
    select(&mut app, &owner);
    app.toggle_fold();
    assert!(app.is_collapsed(&owner));

    app.handle_key(key(KeyCode::Char('b')));
    app.handle_key(key(KeyCode::Char('b')));
    // Showing the rows is not a reason to open a branch the user closed.
    assert!(app.is_collapsed(&owner));
    assert!(
        !visible_ids(&app)
            .iter()
            .any(|id| matches!(id, RowId::Task(_))),
        "the folded branch still hides its children"
    );
}

#[test]
fn b_while_entering_filter_text_is_filter_text() {
    let state = fleet(&[Agent::new("wH:p1").session(session('1'))]);
    let mut app = app_for(&state);
    publish(
        &mut app,
        session('1'),
        None,
        vec![task("bg-1", TaskState::Running)],
    );

    app.handle_key(key(KeyCode::Char('/')));
    app.handle_key(key(KeyCode::Char('b')));
    assert_eq!(app.filter_query(), "b");
    assert!(!app.shows_tasks(), "the agents view's default is untouched");

    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('b')));
    assert!(app.shows_tasks(), "outside entry it is the toggle again");
}

const REGISTRY_BOOT: &str = "3f1c2a9e-0000-4000-8000-000000000001";
const REGISTRY_INCARNATION: &str = "11111111-1111-4111-8111-111111111111";
const REGISTRY_SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// A registration as `agent.list` serves it, with the reports it holds. A
/// claimed registration names the birth identity the registry verifies.
fn registration(
    registration_id: &str,
    location: Value,
    claimed: bool,
    reports: Vec<Value>,
) -> RegistryAgent {
    let claim =
        claimed.then(|| json!({"boot_id": REGISTRY_BOOT, "pid": 4242, "start_ticks": 1_000}));
    let channel = |name: &str| {
        reports
            .iter()
            .find(|report| report["channel"] == name)
            .cloned()
            .unwrap_or(Value::Null)
    };
    serde_json::from_value(json!({
        "registration": {
            "version": 1,
            "registration_id": registration_id,
            "registered_at": "2026-01-01T00:00:00.000Z",
            "source": "herdsman",
            "incarnation": REGISTRY_INCARNATION,
            "session": REGISTRY_SESSION,
            "label": format!("{registration_id} label"),
            "location": location,
            "process_claimed": claimed,
            "launch": {"available": false}
        },
        "process": {"claimed": claimed, "claim": claim, "verification": "unavailable"},
        "execution": channel("execution"),
        "assignment": channel("assignment"),
        "facts": channel("facts")
    }))
    .expect("registration fixture decodes")
}

/// One channel's report at `freshness`. The owner publishes the assignment for
/// its own owner; a session publishes its execution and facts itself.
fn report(channel: &str, freshness: &str, snapshot: Value) -> Value {
    let (source, owner) = if channel == "assignment" {
        ("herdsman", Some("herdsman-owner"))
    } else {
        ("pi", None)
    };
    json!({
        "version": 1,
        "channel": channel,
        "writer": {"source": source, "incarnation": REGISTRY_INCARNATION, "generation": 1, "sequence": 3},
        "snapshot": {
            "source": source, "incarnation": REGISTRY_INCARNATION, "reporting_owner": owner,
            "generation": 1, "sequence": 3, "lease_ms": 30000,
            "received_at": "2026-01-01T00:00:00.000Z", "expires_at": "2026-01-01T00:00:30.000Z",
            "restored": false, "freshness": freshness, "snapshot": snapshot
        }
    })
}

/// A spawn edge that binds `child` under `parent`, as the daemon serves it.
fn bound_edge(parent: &str, child: &str) -> PublicSpawnTopology {
    serde_json::from_value(json!({
        "version": 2,
        "request_id": format!("request-{child}"),
        "parent": parent,
        "parent_session_id": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
        "exit_observation": {"state": "unknown"},
        "created": "completed",
        "launched": "completed",
        "bound": {"source": "test", "incarnation": REGISTRY_INCARNATION, "registration_id": child,
                  "bound_at": "2026-01-01T00:00:00.000Z"},
        "recorded_at": "2026-01-01T00:00:00.000Z",
        "state": "bound"
    }))
    .expect("a topology fixture")
}

fn at(pane_id: &str) -> Value {
    json!({"backend": "herdr", "workspace": "wA", "tab": "wA:t1", "pane": pane_id})
}

/// The node a row id names, anywhere in the tree.
fn node_of<'a>(nodes: &'a [TreeNode], id: &RowId) -> Option<&'a TreeNode> {
    nodes.iter().find_map(|node| {
        (node.row.id == *id)
            .then_some(node)
            .or_else(|| node_of(&node.children, id))
    })
}

fn wa_inventory(pane_ids: &[&str]) -> FleetObservation {
    FleetObservation {
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: Some("alpha".into()),
            number: None,
            session_path: None,
        }],
        tabs: Vec::new(),
        panes: pane_ids
            .iter()
            .map(|id| Pane {
                location: Location {
                    workspace_id: "wA".into(),
                    tab_id: "wA:t1".into(),
                    pane_id: (*id).into(),
                },
                label: None,
                title: None,
                current_path: None,
            })
            .collect(),
        agents: Vec::new(),
    }
}

fn registry_state(
    inventory: FleetObservation,
    agents: Vec<RegistryAgent>,
    topology: Vec<PublicSpawnTopology>,
) -> ObservationState {
    let read = RegistryRead {
        agents,
        topology,
        ..RegistryRead::default()
    };
    let agents =
        registry_observation::project(&inventory, &BackendIdentity::named("herdr"), read, &[]);
    let mut state = ObservationState::new();
    state.apply_success(FleetObservation {
        agents,
        ..inventory
    });
    state
}

/// Text of one registered row's details, every page open.
fn details(state: &ObservationState, id: &RowId) -> String {
    let mut app = app_for(state);
    select(&mut app, id);
    pages(state, &mut app)
}

/// Registrations that cannot join a pane stay distinct rows that own no pane:
/// the reported pane keeps its own row, and the controls they would reach are
/// refused.
#[test]
fn unresolved_registrations_stay_distinct_rows_that_own_no_pane() {
    let state = registry_state(
        wa_inventory(&["wA:p1"]),
        vec![
            registration("r1", at("wA:p1"), false, Vec::new()),
            registration("r2", at("wA:p1"), false, Vec::new()),
            // Names no place: Radar reaches only what is inside its own server, so
            // it is not a row at all.
            registration("r3", Value::Null, false, Vec::new()),
        ],
        Vec::new(),
    );
    let mut app = app_for(&state);
    {
        let tree = app.tree();
        for id in ["r1", "r2"] {
            assert!(
                node_of(&tree.roots, &RowId::Registered(id.into())).is_some(),
                "{id} lost its row"
            );
        }
        assert!(
            node_of(&tree.roots, &RowId::Registered("r3".into())).is_none(),
            "a registration naming no place in this server was shown"
        );
        assert!(
            node_of(&tree.roots, &RowId::Pane("wA:p1".into())).is_some(),
            "the pane lost its row"
        );
        assert!(
            node_of(&tree.roots, &RowId::Agent("wA:p1".into())).is_none(),
            "a registration claimed the pane"
        );
        assert!(
            node_of(&tree.roots, &RowId::Workspace(String::new())).is_none(),
            "an unplaced root appeared"
        );
        let targets = focus::targets(tree, &state);
        assert!(
            !targets.contains_key(&RowId::Registered("r1".into())),
            "an unresolved row focuses a pane"
        );
        assert_eq!(
            targets.get(&RowId::Pane("wA:p1".into())),
            Some(&Target::Pane("wA:p1".into()))
        );
    }

    select(&mut app, &RowId::Registered("r1".into()));
    app.begin_action(Operation::Delete, &state);
    assert!(
        app.confirmation().is_none(),
        "an unresolved agent was offered a close"
    );
    assert!(
        app.focus_message()
            .is_some_and(|message| message.contains("not joined to a pane")),
        "{:?}",
        app.focus_message()
    );
}

/// Each authority is its own line with the publisher named: the session's
/// execution, the owner's assignment, and the self facts. The owner's waiting
/// reason and outcome come with the assignment they describe.
#[test]
fn each_authority_is_its_own_line_with_its_publisher_named() {
    let state = registry_state(
        wa_inventory(&["wA:p1"]),
        vec![registration(
            "w",
            at("wA:p1"),
            false,
            vec![
                report("execution", "fresh", json!({"activity": "idle"})),
                report(
                    "assignment",
                    "fresh",
                    json!({
                        "activity": "waiting",
                        "waiting_reason": "awaiting review",
                        "last_outcome": {"result": "partial", "detail": "tests pending"}
                    }),
                ),
                report(
                    "facts",
                    "fresh",
                    json!({
                        "model": "gpt-x",
                        "background_tasks": [{"id": "bg-7", "phase": "running"}]
                    }),
                ),
            ],
        )],
        Vec::new(),
    );
    let text = details(&state, &RowId::Registered("w".into()));
    assert!(text.contains("execution: idle · from pi"), "{text}");
    assert!(
        text.contains("assignment: waiting · from herdsman for herdsman-owner"),
        "{text}"
    );
    assert!(text.contains("waiting reason: awaiting review"), "{text}");
    assert!(
        text.contains("last outcome: partial — tests pending"),
        "{text}"
    );
    assert!(text.contains("facts: fresh · from pi"), "{text}");
    assert!(text.contains("model: gpt-x"), "{text}");
    assert!(text.contains("1 background task"), "{text}");
    assert!(text.contains("state: waiting"), "{text}");
}

/// A session's `done` is its execution and nothing more: the owner's assignment
/// keeps its own value, and a registration with no reports says unknown on every
/// line rather than idle or done.
#[test]
fn execution_done_is_not_assignment_completion_and_silence_is_unknown() {
    let state = registry_state(
        wa_inventory(&["wA:p1"]),
        vec![
            registration(
                "done",
                at("wA:p1"),
                false,
                vec![
                    report("execution", "fresh", json!({"activity": "done"})),
                    report("assignment", "fresh", json!({"activity": "working"})),
                ],
            ),
            registration("silent", at("wA:p1"), false, Vec::new()),
        ],
        Vec::new(),
    );
    let done = details(&state, &RowId::Registered("done".into()));
    assert!(done.contains("execution: done · from pi"), "{done}");
    assert!(
        done.contains("assignment: working · from herdsman for herdsman-owner"),
        "{done}"
    );
    assert!(!done.contains("assignment: done"), "{done}");

    let silent = details(&state, &RowId::Registered("silent".into()));
    assert!(
        silent.contains("execution: unknown — nothing reported"),
        "{silent}"
    );
    assert!(
        silent.contains("assignment: unknown — nothing reported"),
        "{silent}"
    );
    assert!(silent.contains("facts: not reported"), "{silent}");
    assert!(silent.contains("state: unknown"), "{silent}");
}

/// Each channel carries its own freshness. An expired execution reads stale
/// while a fresh owner `lost` still decides the state, and an owner `settling`
/// is shown over an idle execution without replacing it.
#[test]
fn freshness_is_per_channel_and_the_owners_state_never_rewrites_execution() {
    let state = registry_state(
        wa_inventory(&["wA:p1"]),
        vec![
            registration(
                "stale",
                at("wA:p1"),
                false,
                vec![
                    report("execution", "stale", json!({"activity": "idle"})),
                    report("assignment", "fresh", json!({"activity": "lost"})),
                ],
            ),
            registration(
                "settles",
                at("wA:p1"),
                false,
                vec![
                    report("execution", "fresh", json!({"activity": "idle"})),
                    report("assignment", "fresh", json!({"activity": "settling"})),
                ],
            ),
        ],
        Vec::new(),
    );
    let lost = details(&state, &RowId::Registered("stale".into()));
    assert!(
        lost.contains("execution: stale — last report expired"),
        "{lost}"
    );
    assert!(
        lost.contains("assignment: lost · from herdsman for herdsman-owner"),
        "{lost}"
    );
    assert!(
        lost.contains("state: lost (the assignment reports lost)"),
        "{lost}"
    );

    let settling = details(&state, &RowId::Registered("settles".into()));
    assert!(settling.contains("execution: idle · from pi"), "{settling}");
    assert!(
        settling.contains("state: settling (the assignment reports settling)"),
        "{settling}"
    );
}

/// A spawn edge names its exact parent by registration id, so two registrations
/// that publish one session are two parents and only the named one is cited. A
/// parent this view does not hold is said to be absent, not guessed at.
#[test]
fn a_child_names_its_exact_parent_and_an_absent_parent_is_said_so() {
    let state = registry_state(
        wa_inventory(&["wA:p1", "wA:p2", "wA:p3", "wA:p4"]),
        vec![
            registration("lead", at("wA:p1"), false, Vec::new()),
            registration("other", at("wA:p2"), false, Vec::new()),
            registration("child", at("wA:p3"), false, Vec::new()),
            registration("orphan", at("wA:p4"), false, Vec::new()),
        ],
        vec![bound_edge("other", "child"), bound_edge("gone", "orphan")],
    );
    let child = details(&state, &RowId::Registered("child".into()));
    assert!(
        child.contains("spawn: child of other label (other) (bound)"),
        "{child}"
    );
    assert!(!child.contains("child of lead"), "{child}");

    let orphan = details(&state, &RowId::Registered("orphan".into()));
    assert!(
        orphan.contains("spawn: child of gone (not in this view) (bound)"),
        "{orphan}"
    );
}
