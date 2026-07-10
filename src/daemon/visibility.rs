//! Managed-window visibility (AF-6/AF-4).
//!
//! `resolve_target_workspace` is a pure, unit-tested function (no I/O). The async
//! `update_visibility` shell asserts the single-visible invariant: the target
//! window is moved to the resolved workspace, every other managed window is
//! parked in `special:spaces`. `follow_mouse` is suppressed only here (OQ-2).

use anyhow::Result;
use tracing::{error, info};

use super::Daemon;
use crate::hypr::{self, WorkspaceTarget};
use crate::hypr_settings::FollowMouseGuard;

/// Resolve the workspace where the next visible managed window should be shown.
///
/// Pure function (R8). Preference order:
/// 1. `cached` if it is > 0 and currently visible;
/// 2. otherwise the first open managed window whose workspace is > 0 and visible;
/// 3. otherwise `cached` if it is > 0;
/// 4. otherwise the first open managed window whose workspace is > 0;
/// 5. otherwise `active_ws`.
///
/// Non-positive workspace ids are always ignored.
pub fn resolve_target_workspace(
    cached: Option<i32>,
    visible_ws: &[i32],
    window_ws: &[i32],
    active_ws: i32,
) -> i32 {
    if let Some(ws) = cached.filter(|w| *w > 0 && visible_ws.contains(w)) {
        return ws;
    }
    if let Some(&ws) = window_ws
        .iter()
        .find(|w| **w > 0 && visible_ws.contains(w))
    {
        return ws;
    }
    if let Some(ws) = cached.filter(|w| *w > 0) {
        return ws;
    }
    if let Some(&ws) = window_ws.iter().find(|w| **w > 0) {
        return ws;
    }
    active_ws
}

impl Daemon {
    /// Show `target_address`, hide all other managed windows (single-visible invariant).
    pub async fn update_visibility(&self, target_address: &str) -> Result<()> {
        let _lock = self.visibility_lock.lock().await;
        info!("Updating visibility: showing {}", target_address);

        // Suppress focus-follows-mouse for the duration of the moves (OQ-2).
        // RAII: if this future is cancelled at any await below, the guard's Drop
        // still restores the setting (the "hover focus randomly dies" bug).
        let follow_mouse_guard = FollowMouseGuard::suppress().await;

        let result: Result<i32> = async {
            let all_windows = self.manager.get_windows().await;
            let clients = hypr::clients().await.unwrap_or_default();
            let monitors = hypr::monitors().await.unwrap_or_default();

            // Determine target workspace BEFORE hiding any windows.
            let cached = *self.current_workspace.read().await;
            let visible_ws = Daemon::visible_workspaces(&monitors);
            let window_ws: Vec<i32> = all_windows
                .iter()
                .filter(|w| w.is_open())
                .filter_map(|w| clients.iter().find(|c| c.address == w.address))
                .map(|c| c.workspace_id)
                .collect();
            let active_ws = hypr::active_workspace_id().await.unwrap_or(1);

            let target_workspace =
                resolve_target_workspace(cached, &visible_ws, &window_ws, active_ws);
            info!("Target workspace: {}", target_workspace);

            // Move target to the resolved workspace silently.
            hypr::move_to_workspace_silent(
                WorkspaceTarget::Id(target_workspace),
                target_address,
            )
            .await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

            // Park all others in special:spaces.
            for win in all_windows.iter() {
                if win.address != target_address && win.is_open() {
                    info!("Hiding window: {}", win.address);
                    if let Err(e) = hypr::move_to_workspace_silent(
                        WorkspaceTarget::Special("spaces".to_string()),
                        &win.address,
                    )
                    .await
                    {
                        error!("Failed to hide window {}: {}", win.address, e);
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

            Ok(target_workspace)
        }
        .await;

        follow_mouse_guard.restore().await;

        let target_workspace = result?;
        *self.current_workspace.write().await = Some(target_workspace);
        info!("Tracked current workspace: {}", target_workspace);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_target_workspace;

    #[test]
    fn test_resolve_target_ws_prefers_cached_visible() {
        // cached=3 present in visible set -> returned.
        assert_eq!(resolve_target_workspace(Some(3), &[1, 3, 5], &[7], 9), 3);
    }

    #[test]
    fn test_resolve_target_ws_falls_back_to_visible_managed() {
        // cached=8 not visible; managed window ws=5 is visible -> 5.
        assert_eq!(resolve_target_workspace(Some(8), &[1, 5], &[5], 9), 5);
    }

    #[test]
    fn test_resolve_target_ws_falls_back_to_cached_any() {
        // Nothing visible matches; cached>0 -> cached.
        assert_eq!(resolve_target_workspace(Some(4), &[1, 2], &[7], 9), 4);
    }

    #[test]
    fn test_resolve_target_ws_defaults_to_active() {
        // No cache, no managed ws -> active.
        assert_eq!(resolve_target_workspace(None, &[1, 2], &[], 9), 9);
    }

    #[test]
    fn test_resolve_target_ws_ignores_nonpositive() {
        // cached=0 and managed ws=-1 are filtered; visible managed ws=2 chosen.
        assert_eq!(resolve_target_workspace(Some(0), &[2], &[-1, 2], 9), 2);
        // All non-positive -> active.
        assert_eq!(resolve_target_workspace(Some(0), &[0], &[0, -3], 9), 9);
    }
}
