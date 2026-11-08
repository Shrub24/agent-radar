//! Runtime observations and their fleet-overview projection.
//! Backend wire formats stay in the adapter; presentation uses normalized facts.

pub mod app;
pub mod attach;
pub mod bus;
pub mod cli;
pub mod collector;
pub mod config;
pub mod content;
pub mod control_plane;
pub mod focus;
pub mod identity;

pub mod lifecycle;
pub mod log;
pub mod managed_scope;
pub mod model;
pub mod observation;
pub mod picker;
pub mod process_control;
pub mod process_icons;
pub mod procfs;
pub mod project;
pub mod registry_observation;
pub mod reset;
pub mod runtime;
pub mod service;
pub mod tasks;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
#[cfg(test)]
pub(crate) mod testing;
pub mod theme;
pub mod timestamp;
pub mod title;
pub mod tmux;
pub mod topology;
pub mod tree;
pub mod ui;
pub mod ui_session;
pub mod workspace;

pub use app::{Action, App, Geometry, Operation, PaneView, RowOrder};
pub use collector::{Collector, CollectorConfig, RegistrySource};
pub use config::{Config, Palette};
pub use control_plane::{
    Category, Code, Daemon, Derived, ErrorBody, MAX_LINE_BYTES, PROTOCOL_VERSION, RecordState,
    Refusal, Request, RequestOutcome, RequestRecord, Response, Store, decode_request,
    install_signal_handlers, socket_path, state_dir,
};
pub use lifecycle::{CloseRequest, Containment};
pub use model::{
    AgentObservation, BinaryFreshness, BinaryIdentity, BinaryUnknown, FleetObservation,
    ForegroundEvidence, LocalFacts, Location, Pane, RuntimeStatus, SessionIdentity, SessionUuid,
    Tab, TerminalMode, Workspace,
};
pub use observation::{ObservationState, RetainedAgent, RetentionBasis, SourceFreshness};
pub use runtime::{CloseOutcome, CloseTarget, FocusOutcome, RuntimeProvider, Target};
pub use tmux::{TmuxConfig, TmuxRuntime};
pub use tree::{
    AgentRow, FleetTree, PaneRow, RowId, RowKind, TaskId, TaskProjection, TaskRow, TaskSource,
    TreeRow,
};
