//! State reconciliation and recovery (AF-1).
//!
//! `resync()` is the single, mutex-guarded reconciliation entry point fed by
//! three independent triggers (any one is sufficient):
//!   A. monitor/config events (debounced 150ms) — the primary DP-wake path;
//!   B. the reconnect supervisor (socket-drop path) — see `events.rs`;
//!   C. a 30s periodic consistency check (safety net) — this module.
//!
//! A failed `resync()` is logged and returned; it NEVER terminates the
//! supervisor or any trigger task. Because Trigger C fires again in <=30s and
//! Trigger B retries after backoff, a momentarily-unavailable compositor
//! recovers automatically once IPC is ready.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, error, info};

use super::rematch;
use super::Daemon;
use crate::geometry::Rect;
use crate::hypr;

/// Debounce interval for Trigger A (coalesces a DP-wake remove+add burst).
const DEBOUNCE_MS: u64 = 150;
/// Interval for Trigger C.
const CONSISTENCY_INTERVAL_SECS: u64 = 30;

/// Run a full reconciliation. Mutex-guarded so concurrent triggers coalesce: a
/// resync already in progress absorbs new requests, and exactly one re-run is
/// scheduled if a trigger arrives mid-resync.
pub async fn resync(daemon: &Daemon) -> Result<()> {
    // If a resync is already running, flag a re-run and return.
    let _guard = match daemon.resync_lock.try_lock() {
        Ok(g) => g,
        Err(_) => {
            daemon.resync_pending.store(true, Ordering::SeqCst);
            debug!("resync already in progress; scheduled a re-run");
            return Ok(());
        }
    };

    loop {
        daemon.resync_pending.store(false, Ordering::SeqCst);
        do_resync(daemon).await?;
        if !daemon.resync_pending.load(Ordering::SeqCst) {
            break;
        }
        debug!("re-running resync (trigger arrived mid-resync)");
    }
    Ok(())
}

async fn do_resync(daemon: &Daemon) -> Result<()> {
    info!("resync: reconciling state with Hyprland");

    // 1. Snapshot. On error, propagate Err (caller logs + retries on next trigger).
    let clients = hypr::clients().await?;
    let _monitors = hypr::monitors().await?;

    // 2. Re-match managed windows to live clients (pure, deterministic).
    let managed = daemon.manager.get_windows().await;
    let outcome = rematch::match_windows(&managed, &clients);
    for (id, addr, pid) in &outcome.matches {
        if let Some(c) = clients.iter().find(|c| &c.address == addr) {
            daemon
                .manager
                .open_window_by_id(id, addr.clone(), c.class.clone(), c.title.clone(), *pid)
                .await;
        }
    }
    for id in &outcome.closed {
        daemon.manager.mark_window_closed_by_id(id).await;
    }
    info!(
        "resync: {} matched, {} marked closed",
        outcome.matches.len(),
        outcome.closed.len()
    );

    // 3. Re-assert the single-visible invariant for the current window.
    let current = daemon.manager.current_window().await.filter(|w| w.is_open());
    if let Some(current) = current {
        if let Err(e) = daemon.update_visibility(&current.address).await {
            error!("resync: failed to re-assert visibility: {}", e);
        }
    }

    // 4. Refresh the overlay from the (possibly changed) monitor layout.
    daemon.update_overlay().await;
    let cfg = daemon.manager.get_overlay_config().await;
    let has_open = daemon
        .manager
        .current_window()
        .await
        .filter(|w| w.is_open())
        .is_some();
    if cfg.enabled && has_open {
        daemon.show_overlay().await;
    } else {
        daemon.hide_overlay().await;
    }

    Ok(())
}

/// Trigger A: schedule a debounced resync (150ms). Cancels any pending debounce
/// so a burst of monitor/config events collapses into ONE resync.
pub async fn schedule_resync(daemon: Arc<Daemon>) {
    let mut timer = daemon.resync_debounce.write().await;
    if let Some(handle) = timer.take() {
        handle.abort();
    }
    let d = daemon.clone();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(DEBOUNCE_MS)).await;
        if let Err(e) = resync(&d).await {
            error!("debounced resync failed (will retry on next trigger): {}", e);
        }
    });
    *timer = Some(handle);
}

/// Trigger C: spawn the 30s periodic consistency check (never exits).
pub fn spawn_consistency_check(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(CONSISTENCY_INTERVAL_SECS));
        interval.tick().await; // consume the immediate first tick
        loop {
            interval.tick().await;
            if let Err(e) = consistency_check(&daemon).await {
                debug!("consistency check skipped (IPC unavailable): {}", e);
            }
        }
    });
}

/// Cheap drift probe. Issues no dispatches unless drift is detected.
async fn consistency_check(daemon: &Daemon) -> Result<()> {
    let clients = hypr::clients().await?;
    let monitors = hypr::monitors().await?;

    let mut drift = false;

    // (1) Current tracked window's address still exists?
    if let Some(current) = daemon.manager.current_window().await.filter(|w| w.is_open()) {
        if !clients.iter().any(|c| c.address == current.address) {
            info!("consistency check: tracked window address gone; drift");
            drift = true;
        }
    }

    // (2) The monitor the last Reposition used still exists with the same geometry?
    if let Some(rs) = daemon.last_reposition.read().await.clone() {
        let ok = monitors.iter().any(|m| {
            m.name == rs.monitor_name
                && Rect::new(m.x, m.y, m.width, m.height) == rs.monitor_rect
        });
        if !ok {
            info!("consistency check: reposition monitor changed/gone; drift");
            drift = true;
        }
    }

    if drift {
        info!("consistency check detected drift; scheduling resync");
        resync(daemon).await?;
    }
    Ok(())
}
