# Space Manager Backend Redesign - Code Review

**Reviewer:** Code Review Agent
**Date:** 2026-07-10
**Branch:** `redesign/backend-reliability` (commit `1312b70`) vs `main`
**Status:** Approved

---

## Overview

The redesign faithfully implements the approved REFINEMENT.md. The fragile pieces (dead event listener, GTK-window-as-Hyprland-client overlay, multiple ad-hoc tokio runtimes, ~40 blocking `hyprctl` subprocess calls) are gone and replaced with the specified resilient equivalents: a single tokio runtime, a single long-lived `gtk4::Application` layer-shell overlay, a typed `hypr` repository layer, and a three-trigger mutex-coalesced `resync()`. `cargo build`, `cargo build --release`, and `cargo clippy --all-targets` are clean (zero warnings); all 52 unit tests pass and map 1:1 to the Test Plan. The findings below are Low/Medium hardening items and design-guarantee nuances; none block approval.

| Category | Rating |
|----------|--------|
| Architecture Compliance | Pass |
| Repository/Data Layer Pattern | Pass |
| Test Coverage | Pass |
| Error Handling | Pass |
| Security / Robustness | Pass |
| Refinement Compliance | Pass |

Build/test evidence:
- `cargo build` — clean.
- `cargo build --release` — `Finished release [optimized]`, no errors/warnings.
- `cargo clippy --all-targets` — no warnings, no errors.
- `cargo test` — `52 passed; 0 failed`.

---

## Critical Issues

None.

## High Priority Issues

None.

## Medium Priority Issues

### 1. `resync()` coalescing has a narrow TOCTOU window that can drop an immediate re-run (Medium)

**File:** `src/daemon/recovery.rs:33-53`

**Refinement promise (R1):** "concurrent triggers coalesce rather than overlap ... exactly one re-run is scheduled if a trigger arrives mid-resync."

**Current state:** Coalescing uses `try_lock()` plus an `AtomicBool` pending flag:

```rust
let _guard = match daemon.resync_lock.try_lock() {
    Ok(g) => g,
    Err(_) => { daemon.resync_pending.store(true, Ordering::SeqCst); return Ok(()); }
};
loop {
    daemon.resync_pending.store(false, Ordering::SeqCst);
    do_resync(daemon).await?;
    if !daemon.resync_pending.load(Ordering::SeqCst) { break; }
}
```

There is a race between the holder reading `pending == false` / breaking and dropping `_guard`. Interleaving:
1. Task A finishes `do_resync`, loads `pending` (false), breaks.
2. Task B calls `resync`, `try_lock` still fails (A holds the guard), sets `pending = true`, returns.
3. Task A drops the guard.

`pending` is now `true` but no task is running, so B's requested re-run is not executed immediately.

**Impact (bounded):** The stale `pending = true` is harmless — it is overwritten to `false` at the top of the next `resync()`, and that next `resync()` performs a full reconciliation anyway. Because Trigger C (30 s consistency check) and Trigger B (backoff reconnect) always fire again, the reconcile B wanted is fulfilled by the next trigger within ≤30 s. So the observable effect is at most a delayed (not lost) reconciliation in a rare interleaving. This does not reproduce the user's failure (the DP-wake burst is debounced into one scheduled resync via Trigger A, and the safety net converges regardless).

**Suggested fix (non-blocking):** Close the window by re-checking under the lock, e.g. use a `tokio::sync::Notify` or acquire the lock via `.lock().await` in a supervisor "resync worker" fed by a bounded/rendezvous request channel, or re-load `pending` after deciding to break while still holding the guard and loop again if it flipped. Not required for correctness given the safety net, but it would make the stated "exactly one re-run" guarantee literally true.

## Low Priority Issues

### 2. Supervisor / trigger tasks survive `Err` but not a panic (Low)

**Files:** `src/daemon/events.rs:47-70` (supervisor), `src/daemon/recovery.rs:126-138` (consistency check), `:110-123` (debounce)

**Refinement checklist:** "Recovery supervisor and all trigger tasks are infinite/self-restarting; a failed `resync()` is logged and retried, never terminal." Focus-area requirement: "no path where an error or panic terminates them silently."

**Current state:** `Err` paths are handled correctly (logged, loop continues). A *panic* inside `resync`/`do_resync`/`consistency_check`, however, would abort the spawned tokio task and it would not restart (the panic hook logs it, but nothing respawns the loop).

**Impact:** No reachable panic exists in these paths today — the daemon paths contain no `unwrap`/`expect`, `rematch` indexing is provably in-bounds, and `build_spaces`/geometry clamp rather than index unsafely. So this is a theoretical hardening gap, not a live defect.

**Suggested hardening (non-blocking):** Wrap each infinite loop body in `std::panic::AssertUnwindSafe(...).catch_unwind()` (or spawn a watchdog that re-spawns a died task) so a future accidental panic cannot silently kill recovery.

### 3. Overlay channel latest-wins coalescing not implemented (as explicitly deferred) (Low)

**File:** `src/overlay/bar.rs:63-77`

R3 specifies cap-64 `try_send` with `warn!` on full is acceptable for the first cut, with latest-wins coalescing as the documented target. The code matches the accepted first cut. Full-channel behavior cannot wedge the bar permanently: the GTK receiver drains continuously on the glib main context (no blocking work runs on the GTK main thread), and a dropped `Show`/`Reposition` is re-sent by the next `resync()`/workspace event (≤30 s worst case via Trigger C). No action required; tracked as the documented follow-up.

### 4. `clamp_current()` helper not extracted; clamping is inlined per mutator (Low)

**File:** `src/manager.rs:354-563`

R6 suggested "Add explicit invariant helper `clamp_current()` invoked by add/insert/remove/swap." The implementation instead clamps inline in each mutator (`add_window`, `insert_window_at`, `remove_window_by_address`, `remove_window_at_index`, `swap_windows`, `remove_all_windows`). This is functionally correct and fully covered by the index-invariant tests, and `current_window()` is bounds-safe via `windows.get(*current)` (`manager.rs:528-533`). The deviation is cosmetic (no shared helper), not a correctness issue.

### 5. `overlay/window_utils.rs` still uses blocking `hyprctl` subprocess for dialog float/center (Low)

**File:** `src/overlay/window_utils.rs:18,34,45`

R4 states "No raw `hyprctl` subprocess anywhere. Any capability the crate lacks must be documented ... in [hypr/mod.rs]." `window_utils` still shells out to `hyprctl keyword windowrule` / `dispatch resizewindowpixel` / `clients -j`. This is within the refinement's scope carve-out — Migration note 5 keeps dialog window rules because the settings/new-space/change-icon dialogs are still normal GTK windows, and the module doc (`window_utils.rs:1-6`) declares the exception. All three functions run on `std::thread::spawn` (verified), so they never block the GTK main thread or a tokio worker. Two nuances worth noting: (a) the AF-4 exception is documented in `window_utils.rs` rather than in `hypr/mod.rs` as R4 asked; (b) `apply_float_center_rules` appends a persistent `windowrule` on every dialog open, which accumulates rules over time (pre-existing behavior, not a redesign regression). Neither affects daemon reliability.

### 6. Guarded-but-present `unwrap()` in the (frozen) input path (Low)

**File:** `src/input.rs:52`

`device.supported_keys().unwrap()` is guarded by the `supported_keys().is_some()` check two lines above, so it cannot panic. `input.rs` is a frozen file per the refinement (behavior unchanged). No action needed; noted for completeness.

---

## Corrections to Automated Analysis

- `Runtime::new`/`block_on` matches in `src/manager.rs:619-620,729` are **test-only** (`#[cfg(test)]`, using a `current_thread` runtime to exercise the async `SpaceManager` API), which is exactly what R8 permits. There is no `Runtime::new`/`block_on` anywhere in production code — the single `#[tokio::main]` runtime is the only runtime. Not a finding.
- `special:spaces` grep hits are all legitimate managed-window parking / the `Cleanup` command (`visibility.rs`, `commands.rs`), **not** overlay code. The overlay uses none of the old pin/`movewindowpixel`/`special:spaces`/title-scan/`saved_position` paths (grep confirmed zero occurrences for the overlay). Not a finding.

---

## Positive Findings

| Area | Status | Notes |
|------|--------|-------|
| Single tokio runtime | Pass | Only `#[tokio::main]` (`bin/daemon.rs:15`); no `Runtime::new`/`block_on` in production. AF-3 resolved. |
| Single GTK Application | Pass | Created once in `overlay/bar.rs:83-100`; `app.hold()` keeps it alive; never recreated. AF-2 second-`Application` bug structurally eliminated. |
| Layer-shell overlay | Pass | `init_layer_shell` + `Overlay` layer + `KeyboardMode::None` + namespace (`bar.rs:114-120`); Show/Hide = `set_visible` (idempotent, no toggle state → AC-d). Anchors reset-then-set on Reposition (`bar.rs:299-312`). |
| Recovery triggers | Pass | Trigger A debounced 150 ms (`recovery.rs:110-123`, `events.rs:114-154`), Trigger B backoff 250 ms→x2→cap 5 s reset-after-30 s (`events.rs:22-70`), Trigger C 30 s drift probe (`recovery.rs:126-172`). All funnel into one mutex-guarded `resync()`. Supervisor loop never returns. |
| DP-wake burst handling | Pass | `monitorremoved`+`monitoradded` both call `schedule_resync`, which aborts the prior debounce timer → the burst collapses to one resync. No thrash; no lock held across the debounce. |
| `hypr` repository layer | Pass | All Hyprland reads/writes go through `hypr::*` typed async wrappers over `hyprland-rs`; local `ClientInfo`/`MonitorInfo` keep crate types from leaking. No blocking `hyprctl` on any tokio worker. AF-4 resolved. |
| Pure `match_windows` | Pass | Deterministic PID → title → oldest-id-first → mark-closed (`rematch.rs`); never speculatively binds; a client is assigned at most once. 9 positive/negative tests assert exact address pairings. resync marks unmatched windows closed (`recovery.rs:73-75`). |
| Atomic writes | Pass | `atomic_write` (`manager.rs:12-30`): sibling `.tmp`, `write_all` + `flush` + `sync_all` (fsync), then `rename` — same filesystem. Used by both `save_state` and `save_config`. AF-6 resolved. |
| Bounds-safe index | Pass | `current_window()` uses `windows.get(*current)`; every mutator clamps `current_index`. Regression test `test_current_window_empty_returns_none`. |
| Graceful shutdown from GTK | Pass | Close button → `ipc_helpers::shutdown_daemon` (threaded, non-blocking) → `Command::Shutdown` → `shutdown()` (save state, close windows, `OverlayMsg::Shutdown`→`app.quit`) then delayed `exit(0)`. No `pkill`/race. AF-6 resolved. |
| Logging + panic hook | Pass | `logging.rs` daily-rotating non-blocking file writer + stdout + `set_hook` capturing payload/location/backtrace at `error!`. Guard held for process lifetime in `bin/daemon.rs:18`. AF-5 resolved. |
| No GTK-main-thread blocking IPC | Pass | Every `ipc_helpers` send spawns a `std::thread`; `get_templates_sync` is invoked inside `std::thread::spawn` (`template_dialogs.rs:51`). The glib receiver does only widget work. |
| Lock ordering | Pass | Consistent ordering: `resync_lock` → `visibility_lock`; within `SpaceManager`, `windows` → `current_index` everywhere. No reverse-order acquisition; no deadlock candidate found. |
| Module split + `lib.rs` | Pass | `daemon/{mod,events,recovery,rematch,commands,visibility,lifecycle}`, `overlay/{bar,model,settings_dialog,template_dialogs}`, `hypr`, `geometry`, `logging` all present; `bin/daemon.rs` is a 27-line consumer. `lib.rs` exposes the pure modules for `cargo test`. |
| IPC backward compat | Pass | Length-prefixed JSON unchanged; `Command::Shutdown` added additively; `test_backward_compat_legacy_command_json` guards the wire format. |

---

## Summary

| # | Severity | Issue | Action Required |
|---|----------|-------|-----------------|
| 1 | Medium | `resync()` coalescing TOCTOU can defer (not lose) a re-run | Optional: re-check pending under lock / use Notify. Mitigated by 30 s safety net |
| 2 | Low | Supervisor/trigger loops survive `Err` but not a panic | Optional hardening: `catch_unwind`/watchdog. No reachable panic today |
| 3 | Low | Channel latest-wins coalescing not implemented (deferred) | None — accepted first cut, cannot wedge permanently |
| 4 | Low | `clamp_current()` inlined instead of extracted | None — functionally correct, fully tested |
| 5 | Low | `window_utils` blocking `hyprctl` for dialogs | Optional: document exception in `hypr/mod.rs`; threaded so no reliability impact |
| 6 | Low | Guarded `unwrap()` in frozen `input.rs` | None — cannot panic; file is out of scope |

---

## Test Coverage vs Test Plan

| Group | Planned | Implemented | Status |
|-------|---------|-------------|--------|
| overlay/model | 5 | 5 | Complete |
| geometry | 12 | 12 | Complete |
| daemon/visibility | 5 | 5 | Complete |
| daemon/rematch | 9 | 9 | Complete |
| manager index invariants | 9 | 9 | Complete |
| config serde | 5 | 5 | Complete |
| ipc framing | 6 | 6 | Complete |
| **Total unit** | **51** | **52** (+1 helper) | **All passing** |

Integration tests (AC-b/c/c2/d/e — reconnect, DPMS, output remove/add, periodic-check recovery, idempotent show/hide, logs) are defined in the plan as **manual/scripted on live Hyprland**. They are not automatable in CI and are correctly out of the unit-test suite. The architecture that makes them pass is in place and verified by inspection; they must be exercised on hardware before the branch is relied upon in the field.

No unit-test gaps. Tests assert meaningful behavior (exact re-match address pairings, exact computed margins, clamped/bounded outputs, truncated-frame errors) rather than tautologies.

---

## Recommendations

### Before Production Deployment
1. Run the manual integration checks on live Hyprland 0.55.4: AC-b (kill event socket), AC-c (`dispatch dpms off/on`), AC-c2 (`keyword monitor <DP>,disable` → re-enable), AC-d (repeated Show/Hide), AC-e (confirm `~/.space-manager/logs/`). These validate the DP-wake regression the redesign targets.
2. During AC-c2, confirm the remove+add burst collapses to a single `resync` in the logs ("scheduling debounced resync" appears twice, "resync: reconciling" once).

### Should Fix Soon
3. Close the `resync()` coalescing TOCTOU (Issue 1) so the "exactly one re-run" guarantee is literal rather than safety-net-dependent.
4. Wrap the supervisor and periodic-check loop bodies in `catch_unwind` (Issue 2) so a future accidental panic cannot silently disable recovery.

### Nice to Have
5. Implement the latest-wins coalescing slot for `UpdateSpaces`/`Reposition` (Issue 3) as the R3 target.
6. Extract `clamp_current()` (Issue 4) and move the `window_utils` subprocess exception note into `hypr/mod.rs` (Issue 5) for spec-literal AF-4 documentation.

---

## Sign-Off

**Reviewer:** Code Review Agent
**Date:** 2026-07-10
**Verdict:** Approved

The implementation matches the approved specification, builds and lints cleanly, passes the full Test Plan, and correctly realizes the reliability architecture (three-trigger coalesced recovery, layer-shell overlay, single runtime, single GTK app, typed `hypr` layer, atomic persistence, graceful shutdown). All findings are Low/Medium hardening items with bounded, non-regressive impact and none block release. Approval is contingent on running the manual integration checks (AC-b/c/c2/d/e) on live hardware, which are inherently outside the automated suite.
