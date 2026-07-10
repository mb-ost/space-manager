//! Startup / restore / shutdown / signal handling (AF-7).

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info};

use super::{commands, events, recovery, Daemon};
use crate::hypr;
use crate::overlay::OverlayMsg;

impl Daemon {
    /// Load saved state and respawn the previously-current window on demand.
    pub async fn restore_state(&self) {
        if let Err(e) = self.manager.load_state().await {
            error!("Failed to load state: {}", e);
            return;
        }

        let saved_windows = self.manager.get_windows().await;
        if saved_windows.is_empty() {
            return;
        }
        info!(
            "Loaded {} windows from previous session (all closed)",
            saved_windows.len()
        );

        let current_index = self.manager.get_current_index().await;
        if let Some(window) = saved_windows.get(current_index) {
            info!(
                "Restoring current window ({}): {}",
                current_index, window.spawn_command
            );
            match self
                .launcher
                .spawn(window.spawn_command.clone(), Some(window.id.clone()))
                .await
            {
                Ok(pid) => info!("Restored current window with PID: {}", pid),
                Err(e) => error!("Failed to restore current window: {}", e),
            }
        }
        info!(
            "{} other windows will be opened on-demand",
            saved_windows.len().saturating_sub(1)
        );
    }

    /// Gracefully close all tracked windows, save state, and quit the overlay.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Space Manager - saving state and closing windows");
        *self.is_shutting_down.write().await = true;

        if let Err(e) = self.manager.save_state().await {
            error!("Failed to save state on shutdown: {}", e);
        }

        let all_windows = self.manager.get_windows().await;
        for window in all_windows.iter() {
            if window.is_open() {
                info!("Closing tracked window: {}", window.address);
                if let Err(e) = hypr::close_window(&window.address).await {
                    error!("Failed to close window {}: {}", window.address, e);
                }
            }
        }

        // Tell the GTK thread to quit.
        self.overlay.send(OverlayMsg::Shutdown);

        info!("All tracked windows closed, state saved for restore");
        Ok(())
    }

    /// Start all subsystems and block on the IPC server (runs forever).
    pub async fn run(self: Arc<Self>) -> Result<()> {
        // Trigger B: reconnecting event-listener supervisor (also runs initial resync).
        // Started before restoring state so the window-open event for the
        // respawned current window is not missed.
        events::spawn_supervisor(self.clone());
        // Input (side mouse buttons).
        self.clone().run_input_listener();
        // Trigger C: periodic consistency check.
        recovery::spawn_consistency_check(self.clone());

        // Load saved state and respawn the previously-current window.
        self.restore_state().await;

        // Signal handling for graceful shutdown.
        let daemon_for_shutdown = self.clone();
        tokio::spawn(async move {
            match install_signal_handlers().await {
                Ok(sig) => info!("Received {}", sig),
                Err(e) => {
                    error!("Signal handler setup failed: {}", e);
                    return;
                }
            }
            if let Err(e) = daemon_for_shutdown.shutdown().await {
                error!("Error during shutdown: {}", e);
            }
            std::process::exit(0);
        });

        commands::run_ipc_server(self).await
    }
}

/// Wait for SIGTERM/SIGINT; returns the received signal name. No `expect`.
async fn install_signal_handlers() -> Result<&'static str> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let name = tokio::select! {
        _ = sigterm.recv() => "SIGTERM",
        _ = sigint.recv() => "SIGINT",
    };
    Ok(name)
}
