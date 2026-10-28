//! One request, one answer over the daemon socket, and what each outcome establishes.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::answer::Answer;
use super::super::store::RequestOutcome;
use super::super::{MAX_LINE_BYTES, PROTOCOL_VERSION, random_uuid};
use crate::runtime::{
    CloseOutcome, CreateOutcome, CreatedIdentity, FocusOutcome, InputOutcome, OutputOutcome,
    ReportOutcome,
};

use super::*;

/// How long one exchange has left, and where it may be interrupted.
#[derive(Clone, Copy)]
pub(super) struct Budget {
    deadline: Instant,
    timeout: Duration,
}

impl Budget {
    pub(super) fn of(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            timeout,
        }
    }

    /// Waits one granularity, or reports that the request was cancelled or is out
    /// of time.
    ///
    /// The message continues "the control daemon at <socket>", because that is
    /// where every caller's failure is reported from.
    pub(super) fn wait(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) {
            return Err("abandoned the request: it was cancelled".to_string());
        }
        if Instant::now() >= self.deadline {
            return Err(format!("did not complete within {:?}", self.timeout));
        }
        thread::sleep(WAIT_GRANULARITY);
        Ok(())
    }
}

/// One request that did not succeed, and what its failure establishes.
pub(super) struct Failure {
    pub(super) message: String,
    pub(super) certainty: Certainty,
}

/// What a failed exchange proves about the operation.
#[derive(Clone, Copy)]
pub(super) enum Certainty {
    /// Nothing was dispatched: the request never reached the daemon intact.
    Refused,
    /// The request may have been dispatched, so its effect is unestablished.
    Unknown,
}

impl Failure {
    pub(super) fn refused(message: String) -> Self {
        Self {
            message,
            certainty: Certainty::Refused,
        }
    }

    pub(super) fn unknown(message: String) -> Self {
        Self {
            message,
            certainty: Certainty::Unknown,
        }
    }

    /// A write that was refused outright, before one byte was accepted,
    /// dispatched nothing; a partial write may have.
    pub(super) fn after_write(written: usize, message: String) -> Self {
        match written {
            0 => Self::refused(message),
            _ => Self::unknown(message),
        }
    }

    /// The same failure as an answer that establishes nothing.
    pub(super) fn settled(self) -> Settled {
        match self.certainty {
            Certainty::Refused => Settled::Refused(self.message),
            Certainty::Unknown => Settled::Unknown(self.message),
        }
    }

    pub(super) fn into_output(self) -> OutputOutcome {
        match self.certainty {
            Certainty::Refused => OutputOutcome::Refused(self.message),
            Certainty::Unknown => OutputOutcome::Unknown(self.message),
        }
    }
}

/// What one effectful operation's record establishes.
pub(super) enum Settled {
    /// The operation ran. A creation's answer names what it created.
    Completed(Option<CreatedIdentity>),
    Refused(String),
    Unknown(String),
}

impl Settled {
    /// Reads the record the daemon answered with.
    ///
    /// A record that is not settled is unknown rather than refused: the daemon
    /// has accepted the request and may still execute it, so nothing here may be
    /// read as "it did not happen".
    pub(super) fn of(result: &Value) -> Result<Self, Failure> {
        let Ok(Answer {
            request: record, ..
        }) = Answer::deserialize(result)
        else {
            return Err(Failure::unknown(
                "the daemon's answer carried no request record".to_string(),
            ));
        };
        let message = |fallback: &str| record.message.clone().unwrap_or_else(|| fallback.into());
        match record.settled() {
            Some(RequestOutcome::Completed) => Ok(Self::Completed(record.created.clone())),
            Some(RequestOutcome::Refused) => {
                Ok(Self::Refused(message("the daemon refused the operation")))
            }
            Some(RequestOutcome::Unknown) => Ok(Self::Unknown(message(
                "the daemon cannot say what the operation did",
            ))),
            None => Err(Failure::unknown(format!(
                "{} is still unresolved: the daemon may yet execute it",
                record.id
            ))),
        }
    }

    pub(super) fn focus(self) -> FocusOutcome {
        match self {
            Self::Completed(_) => FocusOutcome::Completed,
            Self::Refused(message) => FocusOutcome::Refused(message),
            Self::Unknown(message) => FocusOutcome::Unknown(message),
        }
    }

    pub(super) fn close(self) -> CloseOutcome {
        match self {
            Self::Completed(_) => CloseOutcome::Completed,
            Self::Refused(message) => CloseOutcome::Refused(message),
            Self::Unknown(message) => CloseOutcome::Unknown(message),
        }
    }

    pub(super) fn create(self) -> CreateOutcome {
        match self {
            Self::Completed(created) => {
                CreateOutcome::Completed(created.map(location_of).unwrap_or_default())
            }
            Self::Refused(message) => CreateOutcome::Refused(message),
            Self::Unknown(message) => CreateOutcome::Unknown(message),
        }
    }

    pub(super) fn input(self) -> InputOutcome {
        match self {
            Self::Completed(_) => InputOutcome::Completed,
            Self::Refused(message) => InputOutcome::Refused(message),
            Self::Unknown(message) => InputOutcome::Unknown(message),
        }
    }

    pub(super) fn report(self) -> ReportOutcome {
        match self {
            Self::Completed(_) => ReportOutcome::Completed,
            Self::Refused(message) => ReportOutcome::Refused(message),
            Self::Unknown(message) => ReportOutcome::Unknown(message),
        }
    }
}

/// One request, one answer, over the daemon's socket, under a request id the
/// caller already has.
///
/// A confirmed container control carries the operator's own id, so a lost answer
/// is read back under the id that was sent rather than sent again.
///
/// Certainty follows from how far the exchange got: a request that never reached
/// the daemon — no connection, or a write refused before one byte — dispatched
/// nothing and may be refused; anything after that is unknown, because the
/// daemon may have run the operation and lost only the answer.
///
/// Each exchange dials its own connection, which is what the daemon serves: one
/// connection carries lines until its reader fails, and a client that wants a
/// fresh request gets a fresh dial.
pub(super) fn exchange_with_id(
    socket: &Path,
    id: &str,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    // The terminator is what makes it a line: the daemon answers one line per
    // request and reads until it has one.
    let mut line = json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| Failure::refused(format!("could not be reached: {error}")))?;
    let granularity = Some(WAIT_GRANULARITY);
    let _ = stream.set_read_timeout(granularity);
    let _ = stream.set_write_timeout(granularity);
    let budget = Budget::of(EXCHANGE_TIMEOUT);
    write_request(&mut stream, &line, budget, cancel)?;
    let answer = read_answer(&mut stream, budget, cancel).map_err(Failure::unknown)?;
    let answer: crate::control_plane::protocol::Response = serde_json::from_str(&answer)
        .map_err(|error| Failure::unknown(format!("sent an unreadable answer: {error}")))?;
    use crate::control_plane::protocol::{Code, Response};
    if answer.id() != id {
        return Err(Failure::unknown("answered a different request".to_string()));
    }
    match answer {
        Response::Result { result, .. } => Ok(result),
        Response::Error { error, .. } => {
            let code = error.code();
            let message = format!("refused `{method}` ({}): {}", error.code, error.message);
            Err(match code {
                Some(
                    Code::BadVersion
                    | Code::BadRequest
                    | Code::UnknownMethod
                    | Code::BadParams
                    | Code::Refused
                    | Code::Busy
                    | Code::BackendUnavailable
                    | Code::NotFound
                    | Code::NoClient,
                ) => Failure::refused(message),
                Some(Code::OutcomeUnknown | Code::Unknown | Code::Internal) | None => {
                    Failure::unknown(message)
                }
            })
        }
    }
}

/// [`exchange`], with every failure naming the daemon it happened to.
pub(super) fn exchange_at(
    socket: &Path,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    exchange_with_id_at(socket, &random_uuid(), method, params, cancel)
}

/// The id-carrying form of the exchange above, with every failure naming the
/// daemon it happened to.
pub(super) fn exchange_with_id_at(
    socket: &Path,
    id: &str,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    exchange_with_id(socket, id, method, params, cancel).map_err(|failure| Failure {
        message: format!(
            "the control daemon at {} {}",
            socket.display(),
            failure.message
        ),
        certainty: failure.certainty,
    })
}

pub(super) fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Writes the whole request line, waiting out a socket that is momentarily busy.
pub(super) fn write_request(
    stream: &mut UnixStream,
    bytes: &[u8],
    budget: Budget,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(Failure::after_write(
                    written,
                    "closed the connection while the request was being written".to_string(),
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => {
                budget
                    .wait(cancel)
                    .map_err(|message| Failure::after_write(written, message))?;
            }
            Err(error) => {
                return Err(Failure::after_write(
                    written,
                    format!("could not be asked: {error}"),
                ));
            }
        }
    }
    Ok(())
}

/// Reads one answer line, waiting out a socket that is momentarily empty.
pub(super) fn read_answer(
    stream: &mut UnixStream,
    budget: Budget,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut answer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Err("ended the connection without answering".to_string()),
            Ok(count) => {
                answer.extend_from_slice(&chunk[..count]);
                if let Some(end) = answer.iter().position(|byte| *byte == b'\n') {
                    return Ok(String::from_utf8_lossy(&answer[..end]).trim().to_string());
                }
                if answer.len() > MAX_LINE_BYTES {
                    return Err(format!("answered more than {MAX_LINE_BYTES} bytes"));
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => budget.wait(cancel)?,
            Err(error) => return Err(format!("sent no answer: {error}")),
        }
    }
}
