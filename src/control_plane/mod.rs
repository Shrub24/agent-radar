//! The control plane: one daemon that owns mux observation and lifecycle.
//!
//! Radar talks to the multiplexer through an adapter today and will talk to this
//! daemon instead, on a unix socket, with the adapter kept as the fallback for a
//! machine that runs no daemon. [`protocol`] is the wire format, [`store`] is the
//! durable record every operation is derived from, [`server`] is the listener,
//! [`ops`] classifies methods, [`client`] is Radar's own end of the socket, and
//! the normalized backend port reuses the runtime seam without exposing tmux
//! wire types. [`registry`] is the durable agent registration store served by
//! independent trusted-socket methods.

pub mod answer;
pub mod client;
pub(crate) mod command;
pub mod communication;
pub mod containers;
pub mod events;
pub mod ingest;
pub mod lifecycle;
pub mod observation;
pub mod ops;
pub mod protocol;
pub mod public_contracts;
pub mod reconcile;
pub mod registry;
pub mod registry_read;
pub mod server;
pub mod store;
mod subject_read;
pub mod transport;

pub use crate::runtime::{RuntimeProvider, Target};
pub use client::{DaemonRuntime, Selected, connect, connect_at};
pub use ops::{Method, Operation};
pub use protocol::{
    Code, ErrorBody, MAX_LINE_BYTES, PROTOCOL_VERSION, Refusal, Request, Response, decode_request,
};
pub use reconcile::Trigger;
pub use registry::{
    CHILD_CLOSE_VERSION, ChildClose, ChildCloseRequest, CloseAdmission, CloseIntent,
    ContextRequest, ContextValue, LaunchSession, LaunchSpec, LocalProcfsVerifier,
    ProcessVerification, ProcessVerifier, PublicContext, PublicLaunch, PublicRegistration,
    Registration, RegistrationRequest, Registry, RegistryLocation, SessionContext,
};
pub use server::{
    Daemon, MAX_VERIFICATION_JOBS, install_signal_handlers, socket_path, socket_path_in, state_dir,
    state_dir_in, trusted_socket,
};
pub use store::{
    Category, Derived, Event, RecordState, RequestOutcome, RequestRecord, Settlement, Store,
    format_millis, now, now_ms, random_uuid, ttl_seconds,
};
