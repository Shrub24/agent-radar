//! The operation layer: which methods exist, and what each one needs.
//!
//! Every method the daemon serves is classified here, so the connection dispatches
//! on one table rather than on string comparisons scattered through its code, and
//! so a method added later cannot be served by accident.
//!
//! Effectful operations are recorded before dispatch. Reads are never recorded.
//! Unsupported methods are refused as unknown methods rather than emulated.

/// An operation that changes or asks something of the multiplexer, and therefore
/// travels as a durable request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Focus,
    Close,
    Create,
    Input,
    Report,
    /// A managed child: a pane created under a named parent, and the resolved
    /// command launched in it.
    Spawn,
}

/// The `launch` capability required to run a resolved child command.
pub use crate::runtime::LAUNCH_CAPABILITY;

impl Operation {
    /// The operation a method name denotes, if it is one.
    pub fn from_method(method: &str) -> Option<Self> {
        Some(match method {
            "focus" => Self::Focus,
            "close" => Self::Close,
            "create" => Self::Create,
            "input" => Self::Input,
            "report" => Self::Report,
            "spawn" => Self::Spawn,
            _ => return None,
        })
    }

    /// The wire name, which is also the record's method.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Close => "close",
            Self::Create => "create",
            Self::Input => "input",
            Self::Report => "report",
            Self::Spawn => "spawn",
        }
    }

    /// The backend capabilities this operation cannot be served without.
    pub fn capabilities(self) -> &'static [&'static str] {
        match self {
            Self::Focus => &["focus"],
            Self::Close => &["close"],
            Self::Create => &["creation"],
            Self::Input => &["input"],
            Self::Report => &["reporting"],
            // A spawn does both, so a backend that can do only one of them
            // cannot serve it: a pane with no command in it is not a child.
            Self::Spawn => &["creation", LAUNCH_CAPABILITY],
        }
    }

    /// Whether a backend declared every capability this operation needs.
    ///
    /// A capability is the backend's promise that it implements that part of the
    /// operation, not an optimistic request: [`Operation::Spawn`] needs both the
    /// creation it is asked for and the launch that follows it. The message names
    /// the first capability that is missing.
    pub fn ready_for(self, capabilities: &[&str]) -> Result<(), String> {
        self.capabilities()
            .iter()
            .try_for_each(|capability| Self::readiness(self.as_str(), capability, capabilities))
    }

    /// Whether a backend declared the capability required by an operation.
    ///
    /// Use this before recording or dispatch.
    pub fn readiness(
        operation: &str,
        capability: &str,
        capabilities: &[&str],
    ) -> Result<(), String> {
        if capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(format!(
                "{operation} needs the `{capability}` capability; this backend does not provide it"
            ))
        }
    }

    /// The message a record carries while no backend is wired.
    pub fn unavailable_message(self) -> String {
        let capabilities = self.capabilities();
        let named = if capabilities.len() == 1 {
            format!("the `{}` capability", capabilities[0])
        } else {
            let quoted: Vec<String> = capabilities
                .iter()
                .map(|capability| format!("`{capability}`"))
                .collect();
            format!("the {} capabilities", quoted.join(" and "))
        };
        format!(
            "{} needs {named}; this daemon has no backend wired",
            self.as_str()
        )
    }
}

/// How the daemon serves one method name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Liveness and identity: the version, the backend and its capabilities.
    Ping,
    /// Read one recorded request by id.
    Request,
    /// List recent recorded requests, newest first.
    Requests,
    /// A mux read. Served from a backend, and never recorded.
    Read,
    /// Registry reads and writes: the daemon's own durable records, answered
    /// whether or not a mux backend is wired.
    Registry,
    /// An operation, served as a durable request.
    Operation(Operation),
}

/// The methods the daemon serves. Anything else is `unknown_method`, which the
/// connection reports and then keeps serving.
pub fn classify(method: &str) -> Option<Method> {
    Some(match method {
        "ping" => Method::Ping,
        "request" => Method::Request,
        "requests" => Method::Requests,
        "agent.acquire" | "agent.register" | "agent.publish" | "agent.retire" | "agent.context"
        | "agent.get" | "agent.list" => Method::Registry,
        // A spawn read reports the daemon's own edge records; whether the backend
        // still reports a recorded location changes its freshness, never whether
        // the read is served.
        "spawn.get" | "spawn.list" | "child.close" => Method::Registry,
        "observe" | "process_info" | "output" => Method::Read,
        other => Method::Operation(Operation::from_method(other)?),
    })
}

/// The capability a read needs from the backend, so a read is served only by a
/// backend that says it can answer it.
///
/// A read has no record, so nothing about it is durable: what it needs is the
/// backend's own statement that this is a request it implements.
pub fn read_capability(method: &str) -> &'static str {
    match method {
        "process_info" => "process_info",
        "output" => "output",
        _ => "observe",
    }
}

/// The refusal for a read the wired backend does not implement. A read leaves no
/// record, so the refusal is the whole answer.
pub fn read_unsupported(
    method: &str,
    capability: &str,
) -> (crate::control_plane::protocol::Code, String) {
    (
        crate::control_plane::protocol::Code::BackendUnavailable,
        format!("{method} needs the `{capability}` capability; this backend does not provide it"),
    )
}

/// The refusal for a mux read with no backend: an error and no record, because
/// nothing was asked of the world and there is nothing to execute later.
pub fn read_unavailable(method: &str) -> (crate::control_plane::protocol::Code, String) {
    (
        crate::control_plane::protocol::Code::BackendUnavailable,
        format!("{method} needs a mux backend; this daemon has no backend wired"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_method_is_classified() {
        assert_eq!(classify("ping"), Some(Method::Ping));
        assert_eq!(classify("request"), Some(Method::Request));
        assert_eq!(classify("requests"), Some(Method::Requests));
        assert_eq!(classify("agent.context"), Some(Method::Registry));
        assert_eq!(classify("spawn.get"), Some(Method::Registry));
        assert_eq!(classify("spawn.list"), Some(Method::Registry));
        assert_eq!(classify("observe"), Some(Method::Read));
        assert_eq!(classify("process_info"), Some(Method::Read));
        assert_eq!(classify("output"), Some(Method::Read));
        assert_eq!(classify("registry"), None);
        assert_eq!(classify("resume"), None);
        for operation in [
            Operation::Focus,
            Operation::Close,
            Operation::Create,
            Operation::Input,
            Operation::Report,
            Operation::Spawn,
        ] {
            assert_eq!(
                classify(operation.as_str()),
                Some(Method::Operation(operation))
            );
        }
        assert_eq!(classify("close_everything"), None);
        assert_eq!(classify(""), None);
    }

    #[test]
    fn a_reading_method_is_not_an_operation() {
        for method in ["observe", "process_info", "output"] {
            assert_eq!(classify(method), Some(Method::Read));
            assert_eq!(Operation::from_method(method), None);
        }
    }

    #[test]
    fn a_read_names_the_capability_it_needs() {
        assert_eq!(read_capability("observe"), "observe");
        assert_eq!(read_capability("process_info"), "process_info");
        assert_eq!(read_capability("output"), "output");
    }

    #[test]
    fn a_refusal_names_the_capability_it_needs() {
        assert!(Operation::Focus.unavailable_message().contains("`focus`"));
        assert!(
            Operation::Spawn
                .unavailable_message()
                .contains("`creation` and `launch`"),
            "a spawn names everything it needs"
        );
        assert_eq!(
            Operation::readiness("spawn", LAUNCH_CAPABILITY, &["creation", "input"]),
            Err("spawn needs the `launch` capability; this backend does not provide it".into())
        );
        assert_eq!(
            Operation::readiness("spawn", LAUNCH_CAPABILITY, &["launch"]),
            Ok(()),
            "readiness is separate from implementing dispatch"
        );
    }

    #[test]
    fn an_operation_needs_every_capability_it_lists() {
        let able = ["creation", "launch", "reporting"];
        assert_eq!(Operation::Spawn.ready_for(&able), Ok(()));
        assert!(Operation::Create.ready_for(&able).is_ok());
        assert!(Operation::Report.ready_for(&able).is_ok());
        assert_eq!(
            Operation::Spawn.ready_for(&["creation", "input"]),
            Err("spawn needs the `launch` capability; this backend does not provide it".into()),
            "creating a pane is not launching a child"
        );
        assert_eq!(
            Operation::Spawn.ready_for(&["launch"]),
            Err("spawn needs the `creation` capability; this backend does not provide it".into()),
            "launching is not creating the pane a child runs in"
        );
    }
}
