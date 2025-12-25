use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label, Button, Box as GtkBox, Orientation,
          Window, Entry, CheckButton, ComboBoxText, Grid, ScrolledWindow};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

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
    pub async fn show_spaces_indicator(&self, current: usize, total: usize, from: &str, offset_x: i32, offset_y: i32, overlay_size: &str, change_area_fraction: f64, min_change_area_px: i32, from_area: &str, tracked_window_address: Option<&str>) {
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
                old_config.from != new_config.from ||
                old_config.offset_x != new_config.offset_x ||
                old_config.offset_y != new_config.offset_y ||
                old_config.overlay_size != new_config.overlay_size ||
                (old_config.change_area_fraction - new_config.change_area_fraction).abs() > 0.001 ||
                old_config.min_change_area_px != new_config.min_change_area_px ||
                old_config.from_area != new_config.from_area
            } else {
                false
            }
        };

        // Check if window is already created
        let created = *self.window_created.read().await;

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
            ).await;
        }

        // Store the new config
        *self.current_config.write().await = Some(new_config);

        // Generate the indicator text like "1-2-3-[4]-5-6"
        let text = self.generate_indicator_text(current, total);
        info!("Spaces indicator text: {}", text);

        // Update the label text
        *self.label_text.write().await = text.clone();

        // Check if window needs to be created
        if !created {
            info!("Creating new GTK overlay window");
            self.spawn_gtk_window(from, offset_x, offset_y, overlay_size, change_area_fraction, min_change_area_px, from_area).await;
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

        // Unpin the overlay so it's no longer visible on all workspaces
        let _ = std::process::Command::new("hyprctl")
            .arg("dispatch")
            .arg("pin")
            .arg("title:^Space Manager Overlay$")
            .output();

        info!("Unpinned overlay");

        // Move to special workspace where we hide invisible tabs
        let _ = std::process::Command::new("hyprctl")
            .arg("dispatch")
            .arg("movetoworkspacesilent")
            .arg("special:spaces,title:^Space Manager Overlay$")
            .output();

        info!("Moved overlay to special:spaces");

        *self.overlay_visible.write().await = false;
    }


    /// Show the persistent overlay (restore from hidden state)
    pub async fn show_overlay(&self) {
        info!("Showing overlay window");

        // Get the workspace where space manager windows are visible
        // We'll move it to the active workspace first
        let active_workspace = get_active_workspace().unwrap_or(1);

        // Move overlay from special workspace to active workspace
        let _ = std::process::Command::new("hyprctl")
            .arg("dispatch")
            .arg("movetoworkspacesilent")
            .arg(format!("{},title:^Space Manager Overlay$", active_workspace))
            .output();

        info!("Moved overlay to workspace {}", active_workspace);

        // Small delay to ensure move completes
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

        // Restore saved position if available
        if let Some((x, y)) = *self.saved_position.read().await {
            info!("Restoring overlay position: ({}, {})", x, y);
            let _ = std::process::Command::new("hyprctl")
                .arg("dispatch")
                .arg("movewindowpixel")
                .arg(format!("exact {} {},title:^Space Manager Overlay$", x, y))
                .output();
        }

        // Small delay before pinning
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

        // Pin the overlay so it shows on all workspaces
        let _ = std::process::Command::new("hyprctl")
            .arg("dispatch")
            .arg("pin")
            .arg("title:^Space Manager Overlay$")
            .output();

        info!("Pinned overlay");

        *self.overlay_visible.write().await = true;
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
                "bot_left" => (win_x + offset_x, win_y + win_height - overlay_height - offset_y),
                "bot_right" => (win_x + win_width - overlay_width - offset_x, win_y + win_height - overlay_height - offset_y),
                "top_left" => (win_x + offset_x, win_y + offset_y),
                "top_right" => (win_x + win_width - overlay_width - offset_x, win_y + offset_y),
                _ => (win_x + offset_x, win_y + win_height - overlay_height - offset_y),
            };

            info!("Target: {}x{} at ({}, {})", overlay_width, overlay_height, pos_x, pos_y);

            // Get overlay address
            if let Some(addr) = self.get_overlay_window_address() {
                info!("Overlay address: {}", addr);

                // Get current size and resize if needed
                if let Some((current_w, current_h)) = self.get_overlay_window_size() {
                    let delta_w = overlay_width - current_w;
                    let delta_h = overlay_height - current_h;

                    info!("Current: {}x{}, delta: {}x{}", current_w, current_h, delta_w, delta_h);

                    if delta_w != 0 || delta_h != 0 {
                        info!("Resizing overlay from {}x{} to {}x{}", current_w, current_h, overlay_width, overlay_height);

                        // Disable cursor warping to prevent cursor from moving to focused window
                        let cursor_no_warps_output = std::process::Command::new("hyprctl")
                            .arg("getoption")
                            .arg("cursor:no_warps")
                            .arg("-j")
                            .output()
                            .ok();

                        let original_no_warps = cursor_no_warps_output
                            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
                            .and_then(|json| json["int"].as_i64())
                            .unwrap_or(0);

                        // Enable no_warps (1 = cursor doesn't warp to focused windows)
                        let _ = std::process::Command::new("hyprctl")
                            .arg("keyword")
                            .arg("cursor:no_warps")
                            .arg("1")
                            .output();

                        // Focus the overlay window first so resizeactive works
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("focuswindow")
                            .arg(format!("address:{}", addr))
                            .output();

                        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

                        // Now resize using resizeactive exact
                        let resize_cmd = format!("exact {} {}", overlay_width, overlay_height);
                        info!("Executing: hyprctl dispatch resizeactive {}", resize_cmd);

                        let output = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("resizeactive")
                            .arg(&resize_cmd)
                            .output();

                        if let Ok(out) = output {
                            info!("Resize status: {}", out.status);
                            if !out.stderr.is_empty() {
                                error!("Resize error: {}", String::from_utf8_lossy(&out.stderr));
                            }
                        }

                        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

                        // Restore focus to tracked window
                        if let Some(track_addr) = tracked_window_address {
                            let _ = std::process::Command::new("hyprctl")
                                .arg("dispatch")
                                .arg("focuswindow")
                                .arg(format!("address:{}", track_addr))
                                .output();
                        }

                        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

                        // Restore cursor:no_warps setting
                        let _ = std::process::Command::new("hyprctl")
                            .arg("keyword")
                            .arg("cursor:no_warps")
                            .arg(format!("{}", original_no_warps))
                            .output();

                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                }

                // Move to position
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

    fn generate_indicator_text(&self, current: usize, total: usize) -> String {
        let mut parts = Vec::new();
        for i in 0..total {
            if i == current {
                parts.push(format!("[{}]", i + 1));
            } else {
                parts.push(format!("{}", i + 1));
            }
        }
        parts.join("-")
    }

    async fn spawn_gtk_window(&self, from: &str, offset_x: i32, offset_y: i32, overlay_size: &str, change_area_fraction: f64, min_change_area_px: i32, from_area: &str) {
        let label_text = self.label_text.clone();
        let from = from.to_string();
        let overlay_size = overlay_size.to_string();
        let from_area = from_area.to_string();

        // Spawn GTK in a separate thread
        std::thread::spawn(move || {
            // Set float rule so overlay doesn't tile
            let _ = std::process::Command::new("hyprctl")
                .arg("keyword")
                .arg("windowrulev2")
                .arg("float,title:^(Space Manager Overlay)$")
                .output();

            info!("Float rule added for overlay");

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
                window.connect_map(move |_win| {
                    // Now calculate position and move the window via Hyprland
                    let (pos_x, pos_y) = match from_clone2.as_str() {
                        "bot_left" => (win_x + offset_x, win_y + win_height - overlay_height - offset_y),
                        "bot_right" => (win_x + win_width - overlay_width - offset_x, win_y + win_height - overlay_height - offset_y),
                        "top_left" => (win_x + offset_x, win_y + offset_y),
                        "top_right" => (win_x + win_width - overlay_width - offset_x, win_y + offset_y),
                        _ => (win_x + offset_x, win_y + win_height - overlay_height - offset_y),
                    };

                    info!("Window mapped! Moving to position: ({}, {})", pos_x, pos_y);

                    // Move window using hyprctl
                    std::thread::spawn(move || {
                        // Small delay to ensure window is fully created
                        std::thread::sleep(std::time::Duration::from_millis(50));

                        // Position the window
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("movewindowpixel")
                            .arg(format!("exact {} {},title:^Space Manager Overlay$", pos_x, pos_y))
                            .output();

                        info!("Window moved to ({}, {})", pos_x, pos_y);

                        // Small delay before pinning
                        std::thread::sleep(std::time::Duration::from_millis(30));

                        // Pin the overlay so it appears on all workspaces
                        let _ = std::process::Command::new("hyprctl")
                            .arg("dispatch")
                            .arg("pin")
                            .arg("title:^Space Manager Overlay$")
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

                // Create popover menu
                let menu = gtk4::gio::Menu::new();
                menu.append(Some("Settings"), Some("app.settings"));

                let popover = gtk4::PopoverMenu::builder()
                    .menu_model(&menu)
                    .build();

                menu_button.set_popover(Some(&popover));

                // Add settings action
                let app_clone = app.clone();
                let settings_action = gtk4::gio::SimpleAction::new("settings", None);
                settings_action.connect_activate(move |_, _| {
                    info!("Settings menu item clicked, opening settings dialog");
                    show_settings_dialog(&app_clone);
                });
                app.add_action(&settings_action);

                // Create label
                let label = Label::builder()
                    .hexpand(true)
                    .halign(gtk4::Align::Center)
                    .build();

                // Create close button (X)
                let close_button = Button::with_label("✕");
                close_button.set_width_request(28);
                close_button.set_height_request(28);
                close_button.add_css_class("close-button");

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
                hbox.append(&label);
                hbox.append(&close_button);

                // Add CSS styling
                let css_provider = gtk4::CssProvider::new();
                css_provider.load_from_data(
                    "window {
                        background-color: rgba(30, 30, 30, 0.95);
                        border-radius: 6px;
                    }
                    label {
                        color: white;
                        font-size: 14px;
                        font-family: monospace;
                        font-weight: bold;
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
                    }"
                );

                gtk4::style_context_add_provider_for_display(
                    &gtk4::prelude::WidgetExt::display(&window),
                    &css_provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );

                window.set_child(Some(&hbox));

                // Update label periodically
                let label_clone = label.clone();
                let label_text_clone = label_text.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                    let text = glib::MainContext::default().block_on(async {
                        label_text_clone.read().await.clone()
                    });
                    label_clone.set_text(&text);
                    glib::ControlFlow::Continue
                });

                window.present();
                info!("GTK overlay window created and shown");
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

    // Load current settings from state.json
    let state_file = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".space-manager").join("state.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.space-manager/state.json"));

    let settings = if let Ok(content) = std::fs::read_to_string(&state_file) {
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

    // Add float rule for settings dialog
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::process::Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrulev2")
            .arg("float,title:^(Space Manager Settings)$")
            .output();
        let _ = std::process::Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrulev2")
            .arg("center,title:^(Space Manager Settings)$")
            .output();
    });

    // Main vertical box
    let main_box = GtkBox::new(Orientation::Vertical, 12);
    main_box.set_margin_start(20);
    main_box.set_margin_end(20);
    main_box.set_margin_top(20);
    main_box.set_margin_bottom(20);

    // Scrolled window for settings
    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .build();

    // Grid for form fields
    let grid = Grid::builder()
        .row_spacing(12)
        .column_spacing(12)
        .build();

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
    let from_area_value = settings.as_ref()
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
    let from_overlay_value = settings.as_ref()
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
    let overlay_size_value = settings.as_ref()
        .and_then(|s| s["overlay"]["overlay_size"].as_str())
        .unwrap_or("change_area_x");
    overlay_size_entry.set_text(overlay_size_value);
    overlay_size_entry.set_tooltip_text(Some("change_area_x, change_area_y, or pixel value (e.g. 250)"));
    grid.attach(&overlay_size_label, 0, row, 1, 1);
    grid.attach(&overlay_size_entry, 1, row, 1, 1);
    row += 1;

    // Offset X
    let offset_x_label = Label::new(Some("Horizontal Offset (px):"));
    offset_x_label.set_halign(gtk4::Align::Start);
    let offset_x_entry = Entry::new();
    let offset_x_value = settings.as_ref()
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
    let offset_y_value = settings.as_ref()
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
    let fraction_value = settings.as_ref()
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
    let min_px_value = settings.as_ref()
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
    let state_file_clone1 = state_file.clone();
    let settings_clone1 = settings.clone();
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

        // Read all form values
        let new_settings = serde_json::json!({
            "side_mouse_binds": side_mouse_check_clone1.is_active(),
            "overlay": {
                "enabled": overlay_enabled_check_clone1.is_active(),
                "from_area": from_area_combo_clone1.active_id().map(|s| s.to_string()).unwrap_or_else(|| "left".to_string()),
                "from_overlay": from_overlay_combo_clone1.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bot_left".to_string()),
                "overlay_size": overlay_size_entry_clone1.text().to_string(),
                "offset_x": offset_x_entry_clone1.text().parse::<i32>().unwrap_or(8),
                "offset_y": offset_y_entry_clone1.text().parse::<i32>().unwrap_or(26),
                "change_area_fraction": fraction_entry_clone1.text().parse::<f64>().unwrap_or(0.125),
                "min_change_area_px": min_px_entry_clone1.text().parse::<i32>().unwrap_or(250),
            },
            "mouse": {
                "change_area_fraction": fraction_entry_clone1.text().parse::<f64>().unwrap_or(0.125),
                "min_change_area_px": min_px_entry_clone1.text().parse::<i32>().unwrap_or(250),
            }
        });

        // Merge with existing settings (preserve windows and current)
        let mut final_settings = if let Some(existing) = &settings_clone1 {
            existing.clone()
        } else {
            serde_json::json!({})
        };

        if let Some(obj) = final_settings.as_object_mut() {
            obj.insert("side_mouse_binds".to_string(), new_settings["side_mouse_binds"].clone());
            obj.insert("overlay".to_string(), new_settings["overlay"].clone());
            obj.insert("mouse".to_string(), new_settings["mouse"].clone());
        }

        // Save to file
        if let Ok(content) = serde_json::to_string_pretty(&final_settings) {
            if let Err(e) = std::fs::write(&state_file_clone1, content) {
                error!("Failed to save settings: {}", e);
                return;
            } else {
                info!("Settings saved successfully");
            }
        } else {
            error!("Failed to serialize settings");
            return;
        }

        // Send reload config command via IPC
        std::thread::spawn(|| {
            use std::io::{Write, Read};

            let socket_path = std::env::var("XDG_RUNTIME_DIR")
                .map(|d| format!("{}/space-manager.sock", d))
                .unwrap_or_else(|_| "/tmp/space-manager.sock".to_string());

            if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&socket_path) {
                // Serialize the ReloadConfig command
                let cmd = serde_json::json!("ReloadConfig");
                if let Ok(data) = serde_json::to_vec(&cmd) {
                    let len = (data.len() as u32).to_le_bytes();

                    // Write length prefix
                    let _ = stream.write_all(&len);
                    // Write command data
                    let _ = stream.write_all(&data);
                    let _ = stream.flush();

                    // Read response length
                    let mut len_bytes = [0u8; 4];
                    if stream.read_exact(&mut len_bytes).is_ok() {
                        let response_len = u32::from_le_bytes(len_bytes) as usize;
                        let mut response_data = vec![0u8; response_len];
                        if stream.read_exact(&mut response_data).is_ok() {
                            if let Ok(response) = serde_json::from_slice::<serde_json::Value>(&response_data) {
                                info!("Reload config response (Apply): {:?}", response);
                            }
                        }
                    }
                }
            } else {
                error!("Failed to connect to daemon at {}", socket_path);
            }
        });
    });

    // Save button handler (save, reload, and close dialog)
    let dialog_clone2 = dialog.clone();
    save_button.connect_clicked(move |_| {
        info!("Saving settings...");

        // Read all form values
        let new_settings = serde_json::json!({
            "side_mouse_binds": side_mouse_check.is_active(),
            "overlay": {
                "enabled": overlay_enabled_check.is_active(),
                "from_area": from_area_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "left".to_string()),
                "from_overlay": from_overlay_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bot_left".to_string()),
                "overlay_size": overlay_size_entry.text().to_string(),
                "offset_x": offset_x_entry.text().parse::<i32>().unwrap_or(8),
                "offset_y": offset_y_entry.text().parse::<i32>().unwrap_or(26),
                "change_area_fraction": fraction_entry.text().parse::<f64>().unwrap_or(0.125),
                "min_change_area_px": min_px_entry.text().parse::<i32>().unwrap_or(250),
            },
            "mouse": {
                "change_area_fraction": fraction_entry.text().parse::<f64>().unwrap_or(0.125),
                "min_change_area_px": min_px_entry.text().parse::<i32>().unwrap_or(250),
            }
        });

        // Merge with existing settings (preserve windows and current)
        let mut final_settings = if let Some(existing) = &settings {
            existing.clone()
        } else {
            serde_json::json!({})
        };

        if let Some(obj) = final_settings.as_object_mut() {
            obj.insert("side_mouse_binds".to_string(), new_settings["side_mouse_binds"].clone());
            obj.insert("overlay".to_string(), new_settings["overlay"].clone());
            obj.insert("mouse".to_string(), new_settings["mouse"].clone());
        }

        // Save to file
        if let Ok(content) = serde_json::to_string_pretty(&final_settings) {
            if let Err(e) = std::fs::write(&state_file, content) {
                error!("Failed to save settings: {}", e);
                return;
            } else {
                info!("Settings saved successfully");
            }
        } else {
            error!("Failed to serialize settings");
            return;
        }

        // Send reload config command via IPC
        std::thread::spawn(|| {
            use std::io::{Write, Read};

            let socket_path = std::env::var("XDG_RUNTIME_DIR")
                .map(|d| format!("{}/space-manager.sock", d))
                .unwrap_or_else(|_| "/tmp/space-manager.sock".to_string());

            if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&socket_path) {
                // Serialize the ReloadConfig command
                let cmd = serde_json::json!("ReloadConfig");
                if let Ok(data) = serde_json::to_vec(&cmd) {
                    let len = (data.len() as u32).to_le_bytes();

                    // Write length prefix
                    let _ = stream.write_all(&len);
                    // Write command data
                    let _ = stream.write_all(&data);
                    let _ = stream.flush();

                    // Read response length
                    let mut len_bytes = [0u8; 4];
                    if stream.read_exact(&mut len_bytes).is_ok() {
                        let response_len = u32::from_le_bytes(len_bytes) as usize;
                        let mut response_data = vec![0u8; response_len];
                        if stream.read_exact(&mut response_data).is_ok() {
                            if let Ok(response) = serde_json::from_slice::<serde_json::Value>(&response_data) {
                                info!("Reload config response (Save): {:?}", response);
                            }
                        }
                    }
                }
            } else {
                error!("Failed to connect to daemon at {}", socket_path);
            }
        });

        dialog_clone2.close();
    });

    button_box.append(&cancel_button);
    button_box.append(&apply_button);
    button_box.append(&save_button);
    main_box.append(&button_box);

    dialog.set_child(Some(&main_box));

    // Add CSS for dialog - apply to the dialog window only, not globally
    let css_provider = gtk4::CssProvider::new();
    css_provider.load_from_data(
        "window.settings-dialog {
            background-color: #2b2b2b;
        }
        window.settings-dialog label {
            color: #e0e0e0;
            font-size: 13px;
        }
        window.settings-dialog entry {
            background-color: #3c3c3c;
            color: #e0e0e0;
            border: 1px solid #555555;
            border-radius: 4px;
            padding: 6px;
            min-width: 200px;
        }
        window.settings-dialog button {
            background: #4a4a4a;
            color: #e0e0e0;
            border-radius: 4px;
            border: 1px solid #555555;
            padding: 8px 16px;
            font-size: 13px;
        }
        window.settings-dialog button:hover {
            background: #5a5a5a;
        }
        window.settings-dialog button.suggested-action {
            background: #4a90e2;
            color: #ffffff;
            border: 1px solid #357abd;
        }
        window.settings-dialog button.suggested-action:hover {
            background: #5aa0f2;
        }
        window.settings-dialog combobox button {
            min-width: 200px;
        }"
    );

    // Apply CSS to dialog only
    dialog.add_css_class("settings-dialog");
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(&dialog),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    dialog.present();
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

