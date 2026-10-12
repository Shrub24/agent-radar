//! The operation layer: which methods exist, and what each one needs.
//!
//! Every method the daemon serves is classified here, so the connection dispatches
//! on one table rather than on string comparisons scattered through its code, and
//! so a method added later cannot be served by accident.
//!
//! Effectful operations are recorded before dispatch. Reads are never recorded.
//! Unsupported methods are refused as unknown methods rather than emulated.
//!
//! The backend capability vocabulary lives with the seam it describes
//! (`crate::runtime`) and is re-exported here; the daemon's own contracts are
//! derived from the capabilities their methods need rather than pushed by hand,
//! so what the handshake advertises and what the serving paths accept cannot
//! drift apart.

pub use crate::runtime::{
    CLOSE_CAPABILITY, CREATE_CAPABILITY, EXIT_OBSERVATION_CAPABILITY, FOCUS_CAPABILITY,
    INPUT_CAPABILITY, LAUNCH_CAPABILITY, LAUNCH_CONTEXT_CAPABILITY, OBSERVE_CAPABILITY,
    OUTPUT_CAPABILITY, PROCESS_INFO_CAPABILITY, REPORT_CAPABILITY,
};

/// A contract the daemon itself serves, versioned because a consumer pins it.
///
/// The `*.v1` names are the daemon's own, not the backend's: they say which of
/// the daemon's methods a backend's capabilities make servable.
pub const REGISTRY_CONTRACT: &str = "registry.v1";
pub const REGISTRY_FACTS_CONTRACT: &str = "registry.facts.v1";
pub const EVENTS_CONTRACT: &str = "events.v1";
pub const SPAWN_CONTRACT: &str = "spawn.v1";
pub const SPAWN_CLOSE_CONTRACT: &str = "spawn.close.v1";
pub const SPAWN_LAUNCH_CONTEXT_CONTRACT: &str = "spawn.launch_context.v1";
pub const SPAWN_EXIT_OBSERVATION_CONTRACT: &str = "spawn.exit_observation.v1";

/// Every daemon contract, with the backend capabilities its methods are served
/// from.
///
/// Advertising a contract promises that its methods are served, so a contract
/// appears only when every capability it names is present. The registry and the
/// event journal are answered from the daemon's own records and need no backend.
const CONTRACTS: &[(&str, &[&str])] = &[
    (REGISTRY_CONTRACT, &[]),
    (REGISTRY_FACTS_CONTRACT, &[]),
    (EVENTS_CONTRACT, &[]),
    (SPAWN_CONTRACT, &[CREATE_CAPABILITY, LAUNCH_CAPABILITY]),
    (
        SPAWN_CLOSE_CONTRACT,
        &[OBSERVE_CAPABILITY, CLOSE_CAPABILITY],
    ),
    (
        SPAWN_LAUNCH_CONTEXT_CONTRACT,
        &[
            CREATE_CAPABILITY,
            LAUNCH_CAPABILITY,
            LAUNCH_CONTEXT_CAPABILITY,
        ],
    ),
    (
        SPAWN_EXIT_OBSERVATION_CONTRACT,
        &[EXIT_OBSERVATION_CAPABILITY],
    ),
];

/// What the handshake advertises: what the backend declares, then the daemon
/// contracts those capabilities can serve.
pub fn advertised(backend: &[&'static str]) -> Vec<&'static str> {
    let mut advertised: Vec<&'static str> = Vec::new();
    for capability in backend {
        if !advertised.contains(capability) {
            advertised.push(capability);
        }
    }
    advertised.extend(
        CONTRACTS
            .iter()
            .filter(|(_, needs)| needs.iter().all(|needed| backend.contains(needed)))
            .map(|(contract, _)| *contract),
    );
    advertised
}

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

impl Operation {
    /// The operation a method name denotes, if it is one.
    pub fn from_method(method: &str) -> Option<Self> {
        Some(match method {
            "mux.focus" => Self::Focus,
            "mux.close" => Self::Close,
            "mux.create" => Self::Create,
            "mux.input" => Self::Input,
            "mux.report" => Self::Report,
            "spawn.create" => Self::Spawn,
            _ => return None,
        })
    }

    /// The wire name, which is also the record's method.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "mux.focus",
            Self::Close => "mux.close",
            Self::Create => "mux.create",
            Self::Input => "mux.input",
            Self::Report => "mux.report",
            Self::Spawn => "spawn.create",
        }
    }

    /// The backend capabilities this operation cannot be served without.
    pub fn capabilities(self) -> &'static [&'static str] {
        match self {
            Self::Focus => &[FOCUS_CAPABILITY],
            Self::Close => &[CLOSE_CAPABILITY],
            Self::Create => &[CREATE_CAPABILITY],
            Self::Input => &[INPUT_CAPABILITY],
            Self::Report => &[REPORT_CAPABILITY],
            // A spawn does both, so a backend that can do only one of them
            // cannot serve it: a pane with no command in it is not a child.
            Self::Spawn => &[CREATE_CAPABILITY, LAUNCH_CAPABILITY],
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
    /// Resumable read of the durable event journal.
    Events,
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
        "daemon.ping" => Method::Ping,
        "mux.request.get" => Method::Request,
        "mux.request.list" => Method::Requests,
        "events.read" => Method::Events,
        "agent.acquire" | "agent.register" | "agent.publish" | "agent.retire" | "agent.context"
        | "agent.get" | "agent.list" => Method::Registry,
        // A spawn read reports the daemon's own edge records; whether the backend
        // still reports a recorded location changes its freshness, never whether
        // the read is served.
        "spawn.get" | "spawn.list" | "spawn.close" => Method::Registry,
        "mux.observe" | "mux.process_info" | "mux.output" => Method::Read,
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
        "mux.process_info" => PROCESS_INFO_CAPABILITY,
        "mux.output" => OUTPUT_CAPABILITY,
        _ => OBSERVE_CAPABILITY,
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
        assert_eq!(classify("daemon.ping"), Some(Method::Ping));
        assert_eq!(classify("mux.request.get"), Some(Method::Request));
        assert_eq!(classify("mux.request.list"), Some(Method::Requests));
        assert_eq!(classify("events.read"), Some(Method::Events));
        assert_eq!(classify("agent.context"), Some(Method::Registry));
        assert_eq!(classify("spawn.get"), Some(Method::Registry));
        assert_eq!(classify("spawn.list"), Some(Method::Registry));
        assert_eq!(classify("spawn.close"), Some(Method::Registry));
        assert_eq!(classify("mux.observe"), Some(Method::Read));
        assert_eq!(classify("mux.process_info"), Some(Method::Read));
        assert_eq!(classify("mux.output"), Some(Method::Read));
        assert_eq!(classify("registry"), None);
        assert_eq!(classify("resume"), None);
        // The names the first protocol version served are gone, and a client
        // that still uses one is told it named no method rather than served a
        // substituted one.
        for removed in [
            "ping",
            "observe",
            "process_info",
            "output",
            "focus",
            "close",
            "create",
            "input",
            "report",
            "request",
            "requests",
            "spawn",
            "child.close",
        ] {
            assert_eq!(
                classify(removed),
                None,
                "{removed} is not served in version 2"
            );
        }
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
        for method in ["mux.observe", "mux.process_info", "mux.output"] {
            assert_eq!(classify(method), Some(Method::Read));
            assert_eq!(Operation::from_method(method), None);
        }
    }

    #[test]
    fn a_read_names_the_capability_it_needs() {
        assert_eq!(read_capability("mux.observe"), OBSERVE_CAPABILITY);
        assert_eq!(read_capability("mux.process_info"), PROCESS_INFO_CAPABILITY);
        assert_eq!(read_capability("mux.output"), OUTPUT_CAPABILITY);
    }

    /// Every contract a backend can serve is advertised, and each one needs what
    /// its serving path checks. A hand-pushed flag is what this replaces.
    #[test]
    fn every_contract_needs_the_capabilities_its_methods_use() {
        let able = [
            OBSERVE_CAPABILITY,
            PROCESS_INFO_CAPABILITY,
            FOCUS_CAPABILITY,
            CLOSE_CAPABILITY,
            CREATE_CAPABILITY,
            INPUT_CAPABILITY,
            OUTPUT_CAPABILITY,
            REPORT_CAPABILITY,
            LAUNCH_CAPABILITY,
            LAUNCH_CONTEXT_CAPABILITY,
            EXIT_OBSERVATION_CAPABILITY,
        ];
        for (contract, needs) in CONTRACTS {
            assert!(
                advertised(&able).contains(contract),
                "a backend with every capability serves {contract}"
            );
            for needed in *needs {
                let without: Vec<&str> = able
                    .iter()
                    .copied()
                    .filter(|capability| capability != needed)
                    .collect();
                assert!(
                    !advertised(&without).contains(contract),
                    "{contract} is not advertised without {needed}"
                );
            }
        }
        assert_eq!(
            advertised(&[LAUNCH_CAPABILITY]),
            vec![
                LAUNCH_CAPABILITY,
                REGISTRY_CONTRACT,
                REGISTRY_FACTS_CONTRACT,
                EVENTS_CONTRACT
            ],
            "a backend that can only launch serves spawn.create no better than none"
        );
        assert_eq!(
            advertised(&[]),
            vec![REGISTRY_CONTRACT, REGISTRY_FACTS_CONTRACT, EVENTS_CONTRACT],
            "the daemon's own records need no backend"
        );
    }

    #[test]
    fn a_capability_a_backend_states_twice_is_advertised_once() {
        assert_eq!(
            advertised(&[OBSERVE_CAPABILITY, OBSERVE_CAPABILITY]),
            vec![
                OBSERVE_CAPABILITY,
                REGISTRY_CONTRACT,
                REGISTRY_FACTS_CONTRACT,
                EVENTS_CONTRACT
            ]
        );
    }

    /// A contract's capability list is exactly what its operation checks, so an
    /// advertised contract is never refused for a capability it did not name.
    #[test]
    fn a_contract_names_what_its_operation_needs() {
        let spawn_needs = CONTRACTS
            .iter()
            .find(|(contract, _)| *contract == SPAWN_CONTRACT)
            .expect("the spawn contract")
            .1;
        assert_eq!(Operation::Spawn.capabilities(), spawn_needs);
        let close_needs = CONTRACTS
            .iter()
            .find(|(contract, _)| *contract == SPAWN_CLOSE_CONTRACT)
            .expect("the close contract")
            .1;
        assert!(close_needs.contains(&CLOSE_CAPABILITY));
    }

    #[test]
    fn a_refusal_names_the_capability_it_needs() {
        assert!(
            Operation::Focus
                .unavailable_message()
                .contains("`mux.focus`")
        );
        assert!(
            Operation::Spawn
                .unavailable_message()
                .contains("`mux.create` and `mux.launch`"),
            "a spawn names everything it needs"
        );
        assert_eq!(
            Operation::readiness(
                "spawn.create",
                LAUNCH_CAPABILITY,
                &[CREATE_CAPABILITY, INPUT_CAPABILITY]
            ),
            Err(
                "spawn.create needs the `mux.launch` capability; this backend does not provide it"
                    .into()
            )
        );
        assert_eq!(
            Operation::readiness("spawn.create", LAUNCH_CAPABILITY, &[LAUNCH_CAPABILITY]),
            Ok(()),
            "readiness is separate from implementing dispatch"
        );
    }

    #[test]
    fn an_operation_needs_every_capability_it_lists() {
        let able = [CREATE_CAPABILITY, LAUNCH_CAPABILITY, REPORT_CAPABILITY];
        assert_eq!(Operation::Spawn.ready_for(&able), Ok(()));
        assert!(Operation::Create.ready_for(&able).is_ok());
        assert!(Operation::Report.ready_for(&able).is_ok());
        assert_eq!(
            Operation::Spawn.ready_for(&[CREATE_CAPABILITY, INPUT_CAPABILITY]),
            Err(
                "spawn.create needs the `mux.launch` capability; this backend does not provide it"
                    .into()
            ),
            "creating a pane is not launching a child"
        );
        assert_eq!(
            Operation::Spawn.ready_for(&[LAUNCH_CAPABILITY]),
            Err(
                "spawn.create needs the `mux.create` capability; this backend does not provide it"
                    .into()
            ),
            "launching is not creating the pane a child runs in"
        );
    }
}
