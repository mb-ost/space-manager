//! Daemon subsystem (AF-7 split).
//!
//! The daemon owns the single tokio runtime and coordinates: command handling
//! (`commands`), Hyprland event listening + reconnect supervision (`events`),
//! state resync/recovery (`recovery`), managed-window visibility (`visibility`),
//! and lifecycle (`lifecycle`). Pure sub-logic (`rematch`,
//! `visibility::resolve_target_workspace`) is unit-tested.

pub mod commands;
pub mod events;
pub mod lifecycle;
pub mod recovery;
pub mod rematch;
pub mod visibility;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::geometry::{self, Anchor, Rect, OVERLAY_HEIGHT};
use crate::hypr::{self, ClientInfo, MonitorInfo};
use crate::manager::SpaceManager;
use crate::overlay::{OverlayHandle, OverlayMsg};
use crate::process::ProcessLauncher;

/// Geometry the last overlay `Reposition` was computed against; used by the 30s
/// periodic consistency check (Trigger C) to detect drift cheaply.
#[derive(Debug, Clone)]
pub struct RepositionState {
    pub tracked_addr: String,
    pub monitor_name: String,
    pub monitor_rect: Rect,
}

/// The daemon: shared state behind `Arc`s so subsystems can run concurrently.
pub struct Daemon {
    pub manager: Arc<SpaceManager>,
    pub launcher: Arc<ProcessLauncher>,
    pub overlay: OverlayHandle,
    /// Suppress close-event processing during shutdown.
    pub is_shutting_down: Arc<RwLock<bool>>,
    /// Debounced state save handle.
    pub save_current_timer: Arc<RwLock<Option<JoinHandle<()>>>>,
    /// In-memory tracker of the current window's workspace (not persisted).
    pub current_workspace: Arc<RwLock<Option<i32>>>,
    /// Serializes visibility updates to avoid overlap during rapid switching.
    pub visibility_lock: Arc<Mutex<()>>,
    /// Daemon-side view of whether the overlay is shown.
    pub overlay_visible: Arc<RwLock<bool>>,
    /// Coalesces concurrent resync triggers into one running resync.
    pub resync_lock: Arc<Mutex<()>>,
    /// True if a trigger fired while a resync was in progress (schedules a re-run).
    pub resync_pending: Arc<AtomicBool>,
    /// Debounce handle for Trigger A (monitor/config events).
    pub resync_debounce: Arc<RwLock<Option<JoinHandle<()>>>>,
    /// Geometry the last Reposition used (for the periodic drift check).
    pub last_reposition: Arc<RwLock<Option<RepositionState>>>,
}

impl Daemon {
    pub fn new(overlay: OverlayHandle) -> Result<Self> {
        Ok(Self {
            manager: Arc::new(SpaceManager::new()),
            launcher: Arc::new(ProcessLauncher::new()),
            overlay,
            is_shutting_down: Arc::new(RwLock::new(false)),
            save_current_timer: Arc::new(RwLock::new(None)),
            current_workspace: Arc::new(RwLock::new(None)),
            visibility_lock: Arc::new(Mutex::new(())),
            overlay_visible: Arc::new(RwLock::new(false)),
            resync_lock: Arc::new(Mutex::new(())),
            resync_pending: Arc::new(AtomicBool::new(false)),
            resync_debounce: Arc::new(RwLock::new(None)),
            last_reposition: Arc::new(RwLock::new(None)),
        })
    }

    // ---- small shared helpers ----

    /// Schedule saving current window after 5 seconds (debounced).
    pub async fn schedule_save_current(&self) {
        let mut timer = self.save_current_timer.write().await;
        if let Some(handle) = timer.take() {
            handle.abort();
        }
        let manager = self.manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            if let Err(e) = manager.save_state().await {
                error!("Failed to save current window state: {}", e);
            } else {
                info!("Saved current window state (debounced)");
            }
        });
        *timer = Some(handle);
    }

    /// Is the given address the currently focused window?
    pub async fn is_window_active(&self, address: &str) -> bool {
        match hypr::active_window().await {
            Ok(Some(w)) => w.address == address,
            Ok(None) => false,
            Err(e) => {
                error!("Failed to get active window: {}", e);
                false
            }
        }
    }

    /// Is the cursor within the configured edge zone of the active window?
    pub async fn check_mouse_position(&self) -> Result<bool> {
        let overlay_config = self.manager.get_overlay_config().await;
        let (mx, my) = hypr::cursor_position().await?;
        let active = hypr::active_window().await?;
        let Some(win) = active else {
            debug!("No active window found");
            return Ok(false);
        };
        let rect = Rect::new(win.at.0, win.at.1, win.size.0, win.size.1);
        Ok(geometry::in_edge_zone(
            rect,
            (mx, my),
            &overlay_config.from_area,
            overlay_config.change_area_fraction,
            overlay_config.min_change_area_px,
        ))
    }

    /// Focus the currently visible tracked window (best effort).
    pub async fn focus_current_visible(&self) -> Result<()> {
        if let Some(current) = self.manager.current_window().await.filter(|w| w.is_open()) {
            debug!("Focusing current visible: {}", current.address);
            if let Err(e) = hypr::focus_window(&current.address).await {
                error!("Failed to focus current window: {}", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Workspace id of a window by address, if found among live clients.
    pub async fn window_workspace(&self, address: &str, clients: &[ClientInfo]) -> Option<i32> {
        clients
            .iter()
            .find(|c| c.address == address)
            .map(|c| c.workspace_id)
    }

    /// All currently-visible workspace ids across monitors.
    pub fn visible_workspaces(monitors: &[MonitorInfo]) -> Vec<i32> {
        monitors.iter().map(|m| m.active_workspace_id).collect()
    }

    /// Keep the in-memory workspace tracker aligned with the visible managed window.
    pub async fn sync_current_workspace_from_current_window(&self, clients: &[ClientInfo]) {
        if let Some(current) = self.manager.current_window().await.filter(|w| w.is_open()) {
            if let Some(ws) = self.window_workspace(&current.address, clients).await {
                if ws > 0 {
                    *self.current_workspace.write().await = Some(ws);
                    debug!("Synced current workspace from current window: {}", ws);
                }
            }
        }
    }

    // ---- overlay control ----

    /// Send `Show` to the overlay (idempotent) and record the daemon-side state.
    pub async fn show_overlay(&self) {
        self.overlay.send(OverlayMsg::Show);
        *self.overlay_visible.write().await = true;
    }

    /// Send `Hide` to the overlay (idempotent) and record the daemon-side state.
    pub async fn hide_overlay(&self) {
        self.overlay.send(OverlayMsg::Hide);
        *self.overlay_visible.write().await = false;
    }

    pub async fn is_overlay_visible(&self) -> bool {
        *self.overlay_visible.read().await
    }

    /// Push the current spaces model + position to the overlay.
    ///
    /// Sends `UpdateSpaces` always, and `Reposition` (+ records geometry for the
    /// periodic drift check) when an open tracked window is found. Does not force
    /// Show/Hide — visibility is driven by workspace events.
    pub async fn update_overlay(&self) {
        let overlay_config = self.manager.get_overlay_config().await;
        if !overlay_config.enabled {
            debug!("Overlay disabled in config; hiding");
            self.hide_overlay().await;
            return;
        }

        let windows = self.manager.get_windows().await;
        let current_index = self.manager.get_current_index().await;
        let (spaces, current) = crate::overlay::model::build_spaces(&windows, current_index);
        self.overlay.send(OverlayMsg::UpdateSpaces { spaces, current });

        // Reposition from live geometry.
        if let Ok(clients) = hypr::clients().await {
            if let Ok(monitors) = hypr::monitors().await {
                self.reposition_overlay(&clients, &monitors).await;
            }
        }
    }

    /// Compute overlay anchor/margins from live geometry and send `Reposition`.
    /// Returns true if a reposition was sent.
    pub async fn reposition_overlay(
        &self,
        clients: &[ClientInfo],
        monitors: &[MonitorInfo],
    ) -> bool {
        let overlay_config = self.manager.get_overlay_config().await;
        let Some(tracked) = self
            .manager
            .current_window()
            .await
            .filter(|w| w.is_open())
            .map(|w| w.address.clone())
        else {
            debug!("No open tracked window; skipping reposition");
            return false;
        };

        let Some(client) = clients.iter().find(|c| c.address == tracked) else {
            debug!("Tracked window not found among clients; skipping reposition");
            return false;
        };

        let win_rect = Rect::new(client.at.0, client.at.1, client.size.0, client.size.1);

        // Monitor containing the window's top-left corner (fallback: first).
        let mon = monitors
            .iter()
            .find(|m| {
                win_rect.x >= m.x
                    && win_rect.x < m.x + m.width
                    && win_rect.y >= m.y
                    && win_rect.y < m.y + m.height
            })
            .or_else(|| monitors.first());
        let Some(mon) = mon else {
            debug!("No monitors available; skipping reposition");
            return false;
        };
        let mon_rect = Rect::new(mon.x, mon.y, mon.width, mon.height);

        let width = geometry::overlay_width(
            win_rect,
            &overlay_config.overlay_size,
            &overlay_config.from_area,
            overlay_config.change_area_fraction,
            overlay_config.min_change_area_px,
            overlay_config.offset_x,
        );
        let (anchor, margin_x, margin_y): (Anchor, i32, i32) = geometry::compute_anchor_margins(
            win_rect,
            mon_rect,
            &overlay_config.from_overlay,
            overlay_config.offset_x,
            overlay_config.offset_y,
            width,
            OVERLAY_HEIGHT,
        );

        self.overlay.send(OverlayMsg::Reposition {
            anchor,
            margin_x,
            margin_y,
            width,
            monitor: mon.name.clone(),
        });

        *self.last_reposition.write().await = Some(RepositionState {
            tracked_addr: tracked,
            monitor_name: mon.name.clone(),
            monitor_rect: mon_rect,
        });
        true
    }

    /// Reposition using freshly fetched geometry (best effort).
    pub async fn refresh_overlay_position(&self) {
        if let Ok(clients) = hypr::clients().await {
            if let Ok(monitors) = hypr::monitors().await {
                self.reposition_overlay(&clients, &monitors).await;
            }
        }
    }
}

// Re-export for convenience.
pub use commands::{handle_connection, run_ipc_server};
