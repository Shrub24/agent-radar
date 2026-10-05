//! Runtime observations and their fleet-overview projection.
//! Herdr wire formats stay in the connector; presentation uses normalized facts.

pub mod app;
pub mod bus;
pub mod collector;
pub mod config;
pub mod focus;
pub mod herdr;
pub mod model;
pub mod observation;
pub mod process_icons;
pub mod procfs;
pub mod runtime;
pub mod theme;
pub mod title;
pub mod tree;
pub mod ui;

pub use app::{Action, App, Geometry, PaneView, RowOrder};
pub use collector::{Collector, CollectorConfig};
pub use config::{Config, Palette};
pub use focus::Focuser;
pub use herdr::{DecodeError, HerdrConfig, HerdrRuntime, decode_process_info, decode_snapshot};
pub use model::{
    AgentObservation, FleetObservation, ForegroundEvidence, Lineage, LocalFacts, Location, Pane,
    RuntimeStatus, SessionIdentity, SessionUuid, Tab, TerminalMode, Workspace,
};
pub use observation::{ObservationState, RetainedAgent, RetentionBasis, SourceFreshness};
pub use runtime::{RuntimeProvider, Target};
pub use tree::{
    AgentRow, FleetTree, PaneRow, RowId, RowKind, TaskId, TaskProjection, TaskRow, TaskSource,
    TreeRow,
};
