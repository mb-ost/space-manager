use anyhow::Result;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label, Button, Box as GtkBox, Orientation};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

pub struct OverlayManager {
    label_text: Arc<RwLock<String>>,
    window_created: Arc<RwLock<bool>>,
    overlay_visible: Arc<RwLock<bool>>,
    saved_position: Arc<RwLock<Option<(i32, i32)>>>,
}

impl OverlayManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            label_text: Arc::new(RwLock::new(String::new())),
            window_created: Arc::new(RwLock::new(false)),
            overlay_visible: Arc::new(RwLock::new(true)),
            saved_position: Arc::new(RwLock::new(None)),
        })
    }

    /// Show persistent space indicator overlay (1-2-3-[4]-5-6)
    pub async fn show_spaces_indicator(&self, current: usize, total: usize, from: &str, offset_x: i32, offset_y: i32, overlay_size: &str, change_area_fraction: f64, min_change_area_px: i32, from_area: &str) {
        info!("show_spaces_indicator called: current={}, total={}, from={}, offset=({}, {}), overlay_size={}", current, total, from, offset_x, offset_y, overlay_size);

        // Generate the indicator text like "1-2-3-[4]-5-6"
        let text = self.generate_indicator_text(current, total);
        info!("Spaces indicator text: {}", text);

        // Update the label text
        *self.label_text.write().await = text.clone();

        // Check if window is already created
        let created = *self.window_created.read().await;
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
            // We don't set any window rules here - we'll control pin/unpin dynamically
            // Only set float rule so overlay doesn't tile
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
                    .resizable(false)
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

                // Create horizontal box to hold label and button
                let hbox = GtkBox::new(Orientation::Horizontal, 8);
                hbox.set_margin_start(8);
                hbox.set_margin_end(8);
                hbox.set_margin_top(4);
                hbox.set_margin_bottom(4);

                // Create label
                let label = Label::builder()
                    .hexpand(true)
                    .halign(gtk4::Align::Center)
                    .build();

                // Create close button
                let close_button = Button::with_label("Close");
                close_button.set_width_request(50);
                close_button.set_height_request(26);

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
                        background: linear-gradient(to bottom, #4a90e2, #357abd);
                        color: #ffffff;
                        border-radius: 4px;
                        border: none;
                        font-size: 11px;
                        font-weight: normal;
                        min-width: 50px;
                        min-height: 26px;
                        padding: 4px 8px;
                    }
                    button:hover {
                        background: linear-gradient(to bottom, #5aa0f2, #458acd);
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

