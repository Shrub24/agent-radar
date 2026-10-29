//! The operation layer: which methods exist, and what each one needs.
//!
//! Every method the daemon serves is classified here, so the connection dispatches
//! on one table rather than on string comparisons scattered through its code, and
//! so a method added later cannot be served by accident.
//!
//! Effectful operations are recorded before dispatch. Reads are never recorded.
//! Unsupported methods are refused as unknown methods rather than emulated.
//!
//! Backend primitives live with the seam they describe (`crate::runtime`); the
//! daemon's own contracts derive their primitive needs exhaustively rather than
//! being pushed by hand,
//! so what the handshake advertises and what the serving paths accept cannot
//! drift apart.

pub use crate::runtime::Primitives;

/// A daemon contract that an external consumer may pin. Backend primitives are
/// deliberately absent from this public vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Contract {
    #[serde(rename = "registry.v1")]
    Registry,
    #[serde(rename = "registry.facts.v1")]
    RegistryFacts,
    #[serde(rename = "events.v1")]
    Events,
    #[serde(rename = "tasks.list.v1")]
    TasksList,
    #[serde(rename = "spawn.v1")]
    Spawn,
    #[serde(rename = "spawn.close.v1")]
    SpawnClose,
    #[serde(rename = "spawn.launch_context.v1")]
    SpawnLaunchContext,
    #[serde(rename = "spawn.exit_observation.v1")]
    SpawnExitObservation,
    #[serde(rename = "lifecycle.cancel.v1")]
    LifecycleCancel,
    #[serde(rename = "lifecycle.stop.v1")]
    LifecycleStop,
    #[serde(rename = "topology.create.v1")]
    TopologyCreate,
    #[serde(rename = "topology.delete.v1")]
    TopologyDelete,
    #[serde(rename = "topology.reload.v1")]
    TopologyReload,
    #[serde(rename = "topology.get.v1")]
    TopologyGet,
    #[serde(rename = "topology.list.v1")]
    TopologyList,
    #[serde(rename = "topology.reconcile.v1")]
    TopologyReconcile,
    #[serde(rename = "lifecycle.get.v1")]
    LifecycleGet,
    #[serde(rename = "lifecycle.list.v1")]
    LifecycleList,
    #[serde(rename = "lifecycle.enroll.v1")]
    LifecycleEnroll,
    #[serde(rename = "lifecycle.restart.v1")]
    LifecycleRestart,
    #[serde(rename = "lifecycle.abandon.v1")]
    LifecycleAbandon,
    #[serde(rename = "observation.get.v1")]
    ObservationGet,
    #[serde(rename = "communication.messages.v1")]
    CommunicationMessages,
    #[serde(rename = "ui.session.v1")]
    UiSession,
    #[serde(other)]
    Unknown,
}

impl Contract {
    pub const ALL: &'static [Self] = &[
        Self::Registry,
        Self::RegistryFacts,
        Self::Events,
        Self::Spawn,
        Self::SpawnClose,
        Self::SpawnLaunchContext,
        Self::SpawnExitObservation,
        Self::LifecycleCancel,
        Self::LifecycleStop,
        Self::TopologyCreate,
        Self::TopologyDelete,
        Self::TopologyReload,
        Self::TopologyGet,
        Self::TopologyList,
        Self::TopologyReconcile,
        Self::LifecycleGet,
        Self::LifecycleList,
        Self::LifecycleEnroll,
        Self::LifecycleRestart,
        Self::LifecycleAbandon,
        Self::ObservationGet,
        Self::CommunicationMessages,
        Self::UiSession,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry.v1",
            Self::RegistryFacts => "registry.facts.v1",
            Self::Events => "events.v1",
            Self::TasksList => "tasks.list.v1",
            Self::Spawn => "spawn.v1",
            Self::SpawnClose => "spawn.close.v1",
            Self::SpawnLaunchContext => "spawn.launch_context.v1",
            Self::SpawnExitObservation => "spawn.exit_observation.v1",
            Self::LifecycleCancel => "lifecycle.cancel.v1",
            Self::LifecycleStop => "lifecycle.stop.v1",
            Self::TopologyCreate => "topology.create.v1",
            Self::TopologyDelete => "topology.delete.v1",
            Self::TopologyReload => "topology.reload.v1",
            Self::TopologyGet => "topology.get.v1",
            Self::TopologyList => "topology.list.v1",
            Self::TopologyReconcile => "topology.reconcile.v1",
            Self::LifecycleGet => "lifecycle.get.v1",
            Self::LifecycleList => "lifecycle.list.v1",
            Self::LifecycleEnroll => "lifecycle.enroll.v1",
            Self::LifecycleRestart => "lifecycle.restart.v1",
            Self::LifecycleAbandon => "lifecycle.abandon.v1",
            Self::ObservationGet => "observation.get.v1",
            Self::CommunicationMessages => "communication.messages.v1",
            Self::UiSession => "ui.session.v1",
            Self::Unknown => "unknown",
        }
    }

    pub const fn needs(self) -> Primitives {
        use Primitives as P;
        match self {
            Self::Registry
            | Self::RegistryFacts
            | Self::Events
            | Self::TasksList
            | Self::TopologyGet
            | Self::TopologyList => P::NONE,
            Self::Spawn => P::CREATE.union(P::LAUNCH),
            Self::SpawnClose => P::OBSERVE.union(P::CLOSE),
            Self::SpawnLaunchContext => P::CREATE.union(P::LAUNCH).union(P::LAUNCH_CONTEXT),
            Self::SpawnExitObservation => P::EXIT_OBSERVATION,
            Self::LifecycleCancel
            | Self::LifecycleStop
            | Self::LifecycleGet
            | Self::LifecycleList => P::LIFECYCLE_TRACKING,
            Self::TopologyCreate => P::CREATE,
            Self::TopologyDelete => P::CLOSE,
            Self::TopologyReload => P::LAUNCH.union(P::EXIT_OBSERVATION),
            Self::TopologyReconcile => P::OBSERVE,
            Self::LifecycleEnroll => P::LIFECYCLE_TRACKING.union(P::OBSERVE),
            Self::LifecycleRestart => P::LIFECYCLE_TRACKING.union(P::CREATE).union(P::LAUNCH),
            // Abandoning writes one durable record: it reads no backend and
            // dispatches no effect, so it needs no primitive.
            Self::LifecycleAbandon => P::NONE,
            // The snapshot is built from the same observation a `mux.observe`
            // read is, so a backend that cannot observe has nothing to publish.
            Self::ObservationGet | Self::CommunicationMessages => P::NONE,
            // The session reads the daemon's own observation snapshot and holds
            // volatile state; it asks nothing of the backend itself.
            Self::UiSession => P::NONE,
            Self::Unknown => P::NONE,
        }
    }
}

/// The wire revision of this build: it increases whenever a method or a field is
/// added to the protocol. A bundled client needs a daemon at least this new,
/// because the two ship together and only a daemon that outlived an install can
/// be older.
/// Bumped 4 to 5 when lifecycle reads republished the agent as `subject_id` and
/// the attempt as `claim_id`, and stopped publishing a generation. A revision-4
/// client reads those names with the old meanings, so it must not connect.
pub const PROTOCOL_REVISION: u32 = 11;

/// What the handshake advertises: the daemon contracts the backend's primitive set
/// can serve, which is the list an external consumer pins. The backend's own
/// `mux.*` primitives are the daemon's business and are not on the wire.
pub fn advertised(backend: Primitives) -> Vec<Contract> {
    advertised_with_tasks(backend, false)
}

/// Handshake contracts, including the volatile task lane only while its socket
/// is bound and serving.
pub fn advertised_with_tasks(backend: Primitives, tasks_available: bool) -> Vec<Contract> {
    Contract::ALL
        .iter()
        .copied()
        .filter(|contract| contract.needs().missing_name(backend).is_none())
        .chain(tasks_available.then_some(Contract::TasksList))
        .collect()
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
    /// Creation of a location Radar records as one of its own containers. The
    /// primitive create is the same; the container, its label and its provenance
    /// are what this verb adds.
    CreateContainer,
    /// Deletion of a container Radar recorded: the same close a generic one
    /// dispatches, guarded by the container's own binding and the frozen target.
    DeleteContainer,
    /// Reload of a pane container's slot: its recorded recipe runs again in the
    /// same pane, once nothing that may still be running is in it.
    ReloadContainer,
    /// Cancellation of a tracked launch: revocation, then a signal to its captured
    /// process.
    Cancel,
    /// Stop of a tracked launch's captured process, leaving the launch as it is.
    Stop,
    /// Enrollment of a running lead the daemon did not launch, as a restartable subject.
    Enroll,
    /// Operator restart of a restartable lead: stop, then a replacement in its own pane.
    Restart,
    /// Operator abandonment of an unresolved record: its recovery obligation
    /// ends and only its administrative block is released. The uncertainty is
    /// preserved, no outcome is established, and no effect is dispatched.
    Abandon,
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
            "topology.create" => Self::CreateContainer,
            "topology.delete" => Self::DeleteContainer,
            "topology.reload" => Self::ReloadContainer,
            "lifecycle.cancel" => Self::Cancel,
            "lifecycle.stop" => Self::Stop,
            "lifecycle.enroll" => Self::Enroll,
            "lifecycle.restart" => Self::Restart,
            "lifecycle.abandon" => Self::Abandon,
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
            Self::CreateContainer => "topology.create",
            Self::DeleteContainer => "topology.delete",
            Self::ReloadContainer => "topology.reload",
            Self::Cancel => "lifecycle.cancel",
            Self::Stop => "lifecycle.stop",
            Self::Enroll => "lifecycle.enroll",
            Self::Restart => "lifecycle.restart",
            Self::Abandon => "lifecycle.abandon",
        }
    }

    /// The backend primitives this operation cannot be served without.
    pub const fn needs(self) -> Primitives {
        use Primitives as P;
        match self {
            Self::Focus => P::FOCUS,
            Self::Close | Self::DeleteContainer => P::CLOSE,
            Self::Create | Self::CreateContainer => P::CREATE,
            Self::Input => P::INPUT,
            Self::Report => P::REPORT,
            Self::Spawn => P::CREATE.union(P::LAUNCH),
            Self::ReloadContainer => P::LAUNCH.union(P::EXIT_OBSERVATION),
            Self::Cancel | Self::Stop => P::LIFECYCLE_TRACKING,
            Self::Enroll => P::LIFECYCLE_TRACKING.union(P::OBSERVE),
            Self::Restart => P::LIFECYCLE_TRACKING.union(P::CREATE).union(P::LAUNCH),
            Self::Abandon => P::NONE,
        }
    }

    /// Whether a backend declared every primitive this operation needs.
    pub fn ready_for(self, primitives: Primitives) -> Result<(), String> {
        match self.needs().missing_name(primitives) {
            None => Ok(()),
            Some(name) => Err(format!(
                "{} needs the `{name}` capability; this backend does not provide it",
                self.as_str()
            )),
        }
    }

    /// The message a record carries while no backend is wired.
    pub fn unavailable_message(self) -> String {
        let needs = self.needs();
        let names = [
            Primitives::OBSERVE,
            Primitives::PROCESS_INFO,
            Primitives::FOCUS,
            Primitives::CLOSE,
            Primitives::CREATE,
            Primitives::INPUT,
            Primitives::OUTPUT,
            Primitives::REPORT,
            Primitives::LAUNCH,
            Primitives::LAUNCH_CONTEXT,
            Primitives::EXIT_OBSERVATION,
            Primitives::LIFECYCLE_TRACKING,
        ];
        let named: Vec<&str> = names
            .into_iter()
            .filter(|primitive| needs.contains(*primitive))
            .filter_map(|primitive| primitive.missing_name(Primitives::NONE))
            .collect();
        let phrase = if named.len() == 1 {
            format!("the `{}` capability", named[0])
        } else {
            format!(
                "the {} capabilities",
                named
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        };
        format!(
            "{} needs {phrase}; this daemon has no backend wired",
            self.as_str()
        )
    }
}

/// How the daemon serves one method name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Liveness and identity: the version, the backend and its contracts.
    Ping,
    /// Read one recorded request by id.
    Request,
    /// List recent recorded requests, newest first.
    Requests,
    /// A mux read. Served from a backend, and never recorded.
    Read,
    /// Resumable read of the durable event journal.
    Events,
    /// The daemon's shared observation snapshot: one sampler's output for every
    /// reader, revisioned and demand-driven. A read, never recorded.
    Observation,
    /// Durable handoff publication, retrieval and recipient receipt.
    Communication,
    /// Volatile task sessions received from extension publishers.
    Tasks,
    /// Registry reads and writes: the daemon's own durable records, answered
    /// whether or not a mux backend is wired.
    Registry,
    /// The internal claim a tracked launch's `radar run --launch` makes before it
    /// is given its program. Not a public operation: it is served only to a launch the daemon
    /// created, through its correlation.
    Claim,
    /// Admission of an in-place managed launch into a pane the caller occupies.
    Admit,
    /// The internal notification a launch's pane-death hook sends. It is a hint
    /// that reconciles one launch against exact process evidence.
    Wake,
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
        "observation.get" => Method::Observation,
        "communication.publish"
        | "communication.inbox"
        | "communication.get"
        | "communication.ack" => Method::Communication,
        "tasks.list" => Method::Tasks,
        "agent.presence"
        | "agent.release"
        | "agent.acquire"
        | "agent.register"
        | "agent.publish"
        | "agent.retire"
        | "agent.context"
        | "process.get"
        | "subject.get"
        | "subject.list"
        | "subject.children"
        | "subject.child.get"
        | "subject.child.inspect"
        | "subject.child.close"
        | "subject.assignment.acquire"
        | "subject.assignment.publish"
        | "subject.assignment.retire" => Method::Registry,
        // A spawn read reports the daemon's own edge records; whether the backend
        // still reports a recorded location changes its freshness, never whether
        // the read is served.
        "spawn.get" | "spawn.list" | "spawn.close" => Method::Registry,
        // A lifecycle read reports the daemon's own subject records.
        "lifecycle.get" | "lifecycle.list" | "lifecycle.reconcile" => Method::Registry,
        "topology.get" | "topology.list" | "topology.reconcile" => Method::Registry,
        "mux.observe" | "mux.process_info" | "mux.output" => Method::Read,
        "launch.claim" => Method::Claim,
        "launch.admit" => Method::Admit,
        "lifecycle.wake" => Method::Wake,
        other => Method::Operation(Operation::from_method(other)?),
    })
}

/// The primitive a read needs from the backend, so a read is served only by a
/// backend that says it can answer it.
///
/// A read has no record, so nothing about it is durable: what it needs is the
/// backend's own statement that this is a request it implements.
pub fn read_primitives(method: &str) -> Primitives {
    match method {
        "mux.process_info" => Primitives::PROCESS_INFO,
        "mux.output" => Primitives::OUTPUT,
        _ => Primitives::OBSERVE,
    }
}

/// The refusal for a read the wired backend does not implement. A read leaves no
/// record, so the refusal is the whole answer.
pub fn read_unsupported(
    method: &str,
    primitive: &str,
) -> (crate::control_plane::protocol::Code, String) {
    (
        crate::control_plane::protocol::Code::BackendUnavailable,
        format!("{method} needs the `{primitive}` capability; this backend does not provide it"),
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
        assert_eq!(classify("tasks.list"), Some(Method::Tasks));
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
    fn a_read_names_the_primitive_it_needs() {
        assert!(read_primitives("mux.observe").contains(Primitives::OBSERVE));
        assert!(read_primitives("mux.process_info").contains(Primitives::PROCESS_INFO));
        assert!(read_primitives("mux.output").contains(Primitives::OUTPUT));
    }

    #[test]
    fn every_contract_is_advertised_only_when_its_needs_are_met() {
        let primitives = [
            Primitives::OBSERVE,
            Primitives::PROCESS_INFO,
            Primitives::FOCUS,
            Primitives::CLOSE,
            Primitives::CREATE,
            Primitives::INPUT,
            Primitives::OUTPUT,
            Primitives::REPORT,
            Primitives::LAUNCH,
            Primitives::LAUNCH_CONTEXT,
            Primitives::EXIT_OBSERVATION,
            Primitives::LIFECYCLE_TRACKING,
        ];
        let all = primitives
            .into_iter()
            .fold(Primitives::NONE, Primitives::union);
        for contract in Contract::ALL {
            assert!(advertised(all).contains(contract), "{contract:?}");
            for omitted in primitives {
                let without = primitives
                    .into_iter()
                    .filter(|primitive| primitive != &omitted)
                    .fold(Primitives::NONE, Primitives::union);
                if contract.needs().contains(omitted) {
                    assert!(
                        !advertised(without).contains(contract),
                        "{contract:?} without {omitted:?}"
                    );
                }
            }
        }
        let empty = advertised(Primitives::NONE);
        assert_eq!(
            empty,
            vec![
                Contract::Registry,
                Contract::RegistryFacts,
                Contract::Events,
                Contract::TopologyGet,
                Contract::TopologyList,
                // Abandoning writes one durable record: it reads no backend and
                // dispatches no effect, so a daemon with no backend still serves it.
                Contract::LifecycleAbandon,
                Contract::ObservationGet,
                // Communication records are daemon-owned and need no mux primitive.
                Contract::CommunicationMessages,
                // The UI session is daemon-owned volatile state, not a backend read.
                Contract::UiSession,
            ]
        );
    }

    /// A contract the bundled SDK gates on must be one the daemon can advertise,
    /// or the SDK refuses the call as unsupported against its own daemon.
    #[test]
    fn every_contract_the_sdk_pins_is_one_the_daemon_advertises() {
        let sdk = include_str!("../../pi-radar/extension/radar-client.ts");
        let pinned: Vec<&str> = sdk
            .lines()
            .filter(|line| {
                line.starts_with("export const RADAR_") && line.contains("_CONTRACT = \"")
            })
            .filter_map(|line| line.split('"').nth(1))
            .collect();
        assert!(!pinned.is_empty(), "the SDK pins no contracts");
        let known: Vec<&str> = Contract::ALL
            .iter()
            .copied()
            .chain([Contract::TasksList])
            .map(Contract::as_str)
            .collect();
        for name in pinned {
            assert!(
                known.contains(&name),
                "the SDK pins {name}, which no daemon advertises"
            );
        }
    }

    #[test]
    fn contracts_have_stable_wire_names_and_unknown_names_are_tolerated() {
        for contract in Contract::ALL.iter().copied().chain([Contract::TasksList]) {
            assert!(
                matches!(
                    serde_json::from_str::<Contract>(&format!("\"{}\"", contract.as_str())),
                    Ok(decoded) if decoded == contract
                ),
                "serde and the advertised name agree for {contract:?}"
            );
        }
        assert!(matches!(
            serde_json::from_str::<Contract>("\"future.v9\""),
            Ok(Contract::Unknown)
        ));
        assert!(!advertised_with_tasks(Primitives::NONE, false).contains(&Contract::TasksList));
        assert!(advertised_with_tasks(Primitives::NONE, true).contains(&Contract::TasksList));
    }

    #[test]
    fn contract_needs_cover_the_operations_they_advertise() {
        use Primitives as P;
        let checks = [
            (Contract::Spawn, Operation::Spawn.needs()),
            (
                Contract::SpawnClose,
                P::OBSERVE.union(Operation::Close.needs()),
            ),
            (
                Contract::SpawnLaunchContext,
                P::LAUNCH_CONTEXT.union(Operation::Spawn.needs()),
            ),
            (Contract::SpawnExitObservation, P::EXIT_OBSERVATION),
            (Contract::TopologyCreate, Operation::CreateContainer.needs()),
            (Contract::TopologyDelete, Operation::DeleteContainer.needs()),
            (Contract::TopologyReload, Operation::ReloadContainer.needs()),
            (Contract::TopologyReconcile, P::OBSERVE),
            (Contract::LifecycleCancel, Operation::Cancel.needs()),
            (Contract::LifecycleStop, Operation::Stop.needs()),
            (Contract::LifecycleEnroll, Operation::Enroll.needs()),
            (Contract::LifecycleRestart, Operation::Restart.needs()),
            (Contract::LifecycleGet, P::LIFECYCLE_TRACKING),
            (Contract::LifecycleList, P::LIFECYCLE_TRACKING),
        ];
        for (contract, method_needs) in checks {
            assert!(
                contract.needs().contains(method_needs),
                "{contract:?} omits {method_needs:?}"
            );
        }
    }

    #[test]
    fn operation_readiness_refuses_before_dispatch_with_the_missing_primitive_named() {
        let spawn = Primitives::CREATE.union(Primitives::LAUNCH);
        assert!(Operation::Spawn.ready_for(spawn).is_ok());
        assert_eq!(
            Operation::Spawn.ready_for(Primitives::CREATE),
            Err(
                "spawn.create needs the `mux.launch` capability; this backend does not provide it"
                    .into()
            )
        );
        assert_eq!(
            Operation::Spawn.ready_for(Primitives::LAUNCH),
            Err(
                "spawn.create needs the `mux.create` capability; this backend does not provide it"
                    .into()
            )
        );
        assert!(
            Operation::Spawn
                .unavailable_message()
                .contains("`mux.create` and `mux.launch`")
        );
    }
}
