//! Upgrade signaling — primitives for the monitor/agent pipe protocol.
//!
//! The monitor sets `MAGE_AGENT_PIPE_FD` (a temp file path) before spawning
//! the agent. The agent writes the new binary path to it and exits 42.

use std::path::Path;

/// Env var pointing to the upgrade pipe (temp file path).
pub const PIPE_ENV: &str = "MAGE_AGENT_PIPE_FD";

/// Exit code that tells the monitor to upgrade.
pub const UPGRADE_EXIT_CODE: i32 = 42;

/// Check if we're running under a monitor (pipe env is set).
pub fn is_agent_mode() -> bool {
    std::env::var(PIPE_ENV).is_ok()
}

// ---------------------------------------------------------------------------
// Embedded snapshot data
// ---------------------------------------------------------------------------

thread_local! {
    static SNAPSHOT_DATA: std::cell::Cell<&'static [u8]> = const { std::cell::Cell::new(&[]) };
}

/// Register the embedded snapshot data. Called once at startup from the
/// generated main.rs.
pub fn set_snapshot(data: &'static [u8]) {
    SNAPSHOT_DATA.with(|cell| cell.set(data));
}

/// Get the embedded snapshot data.
pub fn get_snapshot() -> &'static [u8] {
    SNAPSHOT_DATA.with(|cell| cell.get())
}

/// Outcome of an upgrade signal attempt.
pub enum UpgradeSignal {
    /// Path written to pipe. Caller should exit with code 42.
    Ready,
    /// Not running under a monitor. Binary was compiled but can't hot-swap.
    NoMonitor,
}

/// Write the new binary path to the upgrade pipe.
///
/// Returns `Ready` if the pipe was written (caller should exit 42),
/// or `NoMonitor` if there's no monitor to receive it.
/// Returns `Err` on I/O failure.
pub fn signal_upgrade(new_binary_path: &Path) -> Result<UpgradeSignal, String> {
    let pipe_path = match std::env::var(PIPE_ENV) {
        Ok(p) => p,
        Err(_) => return Ok(UpgradeSignal::NoMonitor),
    };

    std::fs::write(&pipe_path, format!("{}\n", new_binary_path.display()))
        .map_err(|e| format!("failed to write upgrade path to {pipe_path}: {e}"))?;

    Ok(UpgradeSignal::Ready)
}
