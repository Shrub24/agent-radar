//! The owner-control file boundary against temporary, explicitly pathed owners.
//!
//! Every test acts as the stub owner: it creates its own `inbox` and `results`
//! under a throwaway root and passes that root to the client. No test touches
//! `~/.pi/agent/pi-herdsman/control`, starts a request or runs a worker.
//!
//! The canonical fixture is a byte-identical copy of the upstream document with
//! a single `source` key added; the tests here decode every canonical request
//! and result and exercise the local boundary, leaving owner-only state checks
//! (eligibility, cross-check agreement, live identity) to the owner.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_radar::control::{
    ControlError, ControlRequest, ControlResult, NewRequest, Operation, Outcome, OwnerControl,
    RequestState, timestamp_millis,
};
use serde_json::Value;

const OWNER: &str = "01a10c77-8a6b-7035-8a0e-b1fa607bb507";
const REQUEST: &str = "a0000000-0000-4000-8000-000000000001";
const RUN: &str = "8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71";
const SESSION: &str = "9a7b6c5d-4e3f-4a2b-8c1d-0e9f8a7b6c5d";
/// SHA-256 of the canonical upstream fixture this copy was taken from.
const UPSTREAM_SHA256: &str = "d5e882d418775d3aeea5d94a7360a0c190c0de821084b455e0f30336820409e1";

/// A throwaway control root with a trusted owner directory, created and removed
/// by the test. The client never creates any of it.
struct TestOwner {
    root: PathBuf,
}

impl TestOwner {
    fn new() -> Self {
        let root = unique_root();
        let owner = root.join(OWNER);
        make_private(&owner);
        make_private(&owner.join("inbox"));
        make_private(&owner.join("results"));
        Self { root }
    }

    fn open(&self) -> OwnerControl {
        OwnerControl::open(&self.root, OWNER).expect("a trusted owner directory")
    }

    fn owner_dir(&self) -> PathBuf {
        self.root.join(OWNER)
    }

    fn write_result(&self, request_id: &str, body: &str) {
        let path = self
            .owner_dir()
            .join("results")
            .join(format!("{request_id}.json"));
        fs::write(&path, body).expect("write a result");
    }

    fn write_claim(&self, request_id: &str) {
        let path = self
            .owner_dir()
            .join("inbox")
            .join(format!("{request_id}.claim"));
        fs::write(&path, b"claimed").expect("write a claim");
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
    std::env::temp_dir().join(format!("radar-control-{}-{nanos}", std::process::id()))
}

fn make_private(path: &Path) {
    fs::create_dir_all(path).expect("create a directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private mode");
}

fn new_request(operation: Operation) -> ControlRequest {
    ControlRequest::new(
        NewRequest {
            operation,
            label: "implementer-1".into(),
            run_id: RUN.into(),
            pane_id: Some("w1:p3".into()),
            pi_session_id: Some(SESSION.into()),
            pi_session_path: None,
        },
        UNIX_EPOCH + Duration::from_secs(1_767_225_600),
    )
    .expect("a fresh request")
}

fn flip(operation: Operation) -> Operation {
    match operation {
        Operation::Close => Operation::Restart,
        Operation::Restart => Operation::Close,
    }
}

fn parse_operation(value: &str) -> Operation {
    match value {
        "close" => Operation::Close,
        "restart" => Operation::Restart,
        other => panic!("unknown operation {other}"),
    }
}

fn result_json(request_id: &str, operation: &str, outcome: &str, effects: &str) -> String {
    format!(
        r#"{{"version":1,"requestId":"{request_id}","operation":"{operation}","outcome":"{outcome}","message":"m","effects":[{effects}],"completedAt":"2026-01-01T00:00:01.000Z"}}"#
    )
}

#[test]
fn a_request_is_published_atomically_with_exact_confirmation() {
    let owner = TestOwner::new();
    let control = owner.open();
    let request = new_request(Operation::Close);
    control.publish(&request).expect("publish");

    let inbox = owner.owner_dir().join("inbox");
    assert_eq!(
        fs::read_dir(&inbox).expect("inbox").count(),
        1,
        "exactly one file, no temporary left behind"
    );

    let path = inbox.join(format!("{}.json", request.request_id));
    let metadata = fs::symlink_metadata(&path).expect("the published request");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600, "mode 0600");

    let decoded: ControlRequest =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("decode");
    assert_eq!(decoded, request);
    assert_eq!(decoded.confirmation.label, "implementer-1");
    assert_eq!(decoded.confirmation.run_id, RUN);
    assert_eq!(decoded.confirmation.operation, Operation::Close);
    assert_eq!(decoded.pane_id.as_deref(), Some("w1:p3"));
    assert_eq!(decoded.requester, "agent-radar");
    assert_eq!(
        decoded.expires_at_ms().expect("expiry"),
        timestamp_millis("2026-01-01T00:00:30.000Z").unwrap()
    );
}

#[test]
fn a_collision_refuses_without_altering_the_existing_request() {
    let owner = TestOwner::new();
    let control = owner.open();
    let request = new_request(Operation::Close);
    control.publish(&request).expect("publish");
    let path = owner
        .owner_dir()
        .join("inbox")
        .join(format!("{}.json", request.request_id));
    let before = fs::read(&path).expect("read");

    let error = control.publish(&request).expect_err("a collision refuses");
    assert!(matches!(error, ControlError::Collision(_)), "{error:?}");
    assert_eq!(
        fs::read(&path).expect("read"),
        before,
        "existing file untouched"
    );
    assert_eq!(
        fs::read_dir(owner.owner_dir().join("inbox"))
            .expect("inbox")
            .count(),
        1,
        "no temporary file left behind"
    );
}

#[test]
fn a_missing_or_untrusted_directory_refuses() {
    let root = unique_root();
    make_private(&root);
    assert!(matches!(
        OwnerControl::open(&root, OWNER),
        Err(ControlError::Untrusted(_))
    ));

    // Symlinked owner directory.
    let real = root.join("real");
    make_private(&real.join("inbox"));
    make_private(&real.join("results"));
    let linked = root.join(OWNER);
    symlink(&real, &linked).expect("symlink");
    assert!(matches!(
        OwnerControl::open(&root, OWNER),
        Err(ControlError::Untrusted(_))
    ));
    fs::remove_file(&linked).expect("unlink");

    // Group-accessible, wrong mode.
    make_private(&linked.join("inbox"));
    make_private(&linked.join("results"));
    fs::set_permissions(&linked, fs::Permissions::from_mode(0o770)).expect("loose mode");
    assert!(matches!(
        OwnerControl::open(&root, OWNER),
        Err(ControlError::Untrusted(_))
    ));

    // Owner-only but not exactly 0700: 0750 is refused too.
    fs::set_permissions(&linked, fs::Permissions::from_mode(0o750)).expect("wrong mode");
    assert!(matches!(
        OwnerControl::open(&root, OWNER),
        Err(ControlError::Untrusted(_))
    ));

    fs::set_permissions(&linked, fs::Permissions::from_mode(0o700)).expect("tighten");
    assert!(OwnerControl::open(&root, OWNER).is_ok(), "now trusted");

    // A group-readable inbox.
    fs::set_permissions(linked.join("inbox"), fs::Permissions::from_mode(0o750)).expect("loose");
    assert!(matches!(
        OwnerControl::open(&root, OWNER),
        Err(ControlError::Untrusted(_))
    ));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn trust_is_rechecked_after_the_client_opened() {
    let owner = TestOwner::new();
    let control = owner.open();
    // The directory drifts after open: publish and read must both refuse now.
    fs::set_permissions(owner.owner_dir(), fs::Permissions::from_mode(0o755)).expect("drift");

    let request = new_request(Operation::Close);
    assert!(matches!(
        control.publish(&request),
        Err(ControlError::Untrusted(_))
    ));
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::Untrusted(_))
    ));
    assert!(
        !owner
            .owner_dir()
            .join("inbox")
            .join(format!("{}.json", request.request_id))
            .exists(),
        "nothing was written after the drift"
    );
}

#[test]
fn identifier_traversal_is_refused_before_any_path_is_joined() {
    let owner = TestOwner::new();
    let control = owner.open();

    for bad in ["../escape", "..", "a/b", ""] {
        assert!(
            matches!(
                OwnerControl::open(&owner.root, bad),
                Err(ControlError::InvalidIdentifier(_))
            ),
            "owner id {bad:?} refused"
        );
        assert!(
            matches!(
                control.read_result(bad, Operation::Close),
                Err(ControlError::InvalidIdentifier(_))
            ),
            "request id {bad:?} refused"
        );
    }
    assert!(
        !owner.root.parent().expect("parent").join("escape").exists(),
        "nothing was written outside the root"
    );

    let mut request = new_request(Operation::Close);
    request.request_id = "../escape".into();
    assert!(matches!(
        control.publish(&request),
        Err(ControlError::InvalidIdentifier(_))
    ));
    assert!(
        !owner
            .owner_dir()
            .join("inbox")
            .join("../escape.json")
            .exists()
    );
}

#[test]
fn invalid_identities_are_refused_before_publication() {
    let owner = TestOwner::new();
    let control = owner.open();

    let mut bad_run = new_request(Operation::Close);
    bad_run.run_id = "not-a-uuid".into();
    assert!(matches!(
        control.publish(&bad_run),
        Err(ControlError::InvalidIdentifier(_))
    ));

    let mut bad_session = new_request(Operation::Close);
    bad_session.pi_session_id = Some("../../escape".into());
    assert!(matches!(
        control.publish(&bad_session),
        Err(ControlError::InvalidIdentifier(_))
    ));

    assert_eq!(
        fs::read_dir(owner.owner_dir().join("inbox"))
            .expect("inbox")
            .count(),
        0,
        "neither request was written"
    );
}

#[test]
fn oversized_documents_are_refused() {
    let owner = TestOwner::new();
    let control = owner.open();

    let mut request = new_request(Operation::Close);
    request.pi_session_path = Some("x".repeat(9_000));
    assert_eq!(
        control.publish(&request).unwrap_err(),
        ControlError::Oversized
    );

    owner.write_result(REQUEST, &"x".repeat(9_000));
    assert_eq!(
        control.read_result(REQUEST, Operation::Close).unwrap_err(),
        ControlError::Oversized
    );
}

#[test]
fn malformed_mismatched_and_nonregular_results_are_invalid_evidence() {
    let owner = TestOwner::new();
    let control = owner.open();

    // Malformed JSON.
    owner.write_result(REQUEST, "{ not json");
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::InvalidEvidence(_))
    ));

    // A result that answers a different request.
    owner.write_result(
        REQUEST,
        &result_json(
            "b0000000-0000-4000-8000-000000000002",
            "close",
            "closed",
            "",
        ),
    );
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::InvalidEvidence(_))
    ));

    // An outcome this build does not know.
    owner.write_result(REQUEST, &result_json(REQUEST, "close", "exploded", ""));
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::InvalidEvidence(_))
    ));

    // A result for a different operation than the caller submitted — including
    // a refused or unknown result, whose outcome alone would allow either.
    for outcome in ["refused", "unknown"] {
        owner.write_result(REQUEST, &result_json(REQUEST, "restart", outcome, ""));
        assert!(
            matches!(
                control.read_result(REQUEST, Operation::Close),
                Err(ControlError::InvalidEvidence(_))
            ),
            "a {outcome} result for the wrong operation"
        );
        assert!(
            control.read_result(REQUEST, Operation::Restart).is_ok(),
            "a {outcome} result for its own operation is valid"
        );
    }

    // A closed result whose operation is a restart.
    owner.write_result(REQUEST, &result_json(REQUEST, "restart", "closed", ""));
    assert!(matches!(
        control.read_result(REQUEST, Operation::Restart),
        Err(ControlError::InvalidEvidence(_))
    ));

    // A symlink where a result should be.
    let target = owner.owner_dir().join("results").join("elsewhere.json");
    fs::write(&target, result_json(REQUEST, "close", "closed", "")).expect("target");
    let link = owner
        .owner_dir()
        .join("results")
        .join(format!("{REQUEST}.json"));
    fs::remove_file(&link).expect("remove");
    symlink(&target, &link).expect("symlink result");
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::InvalidEvidence(_))
    ));
    fs::remove_file(&link).expect("unlink");

    // A directory in place of the result file: not a regular file.
    fs::create_dir(&link).expect("directory result");
    assert!(matches!(
        control.read_result(REQUEST, Operation::Close),
        Err(ControlError::InvalidEvidence(_))
    ));
}

#[test]
fn unknown_request_fields_are_tolerated_on_decode() {
    let mut request = serde_json::to_value(new_request(Operation::Close)).expect("encode");
    request["reason"] = Value::String("operator dashboard action".into());
    request["futureField"] = Value::Null;
    let decoded: ControlRequest =
        serde_json::from_value(request).expect("decode with unknown fields");
    decoded.validate().expect("still valid");
}

#[test]
fn a_missing_result_is_not_invalid_evidence() {
    let owner = TestOwner::new();
    let control = owner.open();
    assert_eq!(
        control
            .read_result(REQUEST, Operation::Close)
            .expect("read"),
        None
    );
}

#[test]
fn a_claim_outranks_expiry_and_a_later_result_refines_it() {
    let owner = TestOwner::new();
    let control = owner.open();
    let expiry = timestamp_millis("2026-01-01T00:00:30.000Z").unwrap();
    let after = timestamp_millis("2026-01-01T00:01:00.000Z").unwrap();

    owner.write_claim(REQUEST);
    assert_eq!(
        control
            .derive_state(REQUEST, Operation::Close, expiry, after)
            .expect("state"),
        RequestState::Started
    );

    owner.write_result(
        REQUEST,
        &result_json(REQUEST, "close", "closed", "\"process_ended\""),
    );
    let state = control
        .derive_state(REQUEST, Operation::Close, expiry, after)
        .expect("state");
    assert!(matches!(state, RequestState::Result(ref r) if r.outcome == Outcome::Closed));
    assert!(state.is_terminal());
}

#[test]
fn unclaimed_expiry_is_not_executed_and_before_it_is_pending() {
    let owner = TestOwner::new();
    let control = owner.open();
    let expiry = timestamp_millis("2026-01-01T00:00:30.000Z").unwrap();

    assert_eq!(
        control
            .derive_state(REQUEST, Operation::Close, expiry, expiry - 20_000)
            .expect("state"),
        RequestState::Pending
    );
    assert_eq!(
        control
            .derive_state(REQUEST, Operation::Close, expiry, expiry + 1_000)
            .expect("state"),
        RequestState::NotExecuted
    );
}

#[test]
fn a_refusal_preserves_the_category_and_message() {
    let owner = TestOwner::new();
    let control = owner.open();
    owner.write_result(
        REQUEST,
        r#"{"version":1,"requestId":"a0000000-0000-4000-8000-000000000001","operation":"close","outcome":"refused","category":"agent_busy","message":"implementer-1 is working; restart applies to an idle retained worker.","effects":[],"completedAt":"2026-01-01T00:00:00.500Z"}"#,
    );
    let RequestState::Result(result) = control
        .derive_state(REQUEST, Operation::Close, i64::MAX, 0)
        .expect("state")
    else {
        panic!("a refused result");
    };
    assert_eq!(result.outcome, Outcome::Refused);
    assert_eq!(result.category.as_deref(), Some("agent_busy"));
    assert!(result.message.contains("idle retained worker"));
    assert!(result.effects.is_empty());
    assert!(!result.pane_closed());
}

/// The canonical fixture has no lost-generation close, so its effects contract
/// is pinned by this separate case: `closed` may mean only `process_ended`.
#[test]
fn a_lost_close_reports_only_process_ended() {
    let owner = TestOwner::new();
    let control = owner.open();

    owner.write_result(
        REQUEST,
        &result_json(REQUEST, "close", "closed", "\"process_ended\""),
    );
    let lost = control
        .read_result(REQUEST, Operation::Close)
        .expect("read")
        .expect("present");
    assert_eq!(lost.outcome, Outcome::Closed);
    assert!(lost.has_effect("process_ended"));
    assert!(
        !lost.pane_closed(),
        "a lost generation does not close its pane"
    );

    owner.write_result(
        REQUEST,
        &result_json(
            REQUEST,
            "close",
            "closed",
            "\"process_ended\",\"pane_closed\"",
        ),
    );
    let live = control
        .read_result(REQUEST, Operation::Close)
        .expect("read")
        .expect("present");
    assert!(live.pane_closed());
}

#[test]
fn the_canonical_fixture_is_the_upstream_document_plus_provenance() {
    let text = include_str!("fixtures/herdsman-control.json");
    let fixture: Value = serde_json::from_str(text).expect("fixture");

    assert_eq!(
        fixture["source"]["sha256"], UPSTREAM_SHA256,
        "recorded upstream hash"
    );
    assert_eq!(
        fixture["requests"].as_array().unwrap().len(),
        15,
        "all canonical requests retained"
    );
    assert_eq!(
        fixture["results"].as_array().unwrap().len(),
        10,
        "all canonical results retained"
    );
    assert_eq!(fixture["derivation"].as_array().unwrap().len(), 4);

    // Removing the one added key leaves exactly the upstream top-level keys.
    let mut source_free = fixture.clone();
    source_free.as_object_mut().unwrap().remove("source");
    let mut keys: Vec<&str> = source_free
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["derivation", "directory", "kind", "requests", "results"]
    );
}

#[test]
fn every_canonical_request_is_judged_at_the_local_boundary() {
    let text = include_str!("fixtures/herdsman-control.json");
    let fixture: Value = serde_json::from_str(text).expect("fixture");

    let mut seen = 0;
    for entry in fixture["requests"].as_array().expect("requests") {
        seen += 1;
        let category = entry["expect"]["category"].as_str();
        let decoded = serde_json::from_value::<ControlRequest>(entry["request"].clone());
        // `invalid_request` is the one category the requester can decide locally
        // (version, confirmation). Every other category is an owner-only state
        // check, so the document must decode and validate here.
        if category == Some("invalid_request") {
            match decoded {
                Err(_) => {}
                Ok(request) => assert!(
                    request.validate().is_err(),
                    "{} should be refused locally",
                    entry["case"]
                ),
            }
        } else {
            let request =
                decoded.unwrap_or_else(|error| panic!("{} should decode: {error}", entry["case"]));
            request
                .validate()
                .unwrap_or_else(|error| panic!("{} should validate: {error}", entry["case"]));
        }
    }
    assert_eq!(seen, 15);
}

#[test]
fn every_canonical_result_is_correlated_with_its_request_operation() {
    let text = include_str!("fixtures/herdsman-control.json");
    let fixture: Value = serde_json::from_str(text).expect("fixture");

    let operations: std::collections::HashMap<String, Operation> = fixture["requests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|entry| {
            let id = entry["request"]["requestId"]
                .as_str()
                .expect("id")
                .to_string();
            (
                id,
                parse_operation(entry["request"]["operation"].as_str().expect("operation")),
            )
        })
        .collect();

    let mut seen = 0;
    for entry in fixture["results"].as_array().expect("results") {
        seen += 1;
        let request_id = entry["requestId"].as_str().expect("requestId");
        let operation = *operations
            .get(request_id)
            .expect("a result names a known request");
        let owner = TestOwner::new();
        let control = owner.open();
        owner.write_result(
            request_id,
            &serde_json::to_string(&entry["result"]).expect("encode"),
        );

        let result: ControlResult = control
            .read_result(request_id, operation)
            .unwrap_or_else(|error| panic!("{}: {error}", entry["case"]))
            .expect("a present result");
        assert_eq!(result.request_id, request_id, "{}", entry["case"]);
        // The same document with the other expected operation is invalid, even
        // when its outcome (refused/unknown) would otherwise allow either.
        assert!(
            matches!(
                control.read_result(request_id, flip(operation)),
                Err(ControlError::InvalidEvidence(_))
            ),
            "{} with the wrong operation",
            entry["case"]
        );
    }
    assert_eq!(seen, 10);
}

#[test]
fn every_canonical_derivation_case_matches_the_files() {
    let text = include_str!("fixtures/herdsman-control.json");
    let fixture: Value = serde_json::from_str(text).expect("fixture");

    let operations: std::collections::HashMap<String, Operation> = fixture["requests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|entry| {
            let id = entry["request"]["requestId"]
                .as_str()
                .expect("id")
                .to_string();
            (
                id,
                parse_operation(entry["request"]["operation"].as_str().expect("operation")),
            )
        })
        .collect();
    let results: std::collections::HashMap<String, Value> = fixture["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|entry| {
            (
                entry["requestId"].as_str().expect("id").to_string(),
                entry["result"].clone(),
            )
        })
        .collect();

    for entry in fixture["derivation"].as_array().expect("derivation") {
        let owner = TestOwner::new();
        let control = owner.open();
        let request_id = entry["requestId"].as_str().expect("requestId");
        if entry["claimFile"].as_bool().expect("claimFile") {
            owner.write_claim(request_id);
        }
        if entry["resultFile"].as_bool().expect("resultFile") {
            let result = results
                .get(request_id)
                .expect("a result for the derivation entry");
            owner.write_result(request_id, &serde_json::to_string(result).expect("encode"));
        }
        let operation = operations
            .get(request_id)
            .copied()
            .unwrap_or(Operation::Close);
        let expires = timestamp_millis(entry["expiresAt"].as_str().expect("expiresAt")).unwrap();
        let observed = timestamp_millis(entry["observedAt"].as_str().expect("observedAt")).unwrap();
        let state = control
            .derive_state(request_id, operation, expires, observed)
            .expect("a derived state");
        let named = match state {
            RequestState::Result(_) => "result",
            RequestState::Started => "started",
            RequestState::NotExecuted => "not_executed",
            RequestState::Pending => "pending",
        };
        assert_eq!(
            named,
            entry["state"].as_str().expect("state"),
            "{}",
            entry["case"]
        );
        assert_eq!(
            state.is_terminal(),
            entry["terminal"].as_bool().expect("terminal"),
            "{}",
            entry["case"]
        );
    }
}
