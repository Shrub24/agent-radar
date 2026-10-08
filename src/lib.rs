//! Runtime observations and their fleet-overview projection.
//! Herdr wire formats stay in the connector; presentation uses normalized facts.

pub mod app;
pub mod bus;
pub mod collector;
pub mod config;
pub mod control;
pub mod control_plane;
pub mod focus;
pub mod herdr;
pub mod lifecycle;
pub mod model;
pub mod observation;
pub mod process_icons;
pub mod procfs;
pub mod runtime;
pub mod theme;
pub mod title;
pub mod tree;
pub mod ui;

pub use app::{
    Action, App, Confirmation, Confirmed, Geometry, Notice, Operation, PaneView, RowOrder,
};
pub use collector::{Collector, CollectorConfig};
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
pub use herdr::{DecodeError, HerdrConfig, HerdrRuntime, decode_process_info, decode_snapshot};
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
pub use tree::{
    AgentRow, FleetTree, PaneRow, RowId, RowKind, TaskId, TaskProjection, TaskRow, TaskSource,
    TreeRow,
};
