/// Utilities for managing Hyprland window rules and properties

use std::process::Command;
use std::thread;
use std::time::Duration;

/// Apply float and center rules to a window by title
/// This is used for dialogs and popups that should appear centered
pub fn apply_float_center_rules(window_title: &str) {
    let title = window_title.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));

        let _ = Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrulev2")
            .arg(format!("float,title:^({})$", title))
            .output();

        let _ = Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrulev2")
            .arg(format!("center,title:^({})$", title))
            .output();
    });
}

/// Apply float rule only to a window by class
pub fn apply_float_rule_by_class(window_class: &str) {
    let class = window_class.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));

        let _ = Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrulev2")
            .arg(format!("float,class:^({})$", class))
            .output();
    });
}

/// Pin a window to all workspaces (workspace 0)
pub fn pin_window(address: &str) -> Result<(), String> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg("pin")
        .arg(format!("address:{}", address))
        .output()
        .map_err(|e| format!("Failed to execute hyprctl: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Failed to pin window: {:?}", String::from_utf8_lossy(&output.stderr)))
    }
}

/// Unpin a window from all workspaces
pub fn unpin_window(address: &str) -> Result<(), String> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg("pin")
        .arg(format!("address:{}", address))
        .output()
        .map_err(|e| format!("Failed to execute hyprctl: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Failed to unpin window: {:?}", String::from_utf8_lossy(&output.stderr)))
    }
}

/// Move a window to a specific workspace
pub fn move_to_workspace(address: &str, workspace: &str) -> Result<(), String> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg("movetoworkspacesilent")
        .arg(format!("{},address:{}", workspace, address))
        .output()
        .map_err(|e| format!("Failed to execute hyprctl: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Failed to move window: {:?}", String::from_utf8_lossy(&output.stderr)))
    }
}

/// Get the address of a window by title
pub fn get_window_address_by_title(title: &str) -> Option<String> {
    let output = Command::new("hyprctl")
        .arg("clients")
        .arg("-j")
        .output()
        .ok()?;

    let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    for client in clients.as_array()? {
        if let Some(window_title) = client["title"].as_str() {
            if window_title == title {
                return client["address"].as_str().map(|s| s.to_string());
            }
        }
    }

    None
}

/// Resize a window to exact dimensions
pub fn resize_window_exact(address: &str, width: i32, height: i32) -> Result<(), String> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg("resizewindowpixel")
        .arg(format!("exact {} {},address:{}", width, height, address))
        .output()
        .map_err(|e| format!("Failed to execute hyprctl: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Resize failed: {:?}", String::from_utf8_lossy(&output.stderr)))
    }
}

/// Move a window to exact position
pub fn move_window_exact(address: &str, x: i32, y: i32) -> Result<(), String> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .arg("movewindowpixel")
        .arg(format!("exact {} {},address:{}", x, y, address))
        .output()
        .map_err(|e| format!("Failed to execute hyprctl: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Move failed: {:?}", String::from_utf8_lossy(&output.stderr)))
    }
}

