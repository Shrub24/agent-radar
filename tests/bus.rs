use agent_radar::bus::{BusEvent, BusState, Connection, Message, Task, TaskState, socket_path_in};

#[test]
fn publisher_protocol_decodes_a_session_and_task_snapshot() {
    let mut connection = Connection::new();
    let hello = serde_json::to_vec(&serde_json::json!({
        "type": "hello",
        "v": 1,
        "session": "pi-session-a",
        "pane": "%8"
    }))
    .unwrap();
    assert!(matches!(
        connection.ingest(&hello).unwrap(),
        Some(Message::Hello(message)) if message.session == "pi-session-a"
    ));

    let tasks = serde_json::to_vec(&serde_json::json!({"type": "tasks", "tasks": []})).unwrap();
    assert!(matches!(
        connection.ingest(&tasks).unwrap(),
        Some(Message::Tasks(tasks)) if tasks.is_empty()
    ));
}

#[test]
fn task_projection_removes_a_session_when_its_connection_ends() {
    let mut state = BusState::new();
    state.apply(BusEvent::Connected {
        session: "pi-session-a".to_string(),
        pane: Some("%8".to_string()),
    });
    state.apply(BusEvent::Tasks {
        session: "pi-session-a".to_string(),
        tasks: vec![Task {
            id: "task-1".to_string(),
            state: TaskState::Running,
            command: Some("sleep".to_string()),
            cwd: Some("/tmp".to_string()),
            pid: Some(42),
            started_at: Some(1),
            last_output_at: Some(2),
            output_bytes: Some(3),
            exit_code: None,
        }],
    });
    assert_eq!(state.get("pi-session-a").unwrap().running(), 1);
    state.apply(BusEvent::Disconnected {
        session: "pi-session-a".to_string(),
    });
    assert!(state.get("pi-session-a").is_none());
}

#[test]
fn bus_socket_path_uses_the_configured_socket_then_runtime_directory() {
    let configured = socket_path_in(
        Some(std::ffi::OsStr::new("/custom/bus.sock")),
        Some(std::ffi::OsStr::new("/run/user/1000")),
        1000,
    );
    assert_eq!(configured.to_string_lossy(), "/custom/bus.sock");
    let runtime = socket_path_in(None, Some(std::ffi::OsStr::new("/run/user/1000")), 1000);
    assert_eq!(
        runtime.to_string_lossy(),
        "/run/user/1000/agent-radar/radar.sock"
    );
}
