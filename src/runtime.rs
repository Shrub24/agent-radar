//! The runtime seam: what Radar asks of the terminal multiplexer it reads and
//! acts on.
//!
//! Both workers reach a runtime through this one small interface, in Radar's
//! own normalized values. Collection reads a [`FleetObservation`] and
//! [`ForegroundEvidence`]; focus is asked to move to a [`Target`]. Nothing on
//! either path names a runtime's executable, CLI grammar, JSON shapes or
//! socket; those live in the adapter ([`crate::herdr`]), the only module that
//! sees them. A second runtime is a second adapter at assembly time, not a
//! change to the collector, the focuser, the reconciler or the view.
//!
//! Properties every implementation owes its caller:
//!
//! - **Cancellation is prompt.** `cancel` is set when Radar is shutting down.
//!   An implementation must abandon an outstanding command and return without
//!   waiting out its timeout, and must leave no child process behind.
//! - **Unreadable evidence is not disappearance.** A pane whose foreground
//!   query fails, times out or cannot be decoded is
//!   [`ForegroundEvidence::Inconclusive`], never a failed refresh and never
//!   proof that the pane went away.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::model::{FleetObservation, ForegroundEvidence};

/// Where `Enter` wants a runtime's focus moved: an existing normalized
/// location the view already produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A pane row: its workspace, tab and pane in one request.
    Pane(String),
    /// A workspace row.
    Workspace(String),
}

/// What a lifecycle action asks a runtime to close: an existing normalized
/// location. A runtime that cannot close refuses explicitly through [`Err`];
/// the operator flow is the same either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseTarget {
    Pane(String),
    Tab(String),
}

impl CloseTarget {
    /// What the target is, for a one-line message: `pane wA:p1`.
    pub fn description(&self) -> String {
        match self {
            Self::Pane(pane_id) => format!("pane {pane_id}"),
            Self::Tab(tab_id) => format!("tab {tab_id}"),
        }
    }
}

/// What Radar asks of one runtime: its facts and the one action on it.
pub trait RuntimeProvider: Send + Sync {
    /// The whole normalized inventory, or a user-facing diagnostic.
    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String>;

    /// Foreground evidence for one pane, inconclusive when unreadable.
    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence;

    /// Moves the runtime's focus to `target`.
    ///
    /// `Err` carries the one-line refusal or failure a user reads. Like every
    /// operation here, a cancelled request must abandon an outstanding
    /// command or exchange, return without waiting out its deadline, and leave
    /// no child process behind.
    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String>;

    /// Closes `target` through the runtime, for a location Radar has positive
    /// evidence is unmanaged. An adapter that cannot close returns an explicit
    /// refusal; nothing here may kill a process directly.
    ///
    /// Same cancellation duty as [`Self::focus`]. A close already handed to the
    /// runtime is not retracted by cancelling: cancellation only abandons a
    /// request still waiting.
    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String>;
}

/// A shared handle is a provider too, so the collector and focuser can own the
/// seam while their caller keeps one adapter handle.
impl<T: RuntimeProvider + ?Sized> RuntimeProvider for Arc<T> {
    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        (**self).inventory(cancel)
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        (**self).foreground_evidence(pane_id, cancel)
    }

    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        (**self).focus(target, cancel)
    }

    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        (**self).close(target, cancel)
    }
}
