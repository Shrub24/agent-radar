//! Reconciliation: re-establishing an external observation baseline.
//!
//! Reconciliation is not the periodic observation pass. Steady-state observation
//! — process evidence, pane evidence and container observation — runs
//! continuously in the daemon's observation pass, because it is what keeps
//! lifecycle coverage alive with no client connected. Reconciliation is for the
//! moments when continuity or confidence was lost and there is durable state
//! whose external effect nobody has established: a daemon start, an operation a
//! crash left unknown, a serving identity that changed, a detected inconsistency,
//! or an operator asking.
//!
//! It reads current evidence and the durable record, commits only the transitions
//! that evidence justifies, and reports everything it could not establish with
//! its reason. It never dispatches an effect and never retries one: a record it
//! resolves is settled with an outcome, and a record it cannot resolve stays
//! unresolved and keeps blocking the target it named.

use serde::{Deserialize, Serialize};

use super::lifecycle::{self, State};
use super::registry::{ProcessVerifier, Registry};
use super::store::{RecordState, RequestOutcome, RequestRecord, Store};

/// Most unresolved records one pass reports. A pass is bounded so a long history
/// cannot make one response unbounded; `examined` still counts them all.
pub const MAX_REPORTED: usize = 32;

/// What asked for a reconciliation.
///
/// There are two triggers because there are two callers. A serving identity that
/// changed, an operation a crash left unknown and a detected inconsistency are
/// the conditions that leave durable state without an established external
/// effect; they do not schedule a pass of their own, they are what a pass exists
/// to resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The daemon started, so no external baseline carries over from the last one.
    DaemonStart,
    /// An operator asked for one.
    OperatorRequest,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DaemonStart => "daemon_start",
            Self::OperatorRequest => "operator_request",
        }
    }
}

/// One unresolved operation, with the reason it could not be established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unresolved {
    pub request_id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub reason: String,
}

/// What one reconciliation pass did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub trigger: Trigger,
    /// Every unresolved record the pass examined.
    pub examined: usize,
    /// Records whose external effect the pass established and settled.
    pub resolved: usize,
    /// The ones it could not establish, bounded to [`MAX_REPORTED`].
    pub unresolved: Vec<Unresolved>,
    /// How many unresolved records the report above omits.
    pub omitted: usize,
}

/// Re-establishes what can be established about every unresolved operation.
///
/// A record is settled only from independent evidence: a lifecycle operation is
/// established by the captured process's own birth identity, and a spawn by the
/// outcomes its edge already recorded. Anything else stays unresolved and says
/// why. Nothing here dispatches an effect, so a pass is safe to run at any time.
pub fn unresolved(
    store: &Store,
    registry: &Registry,
    verifier: &dyn ProcessVerifier,
    trigger: Trigger,
    now_ms: i64,
) -> Result<Outcome, String> {
    let records = store.unfinished()?;
    let examined = records.len();
    let mut resolved = 0;
    let mut unresolved = Vec::new();
    let mut omitted = 0;
    for record in records {
        let (established, reason) = establish(registry, &record, verifier, now_ms)?;
        let Some(outcome) = established else {
            push(
                &mut unresolved,
                &mut omitted,
                Unresolved {
                    request_id: record.id.clone(),
                    method: record.method.clone(),
                    target: record.target.clone(),
                    reason,
                },
            );
            continue;
        };
        // A record that already carries an outcome is never rewritten: an unknown
        // outcome is a recorded answer, and this pass only adds evidence it did
        // not have.
        if record.state == RecordState::Completed {
            push(
                &mut unresolved,
                &mut omitted,
                Unresolved {
                    request_id: record.id.clone(),
                    method: record.method.clone(),
                    target: record.target.clone(),
                    reason: format!(
                        "the recorded outcome is unknown and is never rewritten: {reason}"
                    ),
                },
            );
            continue;
        }
        store.write(&record.complete(outcome, None, Some(&reason), Vec::new(), now_ms))?;
        resolved += 1;
    }
    Ok(Outcome {
        trigger,
        examined,
        resolved,
        unresolved,
        omitted,
    })
}

fn push(into: &mut Vec<Unresolved>, omitted: &mut usize, item: Unresolved) {
    if into.len() < MAX_REPORTED {
        into.push(item);
    } else {
        *omitted += 1;
    }
}

/// What current evidence establishes about one unresolved record, and a reason.
///
/// The first field is the outcome the evidence establishes, or `None` when
/// nothing does; the reason describes the evidence either way, so an unresolved
/// record is never reported without one.
type Established = (Option<RequestOutcome>, String);

fn establish(
    registry: &Registry,
    record: &RequestRecord,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Established, String> {
    match record.method.as_str() {
        "lifecycle.cancel" | "lifecycle.stop" | "lifecycle.restart" => {
            establish_lifecycle(registry, record, verifier, now_ms)
        }
        "spawn.create" => establish_spawn(registry, record),
        other => Ok((
            None,
            format!(
                "`{other}` has no independent evidence source; its effect is established only by its own reply"
            ),
        )),
    }
}

fn establish_lifecycle(
    registry: &Registry,
    record: &RequestRecord,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Established, String> {
    let Some(target) = record.target.as_deref() else {
        return Ok((None, "the record names no subject to verify".into()));
    };
    let Some(subject) = lifecycle::current_attachment(registry, target)? else {
        return Ok((
            None,
            format!("the subject `{target}` it named is not recorded"),
        ));
    };
    // A launch revoked before any process was released has nothing left to stop:
    // the revocation is the operation's whole effect.
    if subject.process.state == State::Revoked && subject.process.identity.is_none() {
        return Ok((
            Some(RequestOutcome::Completed),
            "the launch was revoked before a process was released".into(),
        ));
    }
    if subject.process.identity.is_none() {
        return Ok((
            None,
            "the launch never captured a process, so nothing was signalled".into(),
        ));
    }
    if subject.process.exited_at.is_some() {
        return Ok((
            Some(RequestOutcome::Completed),
            "the captured process is verifiably gone".into(),
        ));
    }
    // The one evidence read this pass makes: the same fenced birth-identity check
    // the observation pass uses, so a process that died while nobody watched is
    // established here and recorded through the same reducer.
    match lifecycle::observe_exit(registry, &subject.claim_id, verifier, now_ms) {
        Ok(true) => Ok((
            Some(RequestOutcome::Completed),
            "the captured process is verifiably gone".into(),
        )),
        Ok(false) => Ok((
            None,
            "the captured process is still running its verified birth identity".into(),
        )),
        Err(error) => Ok((None, format!("process evidence is unavailable: {error}"))),
    }
}

fn establish_spawn(registry: &Registry, record: &RequestRecord) -> Result<Established, String> {
    let Some(edge) = registry.spawn_edge(&record.id)? else {
        return Ok((
            None,
            "no spawn edge is recorded for this request, so no effect was established".into(),
        ));
    };
    if edge.created == RequestOutcome::Refused || edge.launched == RequestOutcome::Refused {
        return Ok((
            Some(RequestOutcome::Refused),
            "the backend refused the create or the launch, and the edge records it".into(),
        ));
    }
    if edge.created == RequestOutcome::Completed && edge.launched == RequestOutcome::Completed {
        return Ok((
            Some(RequestOutcome::Completed),
            "the create and the launch are both recorded on the edge".into(),
        ));
    }
    Ok((
        None,
        "the edge records no confirmed create or launch, so a possibly-dispatched effect is not replayed"
            .into(),
    ))
}
