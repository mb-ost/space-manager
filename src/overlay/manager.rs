use crate::hypr_settings::FollowMouseGuard;
use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, ComboBoxText, Entry, Grid,
    Label, Orientation, ScrolledWindow, Window,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::dialog_utils;
use super::ipc_helpers;
use super::theme;
use super::ui_components;
use super::window_utils;

pub struct OverlayManager {
    label_text: Arc<RwLock<String>>,
    pub window_created: Arc<RwLock<bool>>,
    overlay_visible: Arc<RwLock<bool>>,
    saved_position: Arc<RwLock<Option<(i32, i32)>>>,
    // Store the current overlay config so we can detect changes
    current_config: Arc<RwLock<Option<OverlayConfig>>>,
}

#[derive(Clone, Debug)]
struct OverlayConfig {
    from: String,
    offset_x: i32,
    offset_y: i32,
    overlay_size: String,
    change_area_fraction: f64,
    min_change_area_px: i32,
    from_area: String,
}

impl OverlayManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            label_text: Arc::new(RwLock::new(String::new())),
            window_created: Arc::new(RwLock::new(false)),
            overlay_visible: Arc::new(RwLock::new(true)),
            saved_position: Arc::new(RwLock::new(None)),
            current_config: Arc::new(RwLock::new(None)),
        })
    }

    /// Show persistent space indicator overlay (1-2-3-[4]-5-6)
    pub async fn show_spaces_indicator(
        &self,
        current: usize,
        total: usize,
        windows: &[crate::types::ManagedWindow],
        from: &str,
        offset_x: i32,
        offset_y: i32,
        overlay_size: &str,
        change_area_fraction: f64,
        min_change_area_px: i32,
        from_area: &str,
        tracked_window_address: Option<&str>,
    ) {
        info!("show_spaces_indicator called: current={}, total={}, from={}, offset=({}, {}), overlay_size={}", current, total, from, offset_x, offset_y, overlay_size);

        // Create config for this call
        let new_config = OverlayConfig {
            from: from.to_string(),
            offset_x,
            offset_y,
            overlay_size: overlay_size.to_string(),
            change_area_fraction,
            min_change_area_px,
            from_area: from_area.to_string(),
        };

        // Check if config has changed
        let config_changed = {
            let current_config = self.current_config.read().await;
            if let Some(old_config) = &*current_config {
                old_config.from != new_config.from
                    || old_config.offset_x != new_config.offset_x
                    || old_config.offset_y != new_config.offset_y
                    || old_config.overlay_size != new_config.overlay_size
                    || (old_config.change_area_fraction - new_config.change_area_fraction).abs()
                        > 0.001
                    || old_config.min_change_area_px != new_config.min_change_area_px
                    || old_config.from_area != new_config.from_area
            } else {
                false
            }
        };

        // Check if window is already created. If Hyprland dropped the surface during a
        // display reset, the boolean can be stale, so verify the actual window exists.
        let mut created = *self.window_created.read().await;
        if created && self.get_overlay_window_address().is_none() {
            info!("Overlay window missing, recreating it");
            *self.window_created.write().await = false;
            created = false;
        }

        if config_changed && created {
            info!("Overlay config changed, calling resize_and_reposition_overlay");
            self.resize_and_reposition_overlay(
                from,
                offset_x,
                offset_y,
                overlay_size,
                change_area_fraction,
                min_change_area_px,
                from_area,
                tracked_window_address,
            )
            .await;
        }

        // Store the new config
        *self.current_config.write().await = Some(new_config);

        // Generate the indicator text like "1-2-3-[4]-5-6" or with custom icons
        let text = self.generate_indicator_text(current, windows);
        info!("Spaces indicator text: {}", text);

        // Update the label text
        *self.label_text.write().await = text.clone();

        // Check if window needs to be created
        if !created {
            info!("Creating new GTK overlay window");
            self.spawn_gtk_window(
                from,
                offset_x,
                offset_y,
                overlay_size,
                change_area_fraction,
                min_change_area_px,
                from_area,
            )
            .await;
            *self.window_created.write().await = true;
        } else {
            info!("Overlay window already exists, text will update on next refresh");
        }

        info!("show_overlay_window completed");
    }

    /// Hide the persistent overlay
    pub async fn hide_overlay(&self) {
        info!("Hiding overlay window");

        // Save current position before hiding
        if let Some((x, y)) = get_overlay_window_position() {
            *self.saved_position.write().await = Some((x, y));
            info!("Saved overlay position: ({}, {})", x, y);
        }

        if let Some(address) = self.get_overlay_window_address() {
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("pin")
                .arg(format!("address:{}", address))
                .output();

            info!("Unpinned overlay");

            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("movetoworkspacesilent")
                .arg(format!("special:spaces,address:{}", address))
                .output();

            info!("Moved overlay to special:spaces");
        } else {
            error!("Could not find overlay window to hide");
        }

        *self.overlay_visible.write().await = false;
    }

    /// Show the persistent overlay (restore from hidden state)
    pub async fn show_overlay(&self) {
        info!("Showing overlay window");

        // Get the workspace where space manager windows are visible
        // We'll move it to the active workspace first
        let active_workspace = get_active_workspace().unwrap_or(1);

        if let Some(address) = self.get_overlay_window_address() {
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("movetoworkspacesilent")
                .arg(format!("{},address:{}", active_workspace, address))
                .output();

            info!("Moved overlay to workspace {}", active_workspace);

            tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

            if let Some((x, y)) = *self.saved_position.read().await {
                info!("Restoring overlay position: ({}, {})", x, y);
                let _ = std::process::Command::new("hyprctl")
                    .arg("dispatch")
                    .arg("movewindowpixel")
                    .arg(format!("exact {} {},address:{}", x, y, address))
                    .output();
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("pin")
                .arg(format!("address:{}", address))
                .output();

            info!("Pinned overlay");
            *self.overlay_visible.write().await = true;
        } else {
            error!("Could not find overlay window to show");
            *self.overlay_visible.write().await = false;
        }
    }

    /// Check if overlay is currently visible
    pub async fn is_overlay_visible(&self) -> bool {
        *self.overlay_visible.read().await
    }

    /// Get the overlay window's address
    fn get_overlay_window_address(&self) -> Option<String> {
        let output = std::process::Command::new("hyprctl")
            .arg("clients")
            .arg("-j")
            .output()
            .ok()?;

        let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

        if let Some(clients_array) = clients.as_array() {
            for client in clients_array {
                if let Some(title) = client["title"].as_str() {
                    if title == "Space Manager Overlay" {
                        return client["address"].as_str().map(|s| s.to_string());
                    }
                }
            }
        }
        None
    }

    /// Get the overlay window's current size
    fn get_overlay_window_size(&self) -> Option<(i32, i32)> {
        let output = std::process::Command::new("hyprctl")
            .arg("clients")
            .arg("-j")
            .output()
            .ok()?;

        let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

        if let Some(clients_array) = clients.as_array() {
            for client in clients_array {
                if let Some(title) = client["title"].as_str() {
                    if title == "Space Manager Overlay" {
                        let w = client["size"][0].as_i64()? as i32;
                        let h = client["size"][1].as_i64()? as i32;
                        return Some((w, h));
                    }
                }
            }
        }
        None
    }

    /// Resize and reposition the overlay window based on new settings
    pub async fn resize_and_reposition_overlay(
        &self,
        from: &str,
        offset_x: i32,
        offset_y: i32,
        overlay_size: &str,
        change_area_fraction: f64,
        min_change_area_px: i32,
        from_area: &str,
        tracked_window_address: Option<&str>,
    ) {
        info!("resize_and_reposition_overlay called");

        // Get geometry of the tracked window
        let tracked_window_geom = tracked_window_address.and_then(|addr| {
            let output = std::process::Command::new("hyprctl")
                .arg("clients")
                .arg("-j")
                .output()
                .ok()?;

            let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

            if let Some(clients_array) = clients.as_array() {
                for client in clients_array {
                    let client_addr = client["address"].as_str()?;
                    if client_addr == addr {
                        let x = client["at"][0].as_i64()? as i32;
                        let y = client["at"][1].as_i64()? as i32;
                        let width = client["size"][0].as_i64()? as i32;
                        let height = client["size"][1].as_i64()? as i32;
                        return Some((x, y, width, height));
                    }
                }
            }
            None
        });

        if let Some((win_x, win_y, win_width, win_height)) = tracked_window_geom {
            // Calculate new overlay width
            let overlay_width = match overlay_size {
                "change_area_x" => {
                    let dimension = if from_area == "left" || from_area == "right" {
                        win_width
                    } else {
                        win_height
                    };
                    let zone_fraction = (dimension as f64 * change_area_fraction) as i32;
                    let zone_size = std::cmp::max(zone_fraction, min_change_area_px);
                    zone_size - (2 * offset_x)
                }
                "change_area_y" => {
                    let dimension = if from_area == "left" || from_area == "right" {
                        win_height
                    } else {
                        win_width
                    };
                    let zone_fraction = (dimension as f64 * change_area_fraction) as i32;
                    let zone_size = std::cmp::max(zone_fraction, min_change_area_px);
                    zone_size - (2 * offset_x)
                }
                fixed_size => fixed_size.parse::<i32>().unwrap_or(250),
            };
            let overlay_height = 36;

            // Calculate new position
            let (pos_x, pos_y) = match from {
                "bot_left" => (
                    win_x + offset_x,
                    win_y + win_height - overlay_height - offset_y,
                ),
                "bot_right" => (
                    win_x + win_width - overlay_width - offset_x,
                    win_y + win_height - overlay_height - offset_y,
                ),
                "top_left" => (win_x + offset_x, win_y + offset_y),
                "top_right" => (
                    win_x + win_width - overlay_width - offset_x,
                    win_y + offset_y,
                ),
                _ => (
                    win_x + offset_x,
                    win_y + win_height - overlay_height - offset_y,
                ),
            };

            info!(
                "Target: {}x{} at ({}, {})",
                overlay_width, overlay_height, pos_x, pos_y
            );

            // Get overlay address
            if let Some(addr) = self.get_overlay_window_address() {
                info!("Overlay address: {}", addr);

                // Suppress follow_mouse while the overlay surface is being moved so the cursor
                // doesn't retarget focus during the address-based repositioning sequence.
                let _follow_mouse_guard = FollowMouseGuard::suppress();

                // Disable cursor warping to prevent cursor from moving to focused window
                let cursor_no_warps_output = std::process::Command::new("hyprctl")
                    .arg("getoption")
                    .arg("cursor:no_warps")
                    .arg("-j")
                    .output()
                    .ok();

                let original_no_warps = cursor_no_warps_output
                    .and_then(|output| {
                        serde_json::from_slice::<serde_json::Value>(&output.stdout).ok()
                    })
                    .and_then(|json| json["int"].as_i64())
                    .unwrap_or(0);

                // Enable no_warps (1 = cursor doesn't warp to focused windows)
                let _ = std::process::Command::new("hyprctl")
                    .arg("keyword")
                    .arg("cursor:no_warps")
                    .arg("1")
                    .output();

                // Get current size and resize if needed
                if let Some((current_w, current_h)) = self.get_overlay_window_size() {
                    let delta_w = overlay_width - current_w;
                    let delta_h = overlay_height - current_h;

                    info!(
                        "Current: {}x{}, delta: {}x{}",
                        current_w, current_h, delta_w, delta_h
                    );

                    if delta_w != 0 || delta_h != 0 {
                        info!(
                            "Resizing overlay from {}x{} to {}x{}",
                            current_w, current_h, overlay_width, overlay_height
                        );

                        // Resize using resizewindowpixel with address selector (doesn't require focus)
                        let resize_cmd = format!(
                            "exact {} {},address:{}",
                            overlay_width, overlay_height, addr
                        );
                        info!(
                            "Executing: hyprctl dispatch resizewindowpixel {}",
                            resize_cmd
                        );

                        let output = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("resizewindowpixel")
                            .arg(&resize_cmd)
                            .output();

                        if let Ok(out) = output {
                            info!("Resize status: {}", out.status);
                            if !out.stderr.is_empty() {
                                error!("Resize error: {}", String::from_utf8_lossy(&out.stderr));
                            }
                        }

                        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
                    }
                }

                // Move to position (doesn't require focus - uses address selector)
                let move_cmd = format!("exact {} {},address:{}", pos_x, pos_y, addr);
                info!("Executing: hyprctl dispatch movewindowpixel {}", move_cmd);

                let output = std::process::Command::new("hyprctl")
                    .arg("dispatch")
                    .arg("movewindowpixel")
                    .arg(&move_cmd)
                    .output();

                if let Ok(out) = output {
                    info!("Move status: {}", out.status);
                    if !out.stderr.is_empty() {
                        error!("Move error: {}", String::from_utf8_lossy(&out.stderr));
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

                // Restore cursor:no_warps setting
                let _ = std::process::Command::new("hyprctl")
                    .arg("keyword")
                    .arg("cursor:no_warps")
                    .arg(format!("{}", original_no_warps))
                    .output();

                info!("Resize and reposition complete");
            } else {
                error!("Could not find overlay window");
            }
        } else {
            error!("Could not find tracked window");
        }
    }

    /// Hide the persistent overlay (legacy method - kept for compatibility)
    pub async fn hide_spaces_indicator(&self) {
        self.hide_overlay().await;
    }

    fn generate_indicator_text(
        &self,
        current: usize,
        windows: &[crate::types::ManagedWindow],
    ) -> String {
        let mut parts = Vec::new();
        for (i, window) in windows.iter().enumerate() {
            let label = window
                .custom_icon
                .as_ref()
                .map(|s| s.clone())
                .unwrap_or_else(|| (i + 1).to_string());

            if i == current {
                parts.push(format!("[{}]", label));
            } else {
                parts.push(label);
            }
        }
        parts.join("-")
    }

    async fn spawn_gtk_window(
        &self,
        from: &str,
        offset_x: i32,
        offset_y: i32,
        overlay_size: &str,
        change_area_fraction: f64,
        min_change_area_px: i32,
        from_area: &str,
    ) {
        let label_text = self.label_text.clone();
        let from = from.to_string();
        let overlay_size = overlay_size.to_string();
        let from_area = from_area.to_string();

        // Spawn GTK in a separate thread
        std::thread::spawn(move || {
            // Set float rule so overlay doesn't tile using new Hyprland 0.53.0 syntax
            let _ = std::process::Command::new("hyprctl")
                .arg("keyword")
                .arg("windowrule")
                .arg("float on, match:class com.spacermanager.overlay")
                .output();

            // Prevent overlay from stealing focus on launch
            let _ = std::process::Command::new("hyprctl")
                .arg("keyword")
                .arg("windowrule")
                .arg("nofocus on, match:class com.spacermanager.overlay")
                .output();

            info!("Float and nofocus rules added for overlay");

            // Wait to ensure rules are registered before creating window
            std::thread::sleep(std::time::Duration::from_millis(100));

            let app = Application::builder()
                .application_id("com.spacermanager.overlay")
                .build();

            let from_clone = from.clone();
            let overlay_size_clone = overlay_size.clone();
            let from_area_clone = from_area.clone();
            app.connect_activate(move |app| {
                // Get active window geometry NOW, when we're creating the GTK window
                let (win_x, win_y, win_width, win_height) = if let Some(geom) = get_active_window_geometry() {
                    info!("Active window geometry: x={}, y={}, w={}, h={}", geom.0, geom.1, geom.2, geom.3);
                    geom
                } else {
                    info!("No active window found, using monitor dimensions");
                    let (screen_width, screen_height) = get_monitor_size();
                    (0, 0, screen_width, screen_height)
                };

                // Calculate overlay width based on overlay_size config
                let overlay_width = match overlay_size_clone.as_str() {
                    "change_area_x" => {
                        // Calculate mouse zone width (same logic as in daemon)
                        let dimension = if from_area_clone == "left" || from_area_clone == "right" {
                            win_width
                        } else {
                            win_height
                        };
                        let zone_fraction = (dimension as f64 * change_area_fraction) as i32;
                        let zone_size = std::cmp::max(zone_fraction, min_change_area_px);
                        // Subtract margins
                        zone_size - (2 * offset_x)
                    }
                    "change_area_y" => {
                        // Use the perpendicular dimension
                        let dimension = if from_area_clone == "left" || from_area_clone == "right" {
                            win_height
                        } else {
                            win_width
                        };
                        let zone_fraction = (dimension as f64 * change_area_fraction) as i32;
                        let zone_size = std::cmp::max(zone_fraction, min_change_area_px);
                        // Subtract margins
                        zone_size - (2 * offset_x)
                    }
                    fixed_size => {
                        // Parse as fixed pixel value
                        fixed_size.parse::<i32>().unwrap_or(250)
                    }
                };
                let overlay_height = 36;

                info!("Calculated overlay dimensions: {}x{} (overlay_size={}, offset_x={})", overlay_width, overlay_height, overlay_size_clone, offset_x);

                let window = ApplicationWindow::builder()
                    .application(app)
                    .title("Space Manager Overlay")
                    .default_width(overlay_width)
                    .default_height(overlay_height)
                    .decorated(false)
                    .resizable(true)  // Must be true to allow Hyprland to resize it
                    .build();

                // Calculate and apply position after window is mapped
                let from_clone2 = from_clone.clone();
                let overlay_width_clone = overlay_width;
                let overlay_height_clone = overlay_height;
                window.connect_map(move |_win| {
                    // Now calculate position and move the window via Hyprland
                    let (pos_x, pos_y) = match from_clone2.as_str() {
                        "bot_left" => (win_x + offset_x, win_y + win_height - overlay_height_clone - offset_y),
                        "bot_right" => (win_x + win_width - overlay_width_clone - offset_x, win_y + win_height - overlay_height_clone - offset_y),
                        "top_left" => (win_x + offset_x, win_y + offset_y),
                        "top_right" => (win_x + win_width - overlay_width_clone - offset_x, win_y + offset_y),
                        _ => (win_x + offset_x, win_y + win_height - overlay_height_clone - offset_y),
                    };

                    info!("Window mapped! Moving to position: ({}, {})", pos_x, pos_y);

                    // Move and resize window using hyprctl
                    std::thread::spawn(move || {
                        // Wait for Hyprland to register the window before moving it
                        let max_attempts = 50; // 50 * 20ms = 1 second max wait
                        let mut overlay_address = None;
                        for attempt in 0..max_attempts {
                            if let Ok(output) = std::process::Command::new("hyprctl")
                                .arg("clients")
                                .arg("-j")
                                .output()
                            {
                                if let Ok(clients) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                                    if let Some(clients_array) = clients.as_array() {
                                        overlay_address = clients_array.iter().find_map(|client| {
                                            if client["title"].as_str() == Some("Space Manager Overlay") {
                                                client["address"].as_str().map(|address| address.to_string())
                                            } else {
                                                None
                                            }
                                        });
                                    }
                                }
                            }
                            if overlay_address.is_some() {
                                info!("Hyprland registered overlay window after {}ms", attempt * 20);
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }

                        let Some(overlay_address) = overlay_address else {
                            error!("Timeout waiting for Hyprland to register overlay window");
                            return;
                        };

                        // First, set the exact size for the floating window
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("resizewindowpixel")
                            .arg(format!("exact {} {},address:{}", overlay_width_clone, overlay_height_clone, overlay_address))
                            .output();

                        info!("Window resized to {}x{}", overlay_width_clone, overlay_height_clone);

                        // Small delay between operations
                        std::thread::sleep(std::time::Duration::from_millis(30));

                        // Position the window
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("movewindowpixel")
                            .arg(format!("exact {} {},address:{}", pos_x, pos_y, overlay_address))
                            .output();

                        info!("Window moved to ({}, {})", pos_x, pos_y);

                        // Small delay before pinning
                        std::thread::sleep(std::time::Duration::from_millis(30));

                        // Pin the overlay so it appears on all workspaces
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("pin")
                            .arg(format!("address:{}", overlay_address))
                            .output();

                        info!("Overlay pinned");
                    });
                });

                // Create horizontal box to hold label and buttons
                let hbox = GtkBox::new(Orientation::Horizontal, 6);
                hbox.set_margin_start(8);
                hbox.set_margin_end(8);
                hbox.set_margin_top(4);
                hbox.set_margin_bottom(4);

                // Create hamburger menu button (⋮)
                let menu_button = gtk4::MenuButton::new();
                menu_button.set_icon_name("open-menu-symbolic");
                menu_button.set_width_request(28);
                menu_button.set_height_request(28);
                menu_button.add_css_class("menu-button");
                menu_button.set_cursor_from_name(Some("pointer"));

                // Create popover menu
                let menu = gtk4::gio::Menu::new();
                menu.append(Some("New Space..."), Some("app.new_space"));
                menu.append(Some("Reset Position"), Some("app.reset_position"));
                menu.append(Some("Settings"), Some("app.settings"));

                let popover = gtk4::PopoverMenu::builder()
                    .menu_model(&menu)
                    .has_arrow(false)
                    .build();

                menu_button.set_popover(Some(&popover));

                // Add "New Space" action
                let new_space_action = gtk4::gio::SimpleAction::new("new_space", None);
                new_space_action.connect_activate(move |_, _| {
                    info!("New Space clicked, opening window");
                    show_new_space_window();
                });
                app.add_action(&new_space_action);

                // Add "Reset Position" action
                let reset_position_action = gtk4::gio::SimpleAction::new("reset_position", None);
                reset_position_action.connect_activate(move |_, _| {
                    info!("Reset Position clicked");
                    ipc_helpers::reset_overlay_position();
                });
                app.add_action(&reset_position_action);

                // Add settings action
                let app_clone = app.clone();
                let settings_action = gtk4::gio::SimpleAction::new("settings", None);
                settings_action.connect_activate(move |_, _| {
                    info!("Settings menu item clicked, opening settings dialog");
                    show_settings_dialog(&app_clone);
                });
                app.add_action(&settings_action);


                // Create horizontal box for space buttons
                let spaces_box = GtkBox::new(Orientation::Horizontal, 4);
                spaces_box.set_halign(gtk4::Align::Center);

                // Wrap the spaces box in a scrolled window for horizontal scrolling
                let scrolled_window = gtk4::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk4::PolicyType::External)  // External means we handle scrolling, no space reserved
                    .vscrollbar_policy(gtk4::PolicyType::Never)
                    .hexpand(true)
                    .propagate_natural_width(false)  // Don't propagate width - allow scrolling
                    .kinetic_scrolling(true)  // Enable smooth kinetic scrolling
                    .has_frame(false)  // No frame
                    .build();
                scrolled_window.set_overlay_scrolling(true);  // Overlay scrolling doesn't take layout space
                scrolled_window.set_child(Some(&spaces_box));

                // We need to get the label text synchronously, so we'll use a blocking approach
                // In GTK context, we can use the current text value
                let text = {
                    // Read initial text - this is in the GTK activation context
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        label_text.read().await.clone()
                    })
                };

                // Extract parts and create initial buttons
                let parts: Vec<&str> = text.split('-').collect();
                let total_spaces = parts.len();

                for (index, part) in parts.iter().enumerate() {
                    let part_str: &str = *part;
                    let is_current = part_str.contains('[');
                    let space_num = part_str.replace('[', "").replace(']', "");

                    let space_button = ui_components::create_space_button(index, space_num, is_current, total_spaces);
                    spaces_box.append(&space_button);
                }

                // Auto-scroll to current tab on initial creation
                if let Some(current_index) = parts.iter().position(|p| p.contains('[')) {
                    let scrolled_window_for_init_scroll = scrolled_window.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                        let adj = scrolled_window_for_init_scroll.hadjustment();
                        let button_width = 32.0; // Approximate button width including margins
                        let viewport_width = adj.page_size();

                        // Calculate position to show current button with 1 button context before it if possible
                        let ideal_start_index = if current_index > 0 {
                            current_index - 1
                        } else {
                            0
                        };

                        let ideal_start = ideal_start_index as f64 * button_width;

                        // Clamp to valid range
                        let max_scroll = (total_spaces as f64 * button_width - viewport_width).max(0.0);
                        let target_pos = ideal_start.min(max_scroll).max(0.0);

                        adj.set_value(target_pos);
                        info!("Initial auto-scroll to position {} for tab {}", target_pos, current_index);
                    });
                }

                // Create close button (X)
                let close_button = Button::with_label("✕");
                close_button.set_width_request(28);
                close_button.set_height_request(28);
                close_button.add_css_class("close-button");
                close_button.set_cursor_from_name(Some("pointer"));

                // Connect close button to kill the daemon
                close_button.connect_clicked(|_| {
                    info!("Close button clicked, shutting down space manager");
                    // Send kill signal to the daemon
                    let _ = std::process::Command::new("pkill")
                        .arg("-x")
                        .arg("space-manager")
                        .output();
                    std::process::exit(0);
                });

                hbox.append(&menu_button);
                hbox.append(&scrolled_window);
                hbox.append(&close_button);

                // Add scroll event controller to the scrolled window for scrolling anywhere
                let scroll_controller = gtk4::EventControllerScroll::new(
                    gtk4::EventControllerScrollFlags::BOTH_AXES
                );
                let scrolled_window_for_scroll = scrolled_window.clone();
                scroll_controller.connect_scroll(move |_, dx, dy| {
                    let adj = scrolled_window_for_scroll.hadjustment();
                    let current = adj.value();
                    let step = 10.0; // Smaller step for smooth scrolling
                    // Use both dx (horizontal scroll) and dy (vertical scroll/wheel)
                    // Most mice scroll vertically, so we want vertical scrolling to scroll horizontally
                    adj.set_value(current + (dy * step) + (dx * step));
                    glib::Propagation::Stop
                });
                scrolled_window.add_controller(scroll_controller);

                // Update buttons periodically when text changes
                let spaces_box_clone = spaces_box.clone();
                let scrolled_window_clone = scrolled_window.clone();
                let label_text_clone = label_text.clone();
                let mut last_text = text.clone();

                glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                    let current_text = glib::MainContext::default().block_on(async {
                        label_text_clone.read().await.clone()
                    });

                    // Only update if text has changed
                    if current_text != last_text {
                        // Remove all existing buttons
                        while let Some(child) = spaces_box_clone.first_child() {
                            spaces_box_clone.remove(&child);
                        }

                        // Recreate buttons with new state
                        let parts: Vec<&str> = current_text.split('-').collect();
                        let total_spaces = parts.len();

                        for (index, part) in parts.iter().enumerate() {
                            let part_str: &str = *part;
                            let is_current = part_str.contains('[');
                            let space_num = part_str.replace('[', "").replace(']', "");

                            let space_button = Button::builder()
                                .label(&space_num)
                                .width_request(28)
                                .height_request(28)
                                .build();

                            if is_current {
                                space_button.add_css_class("space-button-current");
                            } else {
                                space_button.add_css_class("space-button");
                            }

                            // Connect left-click handler
                            let target_index = index;
                            let space_num_clone = space_num.clone();
                            space_button.connect_clicked(move |_| {
                                info!("Space button {} clicked, switching to space {}", space_num_clone, target_index);
                                ipc_helpers::switch_to_space(target_index);
                            });

                            // Add right-click context menu
                            let gesture = gtk4::GestureClick::new();
                            gesture.set_button(3); // Right mouse button

                            let space_button_clone = space_button.clone();
                            let context_index = index;
                            gesture.connect_pressed(move |gesture, _, _x, _y| {
                                info!("Right-click on space button {}", context_index);

                                // Create context menu
                                let popover = gtk4::Popover::new();
                                popover.set_has_arrow(false);
                                popover.set_parent(&space_button_clone);

                                let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

                                // Move Left option
                                if context_index > 0 {
                                    let move_left_btn = Button::with_label("← Move Left");
                                    move_left_btn.add_css_class("context-menu-item");
                                    move_left_btn.set_cursor_from_name(Some("pointer"));

                                    let popover_clone = popover.clone();
                                    move_left_btn.connect_clicked(move |_| {
                                        info!("Move left clicked for index {}", context_index);
                                        popover_clone.popdown();
                                        ipc_helpers::swap_windows(context_index, context_index - 1);
                                    });
                                    menu_box.append(&move_left_btn);
                                }

                                // Move Right option
                                if context_index < total_spaces - 1 {
                                    let move_right_btn = Button::with_label("Move Right →");
                                    move_right_btn.add_css_class("context-menu-item");
                                    move_right_btn.set_cursor_from_name(Some("pointer"));

                                    let popover_clone = popover.clone();
                                    move_right_btn.connect_clicked(move |_| {
                                        info!("Move right clicked for index {}", context_index);
                                        popover_clone.popdown();
                                        ipc_helpers::swap_windows(context_index, context_index + 1);
                                    });
                                    menu_box.append(&move_right_btn);
                                }

                                // Change Icon option
                                let change_icon_btn = Button::with_label("✏ Change Icon");
                                change_icon_btn.add_css_class("context-menu-item");
                                change_icon_btn.set_cursor_from_name(Some("pointer"));

                                let popover_clone = popover.clone();
                                change_icon_btn.connect_clicked(move |_| {
                                    info!("Change icon clicked for index {}", context_index);
                                    popover_clone.popdown();

                                    // Create dialog to get new icon
                                    let dialog = gtk4::Window::builder()
                                        .title("Change Space Icon")
                                        .default_width(300)
                                        .default_height(150)
                                        .modal(true)
                                        .build();

                                    // Add float and center rules with explicit size enforcement
                                    window_utils::apply_float_center_with_size("Change Space Icon", 300, 150);

                                    // Apply centralized theme
                                    theme::apply_template_window_theme(&dialog);

                                    let vbox = dialog_utils::create_standard_container();

                                    let label = gtk4::Label::new(Some("Enter icon (emoji or text):"));
                                    let entry = gtk4::Entry::new();
                                    entry.set_placeholder_text(Some("e.g. 🌐 or Web"));

                                    let button_box = dialog_utils::create_button_box();

                                    let cancel_btn = dialog_utils::create_cancel_button();
                                    let dialog_clone = dialog.clone();
                                    cancel_btn.connect_clicked(move |_| {
                                        dialog_clone.close();
                                    });

                                    let ok_btn = dialog_utils::create_action_button("OK");
                                    let entry_clone = entry.clone();
                                    let dialog_clone2 = dialog.clone();
                                    ok_btn.connect_clicked(move |_| {
                                        let new_icon = entry_clone.text().to_string();
                                        dialog_clone2.close();
                                        ipc_helpers::set_window_icon(context_index, new_icon);
                                    });

                                    button_box.append(&cancel_btn);
                                    button_box.append(&ok_btn);

                                    vbox.append(&label);
                                    vbox.append(&entry);
                                    vbox.append(&button_box);

                                    dialog.set_child(Some(&vbox));
                                    dialog.present();
                                });
                                menu_box.append(&change_icon_btn);

                                // Close Space option
                                let close_space_btn = Button::with_label("✕ Close Space");
                                close_space_btn.add_css_class("context-menu-item");
                                close_space_btn.add_css_class("destructive-action");
                                close_space_btn.set_cursor_from_name(Some("pointer"));

                                let popover_clone = popover.clone();
                                close_space_btn.connect_clicked(move |_| {
                                    info!("Close space clicked for index {}", context_index);
                                    popover_clone.popdown();
                                    ipc_helpers::close_space(context_index);
                                });
                                menu_box.append(&close_space_btn);

                                popover.set_child(Some(&menu_box));
                                popover.popup();

                                gesture.set_state(gtk4::EventSequenceState::Claimed);
                            });

                            space_button.add_controller(gesture);
                            space_button.set_cursor_from_name(Some("pointer"));
                            spaces_box_clone.append(&space_button);
                        }

                        // Auto-scroll to keep the current button visible with context
                        if let Some(current_index) = parts.iter().position(|p| p.contains('[')) {
                            dialog_utils::auto_scroll_to_item(&scrolled_window_clone, current_index, total_spaces, 32.0);
                        }

                        last_text = current_text;
                    }

                    glib::ControlFlow::Continue
                });

                // Add CSS styling
                let css_provider = gtk4::CssProvider::new();
                css_provider.load_from_data(
                    "window {
                        background-color: rgba(30, 30, 30, 0.95);
                        border-radius: 6px;
                    }
                    scrolledwindow {
                        background: transparent;
                        border: none;
                    }
                    scrolledwindow > scrollbar {
                        background: transparent;
                        border: none;
                        min-width: 0px;
                        min-height: 0px;
                        opacity: 0;
                    }
                    scrolledwindow > scrollbar > slider {
                        background: transparent;
                        border-radius: 2px;
                        min-width: 0px;
                        min-height: 0px;
                        opacity: 0;
                    }
                    scrolledwindow > scrollbar > slider:hover {
                        background: transparent;
                        opacity: 0;
                    }
                    button {
                        background: transparent;
                        color: #cccccc;
                        border-radius: 4px;
                        border: none;
                        font-size: 16px;
                        font-weight: normal;
                        min-width: 28px;
                        min-height: 28px;
                        padding: 0px;
                        margin: 0px;
                    }
                    button:hover {
                        background: rgba(255, 255, 255, 0.1);
                        color: #ffffff;
                    }
                    button:active {
                        background: rgba(255, 255, 255, 0.15);
                    }
                    button.space-button {
                        color: #aaaaaa;
                        font-size: 14px;
                        font-weight: normal;
                    }
                    button.space-button:hover {
                        color: #ffffff;
                        background: rgba(255, 255, 255, 0.15);
                    }
                    button.space-button-current {
                        color: #ffffff;
                        font-size: 14px;
                        font-weight: bold;
                        background: rgba(100, 150, 255, 0.3);
                        border: 1px solid rgba(100, 150, 255, 0.5);
                    }
                    button.space-button-current:hover {
                        background: rgba(100, 150, 255, 0.4);
                    }
                    button.close-button {
                        font-size: 14px;
                        color: #999999;
                    }
                    button.close-button:hover {
                        color: #ff6666;
                        background: rgba(255, 102, 102, 0.1);
                    }
                    button.menu-button {
                        font-size: 18px;
                        font-weight: bold;
                    }
                    popover {
                        background-color: rgba(30, 30, 30, 0.95);
                        border: 1px solid rgba(255, 255, 255, 0.1);
                        border-radius: 4px;
                        padding: 0;
                        box-shadow: none;
                    }
                    popover > contents {
                        background-color: rgba(30, 30, 30, 0.95);
                        padding: 0;
                    }
                    popover modelbutton {
                        background-color: transparent;
                        color: #e0e0e0;
                        border-radius: 0;
                        padding: 8px 16px;
                        min-width: 100px;
                    }
                    popover modelbutton:hover {
                        background-color: rgba(255, 255, 255, 0.1);
                        color: #ffffff;
                    }
                    button.context-menu-item {
                        background-color: transparent;
                        color: #e0e0e0;
                        border-radius: 0;
                        padding: 8px 16px;
                        min-width: 120px;
                        font-size: 14px;
                        text-align: left;
                    }
                    button.context-menu-item:hover {
                        background-color: rgba(255, 255, 255, 0.1);
                        color: #ffffff;
                    }"
                );

                gtk4::style_context_add_provider_for_display(
                    &gtk4::prelude::WidgetExt::display(&window),
                    &css_provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );

                window.set_child(Some(&hbox));


                window.present();
                info!("GTK overlay window created and shown");

                // Perform initial auto-scroll after window is presented
                // This ensures the adjustment is fully initialized
                if let Some(current_index) = parts.iter().position(|p| p.contains('[')) {
                    let scrolled_window_for_present_scroll = scrolled_window.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                        let adj = scrolled_window_for_present_scroll.hadjustment();
                        let button_width = 32.0;
                        let viewport_width = adj.page_size();

                        let ideal_start_index = if current_index > 0 {
                            current_index - 1
                        } else {
                            0
                        };

                        let ideal_start = ideal_start_index as f64 * button_width;
                        let max_scroll = (total_spaces as f64 * button_width - viewport_width).max(0.0);
                        let target_pos = ideal_start.min(max_scroll).max(0.0);

                        adj.set_value(target_pos);
                        info!("Post-present auto-scroll to position {} for tab {} (viewport: {}, max: {})", target_pos, current_index, viewport_width, max_scroll);
                    });
                }
            });

            info!("Starting GTK application");
            app.run();
        });
    }

    pub async fn show_hud(&self, text: String) {
        debug!("HUD: {}", text);
        let _ = tokio::process::Command::new("notify-send")
            .arg("-t")
            .arg("1000")
            .arg("Space Manager")
            .arg(text)
            .spawn();
    }

    pub async fn show_input_box(&self) -> Option<String> {
        error!("Input box not yet implemented - use 'spacectl spawn <command>' instead");
        None
    }
}

impl Default for OverlayManager {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

fn show_settings_dialog(app: &Application) {
    info!("Creating settings dialog");

    // Load current settings from config.json
    let config_file = std::env::var("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".space-manager")
                .join("config.json")
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.space-manager/config.json"));

    let settings = if let Ok(content) = std::fs::read_to_string(&config_file) {
        serde_json::from_str::<serde_json::Value>(&content).ok()
    } else {
        None
    };

    // Create dialog window
    let dialog = Window::builder()
        .application(app)
        .title("Space Manager Settings")
        .default_width(500)
        .default_height(600)
        .modal(true)
        .build();

    // Add float and center rules with explicit size enforcement
    window_utils::apply_float_center_with_size("Space Manager Settings", 500, 600);

    // Main vertical box with standard margins
    let main_box = dialog_utils::create_standard_container();

    // Scrolled window for settings
    let scrolled = ScrolledWindow::builder().vexpand(true).build();

    // Grid for form fields
    let grid = Grid::builder().row_spacing(12).column_spacing(12).build();

    let mut row = 0;

    // Side Mouse Binds
    let side_mouse_label = Label::new(Some("Enable Side Mouse Buttons:"));
    side_mouse_label.set_halign(gtk4::Align::Start);
    let side_mouse_check = CheckButton::new();
    if let Some(s) = &settings {
        side_mouse_check.set_active(s["side_mouse_binds"].as_bool().unwrap_or(true));
    } else {
        side_mouse_check.set_active(true);
    }
    grid.attach(&side_mouse_label, 0, row, 1, 1);
    grid.attach(&side_mouse_check, 1, row, 1, 1);
    row += 1;

    // Overlay Enabled
    let overlay_enabled_label = Label::new(Some("Enable Overlay:"));
    overlay_enabled_label.set_halign(gtk4::Align::Start);
    let overlay_enabled_check = CheckButton::new();
    if let Some(s) = &settings {
        overlay_enabled_check.set_active(s["overlay"]["enabled"].as_bool().unwrap_or(true));
    } else {
        overlay_enabled_check.set_active(true);
    }
    grid.attach(&overlay_enabled_label, 0, row, 1, 1);
    grid.attach(&overlay_enabled_check, 1, row, 1, 1);
    row += 1;

    // From Area (where mouse change area is)
    let from_area_label = Label::new(Some("Mouse Change Area Position:"));
    from_area_label.set_halign(gtk4::Align::Start);
    let from_area_combo = ComboBoxText::new();
    from_area_combo.append(Some("left"), "Left");
    from_area_combo.append(Some("right"), "Right");
    from_area_combo.append(Some("top"), "Top");
    from_area_combo.append(Some("bottom"), "Bottom");
    let from_area_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["from_area"].as_str())
        .unwrap_or("left");
    from_area_combo.set_active_id(Some(from_area_value));
    grid.attach(&from_area_label, 0, row, 1, 1);
    grid.attach(&from_area_combo, 1, row, 1, 1);
    row += 1;

    // From Overlay (where overlay appears)
    let from_overlay_label = Label::new(Some("Overlay Position:"));
    from_overlay_label.set_halign(gtk4::Align::Start);
    let from_overlay_combo = ComboBoxText::new();
    from_overlay_combo.append(Some("bot_left"), "Bottom Left");
    from_overlay_combo.append(Some("bot_right"), "Bottom Right");
    from_overlay_combo.append(Some("top_left"), "Top Left");
    from_overlay_combo.append(Some("top_right"), "Top Right");
    let from_overlay_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["from_overlay"].as_str())
        .unwrap_or("bot_left");
    from_overlay_combo.set_active_id(Some(from_overlay_value));
    grid.attach(&from_overlay_label, 0, row, 1, 1);
    grid.attach(&from_overlay_combo, 1, row, 1, 1);
    row += 1;

    // Overlay Size
    let overlay_size_label = Label::new(Some("Overlay Width:"));
    overlay_size_label.set_halign(gtk4::Align::Start);
    let overlay_size_entry = Entry::new();
    let overlay_size_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["overlay_size"].as_str())
        .unwrap_or("change_area_x");
    overlay_size_entry.set_text(overlay_size_value);
    overlay_size_entry.set_tooltip_text(Some(
        "change_area_x, change_area_y, or pixel value (e.g. 250)",
    ));
    grid.attach(&overlay_size_label, 0, row, 1, 1);
    grid.attach(&overlay_size_entry, 1, row, 1, 1);
    row += 1;

    // Offset X
    let offset_x_label = Label::new(Some("Horizontal Offset (px):"));
    offset_x_label.set_halign(gtk4::Align::Start);
    let offset_x_entry = Entry::new();
    let offset_x_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["offset_x"].as_i64())
        .unwrap_or(8);
    offset_x_entry.set_text(&offset_x_value.to_string());
    grid.attach(&offset_x_label, 0, row, 1, 1);
    grid.attach(&offset_x_entry, 1, row, 1, 1);
    row += 1;

    // Offset Y
    let offset_y_label = Label::new(Some("Vertical Offset (px):"));
    offset_y_label.set_halign(gtk4::Align::Start);
    let offset_y_entry = Entry::new();
    let offset_y_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["offset_y"].as_i64())
        .unwrap_or(26);
    offset_y_entry.set_text(&offset_y_value.to_string());
    grid.attach(&offset_y_label, 0, row, 1, 1);
    grid.attach(&offset_y_entry, 1, row, 1, 1);
    row += 1;

    // Change Area Fraction
    let fraction_label = Label::new(Some("Change Area Fraction:"));
    fraction_label.set_halign(gtk4::Align::Start);
    let fraction_entry = Entry::new();
    let fraction_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["change_area_fraction"].as_f64())
        .unwrap_or(0.125);
    fraction_entry.set_text(&fraction_value.to_string());
    fraction_entry.set_tooltip_text(Some("Fraction of window dimension (e.g., 0.125 = 1/8)"));
    grid.attach(&fraction_label, 0, row, 1, 1);
    grid.attach(&fraction_entry, 1, row, 1, 1);
    row += 1;

    // Min Change Area Pixels
    let min_px_label = Label::new(Some("Min Change Area (px):"));
    min_px_label.set_halign(gtk4::Align::Start);
    let min_px_entry = Entry::new();
    let min_px_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["min_change_area_px"].as_i64())
        .unwrap_or(250);
    min_px_entry.set_text(&min_px_value.to_string());
    grid.attach(&min_px_label, 0, row, 1, 1);
    grid.attach(&min_px_entry, 1, row, 1, 1);

    scrolled.set_child(Some(&grid));
    main_box.append(&scrolled);

    // Buttons box
    let button_box = GtkBox::new(Orientation::Horizontal, 12);
    button_box.set_halign(gtk4::Align::End);

    let cancel_button = Button::with_label("Cancel");
    let apply_button = Button::with_label("Apply");
    let save_button = Button::with_label("Save");
    save_button.add_css_class("suggested-action");

    // Cancel button handler
    let dialog_clone = dialog.clone();
    cancel_button.connect_clicked(move |_| {
        dialog_clone.close();
    });

    // Apply button handler (save and reload without closing dialog)
    let config_file_clone1 = config_file.clone();
    let side_mouse_check_clone1 = side_mouse_check.clone();
    let overlay_enabled_check_clone1 = overlay_enabled_check.clone();
    let from_area_combo_clone1 = from_area_combo.clone();
    let from_overlay_combo_clone1 = from_overlay_combo.clone();
    let overlay_size_entry_clone1 = overlay_size_entry.clone();
    let offset_x_entry_clone1 = offset_x_entry.clone();
    let offset_y_entry_clone1 = offset_y_entry.clone();
    let fraction_entry_clone1 = fraction_entry.clone();
    let min_px_entry_clone1 = min_px_entry.clone();

    apply_button.connect_clicked(move |_| {
        info!("Applying settings...");

        let follow_mouse_guard = FollowMouseGuard::suppress();

        // Read existing config to preserve templates
        let mut existing_config = if let Ok(content) = std::fs::read_to_string(&config_file_clone1) {
            serde_json::from_str::<serde_json::Value>(&content).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        // Update only the settings fields, preserve templates
        existing_config["side_mouse_binds"] = serde_json::json!(side_mouse_check_clone1.is_active());
        existing_config["overlay"] = serde_json::json!({
            "enabled": overlay_enabled_check_clone1.is_active(),
            "from_area": from_area_combo_clone1.active_id().map(|s| s.to_string()).unwrap_or_else(|| "left".to_string()),
            "from_overlay": from_overlay_combo_clone1.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bot_left".to_string()),
            "overlay_size": overlay_size_entry_clone1.text().to_string(),
            "offset_x": offset_x_entry_clone1.text().parse::<i32>().unwrap_or(8),
            "offset_y": offset_y_entry_clone1.text().parse::<i32>().unwrap_or(26),
            "change_area_fraction": fraction_entry_clone1.text().parse::<f64>().unwrap_or(0.125),
            "min_change_area_px": min_px_entry_clone1.text().parse::<i32>().unwrap_or(250),
        });
        existing_config["mouse"] = serde_json::json!({
            "change_area_fraction": fraction_entry_clone1.text().parse::<f64>().unwrap_or(0.125),
            "min_change_area_px": min_px_entry_clone1.text().parse::<i32>().unwrap_or(250),
        });

        // Save to config file
        if let Ok(content) = serde_json::to_string_pretty(&existing_config) {
            if let Err(e) = std::fs::write(&config_file_clone1, content) {
                error!("Failed to save settings: {}", e);
                return;
            } else {
                info!("Settings saved successfully");
            }
        } else {
            error!("Failed to serialize settings");
            return;
        }

        // Send reload config command via IPC using centralized helper
        ipc_helpers::reload_config();

        // Keep suppression active briefly so any async overlay resize triggered by the reload
        // finishes before follow_mouse is restored.
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            drop(follow_mouse_guard);
        });
    });

    // Save button handler (save, reload, and close dialog)
    let config_file_clone2 = config_file.clone();
    let dialog_clone2 = dialog.clone();
    save_button.connect_clicked(move |_| {
        info!("Saving settings...");

        let follow_mouse_guard = FollowMouseGuard::suppress();

        // Read existing config to preserve templates
        let mut existing_config = if let Ok(content) = std::fs::read_to_string(&config_file_clone2) {
            serde_json::from_str::<serde_json::Value>(&content).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        // Update only the settings fields, preserve templates
        existing_config["side_mouse_binds"] = serde_json::json!(side_mouse_check.is_active());
        existing_config["overlay"] = serde_json::json!({
            "enabled": overlay_enabled_check.is_active(),
            "from_area": from_area_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "left".to_string()),
            "from_overlay": from_overlay_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bot_left".to_string()),
            "overlay_size": overlay_size_entry.text().to_string(),
            "offset_x": offset_x_entry.text().parse::<i32>().unwrap_or(8),
            "offset_y": offset_y_entry.text().parse::<i32>().unwrap_or(26),
            "change_area_fraction": fraction_entry.text().parse::<f64>().unwrap_or(0.125),
            "min_change_area_px": min_px_entry.text().parse::<i32>().unwrap_or(250),
        });
        existing_config["mouse"] = serde_json::json!({
            "change_area_fraction": fraction_entry.text().parse::<f64>().unwrap_or(0.125),
            "min_change_area_px": min_px_entry.text().parse::<i32>().unwrap_or(250),
        });

        // Save to config file
        if let Ok(content) = serde_json::to_string_pretty(&existing_config) {
            if let Err(e) = std::fs::write(&config_file_clone2, content) {
                error!("Failed to save settings: {}", e);
                return;
            } else {
                info!("Settings saved successfully");
            }
        } else {
            error!("Failed to serialize settings");
            return;
        }

        // Send reload config command via IPC using centralized helper
        ipc_helpers::reload_config();

        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            drop(follow_mouse_guard);
        });

        dialog_clone2.close();
    });

    button_box.append(&cancel_button);
    button_box.append(&apply_button);
    button_box.append(&save_button);
    main_box.append(&button_box);

    dialog.set_child(Some(&main_box));

    // Apply centralized theme to dialog
    dialog.add_css_class("settings-dialog");
    theme::apply_template_window_theme(&dialog);

    dialog.present();
}

fn show_new_space_window() {
    info!("Creating new space window");

    let dialog = gtk4::Window::builder()
        .title("New Space")
        .default_width(500)
        .default_height(400)
        .modal(true)
        .build();

    // Add float and center rules with explicit size enforcement
    window_utils::apply_float_center_with_size("New Space", 500, 400);

    // Apply centralized theme to dialog
    theme::apply_template_window_theme(&dialog);

    // Create a container for swappable content
    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    dialog.set_child(Some(&container));

    // Show the template list view initially
    show_template_list_view(&dialog, &container);

    dialog.present();
}

fn show_template_list_view(dialog: &gtk4::Window, container: &gtk4::Box) {
    // Clear existing content
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);

    let title_label = gtk4::Label::new(Some("Create New Space"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);

    // Fetch templates using centralized helper
    let templates = std::thread::spawn(|| ipc_helpers::get_templates_sync())
        .join()
        .ok()
        .flatten();

    // Create scrolled window for templates list
    let scrolled = gtk4::ScrolledWindow::builder().vexpand(true).build();

    let templates_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);

    if let Some(templates_arr) = templates.and_then(|t| t.as_array().cloned()) {
        if templates_arr.is_empty() {
            let empty_label = gtk4::Label::new(Some("No templates yet. Create one below!"));
            empty_label.add_css_class("dim-label");
            templates_box.append(&empty_label);
        } else {
            for template in templates_arr {
                let name = template["name"].as_str().unwrap_or("Unknown").to_string();
                let command = template["command"].as_str().unwrap_or("").to_string();

                // Create a horizontal box for template name and delete button
                let item_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                item_box.set_margin_start(8);
                item_box.set_margin_end(8);
                item_box.set_margin_top(4);
                item_box.set_margin_bottom(4);

                // Template button (takes most space)
                let template_btn = gtk4::Button::with_label(&name);
                template_btn.set_hexpand(true);
                template_btn.add_css_class("template-button");
                template_btn.set_cursor_from_name(Some("pointer"));

                let container_clone = container.clone();
                let dialog_clone = dialog.clone();
                let command_clone = command.clone();
                let name_clone = name.clone();
                template_btn.connect_clicked(move |_| {
                    info!("Template selected: {}", name_clone);
                    show_template_use_view(&dialog_clone, &container_clone, &command_clone);
                });

                // Delete button
                let delete_btn = gtk4::Button::with_label("🗑");
                delete_btn.set_width_request(36);
                delete_btn.add_css_class("delete-button");
                delete_btn.set_cursor_from_name(Some("pointer"));
                delete_btn.set_tooltip_text(Some("Delete this template"));

                let name_for_delete = name.clone();
                let container_clone2 = container.clone();
                let dialog_clone2 = dialog.clone();
                delete_btn.connect_clicked(move |_| {
                    info!("Delete template: {}", name_for_delete);

                    // Send RemoveTemplate command via centralized IPC helper
                    ipc_helpers::remove_template(name_for_delete.clone());

                    // Refresh the template list
                    show_template_list_view(&dialog_clone2, &container_clone2);
                });

                item_box.append(&template_btn);
                item_box.append(&delete_btn);
                templates_box.append(&item_box);
            }
        }
    }

    scrolled.set_child(Some(&templates_box));
    vbox.append(&scrolled);

    // Bottom buttons using standard button box
    let button_box = dialog_utils::create_button_box();

    let add_template_btn = gtk4::Button::with_label("✚ Add Template");
    add_template_btn.set_cursor_from_name(Some("pointer"));

    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    add_template_btn.connect_clicked(move |_| {
        info!("Add Template clicked - switching to add template view");
        show_add_template_view(&dialog_clone, &container_clone);
    });

    let close_btn = gtk4::Button::with_label("Close");
    let dialog_clone2 = dialog.clone();
    close_btn.connect_clicked(move |_| {
        dialog_clone2.close();
    });

    button_box.append(&add_template_btn);
    button_box.append(&close_btn);
    vbox.append(&button_box);

    container.append(&vbox);
}

fn show_template_use_view(dialog: &gtk4::Window, container: &gtk4::Box, command_template: &str) {
    // Clear existing content
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    // Extract variables from template like {{variable}}
    let re = regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap();
    let mut variables: Vec<String> = vec![];
    for cap in re.captures_iter(command_template) {
        if let Some(var) = cap.get(1) {
            let var_name = var.as_str().to_string();
            // Avoid duplicates
            if !variables.contains(&var_name) {
                variables.push(var_name);
            }
        }
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);

    let title_label = gtk4::Label::new(Some("Create Space from Template"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);

    // Show the command template (read-only) with better styling
    let template_label = gtk4::Label::new(Some("Template:"));
    template_label.set_halign(gtk4::Align::Start);
    template_label.add_css_class("field-label");

    let template_display = gtk4::Label::new(Some(command_template));
    template_display.set_halign(gtk4::Align::Start);
    template_display.set_wrap(true);
    template_display.add_css_class("template-display");

    vbox.append(&template_label);
    vbox.append(&template_display);

    // Add separator
    let separator = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    separator.set_margin_top(8);
    separator.set_margin_bottom(8);
    vbox.append(&separator);

    // Icon and Position on the same row
    let icon_position_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);

    // Icon field (left side)
    let icon_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let icon_label = gtk4::Label::new(Some("Icon:"));
    icon_label.set_halign(gtk4::Align::Start);
    icon_label.add_css_class("field-label");
    let icon_entry = gtk4::Entry::new();
    icon_entry.set_placeholder_text(Some("🌐"));
    icon_entry.set_hexpand(true);
    icon_vbox.append(&icon_label);
    icon_vbox.append(&icon_entry);

    // Position field (right side)
    let position_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let position_label = gtk4::Label::new(Some("Position:"));
    position_label.set_halign(gtk4::Align::Start);
    position_label.add_css_class("field-label");
    let position_entry = gtk4::Entry::new();
    position_entry.set_placeholder_text(Some("1, 2, 3..."));
    position_entry.set_width_chars(10);
    position_vbox.append(&position_label);
    position_vbox.append(&position_entry);

    icon_position_box.append(&icon_vbox);
    icon_position_box.append(&position_vbox);
    vbox.append(&icon_position_box);

    // Create entries for each template variable
    let mut variable_entries: Vec<(String, gtk4::Entry)> = vec![];
    for var in &variables {
        let var_label = gtk4::Label::new(Some(&format!("{}:", var)));
        var_label.set_halign(gtk4::Align::Start);
        var_label.add_css_class("field-label");
        let var_entry = gtk4::Entry::new();
        var_entry.set_placeholder_text(Some(&format!("Value for {{{{{}}}}}", var)));

        vbox.append(&var_label);
        vbox.append(&var_entry);
        variable_entries.push((var.clone(), var_entry));
    }

    let button_box = dialog_utils::create_button_box();

    let cancel_btn = dialog_utils::create_cancel_button();
    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        info!("Cancel clicked - returning to template list");
        show_template_list_view(&dialog_clone, &container_clone);
    });

    let create_btn = dialog_utils::create_action_button("Create Space");
    let command_template_owned = command_template.to_string();
    let position_entry_clone = position_entry.clone();
    let icon_entry_clone = icon_entry.clone();
    let dialog_clone2 = dialog.clone();
    create_btn.connect_clicked(move |_| {
        let position_str = position_entry_clone.text().to_string();
        let position_opt: Option<usize> = if position_str.is_empty() {
            None
        } else {
            // User enters 1-based position (1, 2, 3...), convert to 0-based index
            position_str
                .parse::<usize>()
                .ok()
                .and_then(|p| if p > 0 { Some(p - 1) } else { None })
        };

        let icon = icon_entry_clone.text().to_string();
        let icon_opt = if icon.is_empty() { None } else { Some(icon) };

        // Replace variables in command
        let mut final_command = command_template_owned.clone();
        for (var, entry) in &variable_entries {
            let value = entry.text().to_string();
            final_command = final_command.replace(&format!("{{{{{}}}}}", var), &value);
        }

        info!("Spawning with command: {}", final_command);
        dialog_clone2.close();

        // Send SpawnAt command via centralized IPC helper
        if let Some(idx) = position_opt {
            ipc_helpers::spawn_at(idx, final_command, icon_opt);
        } else {
            // Use SpawnAt with no position specified (appends to end)
            // We need to add a spawn helper for this case
            let cmd = serde_json::json!({"Spawn": final_command});
            ipc_helpers::send_command_async(cmd);
        }
    });

    button_box.append(&cancel_btn);
    button_box.append(&create_btn);

    vbox.append(&button_box);

    // CSS is already applied to the dialog via theme::apply_template_window_theme in show_new_space_window

    container.append(&vbox);
}

fn show_add_template_view(dialog: &gtk4::Window, container: &gtk4::Box) {
    // Clear existing content
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    let title_label = gtk4::Label::new(Some("Add Command Template"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);
    // Template name
    let name_label = gtk4::Label::new(Some("Template Name:"));
    name_label.set_halign(gtk4::Align::Start);
    name_label.add_css_class("field-label");
    let name_entry = gtk4::Entry::new();
    name_entry.set_placeholder_text(Some("e.g. Browser Profile"));
    vbox.append(&name_label);
    vbox.append(&name_entry);
    // Command with placeholders
    let command_label = gtk4::Label::new(Some("Command (use {{variable}} for placeholders):"));
    command_label.set_halign(gtk4::Align::Start);
    command_label.add_css_class("field-label");
    let command_entry = gtk4::Entry::new();
    command_entry.set_placeholder_text(Some(
        "e.g. brave --user-data-dir=\"$HOME/.config/{{profile}}\"",
    ));
    vbox.append(&command_label);
    vbox.append(&command_entry);

    let button_box = dialog_utils::create_button_box();
    let cancel_btn = dialog_utils::create_cancel_button();
    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        info!("Cancel clicked - returning to template list");
        show_template_list_view(&dialog_clone, &container_clone);
    });

    let save_btn = dialog_utils::create_action_button("Save");
    let name_entry_clone = name_entry.clone();
    let command_entry_clone = command_entry.clone();
    let container_clone2 = container.clone();
    let dialog_clone2 = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let name = name_entry_clone.text().to_string();
        let command = command_entry_clone.text().to_string();
        if name.is_empty() || command.is_empty() {
            return;
        }

        // Send AddTemplate command via centralized IPC helper
        ipc_helpers::add_template(name, command);

        // Return to template list
        show_template_list_view(&dialog_clone2, &container_clone2);
    });
    button_box.append(&cancel_btn);
    button_box.append(&save_btn);
    vbox.append(&button_box);

    // CSS is already applied to the dialog via theme::apply_template_window_theme in show_new_space_window

    container.append(&vbox);
}
fn get_monitor_size() -> (i32, i32) {
    let output = std::process::Command::new("hyprctl")
        .arg("monitors")
        .arg("-j")
        .output();
    if let Ok(output) = output {
        if let Ok(monitors) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            if let Some(monitor) = monitors.as_array().and_then(|m| m.first()) {
                let width = monitor["width"].as_i64().unwrap_or(1920) as i32;
                let height = monitor["height"].as_i64().unwrap_or(1080) as i32;
                return (width, height);
            }
        }
    }
    // Default to 1920x1080 if we can't get monitor info
    (1920, 1080)
}
fn get_active_window_geometry() -> Option<(i32, i32, i32, i32)> {
    let output = std::process::Command::new("hyprctl")
        .arg("activewindow")
        .arg("-j")
        .output()
        .ok()?;
    let window: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let x = window["at"][0].as_i64()? as i32;
    let y = window["at"][1].as_i64()? as i32;
    let width = window["size"][0].as_i64()? as i32;
    let height = window["size"][1].as_i64()? as i32;
    Some((x, y, width, height))
}
fn get_active_workspace() -> Option<i32> {
    let output = std::process::Command::new("hyprctl")
        .arg("activeworkspace")
        .arg("-j")
        .output()
        .ok()?;
    let workspace: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let id = workspace["id"].as_i64()? as i32;
    Some(id)
}
fn get_overlay_window_position() -> Option<(i32, i32)> {
    let output = std::process::Command::new("hyprctl")
        .arg("clients")
        .arg("-j")
        .output()
        .ok()?;
    let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    for client in clients.as_array()? {
        if let Some(title) = client["title"].as_str() {
            if title == "Space Manager Overlay" {
                let x = client["at"][0].as_i64()? as i32;
                let y = client["at"][1].as_i64()? as i32;
                return Some((x, y));
            }
        }
    }
    None
}
