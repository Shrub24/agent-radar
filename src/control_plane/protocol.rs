//! The control-plane line protocol: one JSON request and one JSON response per
//! line.
//!
//! A client dials the daemon's socket and writes `version`, `id`, `method` and
//! `params` as one object per line; the daemon answers the same line with either
//! a `result` or an `error`, correlated by the id. The envelope is fixed here so
//! that a later method can add params without a second codec, and the version is
//! carried on every request so a client built against another protocol is told
//! so rather than misread.
//!
//! A refusal is a protocol fact, not a mux fact: [`Code`] is the stable
//! vocabulary a caller switches on, and the message is for a human. A line that
//! cannot be read as a request is refused with `bad_request`, and the connection
//! keeps serving — only the reader failing ends it.

use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::SessionUuid;

/// The only protocol version this daemon serves.
pub const PROTOCOL_VERSION: u32 = 1;

/// Line cap in bytes, excluding the terminator. A longer line is refused and
/// drained; it never grows the daemon's memory unboundedly.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Public observation and foreground evidence names.
pub const OBSERVE_METHOD: &str = "observe";
pub const PROCESS_INFO_METHOD: &str = "process_info";
pub const OUTPUT_METHOD: &str = "output";

/// A stable refusal reason. These words are the protocol: a caller switches on
/// them, so they do not change when a message's wording does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The request named a protocol version this daemon does not serve.
    BadVersion,
    /// The method is not one this version implements.
    UnknownMethod,
    /// The method is implemented and its params are not usable.
    BadParams,
    /// The line is not a request at all: not JSON, not an object, no id.
    BadRequest,
    /// The daemon refused the request before recording it, and says why.
    Refused,
    /// Capacity was full; the request was not accepted or recorded.
    Busy,
    /// A method this build needs a mux backend for, with none wired.
    BackendUnavailable,
    /// The named record does not exist.
    NotFound,
    /// The daemon failed while serving: an unreadable store, a write that did
    /// not land. The request may or may not have had an effect.
    Internal,
}

impl Code {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadVersion => "bad_version",
            Self::UnknownMethod => "unknown_method",
            Self::BadParams => "bad_params",
            Self::BadRequest => "bad_request",
            Self::Refused => "refused",
            Self::Busy => "busy",
            Self::BackendUnavailable => "backend_unavailable",
            Self::NotFound => "not_found",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error half of a response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

/// A refusal the daemon can state: the code a caller switches on and the
/// sentence a human reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub message: String,
}

impl Refusal {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The transport's own answer for a body, so a caller only builds refusals
    /// and never an [`ErrorBody`] by hand.
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code.as_str().to_string(),
            message: self.message.clone(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Refusal {}

/// One decoded request, past the version check.
///
/// The id is a canonical UUID: it names the request's record file, so anything
/// that could name another path is refused before it can be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub method: String,
    pub params: Value,
}

impl Request {
    /// A param that is absent, or present as a string.
    ///
    /// A param of another type is an error rather than a coercion: a caller that
    /// sent a number where a name belongs gets told, not silently matched.
    pub fn optional_string(&self, name: &str) -> Result<Option<String>, Refusal> {
        match self.params.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(Refusal::new(
                Code::BadParams,
                format!("`{name}` must be a string"),
            )),
        }
    }

    /// A param that must be present as a non-empty string.
    pub fn required_string(&self, name: &str) -> Result<String, Refusal> {
        match self.optional_string(name)? {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(Refusal::new(
                Code::BadParams,
                format!("`{name}` is required and must be a non-empty string"),
            )),
        }
    }

    /// A bounded count, clamped to `max` and defaulted when absent.
    pub fn limit(&self, default: usize, max: usize) -> Result<usize, Refusal> {
        match self.params.get("limit") {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Number(number)) => match number.as_u64() {
                Some(count) => Ok((count as usize).min(max)),
                None => Err(Refusal::new(Code::BadParams, "`limit` must be a count")),
            },
            Some(_) => Err(Refusal::new(Code::BadParams, "`limit` must be a count")),
        }
    }
}

/// One response line: the request's id and either its result or its error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Result { id: String, result: Value },
    Error { id: String, error: ErrorBody },
}

impl Response {
    pub fn ok(id: &str, result: Value) -> Self {
        Self::Result {
            id: id.to_string(),
            result,
        }
    }

    pub fn refused(id: &str, refusal: &Refusal) -> Self {
        Self::Error {
            id: id.to_string(),
            error: refusal.body(),
        }
    }

    /// The request this answers.
    pub fn id(&self) -> &str {
        match self {
            Self::Result { id, .. } | Self::Error { id, .. } => id,
        }
    }

    /// The line to write, terminator included.
    pub fn line(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).unwrap_or_else(|_| {
            // The response types are plain JSON by construction; a failure here
            // is a bug rather than a state, so the caller gets a stated refusal
            // instead of a dropped line.
            br#"{"id":"","error":{"code":"internal","message":"cannot encode the response"}}"#
                .to_vec()
        });
        bytes.push(b'\n');
        bytes
    }
}

/// Decodes one line into a request, or says why it is not one.
///
/// The connection rules are the only state here: a refusal never ends the
/// connection, so a client can correct its request and ask again.
pub fn decode_request(line: &[u8]) -> Result<Request, Refusal> {
    let value: Value = serde_json::from_slice(line).map_err(|error| {
        Refusal::new(Code::BadRequest, format!("the line is not JSON: {error}"))
    })?;
    let Some(fields) = value.as_object() else {
        return Err(Refusal::new(Code::BadRequest, "a request is a JSON object"));
    };
    match fields.get("version") {
        None => {
            return Err(Refusal::new(
                Code::BadRequest,
                "a request names its protocol version",
            ));
        }
        Some(Value::Number(number)) if number.as_u64() == Some(PROTOCOL_VERSION as u64) => {}
        Some(Value::Number(number)) => {
            let named = number.as_u64().unwrap_or(u64::MAX);
            return Err(Refusal::new(
                Code::BadVersion,
                format!("protocol version {named}; this daemon serves {PROTOCOL_VERSION}"),
            ));
        }
        Some(_) => {
            return Err(Refusal::new(Code::BadRequest, "`version` must be a number"));
        }
    }
    let Some(id) = fields.get("id").and_then(Value::as_str) else {
        return Err(Refusal::new(Code::BadRequest, "a request carries an id"));
    };
    let Some(id) = SessionUuid::parse(id) else {
        return Err(Refusal::new(
            Code::BadRequest,
            "`id` must be a canonical UUID",
        ));
    };
    let Some(method) = fields.get("method").and_then(Value::as_str) else {
        return Err(Refusal::new(Code::BadRequest, "a request names its method"));
    };
    if method.is_empty() {
        return Err(Refusal::new(Code::BadRequest, "`method` is empty"));
    }
    let params = match fields.get("params") {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(Value::Object(_)) => fields.get("params").cloned().unwrap_or(Value::Null),
        Some(_) => {
            return Err(Refusal::new(Code::BadParams, "`params` must be an object"));
        }
    };
    Ok(Request {
        id: id.as_str().to_string(),
        method: method.to_string(),
        params,
    })
}

/// The id a line names, when it names a readable one.
///
/// A refusal before the request is decoded — a foreign version, a malformed
/// request — is still correlated by this: a client that sent an id is entitled to
/// see it echoed. A line with nothing to read is answered with an empty id rather
/// than a guess, and the id is only ever echoed, never used as a path.
pub fn peek_id(line: &[u8]) -> String {
    if line.len() > MAX_LINE_BYTES {
        return String::new();
    }
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return String::new();
    };
    match value.get("id").and_then(Value::as_str) {
        Some(id) if id.len() <= 128 => id.to_string(),
        _ => String::new(),
    }
}

/// What one read from a connection produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// One line's bytes, without its terminator.
    Data(Vec<u8>),
    /// The line exceeded [`MAX_LINE_BYTES`]. It has been drained to its
    /// terminator, so the connection is still at a line boundary.
    TooLong,
    /// The peer closed the connection.
    End,
}

/// Reads one line, bounded by [`MAX_LINE_BYTES`].
///
/// An over-long line is drained rather than accumulated, so a client cannot make
/// the daemon hold a line it will refuse; the caller is free to answer and keep
/// reading.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut overlong = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if line.is_empty() && !overlong {
                Line::End
            } else if overlong {
                Line::TooLong
            } else {
                Line::Data(line)
            });
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => {
                if !overlong {
                    if line.len() + at > MAX_LINE_BYTES {
                        overlong = true;
                        line = Vec::new();
                    } else {
                        line.extend_from_slice(&available[..at]);
                    }
                }
                reader.consume(at + 1);
                return Ok(if overlong {
                    Line::TooLong
                } else {
                    Line::Data(line)
                });
            }
            None => {
                let take = available.len();
                if !overlong {
                    if line.len() + take > MAX_LINE_BYTES {
                        overlong = true;
                        line = Vec::new();
                    } else {
                        line.extend_from_slice(available);
                    }
                }
                reader.consume(take);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, method: &str, params: &str) -> String {
        format!(r#"{{"version":1,"id":"{id}","method":"{method}","params":{params}}}"#)
    }

    const ID: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";

    #[test]
    fn a_request_names_its_version_id_method_and_params() {
        let decoded = decode_request(request(ID, "ping", "{}").as_bytes()).expect("a request");
        assert_eq!(decoded.id, ID);
        assert_eq!(decoded.method, "ping");
        assert!(decoded.params.is_object());
    }

    #[test]
    fn a_missing_params_object_is_empty_rather_than_an_error() {
        let line = format!(r#"{{"version":1,"id":"{ID}","method":"ping"}}"#);
        let decoded = decode_request(line.as_bytes()).expect("a request");
        assert_eq!(decoded.params, Value::Object(serde_json::Map::new()));
    }

    #[test]
    fn another_protocol_version_is_its_own_refusal() {
        let line = format!(r#"{{"version":2,"id":"{ID}","method":"ping","params":{{}}}}"#);
        let refusal = decode_request(line.as_bytes()).expect_err("a refusal");
        assert_eq!(refusal.code, Code::BadVersion);
        assert!(refusal.message.contains("protocol version 2"));
    }

    #[test]
    fn a_line_that_is_not_a_request_is_refused_without_a_version_guess() {
        for line in [
            "not json",
            "[]",
            &format!(r#"{{"id":"{ID}","method":"ping"}}"#),
            &format!(r#"{{"version":"one","id":"{ID}","method":"ping"}}"#),
            r#"{"version":1,"method":"ping"}"#,
            r#"{"version":1,"id":"not-a-uuid","method":"ping"}"#,
            &format!(r#"{{"version":1,"id":"{ID}"}}"#),
            &format!(r#"{{"version":1,"id":"{ID}","method":""}}"#),
        ] {
            let refusal = decode_request(line.as_bytes()).expect_err("a refusal");
            assert!(
                matches!(refusal.code, Code::BadRequest | Code::BadVersion),
                "{line}: {refusal}"
            );
        }
    }

    #[test]
    fn params_of_the_wrong_shape_are_named_rather_than_coerced() {
        let decoded =
            decode_request(request(ID, "request", r#"{"id":4}"#).as_bytes()).expect("a request");
        let refusal = decoded.required_string("id").expect_err("a refusal");
        assert_eq!(refusal.code, Code::BadParams);
        assert!(refusal.message.contains("`id`"));
        assert_eq!(decoded.limit(50, 200).expect("a default"), 50);
    }

    #[test]
    fn a_refusal_still_echoes_the_id_the_line_named() {
        let line = format!(r#"{{"version":9,"id":"{ID}","method":"ping"}}"#);
        assert_eq!(peek_id(line.as_bytes()), ID);
        assert_eq!(peek_id(b"not json"), "");
        assert_eq!(peek_id(br#"{"id":42}"#), "");
    }

    #[test]
    fn a_response_line_is_one_json_object_and_a_terminator() {
        let line = Response::ok(ID, serde_json::json!({"protocol": 1})).line();
        assert_eq!(line.last(), Some(&b'\n'));
        let decoded: Response =
            serde_json::from_slice(&line[..line.len() - 1]).expect("a response");
        assert_eq!(decoded.id(), ID);
        assert!(matches!(decoded, Response::Result { .. }));

        let refused = Response::refused(ID, &Refusal::new(Code::UnknownMethod, "no such method"));
        let line = refused.line();
        let decoded: Response =
            serde_json::from_slice(&line[..line.len() - 1]).expect("a response");
        match decoded {
            Response::Error { error, .. } => {
                assert_eq!(error.code, "unknown_method");
            }
            other => panic!("{other:?}"),
        }
    }
}
