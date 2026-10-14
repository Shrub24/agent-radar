use agent_radar::{
    app::App,
    config::Config,
    control_plane::lifecycle::State as LifecycleState,
    model::{AgentState, FleetObservation, LifecycleObservation},
    observation::ObservationState,
    theme, ui,
};
use ratatui::{Terminal, backend::TestBackend, widgets::ListState};

#[test]
fn non_idle_states_use_their_own_animation_and_retained_rows_stay_still() {
    let config = Config::parse(
        r#"[appearance]
command = "clock"
working = "none"
waiting = "clock"
blocked = "moon"
settling = "none"
lost = "arc"
unknown = "diamond"
"#,
    )
    .expect("all state settings parse");
    let document = config.to_document();
    assert_eq!(
        Config::parse(&document).unwrap().appearance,
        config.appearance
    );
    theme::install(config);

    for (state, name) in [
        (AgentState::Waiting, "clock"),
        (AgentState::Blocked, "moon"),
        (AgentState::Lost, "arc"),
        (AgentState::Unknown, "diamond"),
        (AgentState::Other("new-state".into()), "diamond"),
    ] {
        let frames = theme::frames(name).unwrap();
        assert!(theme::animates(&state, false), "{state:?}");
        assert!(!theme::animates(&state, true), "retained {state:?}");
        for (tick, expected) in frames.iter().enumerate() {
            assert_eq!(theme::agent_state(&state, None, false, tick).0, *expected);
            assert_eq!(theme::agent_state(&state, None, true, tick).0, frames[0]);
        }
    }

    let mut observation = FleetObservation::default();
    observation.lifecycle_subjects.push(LifecycleObservation {
        subject_id: "pending-subject".into(),
        bound_agent_id: None,
        process: None,
        state: LifecycleState::Pending,
        session: None,
        successor: None,
    });
    let mut state = ObservationState::new();
    state.apply_success(observation);
    let mut app = App::new();
    app.refresh(&state);
    assert!(
        app.animates(),
        "a pending lifecycle row uses the command animation"
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test backend");
    terminal
        .draw(|frame| {
            ui::render(frame, &state, &app, &mut ListState::default(), 0);
        })
        .expect("render lifecycle row");
    let rendered = (0..30)
        .map(|y| {
            (0..120)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\\n");
    let loading = theme::frame(theme::command_frames().expect("configured frames"), 0);
    assert!(rendered.contains(&format!("{loading} pending · subject pending-")));

    for (state, mark) in [
        (AgentState::Working, '⣀'),
        (AgentState::Settling, '◌'),
        (AgentState::Idle, '·'),
        (AgentState::Done, '✓'),
    ] {
        assert!(!theme::animates(&state, false), "{state:?}");
        for tick in [0, 1, 99] {
            assert_eq!(theme::agent_state(&state, None, false, tick).0, mark);
        }
    }
}
