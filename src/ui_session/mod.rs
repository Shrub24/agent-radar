//! Semantic UI-session state shared by every frontend. Slice A extracts it here
//! and keeps it in-process; the daemon-hosted session builds on it.

pub mod navigation;
pub use navigation::Navigation;
