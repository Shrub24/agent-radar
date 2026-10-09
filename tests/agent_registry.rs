#![cfg(unix)]
//! Registry storage tests: durable registration files in a private temporary
//! root, read back through the module's own API.
//!
//! Nothing here speaks the control socket: no endpoint serves the registry yet.
//! What is exercised is the record itself — that a publisher incarnation's
//! identity is durable, that the same retry is answered with the same handle
//! while conflicting content is refused, that a second incarnation sharing a
//! session stays its own record, and that private launch information reaches no
//! public read. Every root is temporary and removed on the way out.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use agent_radar::control_plane::random_uuid;
use agent_radar::control_plane::registry::{
    AcquireRequest, Channel, ContextRequest, ContextValue, ExpectedWriter, Freshness,
    LaunchSession, LaunchSpec, LocalProcfsVerifier, MAX_CONTEXT_BYTES, MAX_LISTED,
    MAX_REGISTRATION_BYTES, MAX_TEXT_BYTES, Outcome, ProcessVerification, ProcessVerifier,
    PublicChannel, PublicContext, PublishRequest, PublisherIdentity, RegistrationRequest, Registry,
    RegistryLocation, Snapshot, WriterBinding,
};
use agent_radar::model::ProcessIdentity;

const INCARNATION: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";
const OTHER_INCARNATION: &str = "1c2d3e4f-5678-4abc-9def-0123456789ab";
const OWNER_INCARNATION: &str = "3c2d3e4f-5678-4abc-9def-0123456789ab";
const SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

fn request(incarnation: &str) -> RegistrationRequest {
    RegistrationRequest {
        source: "herdsman".into(),
        incarnation: incarnation.into(),
        session: Some(SESSION.into()),
        owner: None,
        run: None,
        label: Some("worker".into()),
        location: None,
        process: None,
        launch: None,
    }
}

fn launch() -> LaunchSpec {
    LaunchSpec {
        executable: "/usr/bin/pi".into(),
        argv: vec![
            "--resume".into(),
            "--session".into(),
            "/home/dev/.pi/sessions/secret-9.json".into(),
        ],
        cwd: "/home/dev/proj".into(),
        session: Some(LaunchSession {
            uuid: None,
            path: Some("/home/dev/.pi/sessions/secret-9.json".into()),
        }),
        provenance: "herdsman".into(),
        revision: "7".into(),
    }
}

/// A private root under the temporary directory, removed by [`drop_root`].
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "radar-registry-{name}-{}-{}",
        std::process::id(),
        random_uuid()
    ));
    fs::create_dir_all(&root).expect("a temporary root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("a mode");
    root
}

fn drop_root(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

fn files(registry: &Registry) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(registry.directory())
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn two_incarnations_sharing_a_session_stay_distinct() {
    let root = temp_root("session");
    let registry = Registry::open(&root).expect("a registry");

    let first = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");
    let second = registry
        .register(&request(OTHER_INCARNATION), 1_001)
        .expect("a second register");

    assert_ne!(first.agent_id, second.agent_id);
    assert_eq!(first.request.session, second.request.session);
    assert_eq!(
        registry
            .get(&first.agent_id)
            .expect("a read")
            .expect("a record"),
        first
    );
    assert_eq!(
        registry
            .get(&second.agent_id)
            .expect("a read")
            .expect("a record"),
        second
    );
    assert_eq!(registry.list(MAX_LISTED).expect("a list").len(), 2);
    drop_root(&root);
}

#[test]
fn a_retry_of_identical_content_survives_reopening() {
    let root = temp_root("retry");
    let id;
    {
        let registry = Registry::open(&root).expect("a registry");
        id = registry
            .register(&request(INCARNATION), 1_000)
            .expect("a register")
            .agent_id;
    }

    // The record outlives the process that wrote it: a fresh registry over the
    // same root reads it, and the same publisher asking again gets its handle
    // back rather than a second identity.
    let reopened = Registry::open(&root).expect("a registry");
    let restored = reopened.get(&id).expect("a read").expect("the record");
    assert_eq!(restored.agent_id, id);
    assert_eq!(restored.request, request(INCARNATION));

    let again = reopened
        .register(&request(INCARNATION), 9_999)
        .expect("a retry");
    assert_eq!(again.agent_id, id);
    assert_eq!(again.registered_at, restored.registered_at);
    assert_eq!(
        files(&reopened).len(),
        1,
        "a retry must not write a second record"
    );
    drop_root(&root);
}

#[test]
fn conflicting_content_under_one_incarnation_is_refused_and_changes_nothing() {
    let root = temp_root("conflict");
    let registry = Registry::open(&root).expect("a registry");
    let original = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");

    let mut different = request(INCARNATION);
    different.label = Some("lead".into());
    let refusal = registry.register(&different, 2_000).expect_err("a refusal");
    assert!(
        refusal.contains("already names different content"),
        "{refusal}"
    );

    assert_eq!(
        registry
            .get(&original.agent_id)
            .expect("a read")
            .expect("a record"),
        original,
        "the refused write must not have replaced identity"
    );
    assert_eq!(files(&registry).len(), 1);
    drop_root(&root);
}

#[test]
fn private_launch_information_never_reaches_a_public_read() {
    let root = temp_root("privacy");
    let registry = Registry::open(&root).expect("a registry");
    let mut wanted = request(INCARNATION);
    wanted.launch = Some(launch());
    let stored = registry.register(&wanted, 1_000).expect("a register");

    // The persisted record holds the specification: the executor will need it.
    let path = files(&registry).into_iter().next().expect("a file");
    let on_disk = fs::read_to_string(&path).expect("a read");
    assert!(on_disk.contains("/usr/bin/pi"), "{on_disk}");
    assert!(on_disk.contains("secret-9.json"), "{on_disk}");

    // The public projection holds a revision and nothing executable.
    let public = stored.public();
    assert!(public.launch.available);
    assert_eq!(public.launch.revision.as_deref(), Some("7"));
    let json = serde_json::to_string(&public).expect("an encoding");
    for private in [
        "/usr/bin/pi",
        "argv",
        "executable",
        "cwd",
        "/home/dev/proj",
        "secret-9.json",
    ] {
        assert!(
            !json.contains(private),
            "public read leaked {private}: {json}"
        );
    }

    let mut without = request(OTHER_INCARNATION);
    without.launch = None;
    let public = registry
        .register(&without, 1_001)
        .expect("a register")
        .public();
    assert!(!public.launch.available);
    assert!(public.launch.revision.is_none());
    drop_root(&root);
}

#[test]
fn a_supplied_process_identity_is_never_claimed_verified() {
    let root = temp_root("process");
    let registry = Registry::open(&root).expect("a registry");
    let mut wanted = request(INCARNATION);
    wanted.process = Some(ProcessIdentity {
        boot_id: "boot-abc".into(),
        pid: 4242,
        start_ticks: 99,
    });
    let stored = registry.register(&wanted, 1_000).expect("a register");

    let public = stored.public();
    assert!(public.process_claimed);
    let json = serde_json::to_string(&public).expect("an encoding");
    assert!(!json.contains("verified"), "{json}");
    assert!(!json.contains("4242"), "{json}");

    let public = registry
        .register(&request(OTHER_INCARNATION), 1_001)
        .expect("a register")
        .public();
    assert!(!public.process_claimed);

    // The claimed identity round-trips through storage, and an identity this
    // build does not understand is a read failure rather than a weaker fact.
    let path = registry
        .directory()
        .join(format!("{}.json", stored.agent_id));
    assert_eq!(
        registry.get(&stored.agent_id).expect("a read"),
        Some(stored.clone())
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("a read")).expect("a decoding");
    value["request"]["process"]["verified"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&value).expect("an encoding")).expect("a write");
    let error = registry.list(MAX_LISTED).expect_err("a refusal");
    assert!(error.contains("unknown field"), "{error}");
    drop_root(&root);
}

#[test]
fn a_corrupt_registration_fails_explicitly_rather_than_disappearing() {
    let root = temp_root("corrupt");
    let registry = Registry::open(&root).expect("a registry");
    let stored = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");
    let path = registry
        .directory()
        .join(format!("{}.json", stored.agent_id));
    fs::write(&path, b"not json").expect("a planted file");

    let error = registry
        .get(&stored.agent_id)
        .expect_err("a malformed record is an error, not an absence");
    assert!(error.contains("not a registration"), "{error}");
    let error = registry
        .list(MAX_LISTED)
        .expect_err("a list must not silently omit it");
    assert!(error.contains("not a registration"), "{error}");
    drop_root(&root);
}

#[test]
fn a_symlinked_registration_is_not_read() {
    let root = temp_root("symlink");
    let registry = Registry::open(&root).expect("a registry");
    let elsewhere = temp_root("symlink-target");
    let target = elsewhere.join("real.json");
    fs::write(&target, b"{}").expect("a file");
    let linked = registry.directory().join(format!("{}.json", random_uuid()));
    symlink(&target, &linked).expect("a symlink");

    let error = registry.list(MAX_LISTED).expect_err("a refusal");
    assert!(error.contains("not a regular file"), "{error}");
    drop_root(&root);
    drop_root(&elsewhere);
}

#[test]
fn an_oversized_registration_is_refused() {
    let root = temp_root("oversized");
    let registry = Registry::open(&root).expect("a registry");

    // A planted file larger than the record bound fails before being read whole.
    let planted = registry.directory().join(format!("{}.json", random_uuid()));
    fs::write(&planted, vec![b'x'; MAX_REGISTRATION_BYTES + 1]).expect("a planted file");
    let error = registry.list(MAX_LISTED).expect_err("a refusal");
    assert!(error.contains("exceeds"), "{error}");
    fs::remove_file(&planted).expect("a removal");

    // A request whose own field is unbounded is refused before anything is
    // written: a bounded field cannot grow the record past its bound.
    let mut huge = request(INCARNATION);
    huge.label = Some("y".repeat(MAX_TEXT_BYTES + 1));
    let error = registry.register(&huge, 1_000).expect_err("a refusal");
    assert!(error.contains("exceeds"), "{error}");
    assert!(
        files(&registry).is_empty(),
        "nothing may be written for a refused request"
    );
    drop_root(&root);
}

#[test]
fn a_record_whose_id_does_not_match_its_filename_is_refused() {
    let root = temp_root("mismatch");
    let registry = Registry::open(&root).expect("a registry");
    let stored = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");
    let json = fs::read(
        registry
            .directory()
            .join(format!("{}.json", stored.agent_id)),
    )
    .expect("a read");
    fs::write(
        registry.directory().join(format!("{}.json", random_uuid())),
        json,
    )
    .expect("a renamed copy");

    let error = registry.list(MAX_LISTED).expect_err("a refusal");
    assert!(error.contains("does not match its filename"), "{error}");
    drop_root(&root);
}

#[test]
fn a_registration_of_another_version_is_refused() {
    let root = temp_root("version");
    let registry = Registry::open(&root).expect("a registry");
    let stored = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");
    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(
            registry
                .directory()
                .join(format!("{}.json", stored.agent_id)),
        )
        .expect("a read"),
    )
    .expect("a decoding");
    value["version"] = serde_json::json!(2);
    fs::write(
        registry
            .directory()
            .join(format!("{}.json", stored.agent_id)),
        serde_json::to_vec(&value).expect("an encoding"),
    )
    .expect("a write");

    let error = registry.get(&stored.agent_id).expect_err("a refusal");
    assert!(error.contains("is not served"), "{error}");
    drop_root(&root);
}

#[test]
fn an_id_that_is_not_canonical_cannot_name_a_file() {
    let root = temp_root("id");
    let registry = Registry::open(&root).expect("a registry");
    for id in [
        "not-a-uuid",
        "../escape",
        "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b2",
    ] {
        let error = registry.get(id).expect_err("a refusal");
        assert!(error.contains("canonical UUID"), "{id}: {error}");
    }
    drop_root(&root);
}

#[test]
fn a_registration_listing_is_in_stable_id_order_and_bounded() {
    let root = temp_root("listing");
    let registry = Registry::open(&root).expect("a registry");
    let mut ids = Vec::new();
    for (index, incarnation) in [
        INCARNATION,
        OTHER_INCARNATION,
        "3c2d3e4f-5678-4abc-9def-0123456789ab",
    ]
    .into_iter()
    .enumerate()
    {
        let mut wanted = request(incarnation);
        wanted.source = format!("publisher-{index}");
        // Deliberately registering newest receipt first: the listing must
        // follow the durable key, not the receipt clock.
        ids.push(
            registry
                .register(&wanted, 3_000 - index as i64 * 1_000)
                .expect("a register")
                .agent_id,
        );
    }

    let mut expected = ids.clone();
    expected.sort();

    let listed: Vec<String> = registry
        .list(usize::MAX)
        .expect("a list")
        .into_iter()
        .map(|record| record.agent_id)
        .collect();
    assert_eq!(listed, expected, "stable agent-id order");
    assert_eq!(listed.len(), MAX_LISTED.min(ids.len()));

    let first_two: Vec<String> = registry
        .list(2)
        .expect("a list")
        .into_iter()
        .map(|record| record.agent_id)
        .collect();
    assert_eq!(first_two, expected[..2].to_vec());
    drop_root(&root);
}

#[test]
fn concurrent_identical_registrations_admit_one_record() {
    let root = temp_root("concurrent");
    let registry = Arc::new(Registry::open(&root).expect("a registry"));
    const CALLERS: usize = 8;
    let (ready_tx, ready_rx) = mpsc::sync_channel(CALLERS);
    let (start_tx, start_rx) = mpsc::sync_channel::<()>(CALLERS);
    let start_rx = Arc::new(Mutex::new(start_rx));

    let admitted: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = (0..CALLERS)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let ready_tx = ready_tx.clone();
                let start_rx = Arc::clone(&start_rx);
                scope.spawn(move || {
                    ready_tx.send(()).expect("bounded ready");
                    start_rx
                        .lock()
                        .expect("start receiver lock")
                        .recv_timeout(std::time::Duration::from_secs(2))
                        .expect("bounded start");
                    registry
                        .register(&request(INCARNATION), 1_000)
                        .expect("a register")
                        .agent_id
                })
            })
            .collect();
        drop(ready_tx);
        for _ in 0..CALLERS {
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("bounded ready receive");
        }
        for _ in 0..CALLERS {
            start_tx.send(()).expect("bounded start send");
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a caller"))
            .collect()
    });

    assert_eq!(admitted.len(), CALLERS);
    assert!(
        admitted.iter().all(|id| *id == admitted[0]),
        "one incarnation must be answered with one handle: {admitted:?}"
    );
    assert_eq!(files(&registry).len(), 1, "one incarnation, one record");
    drop_root(&root);
}

#[test]
fn a_relative_launch_path_is_refused_before_any_write() {
    let root = temp_root("relative");
    let registry = Registry::open(&root).expect("a registry");
    let mut wanted = request(INCARNATION);
    let mut spec = launch();
    spec.executable = "pi".into();
    wanted.launch = Some(spec);
    let error = registry.register(&wanted, 1_000).expect_err("a refusal");
    assert!(error.contains("must be an absolute path"), "{error}");
    assert!(files(&registry).is_empty());
    drop_root(&root);
}

#[test]
fn a_registry_root_that_is_not_private_is_refused() {
    let root = temp_root("loose");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("a mode");
    let error = Registry::open(&root).expect_err("a refusal");
    assert!(error.contains("is not mode 0700"), "{error}");
    drop_root(&root);
}

#[test]
fn an_unknown_location_backend_survives_a_round_trip() {
    let root = temp_root("vocabulary");
    let registry = Registry::open(&root).expect("a registry");
    let mut wanted = request(INCARNATION);
    wanted.location = Some(RegistryLocation {
        backend: "some-future-mux".into(),
        instance: None,
        workspace: None,
        tab: None,
        pane: Some("wA:p1".into()),
    });
    let stored = registry.register(&wanted, 1_000).expect("a register");

    let listed = registry.list(MAX_LISTED).expect("a list");
    assert_eq!(listed, vec![stored.clone()]);
    assert_eq!(
        stored.public().location.expect("a location").backend,
        "some-future-mux"
    );
    drop_root(&root);
}

#[test]
fn a_planted_state_field_is_refused_by_a_stored_record() {
    let root = temp_root("state");
    let registry = Registry::open(&root).expect("a registry");
    let stored = registry
        .register(&request(INCARNATION), 1_000)
        .expect("a register");
    let path = registry
        .directory()
        .join(format!("{}.json", stored.agent_id));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("a read")).expect("a decoding");
    value["request"]["state"] = serde_json::json!("working");
    fs::write(&path, serde_json::to_vec(&value).expect("an encoding")).expect("a write");

    // State is publication data, not registration data: a record carrying it is
    // invalid stored evidence, not a record this build reads.
    let error = registry.list(MAX_LISTED).expect_err("a refusal");
    assert!(error.contains("unknown field"), "{error}");
    drop_root(&root);
}

// --- Direct publication channels -------------------------------------------------

fn registry_agent(
    registry: &Registry,
    incarnation: &str,
    owner: Option<&str>,
) -> agent_radar::control_plane::registry::Registration {
    let mut wanted = request(incarnation);
    wanted.owner = owner.map(str::to_string);
    registry.register(&wanted, 1_000).expect("a registration")
}

fn publisher(source: &str, incarnation: &str) -> PublisherIdentity {
    PublisherIdentity {
        source: source.into(),
        incarnation: incarnation.into(),
        reporting_owner: None,
    }
}

fn acquire(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
    publisher: PublisherIdentity,
    now_ms: i64,
) -> Result<WriterBinding, String> {
    registry.acquire_writer(
        &AcquireRequest {
            agent_id: agent_id.into(),
            channel,
            publisher,
            replace: None,
        },
        now_ms,
    )
}

fn replace(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
    publisher: PublisherIdentity,
    expected: &WriterBinding,
    now_ms: i64,
) -> Result<WriterBinding, String> {
    registry.acquire_writer(
        &AcquireRequest {
            agent_id: agent_id.into(),
            channel,
            publisher,
            replace: Some(ExpectedWriter {
                generation: expected.generation,
                handle: expected.handle.clone(),
            }),
        },
        now_ms,
    )
}

fn channel_snapshot(activity: &str, outcome: Option<&str>) -> Snapshot {
    Snapshot {
        activity: activity.into(),
        waiting_reason: Some("waiting-for-owner-approval".into()),
        last_outcome: outcome.map(|result| Outcome {
            result: result.into(),
            detail: Some("tool timed out".into()),
        }),
        actions: vec!["resume-after-permission".into()],
    }
}

fn channel_request(
    agent_id: &str,
    channel: Channel,
    handle: &str,
    sequence: u64,
    snapshot: Snapshot,
) -> PublishRequest {
    PublishRequest {
        agent_id: agent_id.into(),
        channel,
        writer_handle: handle.into(),
        sequence,
        lease_ms: Some(10_000),
        observed_at: Some("2001-01-01T00:00:00.000Z".into()),
        snapshot,
    }
}

fn published(registry: &Registry, agent_id: &str, channel: Channel, now_ms: i64) -> PublicChannel {
    registry
        .published(agent_id, channel, now_ms)
        .expect("a read")
        .expect("a channel")
}

#[test]
fn execution_and_assignment_keep_channel_and_writer_provenance_distinct() {
    let root = temp_root("channels");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let owner = registry_agent(&registry, INCARNATION, None);
    let mut child_request = request(OTHER_INCARNATION);
    child_request.owner = Some(owner.request.source.clone());
    let child = registry.register(&child_request, 1_000).expect("a child");

    let execution_writer = acquire(
        &registry,
        &child.agent_id,
        Channel::Execution,
        publisher(&child.request.source, &child.request.incarnation),
        1_000,
    )
    .expect("execution writer");
    // The owner publishes the assignment projection with its own identity and
    // no second registration: a publisher is not an agent record.
    let assignment_writer = acquire(
        &registry,
        &child.agent_id,
        Channel::Assignment,
        PublisherIdentity {
            source: "herdsman-owner".into(),
            incarnation: OWNER_INCARNATION.into(),
            reporting_owner: Some("herdsman-owner".into()),
        },
        1_000,
    )
    .expect("owner assignment writer");

    let execution = registry
        .publish(
            &channel_request(
                &child.agent_id,
                Channel::Execution,
                &execution_writer.handle,
                1,
                channel_snapshot("idle", Some("failed-tool-timeout")),
            ),
            1_001,
        )
        .expect("execution report");
    let assignment = registry
        .publish(
            &channel_request(
                &child.agent_id,
                Channel::Assignment,
                &assignment_writer.handle,
                1,
                channel_snapshot("assignment-open", None),
            ),
            1_002,
        )
        .expect("assignment report");
    assert_eq!(
        execution.snapshot.as_ref().unwrap().snapshot.activity,
        "idle"
    );
    assert_eq!(
        execution
            .snapshot
            .as_ref()
            .unwrap()
            .snapshot
            .last_outcome
            .as_ref()
            .unwrap()
            .result,
        "failed-tool-timeout"
    );
    assert_eq!(
        assignment.snapshot.as_ref().unwrap().snapshot.activity,
        "assignment-open"
    );

    let view = published(&registry, &child.agent_id, Channel::Assignment, 1_003);
    // Public provenance names the actual writer, not the target's registration
    // and not a copied source.
    assert_eq!(view.writer.source, "herdsman-owner");
    assert_eq!(view.writer.incarnation, OWNER_INCARNATION);
    let snapshot = view.snapshot.expect("an assignment snapshot");
    assert_eq!(snapshot.source, "herdsman-owner");
    assert_eq!(snapshot.reporting_owner.as_deref(), Some("herdsman-owner"));
    assert_eq!(snapshot.freshness, Freshness::Fresh);
    // The publisher created no agent registration: only the owner and the child.
    assert_eq!(files(&registry).len(), 2);
    drop_root(&root);
}

#[test]
fn a_channel_is_one_atomic_record() {
    let root = temp_root("one-record");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let writer = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                1,
                channel_snapshot("working", None),
            ),
            1_000,
        )
        .unwrap();

    // The binding and the snapshot share one durable record; there is no second
    // file a partial write could leave behind.
    let records: Vec<_> = fs::read_dir(registry.publications())
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    assert_eq!(records.len(), 1, "one channel is one record");
    assert!(!root.join("writers").exists());
    drop_root(&root);
}

#[test]
fn sequence_replay_conflicts_and_daemon_clock_expiry_are_distinct() {
    let root = temp_root("sequence");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let writer = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    let first_request = channel_request(
        &agent.agent_id,
        Channel::Execution,
        &writer.handle,
        1,
        channel_snapshot("working", None),
    );
    let first = registry.publish(&first_request, 1_000).unwrap();
    assert_eq!(
        first.snapshot.as_ref().unwrap().expires_at,
        agent_radar::control_plane::store::format_millis(11_000)
    );
    assert_eq!(
        registry.publish(&first_request, 9_000).unwrap(),
        first,
        "equal replay cannot refresh"
    );
    let mut conflict = first_request.clone();
    conflict.snapshot = channel_snapshot("idle", None);
    assert!(
        registry
            .publish(&conflict, 9_001)
            .unwrap_err()
            .contains("conflicting")
    );
    assert_eq!(
        published(&registry, &agent.agent_id, Channel::Execution, 10_999)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Fresh
    );
    assert_eq!(
        published(&registry, &agent.agent_id, Channel::Execution, 11_000)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Stale
    );
    drop_root(&root);
}

#[test]
fn replacement_preserves_old_snapshot_authorship_and_fences_the_old_handle() {
    let root = temp_root("replace");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let old = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    let first = registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &old.handle,
                1,
                channel_snapshot("working", None),
            ),
            1_000,
        )
        .unwrap();

    // A publisher restart is a new incarnation UUID, not a second agent record.
    let successor = publisher(&agent.request.source, OTHER_INCARNATION);
    assert!(
        replace(
            &registry,
            &agent.agent_id,
            Channel::Execution,
            successor.clone(),
            &old,
            1_001
        )
        .unwrap_err()
        .contains("fresh")
    );

    registry
        .retire(&agent.agent_id, Channel::Execution, &old.handle, 1_002)
        .unwrap();
    // A replacement must name the exact incumbent it observed.
    let mut wrong = old.clone();
    wrong.handle = random_uuid();
    assert!(
        replace(
            &registry,
            &agent.agent_id,
            Channel::Execution,
            successor.clone(),
            &wrong,
            1_002
        )
        .unwrap_err()
        .contains("incumbent writer changed")
    );
    let replacement = replace(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        successor.clone(),
        &old,
        1_003,
    )
    .unwrap();
    assert_eq!(replacement.generation, old.generation + 1);
    assert_ne!(replacement.handle, old.handle);

    // The previous report keeps its own authorship and reads stale; it is never
    // relabelled as the successor's.
    let prior = published(&registry, &agent.agent_id, Channel::Execution, 1_004);
    assert_eq!(prior.writer.handle, replacement.handle);
    assert_eq!(prior.writer.sequence, 0);
    let snapshot = prior.snapshot.expect("old facts preserved");
    assert_eq!(snapshot.handle, old.handle);
    assert_eq!(snapshot.generation, old.generation);
    assert_eq!(snapshot.source, first.writer.source);
    assert_eq!(snapshot.sequence, 1);
    assert_eq!(snapshot.snapshot.activity, "working");
    assert_eq!(snapshot.freshness, Freshness::Stale);

    assert!(
        registry
            .publish(
                &channel_request(
                    &agent.agent_id,
                    Channel::Execution,
                    &old.handle,
                    2,
                    channel_snapshot("late-old", None)
                ),
                1_005
            )
            .unwrap_err()
            .contains("stale")
    );
    let updated = registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &replacement.handle,
                1,
                channel_snapshot("new-writer", None),
            ),
            1_006,
        )
        .unwrap();
    assert_eq!(updated.snapshot.as_ref().unwrap().source, successor.source);
    assert_eq!(files(&registry).len(), 1, "no fake second registration");
    drop_root(&root);
}

#[test]
fn acquisition_replay_is_generation_safe() {
    let root = temp_root("acquire-replay");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        me.clone(),
        1_000,
    )
    .unwrap();
    // Identical acquisition retry returns the same binding, not a new one.
    let retry = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        me.clone(),
        1_000,
    )
    .unwrap();
    assert_eq!(retry, first);

    // A different publisher cannot overwrite the incumbent implicitly.
    let other = publisher(&agent.request.source, OTHER_INCARNATION);
    assert!(
        acquire(
            &registry,
            &agent.agent_id,
            Channel::Execution,
            other.clone(),
            1_000
        )
        .unwrap_err()
        .contains("replacement is explicit")
    );

    registry
        .retire(&agent.agent_id, Channel::Execution, &first.handle, 1_000)
        .unwrap();
    let second = replace(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        other.clone(),
        &first,
        1_000,
    )
    .unwrap();
    assert_eq!(second.generation, first.generation + 1);

    // A delayed replay of the same replacement cannot overwrite the newer writer.
    assert!(
        replace(
            &registry,
            &agent.agent_id,
            Channel::Execution,
            other.clone(),
            &first,
            1_000
        )
        .unwrap_err()
        .contains("incumbent writer changed")
    );
    assert_eq!(
        registry
            .channel(&agent.agent_id, Channel::Execution)
            .unwrap()
            .unwrap()
            .writer,
        second
    );
    drop_root(&root);
}

#[test]
fn an_unreported_channel_exposes_no_invented_snapshot_and_survives_reopening() {
    let root = temp_root("unreported");
    {
        let registry = Registry::open_at(&root, 1_000).expect("a registry");
        let agent = registry_agent(&registry, INCARNATION, None);
        let writer = acquire(
            &registry,
            &agent.agent_id,
            Channel::Execution,
            publisher(&agent.request.source, &agent.request.incarnation),
            1_000,
        )
        .unwrap();

        // Acquired but not yet reported: no fabricated activity, no freshness.
        let view = published(&registry, &agent.agent_id, Channel::Execution, 1_000);
        assert!(view.snapshot.is_none());
        assert_eq!(view.writer.sequence, 0);
        assert_eq!(view.writer.handle, writer.handle);
    }

    // The binding is durable: reopening restores it byte for byte.
    let reopened = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = reopened.list(MAX_LISTED).unwrap().remove(0);
    let restored = reopened
        .channel(&agent.agent_id, Channel::Execution)
        .unwrap()
        .expect("the channel");
    assert!(restored.snapshot.is_none());
    assert_eq!(restored.writer.sequence, 0);
    assert_eq!(
        reopened
            .published(&agent.agent_id, Channel::Execution, 1_000)
            .unwrap()
            .unwrap()
            .snapshot,
        None
    );
    drop_root(&root);
}

#[test]
fn an_unreported_successor_does_not_inherit_the_retired_lease() {
    let root = temp_root("inherited-lease");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let first = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &first.handle,
                1,
                channel_snapshot("working", None),
            ),
            1_000,
        )
        .unwrap();
    registry
        .retire(&agent.agent_id, Channel::Execution, &first.handle, 1_000)
        .unwrap();

    // The successor is acquired within the previous generation's lease but has
    // not reported. That inherited lease must not make it look fresh.
    let second = replace(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, OTHER_INCARNATION),
        &first,
        1_000,
    )
    .unwrap();
    assert_eq!(
        published(&registry, &agent.agent_id, Channel::Execution, 1_001)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Stale
    );

    // An unreported successor can itself be replaced, still by naming it exactly.
    let third = replace(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, OWNER_INCARNATION),
        &second,
        1_001,
    )
    .expect("an unreported successor can be replaced");
    assert_eq!(third.generation, second.generation + 1);
    drop_root(&root);
}

#[derive(Clone)]
struct FakeVerifier(Result<Option<ProcessIdentity>, String>);

impl ProcessVerifier for FakeVerifier {
    fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
        self.0.clone()
    }
}

#[test]
fn process_verification_distinguishes_full_birth_identity_and_unknown() {
    let claimed = ProcessIdentity {
        boot_id: "boot-a".into(),
        pid: 4242,
        start_ticks: 99,
    };
    assert_eq!(
        agent_radar::control_plane::registry::verify_identity(
            Some(&claimed),
            &FakeVerifier(Ok(Some(claimed.clone())))
        ),
        ProcessVerification::Verified
    );
    for observed in [
        ProcessIdentity {
            boot_id: "boot-b".into(),
            ..claimed.clone()
        },
        ProcessIdentity {
            start_ticks: 100,
            ..claimed.clone()
        },
        ProcessIdentity {
            pid: 4243,
            ..claimed.clone()
        },
    ] {
        assert_eq!(
            agent_radar::control_plane::registry::verify_identity(
                Some(&claimed),
                &FakeVerifier(Ok(Some(observed)))
            ),
            ProcessVerification::Mismatched
        );
    }
    assert_eq!(
        agent_radar::control_plane::registry::verify_identity(
            Some(&claimed),
            &FakeVerifier(Ok(None))
        ),
        ProcessVerification::Absent
    );
    assert_eq!(
        agent_radar::control_plane::registry::verify_identity(
            Some(&claimed),
            &FakeVerifier(Err("permission denied".into()))
        ),
        ProcessVerification::Unavailable
    );
    assert_eq!(
        agent_radar::control_plane::registry::verify_identity(None, &FakeVerifier(Ok(None))),
        ProcessVerification::Unavailable,
        "no claim is unknown, not absent"
    );
}

#[test]
fn local_procfs_verifier_matches_this_process_birth_identity() {
    let verifier = LocalProcfsVerifier;
    let identity = verifier
        .inspect(std::process::id() as i32)
        .expect("procfs evidence")
        .expect("current process exists");
    assert_eq!(identity.pid, std::process::id() as i32);
    assert!(!identity.boot_id.is_empty());
    assert!(identity.start_ticks > 0);
}

#[test]
fn process_verification_does_not_hold_registry_lock_and_freshness_is_independent() {
    let root = temp_root("verify-independent");
    let registry = Arc::new(Registry::open_at(&root, 1_000).expect("a registry"));
    let mut wanted = request(INCARNATION);
    wanted.process = Some(ProcessIdentity {
        boot_id: "boot-a".into(),
        pid: 4242,
        start_ticks: 99,
    });
    let agent = registry.register(&wanted, 1_000).unwrap();
    let writer = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                1,
                channel_snapshot("working", None),
            ),
            1_000,
        )
        .unwrap();
    let snapshot = published(&registry, &agent.agent_id, Channel::Execution, 12_000)
        .snapshot
        .unwrap();
    assert_eq!(snapshot.freshness, Freshness::Stale);

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let verifier = Arc::new(BlockingVerifier {
        result: Ok(Some(agent.request.process.clone().unwrap())),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let verifying_registry = Arc::clone(&registry);
    let agent_id = agent.agent_id.clone();
    let verification =
        thread::spawn(move || verifying_registry.verify_process(&agent_id, verifier.as_ref()));
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("verifier entered");

    // A registry write must complete while the verifier is blocked outside all
    // registry locks.
    let registration = registry.register(&request(OTHER_INCARNATION), 1_001);
    assert!(registration.is_ok(), "registry update blocked by verifier");
    release_tx.send(()).expect("release verifier");
    assert_eq!(
        verification.join().unwrap().unwrap(),
        ProcessVerification::Verified
    );

    // Fresh publication does not turn a missing process into verified evidence.
    let absent = FakeVerifier(Ok(None));
    assert_eq!(
        registry.verify_process(&agent.agent_id, &absent).unwrap(),
        ProcessVerification::Absent
    );
    let updated = registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                2,
                channel_snapshot("reconnected", None),
            ),
            12_001,
        )
        .unwrap();
    assert_eq!(
        updated.snapshot.as_ref().unwrap().sequence,
        2,
        "freshness and process evidence are independent"
    );
    assert_eq!(
        published(&registry, &agent.agent_id, Channel::Execution, 12_002)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Fresh,
        "a fresh publication does not override absent process evidence"
    );
    drop_root(&root);
}

struct BlockingVerifier {
    result: Result<Option<ProcessIdentity>, String>,
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl ProcessVerifier for BlockingVerifier {
    fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
        self.entered.send(()).expect("verification handshake");
        self.release
            .lock()
            .expect("release lock")
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("bounded verifier release");
        self.result.clone()
    }
}

#[test]
fn a_failed_channel_write_leaves_the_record_and_retry_succeeds() {
    let root = temp_root("write-failure");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let writer = acquire(
        &registry,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        1_000,
    )
    .unwrap();
    registry
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                1,
                channel_snapshot("working", None),
            ),
            1_000,
        )
        .unwrap();
    let retry_request = channel_request(
        &agent.agent_id,
        Channel::Execution,
        &writer.handle,
        2,
        channel_snapshot("busy", None),
    );
    // This integration check still exercises real persistence and external
    // retry behavior; deterministic failure injection lives in a unit test.
    let second = registry
        .publish(&retry_request, 2_000)
        .expect("a newer sequence");
    assert_eq!(second.writer.sequence, 2);
    assert_eq!(second.snapshot.as_ref().unwrap().sequence, 2);
    drop_root(&root);
}

#[test]
fn restart_epoch_stales_restored_facts_even_when_clock_repeats_or_goes_back() {
    let root = temp_root("epoch");
    let first = Registry::open_at(&root, 5_000).expect("a registry");
    let agent = registry_agent(&first, INCARNATION, None);
    let writer = acquire(
        &first,
        &agent.agent_id,
        Channel::Execution,
        publisher(&agent.request.source, &agent.request.incarnation),
        5_000,
    )
    .unwrap();
    let old = first
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                1,
                channel_snapshot("working", None),
            ),
            5_000,
        )
        .unwrap();

    let restarted = Registry::open_at(&root, 5_000).expect("reopen at same time");
    assert_ne!(first.serving_epoch(), restarted.serving_epoch());
    let restored = published(&restarted, &agent.agent_id, Channel::Execution, 4_000);
    let snapshot = restored.snapshot.expect("restored facts");
    assert_eq!(snapshot.freshness, Freshness::Stale);
    assert!(snapshot.restored);
    assert_eq!(snapshot.sequence, old.writer.sequence);
    assert_eq!(snapshot.handle, writer.handle);
    assert_eq!(snapshot.source, writer.source);

    // The unreplaced handle reconnects with a newer sequence and becomes fresh.
    let refreshed = restarted
        .publish(
            &channel_request(
                &agent.agent_id,
                Channel::Execution,
                &writer.handle,
                2,
                channel_snapshot("reconnected", None),
            ),
            4_000,
        )
        .unwrap();
    assert_eq!(refreshed.writer.sequence, 2);
    assert_eq!(
        published(&restarted, &agent.agent_id, Channel::Execution, 4_001)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Fresh
    );

    // Replaying the old content against the restored epoch does not freshen it.
    let replay = Registry::open_at(&root, 4_000).expect("a second restore");
    assert_eq!(
        published(&replay, &agent.agent_id, Channel::Execution, 4_001)
            .snapshot
            .unwrap()
            .freshness,
        Freshness::Stale
    );
    drop_root(&root);
}

// --- Mutable current-session context ---------------------------------------------

const NEXT_SESSION: &str = "e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b";

fn context_request(
    agent_id: &str,
    publisher: PublisherIdentity,
    handle: Option<&str>,
    sequence: u64,
    session: Option<&str>,
) -> ContextRequest {
    ContextRequest {
        agent_id: agent_id.into(),
        publisher,
        writer_handle: handle.map(str::to_string),
        replace: None,
        sequence,
        lease_ms: Some(10_000),
        observed_at: None,
        context: ContextValue {
            session: session.map(str::to_string),
        },
    }
}

/// A takeover: it presents no handle and names the incumbent it observed.
fn context_replacement(
    agent_id: &str,
    publisher: PublisherIdentity,
    expected: &WriterBinding,
    sequence: u64,
    session: Option<&str>,
) -> ContextRequest {
    ContextRequest {
        replace: Some(ExpectedWriter {
            generation: expected.generation,
            handle: expected.handle.clone(),
        }),
        ..context_request(agent_id, publisher, None, sequence, session)
    }
}

fn context_path(registry: &Registry, agent_id: &str) -> PathBuf {
    registry
        .publications()
        .join(format!("{agent_id}.context.json"))
}

fn context_of(registry: &Registry, agent_id: &str, now_ms: i64) -> PublicContext {
    registry
        .context(agent_id, now_ms)
        .expect("a read")
        .expect("a context")
}

fn format_millis(value: i64) -> String {
    agent_radar::control_plane::store::format_millis(value)
}

#[test]
fn a_context_switch_republishes_without_changing_identity_or_generation() {
    let root = temp_root("context-switch");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);

    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    assert!(first.warning.is_none());
    assert_eq!(first.record.writer.generation, 1);
    let handle = first.record.writer.handle.clone();
    let path = context_path(&registry, &agent.agent_id);
    assert_eq!(
        fs::metadata(&path).expect("a record").permissions().mode() & 0o777,
        0o600,
        "a context record is private"
    );

    // A session switch is a newer sequence under the same writer: not a new
    // subject, not a new agent id and not a new generation.
    let switched = registry
        .publish_context(
            &context_request(&agent.agent_id, me, Some(&handle), 2, Some(NEXT_SESSION)),
            1_001,
        )
        .expect("a switch");
    assert_eq!(switched.record.writer.generation, 1);
    assert_eq!(switched.record.writer.handle, handle);
    assert_eq!(switched.record.writer.sequence, 2);
    assert_eq!(
        switched.record.context.session.as_deref(),
        Some(NEXT_SESSION)
    );

    let read = context_of(&registry, &agent.agent_id, 1_002);
    assert_eq!(read.agent_id, agent.agent_id);
    assert_eq!(read.writer.generation, 1);
    assert_eq!(read.writer.sequence, 2);
    assert_eq!(read.context.generation, 1);
    assert_eq!(read.context.sequence, 2);
    assert_eq!(read.context.session.as_deref(), Some(NEXT_SESSION));
    assert_eq!(read.context.freshness, Freshness::Fresh);
    assert!(!read.context.restored);
    assert_eq!(
        registry
            .get(&agent.agent_id)
            .expect("a read")
            .expect("a record"),
        agent,
        "publishing context changes no registration"
    );

    // The lease is bounded on the daemon's clock and nothing else.
    assert_eq!(
        context_of(&registry, &agent.agent_id, 11_000)
            .context
            .freshness,
        Freshness::Fresh
    );
    assert_eq!(
        context_of(&registry, &agent.agent_id, 11_001)
            .context
            .freshness,
        Freshness::Stale
    );

    // One agent context is one record: no second file a partial write could
    // leave behind.
    let records: Vec<_> = fs::read_dir(registry.publications())
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    assert_eq!(records.len(), 1, "one agent context is one record");
    drop_root(&root);
}

#[test]
fn an_identical_context_replay_warns_and_refreshes_no_lease() {
    let root = temp_root("context-replay");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let request = context_request(&agent.agent_id, me, None, 1, Some(SESSION));
    let first = registry
        .publish_context(&request, 1_000)
        .expect("a first publish");
    let replay_request = ContextRequest {
        writer_handle: Some(first.record.writer.handle.clone()),
        ..request
    };

    // A replay is answered, not refused — and it says what it did not do.
    let replay = registry
        .publish_context(&replay_request, 9_000)
        .expect("a replay is answered");
    let warning = replay.warning.expect("a replay warns");
    assert!(warning.contains("not refreshed"), "{warning}");
    assert_eq!(replay.record, first.record, "a replay stores nothing new");
    assert_eq!(replay.record.context.expires_at, format_millis(11_000));

    // The lease still ends where the first report put it.
    assert_eq!(
        context_of(&registry, &agent.agent_id, 10_999)
            .context
            .freshness,
        Freshness::Fresh
    );
    assert_eq!(
        context_of(&registry, &agent.agent_id, 11_000)
            .context
            .freshness,
        Freshness::Stale
    );
    drop_root(&root);
}

#[test]
fn context_sequence_conflicts_and_older_reports_are_refused() {
    let root = temp_root("context-sequence");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    let handle = first.record.writer.handle.clone();
    registry
        .publish_context(
            &context_request(
                &agent.agent_id,
                me.clone(),
                Some(&handle),
                2,
                Some(NEXT_SESSION),
            ),
            1_001,
        )
        .expect("a switch");

    // The stored sequence, re-sent with different content, is a conflict — and
    // a different lease is content.
    let conflicting = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), Some(&handle), 2, Some(SESSION)),
            1_002,
        )
        .expect_err("a conflict");
    assert!(conflicting.contains("conflicting"), "{conflicting}");
    let lease_conflict = ContextRequest {
        lease_ms: Some(20_000),
        ..context_request(
            &agent.agent_id,
            me.clone(),
            Some(&handle),
            2,
            Some(NEXT_SESSION),
        )
    };
    assert!(
        registry
            .publish_context(&lease_conflict, 1_003)
            .expect_err("a conflict")
            .contains("conflicting")
    );

    // An older sequence is refused even when its session matches the incumbent's.
    let older = registry
        .publish_context(
            &context_request(&agent.agent_id, me, Some(&handle), 1, Some(SESSION)),
            1_004,
        )
        .expect_err("a refusal");
    assert!(older.contains("older than stored sequence"), "{older}");

    let read = context_of(&registry, &agent.agent_id, 1_005);
    assert_eq!(read.context.sequence, 2);
    assert_eq!(read.context.session.as_deref(), Some(NEXT_SESSION));
    assert_eq!(read.context.expires_at, format_millis(11_001));
    drop_root(&root);
}

#[test]
fn a_non_incumbent_context_writer_is_refused_and_leaves_the_record_alone() {
    let root = temp_root("context-incumbent");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    let handle = first.record.writer.handle.clone();
    let path = context_path(&registry, &agent.agent_id);
    let before = fs::read(&path).expect("the stored record");
    let other = publisher(&agent.request.source, OTHER_INCARNATION);
    let incumbent = format!("the incumbent is generation 1 handle {handle}");

    // A stale handshake names both its reason and the writer to contend with.
    let stale = registry
        .publish_context(
            &context_request(
                &agent.agent_id,
                other.clone(),
                Some(&random_uuid()),
                2,
                Some(NEXT_SESSION),
            ),
            2_000,
        )
        .expect_err("a refusal");
    assert!(
        stale.contains("stale or belongs to another generation"),
        "{stale}"
    );
    assert!(stale.contains(&incumbent), "{stale}");
    let unreported = registry
        .publish_context(
            &context_request(&agent.agent_id, other.clone(), None, 2, Some(NEXT_SESSION)),
            2_000,
        )
        .expect_err("a refusal");
    assert!(unreported.contains("did not present"), "{unreported}");
    assert!(unreported.contains(&incumbent), "{unreported}");

    // Naming the incumbent is not enough while its lease is live.
    let takeover = registry
        .publish_context(
            &context_replacement(
                &agent.agent_id,
                other.clone(),
                &first.record.writer,
                2,
                Some(NEXT_SESSION),
            ),
            2_000,
        )
        .expect_err("a refusal");
    assert!(
        takeover.contains("fresh writer takeover is refused"),
        "{takeover}"
    );
    assert!(takeover.contains(&incumbent), "{takeover}");

    assert_eq!(
        fs::read(&path).expect("the stored record"),
        before,
        "a refused write changes nothing"
    );
    let next = registry
        .publish_context(
            &context_request(&agent.agent_id, me, Some(&handle), 2, Some(NEXT_SESSION)),
            2_001,
        )
        .expect("the incumbent is unaffected");
    assert_eq!(next.record.writer.handle, handle);
    assert_eq!(next.record.context.session.as_deref(), Some(NEXT_SESSION));
    drop_root(&root);
}

#[test]
fn context_replacement_after_retirement_advances_the_generation() {
    let root = temp_root("context-retire");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    let old = first.record.writer.clone();
    let successor = publisher(&agent.request.source, OTHER_INCARNATION);

    registry
        .retire_context(&agent.agent_id, &old.handle, 1_500)
        .expect("a retirement");

    // A replacement must name the exact incumbent it observed.
    let wrong = WriterBinding {
        handle: random_uuid(),
        ..old.clone()
    };
    let mismatch = registry
        .publish_context(
            &context_replacement(
                &agent.agent_id,
                successor.clone(),
                &wrong,
                1,
                Some(NEXT_SESSION),
            ),
            1_500,
        )
        .expect_err("a refusal");
    assert!(
        mismatch.contains("replacement does not apply"),
        "{mismatch}"
    );
    assert!(
        mismatch.contains(&format!("generation 1 handle {}", old.handle)),
        "{mismatch}"
    );

    let replacement = registry
        .publish_context(
            &context_replacement(
                &agent.agent_id,
                successor.clone(),
                &old,
                1,
                Some(NEXT_SESSION),
            ),
            1_500,
        )
        .expect("a replacement");
    assert_eq!(replacement.record.writer.generation, 2);
    assert_ne!(replacement.record.writer.handle, old.handle);
    assert_eq!(replacement.record.writer.sequence, 1);
    assert_eq!(replacement.record.context.incarnation, OTHER_INCARNATION);
    assert_eq!(
        replacement.record.context.session.as_deref(),
        Some(NEXT_SESSION)
    );

    // The old handle is fenced for good, and a delayed replay of the same
    // replacement no longer applies.
    let fenced = registry
        .publish_context(
            &context_request(&agent.agent_id, me, Some(&old.handle), 2, Some(SESSION)),
            1_600,
        )
        .expect_err("a refusal");
    assert!(
        fenced.contains("stale or belongs to another generation"),
        "{fenced}"
    );
    let delayed = registry
        .publish_context(
            &context_replacement(&agent.agent_id, successor, &old, 2, Some(SESSION)),
            1_700,
        )
        .expect_err("a refusal");
    assert!(delayed.contains("replacement does not apply"), "{delayed}");

    let read = context_of(&registry, &agent.agent_id, 1_700);
    assert_eq!(read.writer.generation, 2);
    assert_eq!(read.context.session.as_deref(), Some(NEXT_SESSION));
    let records: Vec<_> = fs::read_dir(registry.publications())
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    assert_eq!(records.len(), 1, "replacement rewrites the one record");
    drop_root(&root);
}

#[test]
fn context_replacement_after_lease_expiry_advances_the_generation() {
    let root = temp_root("context-expiry");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me, None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    let successor = publisher(&agent.request.source, OTHER_INCARNATION);

    // While the lease is live the incumbent still rules.
    let fresh = registry
        .publish_context(
            &context_replacement(
                &agent.agent_id,
                successor.clone(),
                &first.record.writer,
                1,
                Some(NEXT_SESSION),
            ),
            10_999,
        )
        .expect_err("a refusal");
    assert!(
        fresh.contains("fresh writer takeover is refused"),
        "{fresh}"
    );

    // At expiry the reader sees stale facts and the record may be succeeded.
    assert_eq!(
        context_of(&registry, &agent.agent_id, 11_000)
            .context
            .freshness,
        Freshness::Stale
    );
    let replacement = registry
        .publish_context(
            &context_replacement(
                &agent.agent_id,
                successor,
                &first.record.writer,
                1,
                Some(NEXT_SESSION),
            ),
            11_000,
        )
        .expect("a replacement");
    assert_eq!(replacement.record.writer.generation, 2);
    let read = context_of(&registry, &agent.agent_id, 11_000);
    assert_eq!(read.writer.generation, 2);
    assert_eq!(read.context.freshness, Freshness::Fresh);
    drop_root(&root);
}

#[test]
fn a_malformed_context_record_fails_explicitly() {
    let root = temp_root("context-corrupt");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let agent = registry_agent(&registry, INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    registry
        .publish_context(
            &context_request(&agent.agent_id, me, None, 1, Some(SESSION)),
            1_000,
        )
        .expect("a first publish");
    let path = context_path(&registry, &agent.agent_id);
    let valid = fs::read(&path).expect("a read");
    let planted = |value: &serde_json::Value| {
        fs::write(&path, serde_json::to_vec(value).expect("an encoding")).expect("a write")
    };

    fs::write(&path, b"not json").expect("a planted file");
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a malformed record is an error, not an absence");
    assert!(error.contains("not a context record"), "{error}");

    let mut value: serde_json::Value = serde_json::from_slice(&valid).expect("a decoding");
    value["version"] = serde_json::json!(2);
    planted(&value);
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a foreign version");
    assert!(error.contains("is not served"), "{error}");

    let mut value: serde_json::Value = serde_json::from_slice(&valid).expect("a decoding");
    value["context"]["generation"] = serde_json::json!(2);
    planted(&value);
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a report ahead of its writer");
    assert!(
        error.contains("not the current writer generation"),
        "{error}"
    );

    let mut value: serde_json::Value = serde_json::from_slice(&valid).expect("a decoding");
    value["context"]["session"] = serde_json::json!("not-a-uuid");
    planted(&value);
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a malformed session");
    assert!(error.contains("malformed session"), "{error}");

    // A planted file larger than the record bound fails before being read whole.
    fs::write(&path, vec![b'x'; MAX_CONTEXT_BYTES + 1]).expect("a planted file");
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a refusal");
    assert!(error.contains("exceeds"), "{error}");
    fs::remove_file(&path).expect("a removal");

    // A symlink is never followed, and a missing record is an absence.
    let elsewhere = temp_root("context-symlink-target");
    let target = elsewhere.join("real.json");
    fs::write(&target, &valid).expect("a file");
    symlink(&target, &path).expect("a symlink");
    let error = registry
        .context(&agent.agent_id, 1_000)
        .expect_err("a refusal");
    assert!(error.contains("not a regular file"), "{error}");
    fs::remove_file(&path).expect("a removal");
    assert_eq!(
        registry.context(&agent.agent_id, 1_000).expect("a read"),
        None
    );
    drop_root(&root);
    drop_root(&elsewhere);
}

#[test]
fn an_absent_context_and_an_explicit_null_stay_distinguishable() {
    let root = temp_root("context-null");
    let registry = Registry::open_at(&root, 1_000).expect("a registry");
    let silent = registry_agent(&registry, INCARNATION, None);
    assert_eq!(
        registry.context(&silent.agent_id, 1_000).expect("a read"),
        None,
        "never published is an absence, not a null"
    );

    let agent = registry_agent(&registry, OTHER_INCARNATION, None);
    let me = publisher(&agent.request.source, &agent.request.incarnation);
    let first = registry
        .publish_context(
            &context_request(&agent.agent_id, me.clone(), None, 1, None),
            1_000,
        )
        .expect("an explicit null");
    assert_eq!(first.record.context.session, None);

    let cleared = context_of(&registry, &agent.agent_id, 1_000);
    assert_eq!(cleared.context.session, None, "no current session");
    let json = serde_json::to_string(&cleared).expect("an encoding");
    assert!(json.contains("\"session\":null"), "{json}");
    assert!(!json.contains("handle"), "{json}");
    assert!(!json.contains("serving_epoch"), "{json}");
    assert!(!json.contains("context.json"), "{json}");

    // A publisher that later gains a session republishes it under the same
    // record, and still nothing private appears in the read.
    let handle = first.record.writer.handle.clone();
    let switched = registry
        .publish_context(
            &context_request(&agent.agent_id, me, Some(&handle), 2, Some(SESSION)),
            1_001,
        )
        .expect("a switch");
    assert_eq!(switched.record.context.session.as_deref(), Some(SESSION));
    let json =
        serde_json::to_string(&context_of(&registry, &agent.agent_id, 1_001)).expect("an encoding");
    assert!(json.contains(SESSION), "{json}");
    assert!(!json.contains(&handle), "{json}");

    // Context belongs to a registered agent, and only a first publish binds a
    // writer: neither a stranger nor a replacement with nothing to replace is
    // accepted.
    let stranger = registry.publish_context(
        &context_request(
            &random_uuid(),
            publisher("herdsman", INCARNATION),
            None,
            1,
            Some(SESSION),
        ),
        1_000,
    );
    assert!(
        stranger
            .expect_err("a refusal")
            .contains("is not registered")
    );
    let nothing = registry.publish_context(
        &context_replacement(
            &silent.agent_id,
            publisher("herdsman", INCARNATION),
            &first.record.writer,
            1,
            Some(SESSION),
        ),
        1_000,
    );
    assert!(
        nothing
            .expect_err("a refusal")
            .contains("first publish binds one")
    );
    drop_root(&root);
}

#[test]
fn a_restored_context_reads_stale_until_republished() {
    let root = temp_root("context-epoch");
    let agent_id;
    let handle;
    let me = publisher("herdsman", INCARNATION);
    {
        let registry = Registry::open_at(&root, 5_000).expect("a registry");
        let agent = registry_agent(&registry, INCARNATION, None);
        agent_id = agent.agent_id.clone();
        let first = registry
            .publish_context(
                &context_request(&agent_id, me.clone(), None, 1, Some(SESSION)),
                5_000,
            )
            .expect("a first publish");
        handle = first.record.writer.handle.clone();
    }

    let restarted = Registry::open_at(&root, 5_000).expect("a reopen at the same clock");
    let restored = context_of(&restarted, &agent_id, 4_000);
    assert!(restored.context.restored);
    assert_eq!(restored.context.freshness, Freshness::Stale);
    assert_eq!(restored.context.session.as_deref(), Some(SESSION));
    assert!(
        !serde_json::to_string(&restored)
            .expect("an encoding")
            .contains(&handle),
        "no public read names the writer handle"
    );

    // The unreplaced handle reconnects with a newer sequence and becomes fresh,
    // without a replacement and without a new generation.
    let refreshed = restarted
        .publish_context(
            &context_request(&agent_id, me, Some(&handle), 2, Some(NEXT_SESSION)),
            4_000,
        )
        .expect("a reconnect");
    assert_eq!(refreshed.record.writer.generation, 1);
    assert_eq!(refreshed.record.writer.handle, handle);
    assert_eq!(
        context_of(&restarted, &agent_id, 4_001).context.freshness,
        Freshness::Fresh
    );
    drop_root(&root);
}
