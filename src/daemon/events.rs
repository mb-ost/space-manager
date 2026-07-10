//! Hyprland event listening + reconnect supervision + input (AF-1/AF-3).
//!
//! The async `AsyncEventListener` (OQ-1) runs inside the single tokio runtime,
//! wrapped in a supervisor task that never exits: it reconnects with exponential
//! backoff (250ms -> x2 -> cap 5s, reset after a run > 30s) and runs `resync()`
//! on every (re)connect. Monitor/config events feed the debounced resync
//! (Trigger A). Window/workspace events drive live tracking + overlay updates.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use hyprland::event_listener::AsyncEventListener;
use tracing::{debug, error, info, warn};

use super::recovery;
use super::Daemon;
use crate::hypr;
use crate::input::{InputListener, MouseButton};
use crate::types::{Command, ManagedWindow};

const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_millis(5000);
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);

struct Backoff {
    current: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            current: BACKOFF_MIN,
        }
    }
    fn reset(&mut self) {
        self.current = BACKOFF_MIN;
    }
    fn next(&mut self) -> Duration {
        let d = self.current;
        self.current = (self.current * 2).min(BACKOFF_MAX);
        d
    }
}

/// Spawn the reconnecting event-listener supervisor (Trigger B). Never exits.
pub fn spawn_supervisor(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        let mut backoff = Backoff::new();
        loop {
            // Run resync on every (re)connect; a failure is logged, never fatal.
            if let Err(e) = recovery::resync(&daemon).await {
                error!("resync failed (will retry on next trigger/reconnect): {}", e);
            }

            let started = Instant::now();
            match run_listener(daemon.clone()).await {
                Ok(()) => warn!("event listener returned cleanly, reconnecting"),
                Err(e) => error!("event listener error: {}, reconnecting", e),
            }

            if started.elapsed() > BACKOFF_RESET_AFTER {
                backoff.reset();
            }
            let delay = backoff.next();
            debug!("reconnecting event listener in {:?}", delay);
            tokio::time::sleep(delay).await;
        }
    });
}

/// Build a fresh listener, register handlers, and block until the socket drops.
async fn run_listener(daemon: Arc<Daemon>) -> Result<()> {
    let mut listener = AsyncEventListener::new();

    // ---- window/workspace tracking ----
    {
        let d = daemon.clone();
        listener.add_window_opened_handler(move |data| {
            let d = d.clone();
            Box::pin(async move {
                d.handle_window_open(data.window_address.to_string()).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_window_closed_handler(move |addr| {
            let d = d.clone();
            Box::pin(async move {
                d.handle_window_close(addr.to_string()).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_window_moved_handler(move |data| {
            let d = d.clone();
            Box::pin(async move {
                d.handle_window_moved(data.window_address.to_string()).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_workspace_changed_handler(move |data| {
            let d = d.clone();
            Box::pin(async move {
                d.handle_workspace_change(data.id).await;
            })
        });
    }

    // ---- recovery triggers (Trigger A): monitor / layout / config events ----
    {
        let d = daemon.clone();
        listener.add_monitor_added_handler(move |_data| {
            let d = d.clone();
            Box::pin(async move {
                info!("monitoradded -> scheduling debounced resync");
                recovery::schedule_resync(d).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_monitor_removed_handler(move |_name| {
            let d = d.clone();
            Box::pin(async move {
                info!("monitorremoved -> scheduling debounced resync");
                recovery::schedule_resync(d).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_config_reloaded_handler(move || {
            let d = d.clone();
            Box::pin(async move {
                info!("configreloaded -> scheduling debounced resync");
                recovery::schedule_resync(d).await;
            })
        });
    }
    {
        let d = daemon.clone();
        listener.add_active_monitor_changed_handler(move |_data| {
            let d = d.clone();
            Box::pin(async move {
                debug!("activemonitorchanged -> scheduling debounced resync");
                recovery::schedule_resync(d).await;
            })
        });
    }

    info!("Hyprland async event listener started");
    listener.start_listener_async().await?;
    Ok(())
}

impl Daemon {
    // ---- input listener ----

    pub fn run_input_listener(self: Arc<Self>) {
        let (listener, mut receiver) = InputListener::new();
        let listener = Arc::new(listener);
        if let Err(e) = listener.start() {
            error!("Failed to start input listener: {}", e);
            return;
        }
        info!("Input listener started");

        let daemon = self.clone();
        tokio::spawn(async move {
            while let Some(button) = receiver.recv().await {
                daemon.handle_mouse_button(button).await;
            }
        });
    }

    async fn handle_mouse_button(&self, button: MouseButton) {
        if !self.manager.is_side_mouse_binds_enabled().await {
            debug!("Side mouse binds disabled, ignoring button press");
            return;
        }
        // Next/Prev already gate on focus + mouse position, so just dispatch.
        match button {
            MouseButton::Button4 => {
                let _ = self.handle_command(Command::Prev).await;
            }
            MouseButton::Button5 => {
                let _ = self.handle_command(Command::Next).await;
            }
        }
    }

    // ---- window/workspace event handlers ----

    pub async fn handle_window_open(&self, address: String) {
        info!("Window opened: {}", address);

        let clients = match hypr::clients().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to fetch clients on window open: {}", e);
                return;
            }
        };
        let Some(client) = clients.iter().find(|c| c.address == address) else {
            debug!("Opened window not found among clients: {}", address);
            return;
        };
        let pid_value = client.pid;

        info!(
            "Window details - class: '{}', title: '{}', pid: {:?}",
            client.class, client.title, pid_value
        );

        let Some(pending) = self.launcher.match_window(&client.class, pid_value).await else {
            info!("Window not matched - no pending spawn for class '{}'", client.class);
            return;
        };
        info!("Matched spawned window: {} ({})", client.title, client.class);

        let was_reopened = if let Some(ref window_id) = pending.window_id {
            self.manager
                .open_window_by_id(
                    window_id,
                    address.clone(),
                    client.class.clone(),
                    client.title.clone(),
                    pid_value,
                )
                .await
        } else {
            self.manager
                .open_window_by_command(
                    &pending.command,
                    address.clone(),
                    client.class.clone(),
                    client.title.clone(),
                    pid_value,
                )
                .await
        };

        if was_reopened {
            info!("Reopened existing window");
            self.manager.switch_to_address(&address).await;
            if let Err(e) = self.update_visibility(&address).await {
                error!("Failed to update visibility: {}", e);
            }
            self.update_overlay().await;
            self.show_overlay().await;
            self.schedule_save_current().await;
        } else {
            let mut window = ManagedWindow::new(pending.command);
            window.open(
                address.clone(),
                client.class.clone(),
                client.title.clone(),
                pid_value,
            );
            self.manager.add_window(window).await;
            if let Err(e) = self.manager.save_state().await {
                error!("Failed to save state: {}", e);
            }
            if let Err(e) = self.update_visibility(&address).await {
                error!("Failed to update visibility: {}", e);
            }
            self.update_overlay().await;
            self.show_overlay().await;
        }
    }

    pub async fn handle_window_close(&self, address: String) {
        if *self.is_shutting_down.read().await {
            debug!("Ignoring close event during shutdown: {}", address);
            return;
        }
        info!("Window closed: {}", address);

        let windows = self.manager.get_windows().await;
        let closed_index = windows.iter().position(|w| w.address == address);
        let current_index = self.manager.get_current_index().await;

        let Some(closed_idx) = closed_index else {
            debug!("Closed window not tracked: {}", address);
            return;
        };

        info!("Marking window at index {} as closed", closed_idx);
        self.manager.mark_window_closed(&address).await;
        if let Err(e) = self.manager.save_state().await {
            error!("Failed to save state after window close: {}", e);
        }

        // Only advance if the closed window was the currently visible one.
        if closed_idx == current_index {
            let windows = self.manager.get_windows().await;
            if !windows.is_empty() {
                if let Some(next_window) = self.manager.next().await {
                    if next_window.is_open() {
                        if let Err(e) = self.update_visibility(&next_window.address).await {
                            error!("Failed to switch to next window: {}", e);
                        }
                    } else {
                        match self
                            .launcher
                            .spawn(next_window.spawn_command.clone(), Some(next_window.id.clone()))
                            .await
                        {
                            Ok(_) => info!("Spawned next window after close"),
                            Err(e) => error!("Failed to spawn next window: {}", e),
                        }
                    }
                }
            } else {
                info!("No windows remaining after close");
            }
        }

        self.update_overlay().await;
    }

    pub async fn handle_window_moved(&self, address: String) {
        if let Some(current_window) = self.manager.current_window().await {
            if current_window.address == address && current_window.is_open() {
                if let Ok(clients) = hypr::clients().await {
                    if let Some(ws) = self.window_workspace(&address, &clients).await {
                        if ws > 0 {
                            *self.current_workspace.write().await = Some(ws);
                            debug!("Updated tracked workspace to {} (window moved)", ws);
                        }
                    }
                }
                if self.is_overlay_visible().await {
                    self.refresh_overlay_position().await;
                }
            }
        }
    }

    pub async fn handle_workspace_change(&self, workspace_id: i32) {
        info!("Workspace changed to: {}", workspace_id);

        let clients = hypr::clients().await.unwrap_or_default();
        let monitors = hypr::monitors().await.unwrap_or_default();
        let visible_workspaces = Daemon::visible_workspaces(&monitors);

        let all_windows = self.manager.get_windows().await;
        let visible_window = all_windows.iter().find_map(|window| {
            if !window.is_open() {
                return None;
            }
            let ws = clients
                .iter()
                .find(|c| c.address == window.address)
                .map(|c| c.workspace_id)?;
            if visible_workspaces.contains(&ws) {
                Some((window.address.clone(), ws))
            } else {
                None
            }
        });

        let window_visible = if let Some((address, ws)) = visible_window {
            info!("Managed window visible on workspace {}", ws);
            self.manager.switch_to_address(&address).await;
            if ws > 0 {
                *self.current_workspace.write().await = Some(ws);
            }
            true
        } else {
            false
        };

        if window_visible {
            if !self.is_overlay_visible().await {
                info!("Showing overlay (window became visible)");
                self.show_overlay().await;
            }
            self.sync_current_workspace_from_current_window(&clients).await;
            self.update_overlay().await;
        } else if self.is_overlay_visible().await {
            info!("Hiding overlay (window not visible on any visible workspace)");
            self.hide_overlay().await;
        }
    }
}
