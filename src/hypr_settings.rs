//! `follow_mouse` suppression for managed-window moves (OQ-2).
//!
//! Focus-follows-mouse can steal focus while managed browser windows are shuffled
//! between workspaces during rapid switching. These async helpers temporarily set
//! `input:follow_mouse` to 0 and restore it afterwards, going through the typed
//! `hypr` layer (no blocking subprocess, AF-4).
//!
//! The overlay no longer uses this at all (it is a layer-shell surface, not a
//! Hyprland client, so its moves never touch focus). It is scoped strictly to
//! `visibility::update_visibility`.

use tracing::{info, warn};

const FOLLOW_MOUSE_KEY: &str = "input:follow_mouse";

/// Read and stash the current `follow_mouse` value, then set it to 0.
///
/// Returns the original value so the caller can restore it. Returns `None` if the
/// option could not be read (in which case nothing is changed and restore is a
/// no-op).
pub async fn suppress_follow_mouse() -> Option<i64> {
    match crate::hypr::get_option_int(FOLLOW_MOUSE_KEY).await {
        Ok(original) => {
            if let Err(e) = crate::hypr::keyword_set(FOLLOW_MOUSE_KEY, "0").await {
                warn!("Failed to suppress follow_mouse: {}", e);
                return None;
            }
            info!("Suppressed follow_mouse (was {})", original);
            Some(original)
        }
        Err(e) => {
            warn!("Failed to read follow_mouse, not suppressing: {}", e);
            None
        }
    }
}

/// Restore `follow_mouse` to a previously stashed value (no-op on `None`).
pub async fn restore_follow_mouse(original: Option<i64>) {
    if let Some(value) = original {
        if let Err(e) = crate::hypr::keyword_set(FOLLOW_MOUSE_KEY, &value.to_string()).await {
            warn!("Failed to restore follow_mouse to {}: {}", value, e);
        } else {
            info!("Restored follow_mouse to {}", value);
        }
    }
}
