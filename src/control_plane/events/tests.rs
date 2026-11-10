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

fn launch() -> LaunchContext {
    LaunchContext {
        executable: "program-secret".into(),
        argv: vec!["argument-secret".into()],
        cwd: Some("/private-path-secret".into()),
        env: BTreeMap::from([("API_KEY".into(), "environment-secret".into())]),
    }
}

fn create(registry: &Registry) -> lifecycle::Ticket {
    lifecycle::reserved_ticket(registry, launch(), None, 1000)
}

#[test]
fn a_process_can_be_recovered_without_fabricating_a_claim() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let process = lifecycle::create_process(
        &registry,
        &random_uuid(),
        launch(),
        lifecycle::Admission {
            kind: lifecycle::ProcessKind::Agent,
            identity: ProcessIdentity {
                boot_id: "boot".into(),
                pid: 4242,
                start_ticks: 7,
            },
            completion_key: random_uuid(),
            placement: None,
        },
        1000,
    )
    .unwrap();
    assert!(
        lifecycle::list_after(&registry, None, 10)
            .unwrap()
            .is_empty()
    );
    registry
        .events
        .lock()
        .unwrap()
        .append(
            Stream::Process(&process.registration_id),
            "2026-01-01T00:00:00.000Z",
            Source::ProcessControl,
            Transition::SignalRequested,
        )
        .unwrap();
    let process = lifecycle::read_process(&registry, &process.registration_id)
        .unwrap()
        .unwrap();
    let store = crate::control_plane::store::Store::open(&fixture.0).unwrap();
    let pointers = store.events().unwrap();
    for kind in ["process.created", "lifecycle.signal_requested"] {
        assert!(
            pointers.iter().any(|event| event.kind == kind),
            "missing {kind} pointer"
        );
    }
    assert!(
        pointers
            .iter()
            .all(|event| event.subject_id.is_none() && event.claim_id.is_none()),
        "a process-only pointer invented a subject or claim"
    );
    assert!(
        pointers
            .iter()
            .all(|event| event.registration_id.as_ref() == Some(&process.registration_id)),
        "a process-only pointer lost its registration"
    );
    drop(store);
    drop(registry);
    let restored = fixture.registry();
    assert_eq!(
        crate::control_plane::store::Store::open(&fixture.0)
            .unwrap()
            .events()
            .unwrap(),
        pointers
    );
    let recovered = lifecycle::read_process(&restored, &process.registration_id)
        .unwrap()
        .unwrap();
    assert!(
        process == recovered,
        "process identity and private recipe survive replay without a claim"
    );
    assert!(
        crate::control_plane::registry::presence::attachment(&restored, &process.registration_id,)
            .unwrap()
            .is_none()
    );
}

#[test]
fn publication_pointers_distinguish_subjects_from_registrations() {
    use crate::control_plane::registry::publication::{
        AcquireRequest, Channel, PublicationUpdate, PublishRequest, PublisherIdentity, Report,
        Snapshot,
    };
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    let agent = registry
        .register(
            &serde_json::from_value(serde_json::json!({
                "source": "test-publisher",
                "incarnation": "11111111-1111-4111-8111-111111111111"
            }))
            .unwrap(),
            1000,
        )
        .unwrap();
    let store = crate::control_plane::store::Store::open(&fixture.0).unwrap();
    registry.attach_journal(store.clone());
    for (channel, target) in [
        (Channel::Execution, agent.registration_id.to_string()),
        (
            Channel::Assignment,
            ticket.subject_id.clone().expect("a subject").to_string(),
        ),
    ] {
        let before = store.events().unwrap().len();
        let writer = registry
            .acquire_writer(
                &AcquireRequest {
                    target_id: target.clone(),
                    channel,
                    publisher: PublisherIdentity {
                        source: "test-publisher".into(),
                        incarnation: "11111111-1111-4111-8111-111111111111".into(),
                        reporting_owner: (channel == Channel::Assignment).then(|| "owner".into()),
                    },
                    replace: None,
                },
                1000,
            )
            .unwrap();
        registry
            .publish(
                &PublishRequest {
                    target_id: target.clone(),
                    channel,
                    update: PublicationUpdate {
                        writer_handle: writer.handle.clone(),
                        sequence: 1,
                        lease_ms: None,
                        observed_at: None,
                        snapshot: Report::Execution(Snapshot {
                            activity: "working".into(),
                            waiting_reason: None,
                            last_outcome: None,
                            actions: Vec::new(),
                        }),
                    },
                },
                1001,
            )
            .unwrap();
        registry
            .retire(&target, channel, &writer.handle, 1002)
            .unwrap();
        let events = store.events().unwrap();
        let events = &events[before..];
        for kind in [
            "publication.writer".to_string(),
            format!("{}.published", channel.name()),
            format!("{}.retired", channel.name()),
        ] {
            assert!(
                events.iter().any(|event| event.kind == kind),
                "missing {kind}"
            );
        }
        for event in events {
            match channel {
                Channel::Assignment => {
                    assert_eq!(
                        event.subject_id.as_deref(),
                        Some(ticket.subject_id.as_ref().expect("a subject").as_str())
                    );
                    assert!(
                        event.registration_id.is_none(),
                        "an assignment subject became a registration"
                    );
                }
                Channel::Execution => {
                    assert_eq!(event.registration_id.as_ref(), Some(&agent.registration_id));
                    assert!(
                        event.subject_id.is_none(),
                        "an execution registration became a subject"
                    );
                }
                Channel::Facts => unreachable!(),
            }
        }
    }
}

#[test]
fn attachment_views_share_process_evidence_across_replay() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    {
        let mut log = registry.events.lock().unwrap();
        log.append(
            Stream::Process(&ticket.registration_id),
            "2026-01-01T00:00:00.000Z",
            Source::Bootstrap,
            Transition::Captured(ProcessIdentity {
                pid: 10,
                boot_id: "test-boot".into(),
                start_ticks: 1,
            }),
        )
        .unwrap();
        let before_signal = log
            .facts(ticket.claim_id.as_ref().expect("a claim"))
            .unwrap();
        let next_claim = ClaimId::random();
        log.append(
            Stream::Claim(&next_claim),
            "2026-01-01T00:00:01.000Z",
            Source::Registration,
            Transition::Enrolled(Box::new(before_signal.clone())),
        )
        .unwrap();
        // Process evidence belongs to the execution's own stream.
        log.append(
            Stream::Process(&ticket.registration_id),
            "2026-01-01T00:00:02.000Z",
            Source::ProcessControl,
            Transition::SignalRequested,
        )
        .unwrap();
        assert!(
            log.process(&ticket.registration_id)
                .expect("the execution")
                .signalled_at
                .is_some()
        );
        let stale = ClaimId::random();
        log.append(
            Stream::Claim(&stale),
            "2026-01-01T00:00:03.000Z",
            Source::Registration,
            Transition::Enrolled(Box::new(before_signal)),
        )
        .unwrap();
        assert_eq!(
            log.facts(&stale).expect("the attachment"),
            log.facts(&next_claim).expect("the attachment"),
            "an attachment carries no execution facts of its own"
        );
    }
    drop(registry);
    let restored = fixture.registry();
    assert!(
        lifecycle::read_process(&restored, &ticket.registration_id)
            .unwrap()
            .unwrap()
            .signalled_at
            .is_some(),
        "process evidence survives replay independently of the reporting claim"
    );
}

#[test]
fn replay_restores_revocation_without_credentials_in_events() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let ticket = create(&registry);
    lifecycle::cancel(
        &registry,
        &ticket.registration_id,
        &LocalProcfsVerifier,
        2000,
    )
    .unwrap();
    let before = lifecycle::read_claim(&registry, ticket.claim_id.as_ref().expect("a claim"))
        .unwrap()
        .unwrap();
    assert_eq!(before.process.state, State::Revoked);
    assert!(before.process.correlation.is_none());
    drop(registry);
    let restored = fixture.registry();
    let after = lifecycle::read_claim(&restored, ticket.claim_id.as_ref().expect("a claim"))
        .unwrap()
        .unwrap();
    assert!(
        before == after,
        "replay must reproduce the same subject facts and private recipe"
    );
    assert!(
        lifecycle::list_after(&restored, None, 10)
            .unwrap()
            .iter()
            .any(|claim| claim.claim_id == *ticket.claim_id.as_ref().expect("a claim")),
        "the replayed records still resolve the claim"
    );
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
    assert!(
        lifecycle::cancel(
            &registry,
            &ticket.registration_id,
            &LocalProcfsVerifier,
            2000
        )
        .is_err()
    );
    assert_eq!(
        lifecycle::read_claim(&registry, ticket.claim_id.as_ref().expect("a claim"))
            .unwrap()
            .unwrap()
            .process
            .state,
        State::Pending
    );
    fs::rename(&displaced, &directory).unwrap();
    assert!(
        lifecycle::cancel(
            &registry,
            &ticket.registration_id,
            &LocalProcfsVerifier,
            3000
        )
        .is_err(),
        "a possibly committed sequence must not be overwritten"
    );
    drop(registry);
    let restored = fixture.registry();
    assert_eq!(
        lifecycle::read_claim(&restored, ticket.claim_id.as_ref().expect("a claim"))
            .unwrap()
            .unwrap()
            .process
            .state,
        State::Pending
    );
    lifecycle::cancel(
        &restored,
        &ticket.registration_id,
        &LocalProcfsVerifier,
        4000,
    )
    .unwrap();
}

#[test]
fn operation_results_replay_while_private_request_inputs_stay_out_of_events() {
    use crate::control_plane::store::{RequestRecord, Settlement, Store};
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
    let result = request.complete(Settlement::completed(vec!["process exited".into()]), 2000);
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
    lifecycle::cancel(
        &registry,
        &ticket.registration_id,
        &LocalProcfsVerifier,
        2000,
    )
    .unwrap();
    assert!(store.events().is_err());
    assert_eq!(
        lifecycle::read_claim(&registry, ticket.claim_id.as_ref().expect("a claim"))
            .unwrap()
            .unwrap()
            .process
            .state,
        State::Revoked
    );
    fs::remove_dir(&journal).unwrap();
    fs::rename(&displaced, &journal).unwrap();
    let pointers = store.events().unwrap();
    let kinds: Vec<&str> = pointers
        .iter()
        .filter(|event| event.kind.starts_with("lifecycle."))
        .map(|event| event.kind.as_str())
        .collect();
    assert!(
        kinds.contains(&"lifecycle.created") && kinds.contains(&"lifecycle.revoked"),
        "the created and revoked pointers are delivered: {kinds:?}"
    );
    assert!(
        pointers
            .iter()
            .all(|event| event.lifecycle_event_id.is_some())
    );
    assert!(
        pointers.iter().any(|event| {
            event.kind == "process.created"
                && event.registration_id.as_ref().map(RegistrationId::as_str)
                    == Some(ticket.registration_id.as_str())
        }),
        "the reserved execution's own process pointer is delivered"
    );
    let process = pointers
        .iter()
        .find(|event| {
            event.kind == "process.created"
                && event.registration_id.as_ref().map(RegistrationId::as_str)
                    == Some(ticket.registration_id.as_str())
        })
        .expect("the execution's process pointer");
    assert!(
        process.subject_id.is_none() && process.claim_id.is_none(),
        "a process registration is not a claim or subject"
    );
    // A launch reserves an execution, not a subject: a lifecycle pointer names
    // this launch's subject when it has one, and names none when it has not.
    assert!(
        pointers
            .iter()
            .filter(|event| event.kind.starts_with("lifecycle."))
            .filter_map(|event| event.subject_id.as_deref())
            .all(|subject| ticket.subject_id.as_ref().map(|id| id.as_str()) == Some(subject)),
        "a lifecycle pointer never names another subject: {pointers:?}"
    );
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
    while registry.events.lock().unwrap().tail() < 260 {
        create(&registry);
    }
    let source_count = registry.events.lock().unwrap().tail() as usize;
    let (_, tail, _, first, pending) = store.event_snapshot().unwrap();
    assert_eq!(first.len(), 256);
    assert_eq!(tail, 256);
    assert!(pending);
    let (_, tail, _, second, pending) = store.event_snapshot().unwrap();
    assert_eq!(second.len(), source_count);
    assert_eq!(tail as usize, source_count);
    assert!(!pending);
    let ids: std::collections::BTreeSet<_> = second
        .iter()
        .map(|event| event.lifecycle_event_id.clone().unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        source_count,
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
                focus: Some(false),
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

#[test]
fn a_journal_from_another_build_is_named_and_classified() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    create(&registry);
    drop(registry);
    // The same entry, rewritten as a build before this one would have stored it.
    let entry = fixture.directory().join("00000000000000000001.json");
    let stored = fs::read_to_string(&entry).expect("a journal entry");
    fs::write(&entry, stored.replacen("\"version\":2", "\"version\":1", 1)).expect("an old entry");
    let error = Log::open(&fixture.directory()).expect_err("another build's journal is refused");
    assert!(is_unreadable_state(&error), "{error}");
    assert!(error.contains("00000000000000000001.json"), "{error}");
    assert!(error.contains("event version 1"), "{error}");
}
