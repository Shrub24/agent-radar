//! Runtime observations and their fleet-overview projection.
//! Backend wire formats stay in the adapter; presentation uses normalized facts.

pub mod app;
pub mod attach;
pub mod bus;
pub mod cli;
pub mod collector;
pub mod config;
pub mod control;
pub mod control_plane;
pub mod focus;
pub mod lifecycle;
pub mod log;
pub mod managed_control;
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
pub mod snapshot_fetch;
pub mod tasks;
pub mod theme;
pub mod title;
pub mod tmux;
pub mod topology;
pub mod tree;
pub mod ui;
pub mod workspace;

pub use app::{
    Action, App, Confirmation, Confirmed, Geometry, Notice, Operation, PaneView, RowOrder,
};
pub use collector::{Collector, CollectorConfig, RegistrySource};
pub use config::{Config, Palette};
pub use control::{
    ControlError, ControlRequest, ControlResult, NewRequest, Outcome, OwnerControl, RequestState,
};
pub use control_plane::{
    Category, Code, Daemon, Derived, ErrorBody, MAX_LINE_BYTES, PROTOCOL_VERSION, RecordState,
    Refusal, Request, RequestOutcome, RequestRecord, Response, Store, decode_request,
    install_signal_handlers, socket_path, state_dir,
};
pub use focus::Focuser;
pub use lifecycle::{
    CloseRequest, Closer, Containment, ManagedActions, ManagedRequest, Update, UpdateKind,
};
pub use model::{
    AgentObservation, BinaryFreshness, BinaryIdentity, BinaryUnknown, FleetObservation,
    ForegroundEvidence, Lineage, LocalFacts, Location, Pane, RuntimeStatus, SessionIdentity,
    SessionUuid, Tab, TerminalMode, Workspace,
};
pub use observation::{ObservationState, RetainedAgent, RetentionBasis, SourceFreshness};
pub use runtime::{CloseTarget, FocusOutcome, RuntimeProvider, Target};
pub use tmux::{TmuxConfig, TmuxRuntime};
pub use tree::{
    AgentRow, FleetTree, PaneRow, RowId, RowKind, TaskId, TaskProjection, TaskRow, TaskSource,
    TreeRow,
};
