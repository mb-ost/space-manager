//! Space Manager daemon entry point (thin).
//!
//! Parses nothing (no CLI surface), initializes file logging + panic hook, starts
//! the single GTK overlay thread, builds the `Daemon`, and blocks on its run loop.
//! All daemon logic lives in the `space_manager::daemon` library modules.

use std::sync::Arc;

use anyhow::Result;
use space_manager::daemon::Daemon;
use space_manager::logging;
use space_manager::overlay::bar;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // File logging + panic hook; keep the guard alive for the process lifetime.
    let _log_guard = logging::init();
    info!("Starting Space Manager daemon");

    // Single-instance guard: a second launch must not steal the IPC socket
    // from a live daemon (the keybind may fire while one is already running).
    if space_manager::ipc::daemon_already_running().await {
        info!("Another space-manager daemon is already running; exiting");
        return Ok(());
    }

    // Start the single, long-lived GTK overlay thread and get its handle.
    let overlay = bar::start();

    // Build the daemon and run all subsystems (blocks on the IPC server).
    let daemon = Arc::new(Daemon::new(overlay)?);
    daemon.run().await
}
