//! Command dispatch (AF-7): the IPC `handle_command` surface plus the IPC server
//! accept loop. All Hyprland access goes through `hypr::*` (AF-4).

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, error, info};

use super::Daemon;
use crate::hypr;
use crate::ipc::{IpcConnection, IpcServer};
use crate::manager::CommandTemplate;
use crate::types::{Command, ManagedWindow, Response};

impl Daemon {
    pub async fn handle_command(&self, cmd: Command) -> Response {
        match cmd {
            Command::Next => self.handle_switch(SwitchDir::Next).await,
            Command::Prev => self.handle_switch(SwitchDir::Prev).await,
            Command::Spawn(command) => {
                let _ = self.focus_current_visible().await;
                match self.launcher.spawn(command.clone(), None).await {
                    Ok(pid) => {
                        info!("Spawned process with PID: {}", pid);
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to spawn process: {}", e);
                        Response::Error(format!("Failed to spawn: {}", e))
                    }
                }
            }
            Command::SwitchTo(target_index) => {
                info!("Switching to window at index {}", target_index);
                if let Some(window) = self.manager.switch_to(target_index).await {
                    if !window.is_open() {
                        return self.spawn_closed(&window).await;
                    }
                    if let Err(e) = self.update_visibility(&window.address).await {
                        error!("Failed to update visibility: {}", e);
                        return Response::Error(format!("Failed to switch: {}", e));
                    }
                    self.update_overlay().await;
                    self.schedule_save_current().await;
                    Response::Ok
                } else {
                    Response::Error(format!("Invalid window index: {}", target_index))
                }
            }
            Command::SwapWindows(index1, index2) => {
                info!("Swapping windows at indices {} and {}", index1, index2);
                match self.manager.swap_windows(index1, index2).await {
                    Ok(_) => {
                        if let Err(e) = self.manager.save_state().await {
                            error!("Failed to save state after swap: {}", e);
                        }
                        self.update_overlay().await;
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to swap windows: {}", e);
                        Response::Error(format!("Failed to swap: {}", e))
                    }
                }
            }
            Command::SetWindowIcon(index, icon) => {
                info!("Setting icon for window {} to: {}", index, icon);
                match self.manager.set_window_icon(index, icon).await {
                    Ok(_) => {
                        if let Err(e) = self.manager.save_state().await {
                            error!("Failed to save state after icon change: {}", e);
                        }
                        self.update_overlay().await;
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to set icon: {}", e);
                        Response::Error(format!("Failed to set icon: {}", e))
                    }
                }
            }
            Command::List => Response::Windows(self.manager.get_windows().await),
            Command::Cleanup => match self.cleanup_hidden_windows().await {
                Ok(count) => {
                    info!("Closed {} hidden windows", count);
                    Response::Ok
                }
                Err(e) => {
                    error!("Failed to cleanup: {}", e);
                    Response::Error(format!("Cleanup failed: {}", e))
                }
            },
            Command::ReloadConfig => match self.manager.reload_config().await {
                Ok(_) => {
                    info!("Configuration reloaded - updating overlay");
                    self.update_overlay().await;
                    Response::Ok
                }
                Err(e) => {
                    error!("Failed to reload config: {}", e);
                    Response::Error(format!("Failed to reload config: {}", e))
                }
            },
            Command::ResetOverlayPosition => {
                // Recompute margins from live geometry and re-anchor + show.
                // Idempotent; doubles as a manual re-anchor/resync trigger.
                info!("Resetting overlay position (recompute + Reposition + Show)");
                self.refresh_overlay_position().await;
                self.show_overlay().await;
                Response::Ok
            }
            Command::GetTemplates => {
                let templates = self.manager.get_templates().await;
                let json_templates: Vec<serde_json::Value> = templates
                    .into_iter()
                    .map(|t| serde_json::json!({"name": t.name, "command": t.command}))
                    .collect();
                Response::Templates(json_templates)
            }
            Command::AddTemplate(name, command) => {
                let template = CommandTemplate { name, command };
                match self.manager.add_template(template).await {
                    Ok(_) => {
                        info!("Template added successfully");
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to add template: {}", e);
                        Response::Error(format!("Failed to add template: {}", e))
                    }
                }
            }
            Command::RemoveTemplate(name) => match self.manager.remove_template(&name).await {
                Ok(_) => {
                    info!("Template '{}' removed successfully", name);
                    Response::Ok
                }
                Err(e) => {
                    error!("Failed to remove template: {}", e);
                    Response::Error(format!("Failed to remove template: {}", e))
                }
            },
            Command::SpawnAt(index, command, icon) => {
                info!("Spawning new window at index {}: {}", index, command);
                let _ = self.focus_current_visible().await;

                let window = ManagedWindow::new(command.clone());
                let window_id = window.id.clone();
                self.manager.insert_window_at(index, window).await;

                if let Some(icon_str) = icon {
                    if let Err(e) = self.manager.set_window_icon(index, icon_str).await {
                        error!("Failed to set window icon: {}", e);
                    }
                }

                match self.launcher.spawn(command.clone(), Some(window_id)).await {
                    Ok(pid) => {
                        info!("Spawned process with PID: {}", pid);
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to spawn process: {}", e);
                        Response::Error(format!("Failed to spawn: {}", e))
                    }
                }
            }
            Command::CloseSpace(index) => self.handle_close_space(index).await,
            Command::Shutdown => {
                info!("Received Shutdown command via IPC");
                if let Err(e) = self.shutdown().await {
                    error!("Error during shutdown: {}", e);
                }
                // Give the response a moment to flush, then exit gracefully.
                tokio::spawn(async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                    std::process::exit(0);
                });
                Response::Ok
            }
        }
    }

    /// Shared Next/Prev switch logic (gated on focus + mouse edge zone).
    async fn handle_switch(&self, dir: SwitchDir) -> Response {
        let current_window = match self.manager.current_window().await {
            Some(w) if w.is_open() => w,
            _ => {
                debug!("No current window or window not open");
                return Response::Error("No active window".to_string());
            }
        };

        if !self.is_window_active(&current_window.address).await {
            debug!("Current tracked window is not active, ignoring switch");
            return Response::Error("Tracked window not focused".to_string());
        }

        match self.check_mouse_position().await {
            Ok(true) => {}
            Ok(false) => {
                debug!("Mouse not in edge zone, ignoring switch");
                return Response::Error("Mouse not in edge zone of window".to_string());
            }
            Err(e) => {
                error!("Failed to check mouse position: {}", e);
                return Response::Error(format!("Failed to check mouse position: {}", e));
            }
        }

        let window = match dir {
            SwitchDir::Next => self.manager.next().await,
            SwitchDir::Prev => self.manager.prev().await,
        };

        if let Some(window) = window {
            if !window.is_open() {
                return self.spawn_closed(&window).await;
            }
            if let Err(e) = self.update_visibility(&window.address).await {
                error!("Failed to update visibility: {}", e);
                return Response::Error(format!("Failed to switch: {}", e));
            }
            self.update_overlay().await;
            self.schedule_save_current().await;
            Response::Ok
        } else {
            Response::Error("No windows to switch to".to_string())
        }
    }

    /// Spawn a window that is currently closed (respawn on demand).
    async fn spawn_closed(&self, window: &ManagedWindow) -> Response {
        info!("Opening closed window: {}", window.spawn_command);
        match self
            .launcher
            .spawn(window.spawn_command.clone(), Some(window.id.clone()))
            .await
        {
            Ok(_) => {
                info!("Spawned closed window, waiting for it to open...");
                Response::Ok
            }
            Err(e) => {
                error!("Failed to spawn closed window: {}", e);
                Response::Error(format!("Failed to open window: {}", e))
            }
        }
    }

    async fn handle_close_space(&self, index: usize) -> Response {
        info!("Closing space at index {}", index);

        let windows = self.manager.get_windows().await;
        if index >= windows.len() {
            return Response::Error(format!("Invalid index: {}", index));
        }

        let window = &windows[index];
        let address = window.address.clone();

        if window.is_open() {
            info!("Closing window: {}", address);
            if let Err(e) = hypr::close_window(&address).await {
                error!("Failed to close window {}: {}", address, e);
            }
        }

        if self.manager.remove_window_at_index(index).await.is_some() {
            info!("Removed space at index {}", index);
            if let Err(e) = self.manager.save_state().await {
                error!("Failed to save state after closing space: {}", e);
            }

            let windows = self.manager.get_windows().await;
            if !windows.is_empty() {
                if let Some(current_window) = self.manager.current_window().await {
                    if current_window.is_open() {
                        if let Err(e) = self.update_visibility(&current_window.address).await {
                            error!("Failed to switch to current window: {}", e);
                        }
                    } else {
                        match self
                            .launcher
                            .spawn(
                                current_window.spawn_command.clone(),
                                Some(current_window.id.clone()),
                            )
                            .await
                        {
                            Ok(_) => info!("Spawned closed window after closing space"),
                            Err(e) => error!("Failed to spawn window: {}", e),
                        }
                    }
                }
            }
            self.update_overlay().await;
            Response::Ok
        } else {
            Response::Error("Failed to remove space".to_string())
        }
    }

    /// Find and close all windows parked in `special:spaces`.
    pub async fn cleanup_hidden_windows(&self) -> Result<usize> {
        info!("Cleaning up hidden windows in special:spaces");
        let clients = hypr::clients().await?;
        let mut closed_count = 0;
        for client in clients.iter() {
            if client.workspace_name == "special:spaces" {
                info!("Closing hidden window: {} ({})", client.title, client.address);
                if let Err(e) = hypr::close_window(&client.address).await {
                    error!("Failed to close hidden window {}: {}", client.address, e);
                }
                closed_count += 1;
            }
        }
        Ok(closed_count)
    }
}

enum SwitchDir {
    Next,
    Prev,
}

/// Handle a single IPC connection: one command, one response.
pub async fn handle_connection(daemon: &Daemon, conn: &mut IpcConnection) -> Result<()> {
    let command = conn.recv_command().await?;
    debug!("Received command: {:?}", command);
    let response = daemon.handle_command(command).await;
    conn.send_response(&response).await?;
    Ok(())
}

/// Run the IPC server accept loop forever.
pub async fn run_ipc_server(daemon: Arc<Daemon>) -> Result<()> {
    let server = IpcServer::new().await?;
    info!("IPC server started");
    loop {
        match server.accept().await {
            Ok(mut conn) => {
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(&daemon, &mut conn).await {
                        error!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}
