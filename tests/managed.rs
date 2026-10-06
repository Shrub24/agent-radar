//! The owner-control worker against temporary, explicitly pathed stub owners.
//!
//! Every test acts as the stub owner: it creates its own `inbox` and `results`
//! under a throwaway root and passes that root to the worker. No test touches
//! `~/.pi/agent/pi-herdsman/control`, starts a real request or runs a worker.
//! The worker never closes anything through the runtime; the only observable
//! effect is the request file it publishes.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_radar::control::{ControlRequest, Operation as ControlOperation, Outcome};
use agent_radar::lifecycle::managed_request;
use agent_radar::model::{
    AgentObservation, FleetObservation, HerdsmanFacts, Lineage, Location, Pane, RuntimeStatus,
    SessionUuid, Tab, Workspace,
};
use agent_radar::{ManagedActions, Update, UpdateKind};

const OWNER: &str = "01a10c77-8a6b-7035-8a0e-b1fa607bb507";
const RUN: &str = "8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71";
const SESSION: &str = "9a7b6c5d-4e3f-4a2b-8c1d-0e9f8a7b6c5d";

/// How long one update may take before the test calls it stuck.
const PATIENCE: Duration = Duration::from_secs(10);

/// A throwaway control root with a trusted owner directory, created and removed
/// by the test. The worker never creates any of it.
struct TestOwner {
    root: PathBuf,
}

impl TestOwner {
    fn new() -> Self {
        let root = unique_root();
        let owner = root.join(OWNER);
        for directory in [owner.clone(), owner.join("inbox"), owner.join("results")] {
            fs::create_dir_all(&directory).expect("create a directory");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        Self { root }
    }

    fn owner_dir(&self) -> PathBuf {
        self.root.join(OWNER)
    }

    fn write_claim(&self, request_id: &str) {
        let path = self
            .owner_dir()
            .join("inbox")
            .join(format!("{request_id}.claim"));
        fs::write(&path, b"claimed").expect("write a claim");
    }

    fn write_result(&self, request_id: &str, body: &str) {
        let path = self
            .owner_dir()
            .join("results")
            .join(format!("{request_id}.json"));
        fs::write(&path, body).expect("write a result");
    }

    /// The single published request, decoded from the owner's inbox.
    fn published(&self) -> (PathBuf, ControlRequest) {
        let inbox = self.owner_dir().join("inbox");
        let mut files: Vec<PathBuf> = fs::read_dir(&inbox)
            .expect("inbox")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(files.len(), 1, "exactly one request published");
        let path = files.pop().expect("one file");
        let request: ControlRequest =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("decode");
        (path, request)
    }
}

impl Drop for TestOwner {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn unique_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("radar-managed-{}-{nanos}", std::process::id()))
}

/// One workspace, one pane, one managed worker with a complete identity.
fn managed_inventory() -> FleetObservation {
    let location = Location {
        workspace_id: "wA".into(),
        tab_id: "wA:t1".into(),
        pane_id: "wA:p1".into(),
    };
    let session = SessionUuid::parse(SESSION).expect("a UUID");
    let owner = SessionUuid::parse(OWNER).expect("a UUID");
    FleetObservation {
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: Some("main".into()),
            number: None,
        }],
        tabs: vec![Tab {
            tab_id: "wA:t1".into(),
            workspace_id: "wA".into(),
            label: None,
            number: None,
        }],
        panes: vec![Pane {
            location: location.clone(),
            label: None,
            title: None,
        }],
        agents: vec![AgentObservation {
            location,
            name: Some("pi".into()),
            label: None,
            status: Some(RuntimeStatus::Idle),
            session: None,
            lineage: Some(Lineage {
                session,
                parent: Some(owner),
            }),
            facts: HerdsmanFacts {
                managed_metadata: true,
                label: Some("implementer-1".into()),
                run: Some(RUN.into()),
                state: Some(agent_radar::model::SemanticState::Idle),
                ..HerdsmanFacts::default()
            },
        }],
    }
}

/// The next update, or a failure after [`PATIENCE`].
fn next(actions: &mut ManagedActions) -> Update {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(update) = actions.poll() {
            return update;
        }
        assert!(Instant::now() < deadline, "no update arrived");
        thread::sleep(Duration::from_millis(5));
    }
}

fn request_files(root: &Path) -> usize {
    fs::read_dir(root.join(OWNER).join("inbox"))
        .expect("inbox")
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        })
        .count()
}

fn close_request(inventory: &FleetObservation) -> agent_radar::ManagedRequest {
    managed_request(inventory, "wA:p1", ControlOperation::Close).expect("a complete identity")
}

#[test]
fn a_confirmed_managed_close_publishes_the_exact_v1_request() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions
        .start(close_request(&inventory))
        .expect("publish off-thread");

    let update = next(&mut actions);
    assert!(matches!(update.kind, UpdateKind::Submitted), "{update:?}");
    assert_eq!(update.operation, ControlOperation::Close);
    assert_eq!(update.label, "implementer-1");

    let (path, request) = owner.published();
    assert_eq!(
        fs::symlink_metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "mode 0600"
    );
    assert_eq!(request.version, 1);
    assert_eq!(request.operation, ControlOperation::Close);
    assert_eq!(request.agent, "implementer-1");
    assert_eq!(request.run_id, RUN);
    assert_eq!(request.pane_id.as_deref(), Some("wA:p1"));
    assert_eq!(request.pi_session_id.as_deref(), Some(SESSION));
    assert_eq!(request.confirmation.operation, ControlOperation::Close);
    assert_eq!(request.confirmation.label, "implementer-1");
    assert_eq!(request.confirmation.run_id, RUN);
    assert_eq!(request.requester, "agent-radar");
    // 30-second absolute expiry, as the contract fixes it.
    assert_eq!(
        request.expires_at_ms().expect("expiry"),
        agent_radar::control::timestamp_millis(&request.requested_at).expect("requested") + 30_000
    );
    actions.shutdown();
}

#[test]
fn a_confirmed_restart_echoes_its_operation() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let request = managed_request(&inventory, "wA:p1", ControlOperation::Restart)
        .expect("a complete identity");
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(request).expect("publish");
    let update = next(&mut actions);
    assert!(matches!(update.kind, UpdateKind::Submitted), "{update:?}");
    let (_, published) = owner.published();
    assert_eq!(published.operation, ControlOperation::Restart);
    assert_eq!(published.confirmation.operation, ControlOperation::Restart);
    actions.shutdown();
}

#[test]
fn a_duplicate_target_is_refused_while_a_request_is_outstanding() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(close_request(&inventory)).expect("publish");
    // Wait for it to be outstanding (the publication may still be in flight).
    let deadline = Instant::now() + PATIENCE;
    let first = loop {
        if let Some(update) = actions.poll() {
            break update;
        }
        assert!(Instant::now() < deadline, "no publication");
        thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(first.kind, UpdateKind::Submitted), "{first:?}");

    let error = actions
        .start(close_request(&inventory))
        .expect_err("the same exact target is suppressed");
    assert!(error.contains("already outstanding"), "{error}");
    assert_eq!(request_files(&owner.root), 1, "no second request");

    // A restart of the same target is the same exact target, so it too is held.
    let restart = managed_request(&inventory, "wA:p1", ControlOperation::Restart)
        .expect("a complete identity");
    assert!(actions.start(restart).is_err());
    actions.shutdown();
}

#[test]
fn a_claim_reports_started_and_a_later_result_refines_it_without_a_retry() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(close_request(&inventory)).expect("publish");

    let submitted = next(&mut actions);
    assert!(
        matches!(submitted.kind, UpdateKind::Submitted),
        "{submitted:?}"
    );
    let request_id = submitted.id.clone();

    owner.write_claim(&request_id);
    let started = next(&mut actions);
    assert!(matches!(started.kind, UpdateKind::Started), "{started:?}");
    assert_eq!(started.id, request_id, "the same request is refined");
    assert_eq!(
        request_files(&owner.root),
        1,
        "a claim is never retried or duplicated"
    );

    owner.write_result(
        &request_id,
        &format!(
            r#"{{"version":1,"requestId":"{request_id}","operation":"close","outcome":"closed","message":"Closed implementer-1.","effects":["process_ended","pane_closed"],"completedAt":"2026-01-01T00:00:01.000Z"}}"#
        ),
    );
    let answered = next(&mut actions);
    match answered.kind {
        UpdateKind::Answered(result) => {
            assert_eq!(result.outcome, Outcome::Closed);
            assert!(result.pane_closed());
        }
        other => panic!("expected the owner's answer, got {other:?}"),
    }
    assert_eq!(answered.id, request_id);
    actions.shutdown();
}

#[test]
fn an_owner_refusal_is_reported_unchanged() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(close_request(&inventory)).expect("publish");
    let submitted = next(&mut actions);
    let request_id = submitted.id.clone();

    owner.write_result(
        &request_id,
        &format!(
            r#"{{"version":1,"requestId":"{request_id}","operation":"close","outcome":"refused","category":"agent_busy","message":"implementer-1 is working.","effects":[],"completedAt":"2026-01-01T00:00:00.500Z"}}"#
        ),
    );
    let answered = next(&mut actions);
    match answered.kind {
        UpdateKind::Answered(result) => {
            assert_eq!(result.outcome, Outcome::Refused);
            assert_eq!(result.category.as_deref(), Some("agent_busy"));
            assert!(result.effects.is_empty());
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    actions.shutdown();
}

#[test]
fn an_unknown_result_is_reported_but_keeps_the_target_suppressed() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(close_request(&inventory)).expect("publish");
    let submitted = next(&mut actions);
    let request_id = submitted.id.clone();

    // The owner answers that it could not establish what happened.
    owner.write_result(
        &request_id,
        &format!(
            r#"{{"version":1,"requestId":"{request_id}","operation":"close","outcome":"unknown","message":"the owner could not confirm the close","effects":[],"completedAt":"2026-01-01T00:00:00.500Z"}}"#
        ),
    );
    let unknown = next(&mut actions);
    assert!(
        matches!(&unknown.kind, UpdateKind::Answered(result) if result.outcome == Outcome::Unknown),
        "{unknown:?}"
    );
    assert_eq!(unknown.id, request_id);

    // Unknown is not resolution: the exact target stays suppressed, and no
    // second request is published even though the UI saw the answer.
    let error = actions
        .start(close_request(&inventory))
        .expect_err("an unknown outcome must not release the target");
    assert!(error.contains("already outstanding"), "{error}");
    assert_eq!(request_files(&owner.root), 1, "no second request");

    // A later known result refines the same request with no new publication,
    // and only a known resolution releases the target.
    owner.write_result(
        &request_id,
        &format!(
            r#"{{"version":1,"requestId":"{request_id}","operation":"close","outcome":"closed","message":"closed implementer-1","effects":["process_ended","pane_closed"],"completedAt":"2026-01-01T00:00:01.000Z"}}"#
        ),
    );
    let answered = next(&mut actions);
    match answered.kind {
        UpdateKind::Answered(result) => assert_eq!(result.outcome, Outcome::Closed),
        other => panic!("expected the known result, got {other:?}"),
    }
    assert_eq!(answered.id, request_id);
    assert_eq!(request_files(&owner.root), 1, "no new publication");
    actions
        .start(close_request(&inventory))
        .expect("a known resolution releases the target");
    actions.shutdown();
}

#[test]
fn a_mismatched_result_is_invalid_evidence_not_an_answer() {
    let owner = TestOwner::new();
    let inventory = managed_inventory();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions.start(close_request(&inventory)).expect("publish");
    let submitted = next(&mut actions);
    let request_id = submitted.id.clone();

    // A result whose operation is a restart can never answer a close.
    owner.write_result(
        &request_id,
        &format!(
            r#"{{"version":1,"requestId":"{request_id}","operation":"restart","outcome":"refused","category":"agent_busy","message":"m","effects":[],"completedAt":"2026-01-01T00:00:00.500Z"}}"#
        ),
    );
    let invalid = next(&mut actions);
    assert!(
        matches!(invalid.kind, UpdateKind::Invalid(_)),
        "{invalid:?}"
    );
    assert_eq!(invalid.id, request_id);
    actions.shutdown();
}

#[test]
fn a_missing_or_untrusted_root_refuses_and_writes_nothing() {
    let root = unique_root();
    let mut actions = ManagedActions::new(root.join("absent"));
    actions
        .start(close_request(&managed_inventory()))
        .expect("start only checks the target, not the transport");
    let update = next(&mut actions);
    match update.kind {
        UpdateKind::Failed(message) => assert!(!message.is_empty()),
        other => panic!("expected a transport refusal, got {other:?}"),
    }
    assert!(!root.exists(), "the worker created nothing");
}

#[test]
fn shutdown_leaves_a_published_request_intact() {
    let owner = TestOwner::new();
    let mut actions = ManagedActions::new(owner.root.clone());
    actions
        .start(close_request(&managed_inventory()))
        .expect("publish");
    let submitted = next(&mut actions);
    assert!(
        matches!(submitted.kind, UpdateKind::Submitted),
        "{submitted:?}"
    );
    let (path, _) = owner.published();

    let started = Instant::now();
    actions.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown waited for expiry instead of returning"
    );
    assert!(path.exists(), "the published request was not deleted");
    assert_eq!(request_files(&owner.root), 1);
}
