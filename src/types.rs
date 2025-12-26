use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWindow {
    pub id: String,  // Our own unique ID (never changes)
    #[serde(skip)]  // Don't persist temporary Hyprland data
    pub address: String,  // Temporary Hyprland window address
    #[serde(skip)]
    pub class: String,  // Temporary class name
    #[serde(skip)]
    pub title: String,  // Temporary title
    #[serde(skip)]
    pub pid: Option<u32>,  // PID - if Some, window is open; if None, window is closed/virtual
    pub spawn_command: String,  // The command to spawn/respawn this window
    #[serde(default)]
    pub custom_icon: Option<String>,  // Custom icon/emoji for the button
}

impl ManagedWindow {
    /// Create a new managed window with a unique ID
    pub fn new(command: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let id = format!("win_{}", timestamp);

        Self {
            id,
            address: String::new(),
            class: String::new(),
            title: String::from("(Not loaded)"),
            pid: None,
            spawn_command: command,
            custom_icon: None,
        }
    }

    /// Mark this window as opened (set PID and Hyprland details)
    pub fn open(&mut self, address: String, class: String, title: String, pid: Option<u32>) {
        self.address = address;
        self.class = class;
        self.title = title;
        self.pid = pid;
    }

    /// Mark this window as closed (clear PID and Hyprland details)
    pub fn close(&mut self) {
        self.address.clear();
        self.class.clear();
        self.title = String::from("(Closed)");
        self.pid = None;
    }

    /// Check if window is currently open
    pub fn is_open(&self) -> bool {
        self.pid.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Next,
    Prev,
    SwitchTo(usize),  // Switch to window at specific index
    SwapWindows(usize, usize),  // Swap two windows by index
    SetWindowIcon(usize, String),  // Set custom icon/label for a window
    Spawn(String),
    SpawnAt(usize, String, Option<String>),  // Spawn at specific index with optional icon
    List,
    Cleanup,  // Close all windows in special:spaces
    ReloadConfig,  // Reload configuration from config.json
    GetTemplates,  // Get list of command templates
    AddTemplate(String, String),  // Add new template (name, command)
    RemoveTemplate(String),  // Remove template by name
    CloseSpace(usize),  // Close window and remove space at index
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Error(String),
    Windows(Vec<ManagedWindow>),
    Templates(Vec<serde_json::Value>),  // JSON representation of templates
}
