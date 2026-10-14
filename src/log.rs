//! Where Radar's own diagnostics go.
//!
//! The daemon logs to stderr, which its systemd unit sends to the journal. The
//! dashboard owns the terminal, so it logs to a file beside the daemon's records
//! and never to the screen. Both honour `RADAR_LOG` (an `EnvFilter` such as
//! `debug` or `agent_radar=trace`) and default to `info`. Log lines carry
//! identifiers and outcomes, never a command, an environment or a recipe.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// A log that has grown past this is started afresh, so it never needs rotating.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

fn filter() -> EnvFilter {
    EnvFilter::try_from_env("RADAR_LOG").unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Logs the daemon's events to stderr, one line each with no colour, for the journal.
pub fn init_daemon() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .try_init();
}

/// The dashboard's log file: beside the daemon's state directory, not inside it.
pub fn dashboard_log_path() -> PathBuf {
    crate::control_plane::state_dir().with_file_name("tui.log")
}

/// Logs the dashboard's events to `path`. A log that cannot be opened is skipped
/// rather than failing the dashboard, and the reason is returned for one line.
pub fn init_dashboard(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    if fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        options.truncate(false);
        let _ = fs::remove_file(path);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .try_init();
    Ok(())
}
