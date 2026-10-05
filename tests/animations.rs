use agent_radar::{config::Config, model::AgentState, theme};

#[test]
fn non_idle_states_use_their_own_animation_and_retained_rows_stay_still() {
    let config = Config::parse(
        r#"[appearance]
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
