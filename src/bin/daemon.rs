use anyhow::Result;
use browser_spaces::ipc::{IpcServer, IpcConnection};
use browser_spaces::manager::SpaceManager;
use browser_spaces::overlay::OverlayManager;
use browser_spaces::process::ProcessLauncher;
use browser_spaces::types::{Command, ManagedWindow, Response};
use browser_spaces::input::{InputListener, MouseButton};
use hyprland::event_listener::EventListener;
use hyprland::shared::HyprData;
use hyprland::data::Clients;
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber;

struct Daemon {
    manager: Arc<SpaceManager>,
    launcher: Arc<ProcessLauncher>,
    overlay: Arc<OverlayManager>,
    // Shutdown flag to prevent processing close events during shutdown
    is_shutting_down: Arc<tokio::sync::RwLock<bool>>,
    // Debounce timer for saving current window (only save after 5 seconds of no changes)
    save_current_timer: Arc<tokio::sync::RwLock<Option<tokio::task::JoinHandle<()>>>>,
    // In-memory tracking of current window's workspace (NOT persisted to state.json)
    current_workspace: Arc<tokio::sync::RwLock<Option<i32>>>,
}

impl Daemon {
    fn new() -> Result<Self> {
        Ok(Self {
            manager: Arc::new(SpaceManager::new()),
            launcher: Arc::new(ProcessLauncher::new()),
            overlay: Arc::new(OverlayManager::new()?),
            is_shutting_down: Arc::new(tokio::sync::RwLock::new(false)),
            save_current_timer: Arc::new(tokio::sync::RwLock::new(None)),
            current_workspace: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Schedule saving current window after 5 seconds (debounced)
    async fn schedule_save_current(&self) {
        // Cancel existing timer if any
        let mut timer = self.save_current_timer.write().await;
        if let Some(handle) = timer.take() {
            handle.abort();
        }

        // Schedule new timer
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

    /// Check if a window is the currently active (focused) window
    async fn is_window_active(&self, address: &str) -> bool {
        let output = match std::process::Command::new("hyprctl")
            .arg("activewindow")
            .arg("-j")
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                error!("Failed to get active window: {}", e);
                return false;
            }
        };

        if let Ok(window) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            if let Some(active_address) = window["address"].as_str() {
                return active_address == address;
            }
        }

        false
    }

    /// Check if mouse is within allowed distance from configured edge of active window
    async fn check_mouse_position(&self) -> Result<bool> {
        // Get overlay config from manager (which now contains the mouse area config)
        let overlay_config = self.manager.get_overlay_config().await;

        // Get cursor position
        let cursor_output = std::process::Command::new("hyprctl")
            .arg("cursorpos")
            .arg("-j")
            .output()?;

        let cursor: serde_json::Value = serde_json::from_slice(&cursor_output.stdout)?;
        let mouse_x = cursor["x"].as_f64().unwrap_or(0.0) as i32;
        let mouse_y = cursor["y"].as_f64().unwrap_or(0.0) as i32;

        // Get active window
        let active_output = std::process::Command::new("hyprctl")
            .arg("activewindow")
            .arg("-j")
            .output()?;

        let window: serde_json::Value = serde_json::from_slice(&active_output.stdout)?;
        let window_x = window["at"][0].as_i64().unwrap_or(0) as i32;
        let window_y = window["at"][1].as_i64().unwrap_or(0) as i32;
        let window_width = window["size"][0].as_i64().unwrap_or(0) as i32;
        let window_height = window["size"][1].as_i64().unwrap_or(0) as i32;

        if window_width == 0 || window_height == 0 {
            debug!("No active window found");
            return Ok(false);
        }

        // Check mouse position based on from_area config
        let in_change_area = match overlay_config.from_area.as_str() {
            "left" => {
                let distance_from_left = mouse_x - window_x;
                let max_by_fraction = (window_width as f64 * overlay_config.change_area_fraction) as i32;
                let max_allowed = std::cmp::max(max_by_fraction, overlay_config.min_change_area_px);
                debug!("Mouse check (left): distance={}, max={}", distance_from_left, max_allowed);
                distance_from_left >= 0 && distance_from_left <= max_allowed
            }
            "right" => {
                let distance_from_right = (window_x + window_width) - mouse_x;
                let max_by_fraction = (window_width as f64 * overlay_config.change_area_fraction) as i32;
                let max_allowed = std::cmp::max(max_by_fraction, overlay_config.min_change_area_px);
                debug!("Mouse check (right): distance={}, max={}", distance_from_right, max_allowed);
                distance_from_right >= 0 && distance_from_right <= max_allowed
            }
            "top" => {
                let distance_from_top = mouse_y - window_y;
                let max_by_fraction = (window_height as f64 * overlay_config.change_area_fraction) as i32;
                let max_allowed = std::cmp::max(max_by_fraction, overlay_config.min_change_area_px);
                debug!("Mouse check (top): distance={}, max={}", distance_from_top, max_allowed);
                distance_from_top >= 0 && distance_from_top <= max_allowed
            }
            "bottom" => {
                let distance_from_bottom = (window_y + window_height) - mouse_y;
                let max_by_fraction = (window_height as f64 * overlay_config.change_area_fraction) as i32;
                let max_allowed = std::cmp::max(max_by_fraction, overlay_config.min_change_area_px);
                debug!("Mouse check (bottom): distance={}, max={}", distance_from_bottom, max_allowed);
                distance_from_bottom >= 0 && distance_from_bottom <= max_allowed
            }
            _ => {
                // Default to left for backward compatibility
                let distance_from_left = mouse_x - window_x;
                let max_by_fraction = (window_width as f64 * overlay_config.change_area_fraction) as i32;
                let max_allowed = std::cmp::max(max_by_fraction, overlay_config.min_change_area_px);
                debug!("Mouse check (default left): distance={}, max={}", distance_from_left, max_allowed);
                distance_from_left >= 0 && distance_from_left <= max_allowed
            }
        };

        Ok(in_change_area)
    }

    async fn handle_command(&self, cmd: Command) -> Response {
        match cmd {
            Command::Next => {
                // 1. Check if the current visible window is focused
                let current_window = match self.manager.current_window().await {
                    Some(w) if w.is_open() => w,
                    _ => {
                        debug!("No current window or window not open");
                        return Response::Error("No active window".to_string());
                    }
                };

                // Check if the current tracked window is the active window
                if !self.is_window_active(&current_window.address).await {
                    debug!("Current tracked window is not active, ignoring next command");
                    return Response::Error("Tracked window not focused".to_string());
                }

                // 2. Check mouse position (must be in left edge of the active window)
                match self.check_mouse_position().await {
                    Ok(true) => {
                        debug!("Mouse position OK, proceeding with next");
                    }
                    Ok(false) => {
                        debug!("Mouse not in left edge, ignoring next command");
                        return Response::Error("Mouse not in left edge of window".to_string());
                    }
                    Err(e) => {
                        error!("Failed to check mouse position: {}", e);
                        return Response::Error(format!("Failed to check mouse position: {}", e));
                    }
                }

                // 2. Update index to next
                if let Some(window) = self.manager.next().await {
                    // 3. Check if window is closed (no PID)
                    if !window.is_open() {
                        info!("Opening closed window: {}", window.spawn_command);
                        match self.launcher.spawn(window.spawn_command.clone()).await {
                            Ok(_) => {
                                // Window will be opened in handle_window_open
                                info!("Spawned closed window, waiting for it to open...");
                                /*
                                let index = self.manager.get_current_index().await + 1;
                                let total = self.manager.window_count().await;
                                self.overlay
                                    .show_hud(format!("Opening Space {} / {}", index, total))
                                    .await;
                                 */
                                return Response::Ok;
                            }
                            Err(e) => {
                                error!("Failed to spawn closed window: {}", e);
                                return Response::Error(format!("Failed to open window: {}", e));
                            }
                        }
                    }

                    // 4. Show next window, hide all others
                    if let Err(e) = self.update_visibility(&window.address).await {
                        error!("Failed to update visibility: {}", e);
                        return Response::Error(format!("Failed to switch: {}", e));
                    }

                    // Update overlay
                    self.update_overlay().await;

                    // Schedule saving current window after 5 seconds
                    self.schedule_save_current().await;

                    /*
                    let index = self.manager.get_current_index().await + 1;
                    let total = self.manager.window_count().await;
                    self.overlay
                        .show_hud(format!("Space {} / {}", index, total))
                        .await;
                     */

                    Response::Ok
                } else {
                    Response::Error("No windows to switch to".to_string())
                }
            }
            Command::Prev => {
                // 1. Check if the current visible window is focused
                let current_window = match self.manager.current_window().await {
                    Some(w) if w.is_open() => w,
                    _ => {
                        debug!("No current window or window not open");
                        return Response::Error("No active window".to_string());
                    }
                };

                // Check if the current tracked window is the active window
                if !self.is_window_active(&current_window.address).await {
                    debug!("Current tracked window is not active, ignoring prev command");
                    return Response::Error("Tracked window not focused".to_string());
                }

                // 2. Check mouse position (must be in left edge of the active window)
                match self.check_mouse_position().await {
                    Ok(true) => {
                        debug!("Mouse position OK, proceeding with prev");
                    }
                    Ok(false) => {
                        debug!("Mouse not in left edge, ignoring prev command");
                        return Response::Error("Mouse not in left edge of window".to_string());
                    }
                    Err(e) => {
                        error!("Failed to check mouse position: {}", e);
                        return Response::Error(format!("Failed to check mouse position: {}", e));
                    }
                }

                // 2. Update index to prev
                if let Some(window) = self.manager.prev().await {
                    // 3. Check if window is closed (no PID)
                    if !window.is_open() {
                        info!("Opening closed window: {}", window.spawn_command);
                        match self.launcher.spawn(window.spawn_command.clone()).await {
                            Ok(_) => {
                                // Window will be opened in handle_window_open
                                info!("Spawned closed window, waiting for it to open...");
                                /*
                                let index = self.manager.get_current_index().await + 1;
                                let total = self.manager.window_count().await;
                                self.overlay
                                    .show_hud(format!("Opening Space {} / {}", index, total))
                                    .await;
                                 */
                                return Response::Ok;
                            }
                            Err(e) => {
                                error!("Failed to spawn closed window: {}", e);
                                return Response::Error(format!("Failed to open window: {}", e));
                            }
                        }
                    }

                    // 4. Show prev window, hide all others
                    if let Err(e) = self.update_visibility(&window.address).await {
                        error!("Failed to update visibility: {}", e);
                        return Response::Error(format!("Failed to switch: {}", e));
                    }

                    // Update overlay
                    self.update_overlay().await;

                    // Schedule saving current window after 5 seconds
                    self.schedule_save_current().await;

                    /*
                    let index = self.manager.get_current_index().await + 1;
                    let total = self.manager.window_count().await;
                    self.overlay
                        .show_hud(format!("Space {} / {}", index, total))
                        .await;
                     */

                    Response::Ok
                } else {
                    Response::Error("No windows to switch to".to_string())
                }
            }
            Command::Spawn(command) => {
                // 1. Focus current visible window BEFORE spawning
                let _ = self.focus_current_visible().await;

                // 2. Now spawn the process (new window will open next to focused window)
                match self.launcher.spawn(command.clone()).await {
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
            Command::List => {
                let windows = self.manager.get_windows().await;
                Response::Windows(windows)
            }
            Command::Cleanup => {
                match self.cleanup_hidden_windows().await {
                    Ok(count) => {
                        info!("Closed {} hidden windows", count);
                        Response::Ok
                    }
                    Err(e) => {
                        error!("Failed to cleanup: {}", e);
                        Response::Error(format!("Cleanup failed: {}", e))
                    }
                }
            }
        }
    }

    /// Focus the currently visible tracked window
    async fn focus_current_visible(&self) -> Result<()> {
        if let Some(current) = self.manager.current_window().await {
            debug!("Focusing current visible: {}", current.address);
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("focuswindow")
                .arg(format!("address:{}", current.address))
                .output();

            // Give Hyprland time to process the focus change
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Get the workspace ID of a window by its address
    fn get_window_workspace(&self, address: &str) -> Option<i32> {
        if let Ok(clients) = Clients::get() {
            if let Some(client) = clients.iter().find(|c| c.address.to_string() == address) {
                return Some(client.workspace.id);
            }
        }
        None
    }

    /// Update visibility: show target window, hide all others
    async fn update_visibility(&self, target_address: &str) -> Result<()> {
        info!("Updating visibility: showing {}", target_address);

        // Get all tracked windows
        let all_windows = self.manager.get_windows().await;

        // Determine target workspace BEFORE hiding any windows
        // Try to get workspace from current_workspace in memory, or find any window that's NOT in the special workspace
        let target_workspace = {
            let current_ws = self.current_workspace.read().await;
            if let Some(ws) = *current_ws {
                info!("Using tracked workspace from memory: {}", ws);
                ws
            } else {
                // Fallback: Find any window that's NOT in the special workspace (workspace ID < 0 means special)
                all_windows
                    .iter()
                    .filter_map(|win| {
                        let ws = self.get_window_workspace(&win.address)?;
                        if ws > 0 {
                            Some(ws)
                        } else {
                            None
                        }
                    })
                    .next()
                    .unwrap_or(1) // Default to workspace 1 if all windows are in special workspace
            }
        };

        info!("Target workspace: {}", target_workspace);

        // FIRST: Hide all other tracked windows BEFORE showing target
        for win in all_windows.iter() {
            if win.address != target_address {
                info!("Hiding window: {}", win.address);
                let output = std::process::Command::new("hyprctl")
                    .arg("dispatch")
                    .arg("movetoworkspacesilent")
                    .arg(format!("special:spaces,address:{}", win.address))
                    .output()?;

                if !output.status.success() {
                    error!("Failed to hide window {}: {}", win.address, String::from_utf8_lossy(&output.stderr));
                }
            }
        }

        // Small delay to ensure hides are processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // THEN: Show the target window by moving it to the target workspace
        info!("Showing window: {}", target_address);

        // Save current cursor position
        let cursor_pos = std::process::Command::new("hyprctl")
            .arg("cursorpos")
            .arg("-j")
            .output()
            .ok()
            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
            .and_then(|cursor| {
                let x = cursor["x"].as_f64()?;
                let y = cursor["y"].as_f64()?;
                Some((x as i32, y as i32))
            });

        // Move to the target workspace (not silent, so it shows)
        let output = std::process::Command::new("hyprctl")
            .arg("dispatch")
            .arg("movetoworkspace")
            .arg(format!("{},address:{}", target_workspace, target_address))
            .output()?;

        if !output.status.success() {
            error!("Failed to move window to workspace: {}", String::from_utf8_lossy(&output.stderr));
        }

        // Small delay after showing
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Restore cursor position
        if let Some((x, y)) = cursor_pos {
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("movecursor")
                .arg(format!("{} {}", x, y))
                .output();
        }
        // Update tracked workspace in memory
        *self.current_workspace.write().await = Some(target_workspace);
        info!("Tracked current workspace: {}", target_workspace);

        Ok(())
    }

    async fn handle_window_open(&self, address: String) {
        info!("Window opened: {}", address);

        // Get window details from Hyprland
        if let Ok(clients) = Clients::get() {
            if let Some(client) = clients.iter().find(|c| c.address.to_string() == address) {
                // pid is i32, convert to u32
                let pid_value = if client.pid >= 0 {
                    Some(client.pid as u32)
                } else {
                    None
                };

                info!("Window details - class: '{}', title: '{}', pid: {:?}",
                      client.class, client.title, pid_value);

                // Check if this window matches a pending spawn
                if let Some(pending) = self.launcher.match_window(&client.class, pid_value).await {
                    info!("✓ Matched spawned window: {} ({})", client.title, client.class);

                    // Check if this is opening an existing closed window
                    let was_reopened = self.manager.open_window_by_command(
                        &pending.command,
                        address.clone(),
                        client.class.clone(),
                        client.title.clone(),
                        pid_value,
                    ).await;

                    if was_reopened {
                        info!("✓ Reopened existing window");

                        // Show the reopened window
                        if let Err(e) = self.update_visibility(&address).await {
                            error!("Failed to update visibility: {}", e);
                        }

                        // Update overlay
                        self.update_overlay().await;

                        // Schedule saving current window after 5 seconds
                        self.schedule_save_current().await;

                        /*
                        let index = self.manager.get_current_index().await + 1;
                        let total = self.manager.window_count().await;
                        self.overlay
                            .show_hud(format!("Space {} / {}", index, total))
                            .await;
                         */
                    } else {
                        // This is a brand new window - create with command
                        let mut window = ManagedWindow::new(pending.command);
                        window.open(address.clone(), client.class.clone(), client.title.clone(), pid_value);

                        // Add new window and make it current
                        self.manager.add_window(window).await;

                        // Save state after adding window
                        if let Err(e) = self.manager.save_state().await {
                            error!("Failed to save state: {}", e);
                        }

                        // Show new window, hide all others
                        if let Err(e) = self.update_visibility(&address).await {
                            error!("Failed to update visibility: {}", e);
                        }

                        // Update overlay
                        self.update_overlay().await;

                        /*
                        let total = self.manager.window_count().await;
                        self.overlay
                            .show_hud(format!("New space {} / {}", total, total))
                            .await;
                         */
                    }
                } else {
                    info!("✗ Window not matched - no pending spawn for class: '{}'", client.class);
                }
            }
        }
    }

    async fn handle_window_close(&self, address: String) {
        // Don't process close events during shutdown
        if *self.is_shutting_down.read().await {
            debug!("Ignoring close event during shutdown: {}", address);
            return;
        }

        info!("Window closed: {}", address);

        // Remove window entirely from the list
        if let Some(_closed_index) = self.manager.remove_window_by_address(&address).await {
            info!("Removed closed window from list");

            // Save state after removal
            if let Err(e) = self.manager.save_state().await {
                error!("Failed to save state after window close: {}", e);
            }

            // Switch to next window (open or spawn closed one in same workspace)
            let windows = self.manager.get_windows().await;

            if !windows.is_empty() {
                if let Some(next_window) = self.manager.next().await {
                    if next_window.is_open() {
                        info!("Auto-switching to next open window");
                        if let Err(e) = self.update_visibility(&next_window.address).await {
                            error!("Failed to switch to next window: {}", e);
                        }
                        // Update overlay
                        self.update_overlay().await;
                    } else {
                        // Next window is closed - spawn it in the same workspace as the closed window
                        info!("Next window is closed, spawning in same workspace");

                        // The current_workspace should still have the workspace of the closed window
                        let workspace = *self.current_workspace.read().await;
                        info!("Spawning in workspace: {:?}", workspace);

                        // Spawn the command - it will inherit the workspace because we track it in memory
                        match self.launcher.spawn(next_window.spawn_command.clone()).await {
                            Ok(_) => {
                                info!("Spawned closed window in same workspace");
                            }
                            Err(e) => {
                                error!("Failed to spawn closed window: {}", e);
                            }
                        }
                    }
                }
            } else {
                info!("No windows remaining after close");
            }
        } else {
            debug!("Closed window not tracked: {}", address);
        }
    }

    async fn handle_window_moved(&self, address: String) {
        // Check if this is a tracked window
        let current = self.manager.current_window().await;

        // Only track workspace if this is the current visible window
        if let Some(current_window) = current {
            if current_window.address == address && current_window.is_open() {
                // Update the tracked workspace
                if let Some(workspace) = self.get_window_workspace(&address) {
                    *self.current_workspace.write().await = Some(workspace);
                    debug!("Updated tracked workspace to {} (window moved)", workspace);
                }
            }
        }
    }

    /// Update the persistent overlay if enabled
    async fn update_overlay(&self) {
        info!("update_overlay called");
        let overlay_config = self.manager.get_overlay_config().await;
        info!("Overlay config: enabled={}, from_area={}, from_overlay={}",
              overlay_config.enabled, overlay_config.from_area, overlay_config.from_overlay);

        if !overlay_config.enabled {
            info!("Overlay disabled in config");
            return;
        }

        let current_index = self.manager.get_current_index().await;
        let total = self.manager.window_count().await;

        info!("Updating overlay: {} / {}", current_index + 1, total);

        if total > 0 {
            // Spawn overlay update in background to avoid blocking
            let overlay = self.overlay.clone();
            let from = overlay_config.from_overlay.clone();
            let offset_x = overlay_config.offset_x;
            let offset_y = overlay_config.offset_y;
            let overlay_size = overlay_config.overlay_size.clone();
            let change_area_fraction = overlay_config.change_area_fraction;
            let min_change_area_px = overlay_config.min_change_area_px;
            let from_area = overlay_config.from_area.clone();

            info!("Spawning overlay task");
            tokio::spawn(async move {
                info!("Overlay task started");
                overlay.show_spaces_indicator(
                    current_index,
                    total,
                    &from,
                    offset_x,
                    offset_y,
                    &overlay_size,
                    change_area_fraction,
                    min_change_area_px,
                    &from_area
                ).await;
                info!("Overlay task finished");
            });
        } else {
            info!("No windows to show overlay for");
        }
    }

    async fn handle_mouse_button(&self, button: MouseButton) {
        // Check if side mouse binds are enabled
        if !self.manager.is_side_mouse_binds_enabled().await {
            debug!("Side mouse binds disabled, ignoring button press");
            return;
        }

        // Check if the current visible window is focused
        let current_window = match self.manager.current_window().await {
            Some(w) if w.is_open() => w,
            _ => {
                debug!("No current window or window not open");
                return;
            }
        };

        // Check if the current tracked window is the active window
        if !self.is_window_active(&current_window.address).await {
            debug!("Current tracked window is not active, ignoring mouse button");
            return;
        }

        // Check mouse position (must be in left edge of the active window)
        match self.check_mouse_position().await {
            Ok(true) => {
                debug!("Mouse position OK, handling button press");
            }
            Ok(false) => {
                debug!("Mouse not in left edge, ignoring button press");
                return;
            }
            Err(e) => {
                error!("Failed to check mouse position: {}", e);
                return;
            }
        }

        // Handle the button press
        match button {
            MouseButton::Button4 => {
                debug!("Handling Button4 (prev) via input listener");
                let _ = self.handle_command(Command::Prev).await;
            }
            MouseButton::Button5 => {
                debug!("Handling Button5 (next) via input listener");
                let _ = self.handle_command(Command::Next).await;
            }
        }
    }

    fn run_input_listener(self: Arc<Self>) {
        let (listener, mut receiver) = InputListener::new();
        let listener = Arc::new(listener);

        // Start the input listener
        if let Err(e) = listener.start() {
            error!("Failed to start input listener: {}", e);
            return;
        }

        info!("Input listener started");

        // Handle mouse button events
        let daemon = self.clone();
        tokio::spawn(async move {
            while let Some(button) = receiver.recv().await {
                daemon.handle_mouse_button(button).await;
            }
        });
    }

    fn run_event_listener(self: Arc<Self>) {
        let daemon = self.clone();
        let daemon2 = self.clone();
        let daemon3 = self.clone();

        std::thread::spawn(move || {
            let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());

            let mut listener = EventListener::new();

            listener.add_window_opened_handler({
                let daemon = daemon.clone();
                let runtime = runtime.clone();
                move |data| {
                    // data is WindowOpenEvent
                    let address = data.window_address.to_string();
                    let daemon = daemon.clone();
                    runtime.spawn(async move {
                        daemon.handle_window_open(address).await;
                    });
                }
            });

            listener.add_window_closed_handler({
                let daemon = daemon2.clone();
                let runtime = runtime.clone();
                move |data| {
                    // data is WindowCloseEvent (just an Address)
                    let address = data.to_string();
                    let daemon = daemon.clone();
                    runtime.spawn(async move {
                        daemon.handle_window_close(address).await;
                    });
                }
            });

            listener.add_window_moved_handler({
                let daemon = daemon3.clone();
                let runtime = runtime.clone();
                move |data| {
                    // data is WindowMoveEvent
                    let address = data.window_address.to_string();
                    let daemon = daemon.clone();
                    runtime.spawn(async move {
                        daemon.handle_window_moved(address).await;
                    });
                }
            });

            info!("Hyprland event listener started");
            if let Err(e) = listener.start_listener() {
                error!("Event listener error: {}", e);
            }
        });
    }

    async fn run_ipc_server(self: Arc<Self>) -> Result<()> {
        let server = IpcServer::new().await?;
        info!("IPC server started");

        loop {
            match server.accept().await {
                Ok(mut conn) => {
                    let daemon = self.clone();
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

    /// Find and close all windows in special:spaces workspace
    async fn cleanup_hidden_windows(&self) -> Result<usize> {
        info!("Cleaning up hidden windows in special:spaces");

        let clients = Clients::get()?;
        let mut closed_count = 0;

        for client in clients.iter() {
            // Check if window is in special:spaces workspace
            if client.workspace.name == "special:spaces" {
                info!("Closing hidden window: {} ({})", client.title, client.address);
                let _ = std::process::Command::new("hyprctl")
                    .arg("dispatch")
                    .arg("closewindow")
                    .arg(format!("address:{}", client.address))
                    .output();
                closed_count += 1;
            }
        }

        Ok(closed_count)
    }

    /// Gracefully close all tracked windows and save state on shutdown
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Space Manager - saving state and closing windows");

        // Set shutdown flag to prevent close events from modifying state
        *self.is_shutting_down.write().await = true;

        // Save state FIRST before closing windows (so we can restore on restart)
        if let Err(e) = self.manager.save_state().await {
            error!("Failed to save state on shutdown: {}", e);
        }

        let all_windows = self.manager.get_windows().await;

        // Close all tracked windows gracefully
        for window in all_windows.iter() {
            info!("Closing tracked window: {}", window.address);
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("closewindow")
                .arg(format!("address:{}", window.address))
                .output();
        }

        info!("All tracked windows closed, state saved for restore");
        Ok(())
    }
}

async fn handle_connection(daemon: &Daemon, conn: &mut IpcConnection) -> Result<()> {
    let command = conn.recv_command().await?;
    debug!("Received command: {:?}", command);

    let response = daemon.handle_command(command).await;
    conn.send_response(&response).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("Starting Space Manager daemon");

    let daemon = Arc::new(Daemon::new()?);

    // Load saved state (all windows loaded as closed - PID will be None)
    if let Err(e) = daemon.manager.load_state().await {
        error!("Failed to load state: {}", e);
    } else {
        let saved_windows = daemon.manager.get_windows().await;
        if !saved_windows.is_empty() {
            info!("Loaded {} windows from previous session (all closed)", saved_windows.len());

            // Only restore the current window immediately, others stay closed
            let current_index = daemon.manager.get_current_index().await;
            if current_index < saved_windows.len() {
                let command = &saved_windows[current_index].spawn_command;
                info!("Restoring current window ({}): {}", current_index, command);
                match daemon.launcher.spawn(command.clone()).await {
                    Ok(pid) => {
                        info!("Restored current window with PID: {}", pid);
                    }
                    Err(e) => {
                        error!("Failed to restore current window: {}", e);
                    }
                }
            }

            info!("{} other windows will be opened on-demand", saved_windows.len() - 1);
        }
    }

    // Start Hyprland event listener
    daemon.clone().run_event_listener();

    // Start input listener for mouse buttons
    daemon.clone().run_input_listener();

    // Set up signal handlers for graceful shutdown
    let daemon_for_shutdown = daemon.clone();
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM handler");
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to create SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT");
            }
        }

        if let Err(e) = daemon_for_shutdown.shutdown().await {
            error!("Error during shutdown: {}", e);
        }
        std::process::exit(0);
    });

    // Start IPC server (this will run indefinitely)
    daemon.run_ipc_server().await?;

    Ok(())
}
