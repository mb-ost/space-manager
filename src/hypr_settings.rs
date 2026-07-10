//! `follow_mouse` suppression for managed-window moves (OQ-2).
//!
//! Focus-follows-mouse can steal focus while managed browser windows are shuffled
//! between workspaces during rapid switching. [`FollowMouseGuard`] temporarily sets
//! `input:follow_mouse` to 0 and restores it, going through the typed `hypr`
//! layer (no blocking subprocess, AF-4).
//!
//! The overlay no longer uses this at all (it is a layer-shell surface, not a
//! Hyprland client, so its moves never touch focus). It is scoped strictly to
//! `visibility::update_visibility`.
//!
//! Restoration must survive task cancellation: the suppressed section contains
//! await points, and a cancelled future never reaches an explicit restore call.
//! The guard therefore restores from `Drop` (by spawning onto the current
//! runtime), and the stashed original is also mirrored in a global so
//! [`force_restore`] can undo a live suppression during daemon shutdown.

use std::sync::Mutex;

use tracing::{info, warn};

const FOLLOW_MOUSE_KEY: &str = "input:follow_mouse";

/// Original `follow_mouse` value while a suppression is live, `None` otherwise.
/// `update_visibility` is serialized by `visibility_lock`, so at most one
/// suppression exists at a time.
static SUPPRESSED_ORIGINAL: Mutex<Option<i64>> = Mutex::new(None);

fn stash(original: Option<i64>) {
    *SUPPRESSED_ORIGINAL.lock().unwrap_or_else(|p| p.into_inner()) = original;
}

fn take_stash() -> Option<i64> {
    SUPPRESSED_ORIGINAL
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
}

async fn set_follow_mouse(value: i64) {
    if let Err(e) = crate::hypr::keyword_set(FOLLOW_MOUSE_KEY, &value.to_string()).await {
        warn!("Failed to set follow_mouse to {}: {}", value, e);
    } else {
        info!("Restored follow_mouse to {}", value);
    }
}

/// RAII suppression of focus-follows-mouse.
///
/// Restore happens on [`FollowMouseGuard::restore`] or, if the owning future is
/// cancelled/dropped first, from `Drop` via a task spawned on the current
/// runtime. Prefer calling `restore()` explicitly so the value is back before
/// the caller continues.
pub struct FollowMouseGuard {
    original: Option<i64>,
}

impl FollowMouseGuard {
    /// Read and stash the current `follow_mouse` value, then set it to 0.
    ///
    /// If the option cannot be read, nothing is changed and the guard is inert.
    pub async fn suppress() -> Self {
        match crate::hypr::get_option_int(FOLLOW_MOUSE_KEY).await {
            Ok(original) => {
                if let Err(e) = crate::hypr::keyword_set(FOLLOW_MOUSE_KEY, "0").await {
                    warn!("Failed to suppress follow_mouse: {}", e);
                    return Self { original: None };
                }
                info!("Suppressed follow_mouse (was {})", original);
                stash(Some(original));
                Self {
                    original: Some(original),
                }
            }
            Err(e) => {
                warn!("Failed to read follow_mouse, not suppressing: {}", e);
                Self { original: None }
            }
        }
    }

    /// Explicitly restore the original value (the normal path).
    pub async fn restore(mut self) {
        if let Some(value) = self.original.take() {
            take_stash();
            set_follow_mouse(value).await;
        }
    }
}

impl Drop for FollowMouseGuard {
    fn drop(&mut self) {
        // Only reached when the owning future was cancelled or dropped early —
        // the explicit restore() path clears `original` first.
        if let Some(value) = self.original.take() {
            take_stash();
            warn!("FollowMouseGuard dropped without restore (cancelled?); restoring async");
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(set_follow_mouse(value));
            } else {
                // No runtime (process teardown): best-effort blocking fallback.
                // Documented exception to the no-subprocess rule (AF-4).
                let _ = std::process::Command::new("hyprctl")
                    .args(["keyword", FOLLOW_MOUSE_KEY, &value.to_string()])
                    .output();
            }
        }
    }
}

/// Undo a live suppression, if any. Called during daemon shutdown so an
/// in-flight `update_visibility` can never leave `follow_mouse` at 0 after the
/// process exits.
pub async fn force_restore() {
    if let Some(value) = take_stash() {
        warn!("Shutdown with follow_mouse still suppressed; restoring");
        set_follow_mouse(value).await;
    }
}
