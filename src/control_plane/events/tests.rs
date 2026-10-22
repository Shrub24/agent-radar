use super::*;
use crate::control_plane::containers;
use crate::control_plane::lifecycle::{self, LaunchContext};
use crate::control_plane::registry::{LocalProcfsVerifier, Registry};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("radar-events-{}", random_uuid())))
    }
    fn registry(&self) -> Registry {
        Registry::open_at(&self.0, 1000).unwrap()
    }
    fn directory(&self) -> PathBuf {
        self.0.join("events")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create(registry: &Registry) -> lifecycle::Ticket {
    lifecycle::create(
        registry,
        "5b2e8c41-0a7d-4f36-b1d9-6c3a7e5f8b1d",
        LaunchContext {
            executable: "program-secret".into(),
            argv: vec!["argument-secret".into()],
            cwd: Some("/private-path-secret".into()),
            env: BTreeMap::from([("API_KEY".into(), "environment-secret".into())]),
        },
        None,
        1000,
    )
    .unwrap()
}

#[test]
fn replay_restores_revocation_without_credentials_in_events() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    lifecycle::cancel(&registry, &ticket.claim_id, &LocalProcfsVerifier, 2000).unwrap();
    let before = lifecycle::read(&registry, &ticket.claim_id)
        .unwrap()
        .unwrap();
    assert_eq!(before.execution.state, State::Revoked);
    assert!(before.execution.correlation.is_none());
    drop(registry);
    let restored = fixture.registry();
    let after = lifecycle::read(&restored, &ticket.claim_id)
        .unwrap()
        .unwrap();
    assert!(
        before == after,
        "replay must reproduce the same subject facts and private recipe"
    );
    assert_eq!(lifecycle::list_after(&restored, None, 10).unwrap().len(), 1);
    for entry in fs::read_dir(fixture.directory()).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        for secret in [
            &ticket.correlation,
            "program-secret",
            "argument-secret",
            "private-path-secret",
            "environment-secret",
        ] {
            assert!(
                !text.contains(secret),
                "private launch material entered the event log"
            );
        }
    }
}

#[test]
fn failed_commit_preserves_projection_and_freezes_mutations_until_reopen() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    let directory = fixture.directory();
    let displaced = fixture.0.join("displaced-events");
    fs::rename(&directory, &displaced).unwrap();
    assert!(lifecycle::cancel(&registry, &ticket.claim_id, &LocalProcfsVerifier, 2000).is_err());
    assert_eq!(
        lifecycle::read(&registry, &ticket.claim_id)
            .unwrap()
            .unwrap()
            .execution
            .state,
        State::Pending
    );
    fs::rename(&displaced, &directory).unwrap();
    assert!(
        lifecycle::cancel(&registry, &ticket.claim_id, &LocalProcfsVerifier, 3000).is_err(),
        "a possibly committed sequence must not be overwritten"
    );
    drop(registry);
    let restored = fixture.registry();
    assert_eq!(
        lifecycle::read(&restored, &ticket.claim_id)
            .unwrap()
            .unwrap()
            .execution
            .state,
        State::Pending
    );
    lifecycle::cancel(&restored, &ticket.claim_id, &LocalProcfsVerifier, 4000).unwrap();
}

#[test]
fn operation_results_replay_while_private_request_inputs_stay_out_of_events() {
    use crate::control_plane::store::{RequestOutcome, RequestRecord, Store};
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let id = random_uuid();
    let request = RequestRecord::new(&id, "lifecycle.stop", None, None, 1000)
        .with_params(serde_json::json!({"request": {"private": "operation-secret"}}));
    store.create(&request).unwrap();
    store.write(&request.clone().claim().start()).unwrap();
    drop(store);
    let store = Store::open(&fixture.0).unwrap();
    let interrupted = store.get(&id).unwrap().unwrap();
    assert_eq!(
        interrupted.state,
        crate::control_plane::store::RecordState::Started
    );
    assert_eq!(
        interrupted.derive(i64::MAX),
        crate::control_plane::store::Derived::Unknown
    );
    assert!(
        store.create(&request).is_err(),
        "an interrupted intent remains reserved rather than dispatching again"
    );
    let result = request.complete(
        RequestOutcome::Completed,
        None,
        None,
        vec!["process exited".into()],
        2000,
    );
    store.write(&result).unwrap();
    assert_eq!(store.get(&id).unwrap(), Some(result.clone()));
    drop(store);
    let restored = Store::open(&fixture.0).unwrap();
    assert_eq!(restored.get(&id).unwrap(), Some(result.clone()));
    assert_eq!(restored.recent(10).unwrap(), vec![result.clone()]);
    assert!(
        restored.create(&result).is_err(),
        "replay cannot admit the same ID again"
    );
    for entry in fs::read_dir(fixture.directory()).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(
            !text.contains("operation-secret"),
            "operation params entered the event log"
        );
    }
    assert_eq!(fs::read_dir(fixture.directory()).unwrap().count(), 3);
}

#[test]
fn failed_pointer_delivery_recovers_from_the_log_without_duplicates() {
    use crate::control_plane::store::Store;
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let store = Store::open(&fixture.0).unwrap();
    let ticket = create(&registry);
    let journal = fixture.0.join("events.json");
    let displaced = fixture.0.join("displaced-pointer-journal");
    fs::rename(&journal, &displaced).unwrap();
    fs::create_dir(&journal).unwrap();
    lifecycle::cancel(&registry, &ticket.claim_id, &LocalProcfsVerifier, 2000).unwrap();
    assert!(store.events().is_err());
    assert_eq!(
        lifecycle::read(&registry, &ticket.claim_id)
            .unwrap()
            .unwrap()
            .execution
            .state,
        State::Revoked
    );
    fs::remove_dir(&journal).unwrap();
    fs::rename(&displaced, &journal).unwrap();
    let pointers = store.events().unwrap();
    assert_eq!(
        pointers
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["lifecycle.created", "lifecycle.revoked"]
    );
    assert!(pointers.iter().all(
        |event| event.subject_id.as_deref() == Some(&ticket.claim_id)
            && event.lifecycle_event_id.is_some()
    ));
    assert_eq!(store.events().unwrap(), pointers);
    drop(store);
    drop(registry);
    assert_eq!(Store::open(&fixture.0).unwrap().events().unwrap(), pointers);
}

#[test]
fn pointer_delivery_is_bounded_and_reports_its_remaining_source_backlog() {
    use crate::control_plane::store::Store;
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let store = Store::open(&fixture.0).unwrap();
    for _ in 0..260 {
        create(&registry);
    }
    let (_, tail, _, first, pending) = store.event_snapshot().unwrap();
    assert_eq!(first.len(), 256);
    assert_eq!(tail, 256);
    assert!(pending);
    let (_, tail, _, second, pending) = store.event_snapshot().unwrap();
    assert_eq!(second.len(), 260);
    assert_eq!(tail, 260);
    assert!(!pending);
    let ids: std::collections::BTreeSet<_> = second
        .iter()
        .map(|event| event.lifecycle_event_id.clone().unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        260,
        "catch-up must not duplicate committed source events"
    );
}

#[test]
fn a_missing_sequence_refuses_replay_instead_of_inventing_state() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    create(&registry);
    drop(registry);
    fs::rename(
        fixture.directory().join("00000000000000000001.json"),
        fixture.directory().join("00000000000000000002.json"),
    )
    .unwrap();
    assert!(Registry::open_at(&fixture.0, 2000).is_err());
}

#[test]
fn legacy_subject_is_explicitly_imported_before_its_next_transition() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    let mut legacy = lifecycle::read(&registry, &ticket.claim_id)
        .unwrap()
        .unwrap();
    legacy.execution.correlation = Some(ticket.correlation);
    fs::write(
        fixture
            .0
            .join("lifecycle")
            .join(format!("{}.json", ticket.claim_id)),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    drop(registry);
    fs::remove_dir_all(fixture.directory()).unwrap();
    let restored = fixture.registry();
    lifecycle::cancel(&restored, &ticket.claim_id, &LocalProcfsVerifier, 2000).unwrap();
    let first: Event = serde_json::from_slice(
        &fs::read(fixture.directory().join("00000000000000000001.json")).unwrap(),
    )
    .unwrap();
    assert!(matches!(first.transition, Transition::Imported(_)));
    let second: Event = serde_json::from_slice(
        &fs::read(fixture.directory().join("00000000000000000002.json")).unwrap(),
    )
    .unwrap();
    assert!(matches!(second.transition, Transition::Revoked));
}

#[test]
fn replay_restores_a_container_without_rewriting_it() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let backend = crate::runtime::BackendIdentity::named("fake");
    let made = containers::record_created(
        &registry,
        containers::Confirmed {
            id: crate::control_plane::store::random_uuid(),
            recipe: false,
            request: &crate::runtime::CreateRequest::Tab {
                workspace_id: "wA".into(),
                focus: false,
            },
            created: &crate::runtime::CreatedLocation {
                workspace_id: Some("wA".into()),
                tab_id: Some("wA:t2".into()),
                pane_id: None,
            },
            backend: &backend,
            incarnation: Some(crate::model::ProcessIdentity {
                boot_id: "boot-1".into(),
                pid: 42,
                start_ticks: 100,
            }),
            label: None,
            created_by: None,
        },
        "1000",
    )
    .unwrap();
    assert_eq!(made.len(), 1);
    let id = made[0].id.clone();
    drop(registry);
    let restored = fixture.registry();
    let after = containers::read(&restored, &id).unwrap().unwrap();
    assert!(
        made[0] == after,
        "replay must reproduce the same container record and binding"
    );
    assert_eq!(
        containers::list_after(&restored, None, 10).unwrap().len(),
        1
    );
}
