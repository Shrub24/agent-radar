//! Semantic UI-session state shared by every frontend. Slice A extracts it here
//! and keeps it in-process; the daemon-hosted session builds on it.

pub mod navigation;
pub use navigation::{NavState, NavStep, Navigation};
pub mod session;
pub use session::Session;
pub mod actor;
pub mod client;
pub mod confirm;
pub mod gate;
pub mod presentation;
pub mod publication;
pub mod targets;
