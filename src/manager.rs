use crate::types::ManagedWindow;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    windows: Vec<ManagedWindow>,
    current: Option<String>,  // ID of the current window
    #[serde(default = "default_side_mouse_binds")]
    side_mouse_binds: bool,
    #[serde(default = "default_overlay_config")]
    overlay: OverlayConfig,
    #[serde(default = "default_mouse_config")]
    mouse: MouseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayConfig {
    pub enabled: bool,
    pub offset_x: i32,  // pixels from left
    pub offset_y: i32,  // pixels from bottom
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseConfig {
    pub change_area_fraction: f64,  // fraction of window width (e.g., 0.125 = 1/8)
    pub min_change_area_px: i32,    // minimum area in pixels
}

fn default_side_mouse_binds() -> bool {
    true
}

fn default_overlay_config() -> OverlayConfig {
    OverlayConfig {
        enabled: true,
        offset_x: 8,
        offset_y: 26,
    }
}

fn default_mouse_config() -> MouseConfig {
    MouseConfig {
        change_area_fraction: 0.125,  // 1/8
        min_change_area_px: 250,
    }
}

pub struct SpaceManager {
    windows: Arc<RwLock<Vec<ManagedWindow>>>,
    current_index: Arc<RwLock<usize>>,
    state_file: PathBuf,
    side_mouse_binds: Arc<RwLock<bool>>,
    overlay_config: Arc<RwLock<OverlayConfig>>,
    mouse_config: Arc<RwLock<MouseConfig>>,
}

impl SpaceManager {
    pub fn new() -> Self {
        let state_file = Self::get_state_file_path();
        Self {
            windows: Arc::new(RwLock::new(Vec::new())),
            current_index: Arc::new(RwLock::new(0)),
            state_file,
            side_mouse_binds: Arc::new(RwLock::new(true)),
            overlay_config: Arc::new(RwLock::new(default_overlay_config())),
            mouse_config: Arc::new(RwLock::new(default_mouse_config())),
        }
    }

    fn get_state_file_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/tmp"));
        PathBuf::from(home).join(".space-manager").join("state.json")
    }

    /// Load state from disk (all windows loaded as closed - PID will be None)
    pub async fn load_state(&self) -> Result<()> {
        if !self.state_file.exists() {
            info!("No saved state found at {:?}", self.state_file);
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.state_file).await?;
        let state: StateFile = serde_json::from_str(&content)?;

        let mut windows = self.windows.write().await;
        *windows = state.windows;

        // Load side_mouse_binds setting
        let mut side_mouse_binds = self.side_mouse_binds.write().await;
        *side_mouse_binds = state.side_mouse_binds;
        info!("Loaded side_mouse_binds setting: {}", state.side_mouse_binds);

        // Load overlay config
        let mut overlay_config = self.overlay_config.write().await;
        *overlay_config = state.overlay;
        info!("Loaded overlay config: enabled={}, offset=({}, {})",
              overlay_config.enabled, overlay_config.offset_x, overlay_config.offset_y);

        // Load mouse config
        let mut mouse_config = self.mouse_config.write().await;
        *mouse_config = state.mouse;
        info!("Loaded mouse config: fraction={}, min_px={}",
              mouse_config.change_area_fraction, mouse_config.min_change_area_px);

        // Set current index based on saved current window ID
        if let Some(current_id) = state.current {
            if let Some(index) = windows.iter().position(|w| w.id == current_id) {
                let mut current = self.current_index.write().await;
                *current = index;
                info!("Restored current window index: {}", index);
            }
        }

        // All loaded windows will have pid=None (closed state) since we use #[serde(skip)]
        info!("Loaded {} windows from saved state (all closed)", windows.len());
        Ok(())
    }

    /// Save state to disk
    pub async fn save_state(&self) -> Result<()> {
        // Create directory if it doesn't exist
        if let Some(parent) = self.state_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let windows = self.windows.read().await;
        let current_index = *self.current_index.read().await;
        let side_mouse_binds = *self.side_mouse_binds.read().await;
        let overlay_config = self.overlay_config.read().await.clone();
        let mouse_config = self.mouse_config.read().await.clone();

        // Get current window ID
        let current_id = if current_index < windows.len() {
            Some(windows[current_index].id.clone())
        } else {
            None
        };

        let state = StateFile {
            windows: windows.clone(),
            current: current_id,
            side_mouse_binds,
            overlay: overlay_config,
            mouse: mouse_config,
        };

        let content = serde_json::to_string_pretty(&state)?;
        tokio::fs::write(&self.state_file, content).await?;

        info!("Saved {} windows to state file", windows.len());
        Ok(())
    }

    pub async fn is_side_mouse_binds_enabled(&self) -> bool {
        *self.side_mouse_binds.read().await
    }

    pub async fn get_overlay_config(&self) -> OverlayConfig {
        self.overlay_config.read().await.clone()
    }

    pub async fn get_mouse_config(&self) -> MouseConfig {
        self.mouse_config.read().await.clone()
    }

    pub async fn add_window(&self, window: ManagedWindow) {
        let mut windows = self.windows.write().await;
        windows.push(window);

        // Update current index to point to the new window
        let mut current = self.current_index.write().await;
        *current = windows.len() - 1;
    }

    /// Remove a window entirely by address
    pub async fn remove_window_by_address(&self, address: &str) -> Option<usize> {
        let mut windows = self.windows.write().await;
        if let Some(pos) = windows.iter().position(|w| w.address == address) {
            windows.remove(pos);

            // Adjust current index if necessary
            let mut current = self.current_index.write().await;
            if windows.is_empty() {
                *current = 0;
            } else if *current >= windows.len() {
                *current = windows.len() - 1;
            }

            Some(pos)
        } else {
            None
        }
    }

    /// Find window by ID and mark as open
    pub async fn open_window_by_id(&self, id: &str, address: String, class: String, title: String, pid: Option<u32>) -> bool {
        let mut windows = self.windows.write().await;
        if let Some(window) = windows.iter_mut().find(|w| w.id == id) {
            window.open(address, class, title, pid);
            true
        } else {
            false
        }
    }

    /// Find window index by address
    pub async fn find_window_index_by_address(&self, address: &str) -> Option<usize> {
        let windows = self.windows.read().await;
        windows.iter().position(|w| w.address == address)
    }

    pub async fn remove_all_windows(&self) {
        let mut windows = self.windows.write().await;
        windows.clear();
        let mut current = self.current_index.write().await;
        *current = 0;
    }

    pub async fn next(&self) -> Option<ManagedWindow> {
        let windows = self.windows.read().await;
        if windows.is_empty() {
            return None;
        }

        let mut current = self.current_index.write().await;
        *current = (*current + 1) % windows.len();
        Some(windows[*current].clone())
    }

    pub async fn prev(&self) -> Option<ManagedWindow> {
        let windows = self.windows.read().await;
        if windows.is_empty() {
            return None;
        }

        let mut current = self.current_index.write().await;
        if *current == 0 {
            *current = windows.len() - 1;
        } else {
            *current -= 1;
        }
        Some(windows[*current].clone())
    }

    pub async fn current_window(&self) -> Option<ManagedWindow> {
        let windows = self.windows.read().await;
        let current = self.current_index.read().await;

        if windows.is_empty() {
            None
        } else {
            Some(windows[*current].clone())
        }
    }

    pub async fn get_windows(&self) -> Vec<ManagedWindow> {
        self.windows.read().await.clone()
    }

    pub async fn get_current_index(&self) -> usize {
        *self.current_index.read().await
    }

    pub async fn window_count(&self) -> usize {
        self.windows.read().await.len()
    }

    /// Find closed window by command and mark as open
    pub async fn open_window_by_command(&self, command: &str, address: String, class: String, title: String, pid: Option<u32>) -> bool {
        let mut windows = self.windows.write().await;
        if let Some(window) = windows.iter_mut().find(|w| {
            !w.is_open() && w.spawn_command == command
        }) {
            window.open(address, class, title, pid);
            true
        } else {
            false
        }
    }
}

impl Default for SpaceManager {
    fn default() -> Self {
        Self::new()
    }
}
