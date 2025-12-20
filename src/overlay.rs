use anyhow::Result;
use cairo::{Context, Format, ImageSurface};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, error, warn};
/*
pub struct OverlayManager {
    overlay_path: PathBuf,
    overlay_pid: std::sync::Arc<tokio::sync::RwLock<Option<u32>>>,
}

impl OverlayManager {
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/tmp"));
        let overlay_path = PathBuf::from(home).join(".space-manager").join("overlay.png");

        Ok(Self {
            overlay_path,
            overlay_pid: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Show persistent space indicator overlay (1-2-3-[4]-5-6)
    pub async fn show_spaces_indicator(&self, current: usize, total: usize, _offset_x: i32, _offset_y: i32) {
        // Generate the indicator text like "1-2-3-[4]-5-6"
        let text = self.generate_indicator_text(current, total);
        debug!("Spaces indicator: {}", text);

        // For now, just use the HUD notification system
        // TODO: Implement proper persistent overlay with layer-shell or eww widget
        self.show_hud(text).await;
    }

    /// Hide the persistent overlay
    pub async fn hide_spaces_indicator(&self) {
        let pid = self.overlay_pid.read().await;
        if let Some(pid) = *pid {
            debug!("Hiding spaces indicator (PID: {})", pid);
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
        *self.overlay_pid.write().await = None;
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

    fn render_indicator_image(&self, text: &str) -> Result<()> {
        // Create directory if it doesn't exist
        if let Some(parent) = self.overlay_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Create cairo surface and context
        let surface = match ImageSurface::create(Format::ARgb32, 400, 40) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to create Cairo surface. Cairo libraries may not be installed.");
                error!("Install with: sudo pacman -S cairo  # or: sudo apt install libcairo2-dev");
                return Err(anyhow::anyhow!("Cairo error: {}", e));
            }
        };

        let cr = Context::new(&surface)?;

        // Clear background (transparent)
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint()?;

        // Draw semi-transparent background
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
        cr.rectangle(0.0, 0.0, 400.0, 40.0);
        cr.fill()?;

        // Draw text
        cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(16.0);

        cr.move_to(10.0, 25.0);
        cr.show_text(text)?;

        // Save to file
        let mut file = fs::File::create(&self.overlay_path)?;
        match surface.write_to_png(&mut file) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("Failed to write PNG. Cairo libraries may not be properly installed.");
                error!("Install with: sudo pacman -S cairo  # or: sudo apt install libcairo2-dev");
                Err(anyhow::anyhow!("PNG write error: {}", e))
            }
        }
    }

    async fn show_overlay_window(&self, offset_x: i32, offset_y: i32) {
        // First, hide any existing overlay
        self.hide_spaces_indicator().await;

        // Use imv (image viewer) to show the overlay
        // imv can show images in a borderless window
        let result = tokio::process::Command::new("imv")
            .arg("-f")  // Fullscreen/overlay mode
            .arg("-b")  // Background color
            .arg("none")
            .arg(&self.overlay_path)
            .spawn();

        match result {
            Ok(child) => {
                if let Some(pid) = child.id() {
                    *self.overlay_pid.write().await = Some(pid);
                    debug!("Overlay window spawned with PID: {}", pid);

                    // Position the window using hyprctl
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    let _ = std::process::Command::new("hyprctl")
                        .arg("dispatch")
                        .arg("movewindowpixel")
                        .arg(format!("exact {} {}", offset_x, offset_y))
                        .arg("class:imv")
                        .output();
                }
            }
            Err(e) => {
                error!("Failed to spawn overlay window: {}", e);
                error!("The 'imv' image viewer is not installed or not in PATH.");
                error!("Install with: sudo pacman -S imv  # or: sudo apt install imv");
                error!("Alternatively, disable overlay in ~/.space-manager/state.json by setting overlay.enabled to false");
            }
        }
    }

    pub async fn show_hud(&self, text: String) {
        debug!("HUD: {}", text);
        // Show a temporary notification
        let _ = tokio::process::Command::new("notify-send")
            .arg("-t")
            .arg("1000")
            .arg("Space Manager")
            .arg(text)
            .spawn();
    }

    pub async fn show_input_box(&self) -> Option<String> {
        warn!("Input box not yet implemented - use 'spacectl spawn <command>' instead");
        None
    }
}

impl Default for OverlayManager {
    fn default() -> Self {
        Self::new().unwrap()
    }
}
*/