use anyhow::Result;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use tracing::{error, info};

/// Get the socket path for the Space Manager daemon
fn get_socket_path() -> String {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| format!("{}/space-manager.sock", d))
        .unwrap_or_else(|_| "/tmp/space-manager.sock".to_string())
}

/// Send a command to the daemon without waiting for a response
pub fn send_command_async(cmd: Value) {
    std::thread::spawn(move || {
        if let Err(e) = send_command_sync(cmd) {
            error!("Failed to send IPC command: {}", e);
        }
    });
}

/// Send a command to the daemon and wait for a response
pub fn send_command_with_response_async<F>(cmd: Value, callback: F)
where
    F: FnOnce(Result<Value>) + Send + 'static,
{
    std::thread::spawn(move || {
        let result = send_command_with_response_sync(cmd);
        callback(result);
    });
}

/// Send a command synchronously (blocking)
fn send_command_sync(cmd: Value) -> Result<()> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)?;

    let data = serde_json::to_vec(&cmd)?;
    let len = (data.len() as u32).to_le_bytes();

    stream.write_all(&len)?;
    stream.write_all(&data)?;
    stream.flush()?;

    Ok(())
}

/// Send a command and read the response synchronously (blocking)
fn send_command_with_response_sync(cmd: Value) -> Result<Value> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(&socket_path)?;

    // Send command
    let data = serde_json::to_vec(&cmd)?;
    let len = (data.len() as u32).to_le_bytes();
    stream.write_all(&len)?;
    stream.write_all(&data)?;
    stream.flush()?;

    // Read response
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let response_len = u32::from_le_bytes(len_bytes) as usize;

    let mut response_data = vec![0u8; response_len];
    stream.read_exact(&mut response_data)?;

    let response: Value = serde_json::from_slice(&response_data)?;
    Ok(response)
}

// Convenience functions for common commands

/// Switch to a specific space by index
pub fn switch_to_space(index: usize) {
    info!("Switching to space {}", index);
    send_command_async(serde_json::json!({"SwitchTo": index}));
}

/// Swap two windows by their indices
pub fn swap_windows(index1: usize, index2: usize) {
    info!("Swapping windows at indices {} and {}", index1, index2);
    send_command_async(serde_json::json!({"SwapWindows": [index1, index2]}));
}

/// Set a custom icon for a window
pub fn set_window_icon(index: usize, icon: String) {
    info!("Setting icon for window {} to: {}", index, icon);
    send_command_async(serde_json::json!({"SetWindowIcon": [index, icon]}));
}

/// Close a space at the given index
pub fn close_space(index: usize) {
    info!("Closing space at index {}", index);
    send_command_async(serde_json::json!({"CloseSpace": index}));
}

/// Reload configuration from config.json
pub fn reload_config() {
    info!("Reloading configuration");
    send_command_async(serde_json::json!("ReloadConfig"));
}

/// Get list of command templates
pub fn get_templates<F>(callback: F)
where
    F: FnOnce(Result<Value>) + Send + 'static,
{
    send_command_with_response_async(serde_json::json!("GetTemplates"), callback);
}

/// Add a new command template
pub fn add_template(name: String, command: String) {
    info!("Adding template: {} -> {}", name, command);
    send_command_async(serde_json::json!({"AddTemplate": [name, command]}));
}

/// Remove a command template
pub fn remove_template(name: String) {
    info!("Removing template: {}", name);
    send_command_async(serde_json::json!({"RemoveTemplate": name}));
}

/// Spawn a new space at a specific index
pub fn spawn_at(index: usize, command: String, icon: Option<String>) {
    info!("Spawning new space at index {} with command: {}", index, command);
    send_command_async(serde_json::json!({"SpawnAt": [index, command, icon]}));
}

/// Get templates synchronously (blocking call for GTK thread)
pub fn get_templates_sync() -> Option<Value> {
    let cmd = serde_json::json!("GetTemplates");
    match send_command_with_response_sync(cmd) {
        Ok(response) => {
            if let Some(templates) = response.get("Templates") {
                Some(templates.clone())
            } else {
                None
            }
        }
        Err(e) => {
            error!("Failed to get templates: {}", e);
            None
        }
    }
}

