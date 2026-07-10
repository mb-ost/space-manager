//! Float/center helpers for the dialog windows (settings / new-space / change-icon).
//!
//! These run on the GTK thread (not a tokio worker) and only affect the regular
//! GTK dialog windows, so they are deliberately out of scope for AF-4's "no
//! blocking subprocess" rule (which targets the async daemon paths). The overlay
//! itself is layer-shell and does not use anything here.

use std::process::Command;
use std::thread;
use std::time::Duration;

/// Apply float and center rules to a window by title.
pub fn apply_float_center_rules(window_title: &str) {
    let title = window_title.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let rule = format!("float on, center on, match:title {}", title);
        let _ = Command::new("hyprctl")
            .arg("keyword")
            .arg("windowrule")
            .arg(&rule)
            .output();
    });
}

/// Apply float and center rules with explicit size enforcement.
pub fn apply_float_center_with_size(window_title: &str, width: i32, height: i32) {
    let title = window_title.to_string();
    apply_float_center_rules(&title);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        if let Some(address) = get_window_address_by_title(&title) {
            let _ = Command::new("hyprctl")
                .arg("dispatch")
                .arg("resizewindowpixel")
                .arg(format!("exact {} {},address:{}", width, height, address))
                .output();
        }
    });
}

/// Get the address of a window by title (used to enforce dialog size).
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
