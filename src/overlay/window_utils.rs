//! Float/center helpers for the dialog windows (settings / new-space / change-icon).
//!
//! These run on the GTK thread (not a tokio worker) and only affect the regular
//! GTK dialog windows, so they are deliberately out of scope for AF-4's "no
//! blocking subprocess" rule (which targets the async daemon paths). The overlay
//! itself is layer-shell and does not use anything here.
//!
//! Writes use Lua `hyprctl eval` / `dispatch` — hyprlang `keyword windowrule`
//! and `dispatch resizewindowpixel` are rejected by Hyprland's Lua config manager.

use std::thread;
use std::time::Duration;

use crate::hypr;

/// Apply float and center rules to a window by title.
pub fn apply_float_center_rules(window_title: &str) {
    let title = window_title.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let expr = hypr::lua_float_center_rule(&title);
        let _ = hypr::run_hyprctl_blocking(&["eval", &expr]);
    });
}

/// Apply float and center rules with explicit size enforcement.
pub fn apply_float_center_with_size(window_title: &str, width: i32, height: i32) {
    let title = window_title.to_string();
    apply_float_center_rules(&title);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        if let Some(address) = get_window_address_by_title(&title) {
            let expr = hypr::lua_resize_window(&address, width, height);
            let _ = hypr::run_hyprctl_blocking(&["dispatch", &expr]);
        }
    });
}

/// Get the address of a window by title (used to enforce dialog size).
pub fn get_window_address_by_title(title: &str) -> Option<String> {
    let output = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
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
