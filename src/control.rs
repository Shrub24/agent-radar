//! The owner-control file boundary: ask a managed worker's owner to close or
//! restart it, and read the answer.
//!
//! This is the requester half of `herdsman-control/v1`. The filesystem is the
//! whole transport: a request is published atomically into a trusted owner
//! directory's `inbox`, and an outcome is read from `results`, with the claim
//! file and absolute expiry giving the remaining terminal states. There is no
//! socket, no daemon and no retry loop here.
//!
//! Trust is the precondition for writing anything. The owner creates its own
//! `inbox` and `results`, so this client never creates a directory and refuses
//! a directory that is missing, a symlink, foreign-owned or not mode `0700`.
//! Trust is re-checked on every publish and read, so an [`OwnerControl`] opened
//! earlier cannot keep acting after the directory or its permissions drift.
//! Radar never operates a worker directly; a request targets an exact published
//! identity (label and run UUID) and the owner decides whether to act.
//!
//! Nothing here waits on a request or interprets a local timeout as a verdict.
//! Publication is a bounded write into a temporary sibling followed by an
//! atomic link into place; a read is one bounded parse. The caller owns all
//! timing, and the terminal states derive from files alone.

use std::fs::{self, File, OpenOptions, symlink_metadata};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::model::SessionUuid;

/// The contract this client speaks.
const VERSION: u32 = 1;
/// The only surface that writes control requests today.
const REQUESTER: &str = "agent-radar";
/// Requests and results are bounded to 8 KiB by the contract.
pub const MAX_FILE_BYTES: usize = 8 * 1024;
/// Absolute expiry measured from publication.
pub const REQUEST_TTL: Duration = Duration::from_secs(30);

/// What the owner is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Close,
    Restart,
}

/// The identity the operator saw, echoed so the owner can refuse a mismatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub operation: Operation,
    pub label: String,
    pub run_id: String,
}

/// A `herdsman-control/v1` request document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRequest {
    pub version: u32,
    pub request_id: String,
    pub operation: Operation,
    /// Exact runtime label of the target.
    pub agent: String,
    pub run_id: String,
    /// Optional cross-check: the herdr pane of that run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// Optional cross-check: the Pi session of that run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_session_id: Option<String>,
    /// Optional cross-check: the persisted session file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_session_path: Option<String>,
    pub confirmation: Confirmation,
    pub requested_at: String,
    pub expires_at: String,
    pub requester: String,
}

/// The inputs a caller supplies for a new request. Identity, request id and
/// timestamps are filled in by [`ControlRequest::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRequest {
    pub operation: Operation,
    /// Exact runtime label of the target.
    pub label: String,
    pub run_id: String,
    pub pane_id: Option<String>,
    pub pi_session_id: Option<String>,
    pub pi_session_path: Option<String>,
}

impl ControlRequest {
    /// Builds a request with a fresh UUID and the contract's absolute expiry
    /// from `now`.
    pub fn new(spec: NewRequest, now: SystemTime) -> Result<Self, ControlError> {
        Self::with_ttl(spec, now, REQUEST_TTL)
    }

    /// Builds a request with an explicit absolute lifetime instead of the
    /// contract's [`REQUEST_TTL`].
    ///
    /// Production always uses [`Self::new`]; the shorter lifetime exists so a
    /// test can reach the expiry states without waiting out the contract's 30
    /// seconds.
    pub fn with_ttl(
        spec: NewRequest,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<Self, ControlError> {
        let requested = unix_ms(now);
        let operation = spec.operation;
        Ok(Self {
            version: VERSION,
            request_id: random_uuid()?,
            operation,
            agent: spec.label.clone(),
            run_id: spec.run_id.clone(),
            pane_id: spec.pane_id,
            pi_session_id: spec.pi_session_id,
            pi_session_path: spec.pi_session_path,
            confirmation: Confirmation {
                operation,
                label: spec.label,
                run_id: spec.run_id,
            },
            requested_at: format_timestamp(requested),
            expires_at: format_timestamp(requested + ttl.as_millis() as i64),
            requester: REQUESTER.to_string(),
        })
    }

    /// The absolute expiry as Unix milliseconds.
    pub fn expires_at_ms(&self) -> Result<i64, ControlError> {
        parse_timestamp(&self.expires_at)
            .ok_or_else(|| ControlError::InvalidEvidence("expiresAt is not ISO 8601".into()))
    }

    /// Whether the document is one this contract can act on. Version, request
    /// id, identity and the confirmation echo are all checked here; the owner
    /// repeats its own preflight regardless.
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.version != VERSION {
            return Err(ControlError::InvalidEvidence(format!(
                "version {} is not supported",
                self.version
            )));
        }
        validate_uuid("requestId", &self.request_id)?;
        validate_uuid("runId", &self.run_id)?;
        if let Some(session) = &self.pi_session_id {
            validate_uuid("piSessionId", session)?;
        }
        if self.agent.is_empty() {
            return Err(ControlError::InvalidEvidence(
                "a request needs a label".into(),
            ));
        }
        if self.confirmation.operation != self.operation
            || self.confirmation.label != self.agent
            || self.confirmation.run_id != self.run_id
        {
            return Err(ControlError::InvalidEvidence(
                "the confirmation does not match the target".into(),
            ));
        }
        parse_timestamp(&self.requested_at)
            .ok_or_else(|| ControlError::InvalidEvidence("requestedAt is not ISO 8601".into()))?;
        self.expires_at_ms()?;
        Ok(())
    }
}

/// The owner's answer. `effects` names what was actually applied, in order.
///
/// An outcome this build does not know is a deserialization failure, which the
/// reader reports as invalid evidence rather than a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Closed,
    Restarted,
    Refused,
    Unknown,
}

/// A `herdsman-control/v1` result document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlResult {
    pub version: u32,
    pub request_id: String,
    pub operation: Operation,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub message: String,
    #[serde(default)]
    pub effects: Vec<String>,
    pub completed_at: String,
}

impl ControlResult {
    /// Whether the result names an effect, e.g. `pane_closed`.
    pub fn has_effect(&self, effect: &str) -> bool {
        self.effects.iter().any(|named| named == effect)
    }

    /// Whether the pane was actually closed. Derived from effects alone: a
    /// `closed` outcome over a lost generation reports only `process_ended`,
    /// and its surviving shell pane must not be shown as closed.
    pub fn pane_closed(&self) -> bool {
        self.has_effect("pane_closed")
    }

    /// Whether the document matches the request it claims to answer.
    ///
    /// The operation must be the one the caller submitted in every outcome,
    /// including `refused` and `unknown`, which are allowed for either
    /// operation on their own but never the wrong one.
    fn validate(&self, request_id: &str, operation: Operation) -> Result<(), ControlError> {
        if self.version != VERSION {
            return Err(ControlError::InvalidEvidence(format!(
                "version {} is not supported",
                self.version
            )));
        }
        if self.request_id != request_id {
            return Err(ControlError::InvalidEvidence(
                "the result answers a different request".into(),
            ));
        }
        if self.operation != operation {
            return Err(ControlError::InvalidEvidence(
                "the result names a different operation".into(),
            ));
        }
        match self.outcome {
            Outcome::Closed if operation != Operation::Close => {
                return Err(ControlError::InvalidEvidence(
                    "a closed result names a restart".into(),
                ));
            }
            Outcome::Restarted if operation != Operation::Restart => {
                return Err(ControlError::InvalidEvidence(
                    "a restarted result names a close".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

/// What the files alone say about a request id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestState {
    /// A result exists: terminal, and the outcome is settled.
    Result(ControlResult),
    /// A claim exists without a result: execution started, outcome unknown.
    Started,
    /// Neither result nor claim, and the absolute expiry has passed.
    NotExecuted,
    /// Neither result nor claim, and the expiry has not passed.
    Pending,
}

impl RequestState {
    /// Whether the state will not change on its own.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// Why a control operation could not be carried out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlError {
    /// A directory failed the trust check, or was missing.
    Untrusted(String),
    /// An identifier was not a canonical UUID, so it cannot name a path.
    InvalidIdentifier(String),
    /// A document exceeded the contract's 8 KiB bound.
    Oversized,
    /// A request id already exists in the inbox.
    Collision(String),
    /// A file that exists could not be read or parsed as the contract requires.
    InvalidEvidence(String),
    /// Any other filesystem failure, kept verbatim.
    Io(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Untrusted(message)
            | Self::InvalidIdentifier(message)
            | Self::Collision(message)
            | Self::InvalidEvidence(message)
            | Self::Io(message) => f.write_str(message),
            Self::Oversized => write!(f, "a control document exceeds {MAX_FILE_BYTES} bytes"),
        }
    }
}

impl std::error::Error for ControlError {}

/// A validated owner control directory: its `inbox` and `results` subdirectories.
///
/// Opening never creates anything. The owner directory, `inbox` and `results`
/// must each already exist and pass the trust check.
#[derive(Clone, Debug)]
pub struct OwnerControl {
    owner_dir: PathBuf,
    inbox: PathBuf,
    results: PathBuf,
}

impl OwnerControl {
    /// Opens `base/<owner_session_id>/`, refusing anything untrusted.
    ///
    /// `base` is the control root (`~/.pi/agent/pi-herdsman/control` in
    /// production); tests pass a temporary root. The owner session id must be a
    /// canonical UUID, so it can never escape the root.
    pub fn open(base: &Path, owner_session_id: &str) -> Result<Self, ControlError> {
        validate_uuid("owner session", owner_session_id)?;
        let owner_dir = base.join(owner_session_id);
        trusted_directory(&owner_dir)?;
        let inbox = owner_dir.join("inbox");
        let results = owner_dir.join("results");
        trusted_directory(&inbox)?;
        trusted_directory(&results)?;
        Ok(Self {
            owner_dir,
            inbox,
            results,
        })
    }

    /// The owner directory this client is bound to.
    pub fn owner_dir(&self) -> &Path {
        &self.owner_dir
    }

    /// Publishes a request atomically, without replacing an existing one.
    ///
    /// Trust is re-checked here, so a directory that drifted since [`open`]
    /// refuses rather than accepting a write. The document is written to a
    /// temporary sibling and linked into place; the link fails if the id
    /// already exists, so a collision leaves every existing file untouched.
    ///
    /// [`open`]: Self::open
    pub fn publish(&self, request: &ControlRequest) -> Result<(), ControlError> {
        self.verify()?;
        request.validate()?;
        let bytes = serde_json::to_vec(request)
            .map_err(|error| ControlError::Io(format!("cannot encode the request: {error}")))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ControlError::Oversized);
        }
        let final_path = self.inbox.join(format!("{}.json", request.request_id));
        let temporary = self.inbox.join(format!(
            "{}.json.tmp-{}",
            request.request_id,
            random_uuid()?
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| ControlError::Io(format!("cannot open a request file: {error}")))?;
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(ControlError::Io(format!(
                "cannot write a request file: {error}"
            )));
        }
        let linked = fs::hard_link(&temporary, &final_path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
                ControlError::Collision(format!("request {} already exists", request.request_id)),
            ),
            Err(error) => Err(ControlError::Io(format!(
                "cannot publish the request: {error}"
            ))),
        }
    }

    /// Reads the result for a request id, or `None` when there is none.
    ///
    /// The caller passes the operation it submitted; a result that answers the
    /// same id with a different operation is invalid evidence. A file that
    /// exists but is a symlink, not a regular file, oversized, malformed or
    /// answers a different request is an error, never a missing result.
    pub fn read_result(
        &self,
        request_id: &str,
        operation: Operation,
    ) -> Result<Option<ControlResult>, ControlError> {
        self.verify()?;
        validate_uuid("request", request_id)?;
        let path = self.results.join(format!("{request_id}.json"));
        match symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ControlError::Io(format!("cannot read the result: {error}")));
            }
        }
        // O_NOFOLLOW refuses a symlink even if one replaced the file after the
        // check above; O_NONBLOCK keeps a FIFO or device from blocking the open.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|error| {
                ControlError::InvalidEvidence(format!("the result is not readable: {error}"))
            })?;
        let metadata = file
            .metadata()
            .map_err(|error| ControlError::Io(format!("cannot read the result: {error}")))?;
        if !metadata.is_file() {
            return Err(ControlError::InvalidEvidence(
                "the result is not a regular file".into(),
            ));
        }
        // Read at most the bound plus one byte, so the allocation is bounded
        // even when the length is lying or changing.
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ControlError::Io(format!("cannot read the result: {error}")))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ControlError::Oversized);
        }
        let result: ControlResult = serde_json::from_slice(&bytes)
            .map_err(|error| ControlError::InvalidEvidence(format!("malformed result: {error}")))?;
        result.validate(request_id, operation)?;
        Ok(Some(result))
    }

    /// Derives the request's state from the files, in the contract's precedence:
    /// result, then claim, then expiry, then pending.
    ///
    /// `operation` is the operation the caller submitted, checked against any
    /// result. `expires_at_ms` and `now_ms` are Unix milliseconds, so the caller
    /// owns the clock and a test can pin it.
    pub fn derive_state(
        &self,
        request_id: &str,
        operation: Operation,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<RequestState, ControlError> {
        if let Some(result) = self.read_result(request_id, operation)? {
            return Ok(RequestState::Result(result));
        }
        if self.claimed(request_id)? {
            // A claim proves execution started, so it outranks expiry.
            return Ok(RequestState::Started);
        }
        if now_ms >= expires_at_ms {
            return Ok(RequestState::NotExecuted);
        }
        Ok(RequestState::Pending)
    }

    /// Re-checks the owner directory, `inbox` and `results` trust.
    fn verify(&self) -> Result<(), ControlError> {
        trusted_directory(&self.owner_dir)?;
        trusted_directory(&self.inbox)?;
        trusted_directory(&self.results)
    }

    /// Whether the owner has claimed the request.
    fn claimed(&self, request_id: &str) -> Result<bool, ControlError> {
        self.verify()?;
        validate_uuid("request", request_id)?;
        let path = self.inbox.join(format!("{request_id}.claim"));
        match symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(ControlError::Io(format!("cannot read the claim: {error}"))),
        }
    }
}

/// Refuses a directory the owner did not create for this user alone.
fn trusted_directory(path: &Path) -> Result<(), ControlError> {
    let metadata = symlink_metadata(path)
        .map_err(|_| ControlError::Untrusted(format!("{} is not available", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ControlError::Untrusted(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    if metadata.uid() != current_uid() {
        return Err(ControlError::Untrusted(format!(
            "{} is not owned by the current user",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(ControlError::Untrusted(format!(
            "{} is not mode 0700",
            path.display()
        )));
    }
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// Validates a canonical UUID, which is what makes an identifier safe to join.
fn validate_uuid(what: &str, value: &str) -> Result<(), ControlError> {
    if SessionUuid::parse(value).is_none() {
        return Err(ControlError::InvalidIdentifier(format!(
            "the {what} is not a UUID"
        )));
    }
    Ok(())
}

/// A version-4 UUID from the kernel's random source.
fn random_uuid() -> Result<String, ControlError> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut source| std::io::Read::read_exact(&mut source, &mut bytes))
        .map_err(|error| ControlError::Io(format!("cannot read randomness: {error}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn unix_ms(now: SystemTime) -> i64 {
    now.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Parses the contract's ISO 8601 timestamp into Unix milliseconds.
///
/// Exposed so a caller can compare a request's `expires_at` with its own clock.
pub fn timestamp_millis(value: &str) -> Option<i64> {
    parse_timestamp(value)
}

/// Formats Unix milliseconds as the contract's UTC ISO 8601 with milliseconds.
fn format_timestamp(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let sub = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub:03}Z")
}

/// Parses the exact timestamp [`format_timestamp`] writes.
///
/// Returns `None` for anything outside the format, any out-of-range field, or
/// a date that would have to be normalized (for example February 30), so an
/// invalid value fails rather than rolling over or overflowing.
fn parse_timestamp(value: &str) -> Option<i64> {
    let (date, rest) = value.split_once('T')?;
    let rest = rest.strip_suffix('Z')?;
    let (time, fraction) = rest.split_once('.')?;
    if fraction.len() != 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let millis: i64 = fraction.parse().ok()?;
    let days = days_from_civil(year, month, day);
    // Reject a day the calendar would have to roll forward (e.g. 2026-02-30).
    if civil_from_days(days) != (year, month, day) {
        return None;
    }
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?;
    seconds.checked_mul(1000)?.checked_add(millis)
}

/// Days since the Unix epoch for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The Gregorian date for days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip_known_instants() {
        for millis in [
            0,
            1_000,
            1_767_225_600_000, // 2026-01-01T00:00:00.000Z
            1_767_225_630_500, // 2026-01-01T00:00:30.500Z
            1_740_787_199_999, // 2025-03-01T23:59:59.999Z
        ] {
            let formatted = format_timestamp(millis);
            assert_eq!(parse_timestamp(&formatted), Some(millis), "{formatted}");
        }
    }

    #[test]
    fn invalid_timestamps_are_rejected_not_normalized() {
        for value in [
            "2026-02-30T00:00:00.000Z", // a date the calendar would roll forward
            "2026-01-01T24:00:00.000Z", // hour out of range
            "2026-01-01T00:60:00.000Z", // minute out of range
            "2026-01-01T00:00:60.000Z", // second out of range
            "2026-13-01T00:00:00.000Z", // month out of range
            "2026-01-01T00:00:00.00Z",  // wrong fraction width
            "2026-01-01 00:00:00.000Z", // wrong separator
            "999999999999-01-01T00:00:00.000Z", // overflow
        ] {
            assert_eq!(parse_timestamp(value), None, "{value}");
        }
    }

    #[test]
    fn a_fresh_request_is_well_formed() {
        let request = ControlRequest::new(
            NewRequest {
                operation: Operation::Restart,
                label: "implementer-1".into(),
                run_id: "8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71".into(),
                pane_id: Some("w1:p3".into()),
                pi_session_id: None,
                pi_session_path: None,
            },
            UNIX_EPOCH + Duration::from_secs(1_767_225_600),
        )
        .expect("a fresh request");
        request.validate().expect("valid");
        assert_eq!(request.expires_at_ms().unwrap(), 1_767_225_630_000);
    }
}
