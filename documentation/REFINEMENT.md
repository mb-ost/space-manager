# Space Manager Backend Redesign - Planning & Refinement

## Overview

- **Purpose:** Redesign the space-manager daemon backend so that window tracking and the GTK overlay survive DPMS/monitor sleep, Hyprland reloads, and event-socket drops without losing the overlay or crashing.
- **Principle:** This is a *reliability and organization* redesign. It replaces the fragile parts of the current architecture (dead event listener, GTK-window-as-Hyprland-client overlay, multiple ad-hoc tokio runtimes, ~40 blocking `hyprctl` subprocess calls) with resilient equivalents. It does **not** change the CLI surface, the `config.json` schema (except tolerated additive fields), or add user-facing features.

Target platform: Rust (edition 2021), Hyprland 0.55.4, GTK4 (gtk4 crate 0.9 / glib 0.20), wlr-layer-shell via `gtk4-layer-shell`.

---

## Problem Statement

The user reports: after a DisplayPort monitor turns off and back on (DPMS/sleep/wake), overlay tracking breaks, the overlay gets lost, and the daemon occasionally crashes. Recovery currently requires a manual daemon restart.

An audit of the current code (`src/bin/daemon.rs`, `src/overlay/manager.rs`, `src/manager.rs`, `src/ipc.rs`, `src/process.rs`, `src/hypr_settings.rs`, `src/input.rs`, `src/types.rs`) confirmed the following root causes. Each is assigned an ID (AF-1..AF-8) and is traced through the rest of this document.

| ID | Severity | Root cause | Evidence |
|----|----------|-----------|----------|
| AF-1 | Critical | Event listener never reconnects. When `EventListener::start_listener()` returns (socket dropped on monitor sleep / Hyprland reload), the error is logged and the spawned thread exits permanently. No further window/workspace events are received; tracking is dead until manual restart. | `daemon.rs:1373-1376` |
| AF-2 | Critical | Overlay is a normal GTK4 `ApplicationWindow` (a regular Hyprland client), floated + nofocus via windowrules, positioned via `hyprctl dispatch movewindowpixel/resizewindowpixel`, located by scanning `hyprctl clients -j` for title `"Space Manager Overlay"`, made sticky via `hyprctl dispatch pin` (a **toggle**), hidden by moving to `special:spaces`. Pin desync permanently inverts show/hide; title-scan is racy; after display reset Hyprland drops the surface; the recreation path spawns a **second** `gtk4::Application` with the same application id `"com.spacermanager.overlay"` in a new thread of the same process, which fails GTK's application-uniqueness rule so the overlay never returns; saved position is lost on wake. | `manager.rs:150-224` (pin/hide/show), `manager.rs:232-251` (title scan), `manager.rs:504-542` (second `Application::builder()` per spawn) |
| AF-3 | High | Three tokio runtimes plus a glib block_on polling tick. `#[tokio::main]` (main), a second `tokio::runtime::Runtime::new().unwrap()` inside the event-listener thread, a third `Runtime::new().unwrap()` created inside GTK `activate` to `block_on` a `RwLock` read, and a 100 ms `glib::timeout_add_local` that calls `glib::MainContext::default().block_on(...)` to diff a label string and rebuild buttons. | `daemon.rs:1277`, `manager.rs:762-766`, `manager.rs:851-854` |
| AF-4 | High | ~40 blocking `std::process::Command("hyprctl")` calls inside async handlers (activewindow, cursorpos, clients, monitors, dispatch, keyword, getoption). These block the tokio worker thread and depend on subprocess exit codes and JSON scraping. | `daemon.rs:67-113,660-691,800-817,1052-1063,1414-1447`; `manager.rs:160-466`; `hypr_settings.rs:17-33` |
| AF-5 | Medium | Zero persisted logs. `tracing_subscriber::fmt::init()` writes to stdout only; launched by Hyprland `exec`, so logs are lost. No panic hook. | `daemon.rs:1523` |
| AF-6 | Medium | Crash/fragility inventory: `Runtime::new().unwrap()` (multiple); overlay close button runs `pkill -x space-manager` + `std::process::exit(0)` from the GTK thread, racing graceful shutdown; `SpaceManager::current_window()` indexes `windows[*current]` and can panic if the index is stale vs. list mutation; `state.json` written non-atomically (direct `tokio::fs::write`). | `manager.rs:822`, `manager.rs:496-505`, `manager.rs:226` |
| AF-7 | Medium | Organization: `daemon.rs` is 1605 lines and `overlay/manager.rs` is 1951 lines, each mixing multiple responsibilities (daemon logic, overlay window, settings dialog, template dialogs). | file sizes |
| AF-8 | Mandatory | No unit tests exist for pure logic (indicator model, edge-zone hit test, target-workspace resolution, index invariants, config serde defaults, IPC frame roundtrip). Violates the TDD golden rule. | absence |

---

## Current Architecture (brief)

```
Hyprland ──events──► [event-listener thread]  (own tokio Runtime)     ── AF-1 no reconnect
                          │ spawns handlers ──► Daemon async methods
                          ▼
spacectl ──unix sock──► [#[tokio::main] runtime] ──► handle_command ──► SpaceManager (RwLock state)
                          │                                    │
                          │                                    ├─► ~40 blocking hyprctl subprocess  ── AF-4
                          │                                    ▼
                          │                             update_visibility (movetoworkspacesilent)
                          ▼
evdev thread ──mpsc──► handle_mouse_button ──► check_mouse_position (hyprctl cursorpos/activewindow)

OverlayManager ──► spawn_gtk_window() ──► [GTK thread] gtk4::Application (client window)  ── AF-2
                       │ 100ms glib tick block_on(RwLock) diff label ── AF-3
                       └► hyprctl movewindowpixel / pin / clients title-scan ── AF-2/AF-4
```

Key state lives in `SpaceManager` (windows `Vec<ManagedWindow>`, `current_index`) behind tokio `RwLock`s. Persistence: `~/.space-manager/state.json` (windows + current id) and `~/.space-manager/config.json` (overlay + mouse + templates). Hyprland window addresses are runtime-only (`#[serde(skip)]`); on restart every window is "closed" (pid `None`) and re-spawned on demand.

---

## Target Architecture

### Design pillars

1. **One tokio runtime.** `#[tokio::main]` owns everything async. No `Runtime::new()` anywhere else.
2. **One GTK thread**, started once at startup, owning a single long-lived `gtk4::Application`. It is never recreated.
3. **Layer-shell overlay.** The overlay is a `gtk4-layer-shell` surface (not a Hyprland client). Show/hide is widget visibility. Position is layer-shell anchors + margins. No windowrules, no `pin`, no `movewindowpixel`, no title scans, no `special:spaces` moves for the overlay.
4. **Typed Hyprland access through one `hypr` repository module** wrapping `hyprland-rs` async API. No raw `hyprctl` subprocess except documented exceptions.
5. **Resilient, event-driven recovery.** `resync()` is triggered by monitor/layout events (primary DP-wake path), by a reconnect-with-backoff supervisor (socket-drop path), and by a 30 s periodic consistency check (safety net) — not by socket reconnect alone. A failed `resync()` never kills the supervisor.
6. **Message-passing** between daemon (tokio) and overlay (GTK): daemon → GTK via `async_channel`; GTK → daemon via the existing unix-socket IPC.

### Module map

```
src/
├── lib.rs                     # module declarations (see R7 for the exact new declaration set)
├── bin/
│   ├── daemon.rs              # thin: parse args, init logging, build Daemon, start subsystems, block on IPC   (~120 lines)
│   └── cli.rs                 # UNCHANGED (CLI surface frozen)
├── daemon/
│   ├── mod.rs                 # Daemon struct + shared state (Arc fields), constructor
│   ├── events.rs              # AF-1 EventListener wiring + reconnect loop + recovery-trigger dispatch
│   ├── recovery.rs            # AF-1 resync() orchestration + 30s periodic consistency check (safety net)
│   ├── rematch.rs             # AF-1/AF-8 PURE match_windows(managed, clients) re-match heuristic
│   ├── commands.rs            # AF-7 Command dispatch (handle_command), incl. ResetOverlayPosition
│   ├── visibility.rs          # AF-6/AF-4 show/hide, target-workspace resolution (pure fn + async shell)
│   └── lifecycle.rs           # startup/restore/shutdown, signal handling
├── hypr/
│   └── mod.rs                 # AF-4 typed hyprland-rs wrapper (repository layer)
├── overlay/
│   ├── mod.rs                 # exports OverlayHandle + OverlayMsg
│   ├── bar.rs                 # AF-2 layer-shell bar UI + GTK thread main
│   ├── model.rs               # AF-8 pure indicator/spaces model (SpaceButton list) generation
│   ├── settings_dialog.rs     # AF-7 settings dialog (moved out of manager.rs)
│   ├── template_dialogs.rs    # AF-7 new-space / change-icon / template dialogs
│   ├── dialog_utils.rs        # UNCHANGED
│   ├── ipc_helpers.rs         # UNCHANGED (GTK → daemon IPC)
│   ├── theme.rs               # UNCHANGED
│   ├── ui_components.rs       # UNCHANGED
│   └── window_utils.rs        # trimmed (float/center helpers still used by dialogs)
├── input.rs                   # evdev listener UNCHANGED behavior; device-hotplug rescan = stretch
├── process.rs                 # ProcessLauncher — UNCHANGED. Spawns USER apps via tokio::process::Command
│                              #   (async, non-blocking). Deliberately OUT OF SCOPE for AF-4's
│                              #   "no blocking subprocess" rule: it launches browsers, not hyprctl.
├── ipc.rs                     # UNCHANGED protocol; frame encode/decode extracted for tests (AF-8)
├── manager.rs                 # SpaceManager; add index-invariant helpers + tests (AF-6/AF-8)
├── hypr_settings.rs           # FollowMouseGuard; overlay no longer uses it (AF-2)
├── logging.rs                 # AF-5 tracing-appender file logging + panic hook
├── geometry.rs                # AF-8 pure geometry: edge-zone hit test, monitor-local conversion, anchor/margin calc
└── types.rs                   # Command/Response enums; ADD Command::Shutdown (backward compatible)
```

> Note: `input.rs` stays a single file (behavior frozen; not restructured into `input/mod.rs`).

### Data flow (target)

```
                     ┌──────────────────────────────────────────────┐
                     │                #[tokio::main]                 │
Hyprland ──events──► │  daemon/events.rs                            │
   (socket)          │   resync() triggers (any one is sufficient): │
                     │    A monitoradded/removed/configreloaded ────┐│
                     │    B reconnect supervisor (socket drop) ─────┤│
                     │    C 30s periodic consistency check ─────────┤│
                     │                                              ▼│
                     │   daemon/recovery.rs resync() (mutex-guarded):│
                     │     hypr::clients()+monitors() → rematch::    │
                     │     match_windows() → re-assert visibility,   │
                     │     send OverlayMsg::UpdateSpaces + Reposition │
                     │     (errors logged, never kill supervisor)    │
                     │                                               │
spacectl ─unix sock► │  daemon/commands.rs handle_command ──────────►│──► hypr::* (typed async) ──► Hyprland
                     │                              │                │
evdev thread ─mpsc─► │  handle_mouse_button ── geometry::in_edge_zone│
                     │                                               │
                     │  daemon/visibility.rs ── hypr::dispatch(...)  │
                     └───────────────┬──────────────────────────────┘
                                     │ async_channel<OverlayMsg>
                                     ▼
                     ┌──────────────────────────────────────────────┐
                     │        GTK thread (started once)             │
                     │  overlay/bar.rs: gtk4::Application (1 only)   │
                     │    layer-shell surface (namespace, anchors)  │
                     │    recv OverlayMsg on glib MainContext:      │
                     │      UpdateSpaces → rebuild buttons          │
                     │      Reposition   → set margins/anchor        │
                     │      Show/Hide     → widget set_visible        │
                     │      Shutdown      → quit GTK app              │
                     │  buttons/menu ──► ipc_helpers ──unix sock──► daemon
                     └──────────────────────────────────────────────┘
```

### OverlayMsg protocol (daemon → GTK)

```rust
pub enum OverlayMsg {
    UpdateSpaces { spaces: Vec<SpaceButton>, current: usize },
    Reposition   { anchor: Anchor, margin_x: i32, margin_y: i32, width: i32, monitor: String },
    Show,
    Hide,
    Shutdown,
}

pub struct SpaceButton { pub label: String, pub is_current: bool }

pub enum Anchor { BotLeft, BotRight, TopLeft, TopRight } // maps from config from_overlay
```

`SpaceButton` list is produced by the pure function `overlay::model::build_spaces(windows, current_index)` (AF-8 unit tested), replacing the `"1-2-[3]-4"` string diffing.

---

## Detailed Requirements

Each requirement is traceable to an audit finding (AF-n) and to acceptance criteria (AC-x).

### R1 — Recovery: event-driven resync + reconnect supervisor + periodic safety net (AF-1, AC-b, AC-c)

Recovery must **not** depend solely on the event socket reconnecting. The user's core regression — a DisplayPort monitor sleeping/waking — usually **keeps the IPC event socket alive** while Hyprland tears down and re-maps outputs and layer surfaces. The current code already registers monitor handlers (`daemon.rs:1346-1370`) precisely because monitor events, not socket drops, are the DP-wake signal. The redesign therefore drives `resync()` from **three independent triggers**, any one of which is sufficient:

**Trigger A — Monitor / layout events (primary DP-wake path).** In `daemon/events.rs`, the following Hyprland events each schedule a debounced `resync()`:
- `monitoradded` / `monitoradded_v2`
- `monitorremoved`
- `configreloaded` (Hyprland reload can drop/remap layer surfaces)
- `activemonitorchanged` (keeps the existing behavior of re-evaluating visibility)

These fire while the event socket is fully connected, so recovery happens without any reconnect. Each schedules the shared `resync()` after a 150 ms debounce (coalescing the burst of remove+add events emitted during a single DP wake into one resync).

**Trigger B — Reconnect supervisor (socket-drop path).** `daemon/events.rs` runs the listener (async `AsyncEventListener` per OQ-1) inside the single tokio runtime, wrapped in a supervisor task that **never exits**:
  ```
  loop {
      if let Err(e) = resync().await {     // fault-tolerant: see below
          error!("resync failed (will retry on next trigger/reconnect): {e}");
      }
      match run_listener().await {          // blocks until socket drops / error
          Ok(())  => warn!("listener returned cleanly, reconnecting"),
          Err(e)  => error!("listener error: {e}, reconnecting"),
      }
      backoff.sleep().await;                // 250ms → x2 → cap 5s, reset on clean run >30s
  }
  ```
  - **Backoff:** exponential from 250 ms, doubling, capped at 5000 ms; reset to 250 ms after any listener run lasting > 30 s. Guarantees reconnect within the 5 s ceiling (AC-b).

**Trigger C — Periodic consistency check (safety net).** `daemon/recovery.rs` runs a 30 s interval task that performs a **cheap** check (not a full resync): via `hypr::clients()`/`hypr::monitors()` verify (1) the current tracked window's address still exists, and (2) the current window's monitor still exists with the geometry the last `Reposition` used. If either check fails, it schedules a full `resync()`. This catches any recovery path missed by Triggers A/B (e.g. a compositor that emits neither a monitor event nor a socket drop). The check is O(clients) and issues no dispatches unless drift is detected, so it is safe to run every 30 s.

All three triggers funnel into **one** `resync()` guarded by a `tokio::sync::Mutex` so concurrent triggers coalesce rather than overlap (a resync already in progress absorbs the new request; a single re-run is scheduled if a trigger arrives mid-resync).

**`resync()` — full reconciliation, idempotent, fault-tolerant (`daemon/recovery.rs`):**
  1. `hypr::clients()` and `hypr::monitors()` — snapshot current Hyprland state. **On error, log at `error!` and return `Err` — the supervisor and trigger schedulers treat this as "retry on next trigger/reconnect"; a failed `resync()` NEVER terminates the supervisor task or any trigger task.** Because Trigger C fires again in ≤30 s and Trigger B retries after backoff, a compositor that is momentarily unavailable (mid-wake, mid-reload) is recovered automatically once IPC is ready.
  2. Re-match managed windows to live clients using the **pure** `rematch::match_windows(&managed, &clients)` function (R9) and apply the results: update runtime `address`/`pid` for matched windows; **mark unmatched windows closed** (never guessed).
  3. Recompute the visible managed window; call `visibility::assert_visibility(current)` to re-assert the single-visible invariant (re-park others in `special:spaces`).
  4. Recompute overlay geometry from the (possibly changed) monitor layout and send `OverlayMsg::UpdateSpaces` + `OverlayMsg::Reposition` (+ `Show`/`Hide`) so the overlay re-anchors to the current window on the correct monitor.
- Listener handlers must not hold locks across `.await` on the hypr layer; they enqueue work onto the runtime.

### R2 — Layer-shell overlay (AF-2, AC-c, AC-d)

- Rewrite the overlay as a `gtk4-layer-shell` surface in `overlay/bar.rs`:
  - `layer = Overlay` (fallback `Top` if a compositor quirk requires it; default Overlay).
  - `keyboard_interactivity = None`.
  - `namespace = "space-manager-overlay"`.
  - Anchored to the monitor edge matching `from_overlay` (`bot_left`/`bot_right`/`top_left`/`top_right`), using `set_anchor` on the two relevant edges and `set_margin` for x/y offsets.
- **Config field naming (avoid confusion):** two similarly-named config fields drive different things and must not be conflated. `from_overlay` (`bot_left`/`bot_right`/`top_left`/`top_right`) selects the **corner where the overlay bar is anchored**. `from_area` (`left`/`right`/`top`/`bottom`) selects the **edge of the tracked window used for the mouse edge-zone hit test** (R8 `in_edge_zone`). They are independent inputs and are passed to different pure functions (`compute_anchor_margins` vs `in_edge_zone`).
- **Positioning at the tracked window's corner is preserved** by computing monitor-local margins in `geometry.rs`:
  - Input: tracked-window rect (global coords from `hypr::clients()`), the window's monitor rect (`hypr::monitors()`), config `from_overlay`, `offset_x`, `offset_y`, computed overlay width.
  - Convert window rect to monitor-local coordinates, then derive `(anchor, margin_x, margin_y)` so the bar sits at the configured corner of the tracked window. Clamp margins to `>= 0` and within monitor bounds.
  - If the tracked window spans multiple monitors, anchor to the monitor containing the window's top-left corner.
- **`Command::ResetOverlayPosition` under layer-shell (existing IPC command, `types.rs:78`, wired to the hamburger menu in commit `eb1246d`):** its handler in `daemon/commands.rs` recomputes overlay width + `(anchor, margin_x, margin_y)` from the **current** tracked-window geometry and current monitor layout (via `geometry.rs`), then emits `OverlayMsg::Reposition` followed by `OverlayMsg::Show`. It no longer touches the deleted `saved_position`/pin/`movewindowpixel` paths. Because it recomputes from live geometry, it doubles as a lightweight **manual resync/re-anchor trigger** the user can invoke from the menu if anything ever looks off. It is idempotent and safe to invoke repeatedly.
- **Show/hide = widget visibility only.** `OverlayMsg::Show` → `window.set_visible(true)`; `Hide` → `set_visible(false)`. Idempotent: setting visibility to its current value is a no-op. This structurally eliminates the pin-toggle inversion (AC-d): there is no toggle state to desync.
- **Eliminated entirely:** overlay windowrules (float/nofocus), `hyprctl dispatch pin`, `movewindowpixel`/`resizewindowpixel` for the overlay, `special:spaces` moves for the overlay, `saved_position` logic, `FollowMouseGuard` usage for overlay moves, `get_overlay_window_address()` title scans, `cursor:no_warps` toggling for overlay moves.
- **Surface survives display reset (AC-c):** layer-shell surfaces are re-mapped by the compositor on output re-enable; the GTK application and window are never destroyed/recreated, so no second-`Application` failure. After a wake, `resync()` (R1) re-sends `Reposition` to correct margins if monitor geometry changed.
- Overlay width computation (`change_area_x` / `change_area_y` / fixed px) moves into `geometry.rs` as a pure function reused by both the initial build and repositioning.

### R3 — Single runtime + message passing (AF-3)

- Remove the event-listener-thread runtime and both GTK-side `Runtime::new()`/`block_on` sites.
- The GTK thread receives `OverlayMsg` via an `async_channel::Receiver` integrated into the glib `MainContext` (`glib::spawn_future_local` or `receiver.attach`). No polling `timeout_add_local` tick, no label-string diffing.
- Button rebuild happens only on `UpdateSpaces`, driven by real state changes, not a 100 ms timer.
- Daemon holds the `async_channel::Sender<OverlayMsg>`; sends are non-blocking (`try_send` with `warn!` on full, bounded capacity 64).
- **Drop semantics (latest-wins intent):** at capacity 64 the channel is not expected to fill in practice. If the GTK thread ever stalls and the channel fills, the correct failure mode for `UpdateSpaces`/`Reposition` is **latest-wins (coalescing)** — a stale overlay is worse than a dropped intermediate frame. Intended implementation: keep the bounded channel for `Show`/`Hide`/`Shutdown`, but store the newest `UpdateSpaces` and newest `Reposition` in an `Arc<Mutex<Option<..>>>` "latest" slot that the GTK receiver drains, so the overlay always converges to the most recent state rather than replaying or dropping-newest. (Cap-64 `try_send` is acceptable for the first cut; document this as the target failure mode.)

### R4 — `hypr` repository module (AF-4, golden rule: data-access layer)

- New `src/hypr/mod.rs` wraps `hyprland-rs` 0.4.0-beta.3 typed async API. All Hyprland reads/writes go through it. Surface (async):
  - `clients() -> Result<Vec<ClientInfo>>` (wraps `Clients::get_async`)
  - `monitors() -> Result<Vec<MonitorInfo>>`
  - `active_workspace_id() -> Result<i32>`
  - `cursor_position() -> Result<(i32, i32)>`
  - `active_window() -> Result<Option<ClientInfo>>`
  - `move_to_workspace_silent(target: WorkspaceIdent, addr: &str) -> Result<()>` (wraps `Dispatch::call_async`)
  - `close_window(addr: &str) -> Result<()>`
  - `focus_window(addr: &str) -> Result<()>`
  - `keyword_set(key: &str, value: &str) -> Result<()>`
  - `get_option_int(key: &str) -> Result<i64>`
- `ClientInfo`/`MonitorInfo` are thin local structs (address, class, title, pid, at, size, workspace id/name, monitor name) so the rest of the codebase does not depend on `hyprland-rs` types directly.
- **No raw `hyprctl` subprocess anywhere.** Any capability the crate lacks must be documented here as an explicit exception. Current audit found none required beyond the typed API; `hypr_settings.rs` (`follow_mouse` get/set) is migrated to `hypr::get_option_int` / `hypr::keyword_set`. See OQ-2 for the one item to verify during implementation.

### R5 — File logging + panic hook (AF-5, AC-e)

- New `src/logging.rs::init()`:
  - `tracing-appender` daily-rotating file writer to `~/.space-manager/logs/` (create dir if missing), filename `space-manager.log`. Keep a non-blocking writer; retain the guard for the process lifetime.
  - `EnvFilter` from `RUST_LOG` (default `info`).
  - Keep a stdout layer as well (harmless when detached).
  - Install `std::panic::set_hook` that logs the panic payload + location + backtrace at `error!` level to the file before default behavior, so crashes are captured.
- Add `tracing-appender` to `Cargo.toml`.

### R6 — Crash/fragility fixes (AF-6, AC-a)

- **No `unwrap()` on runtime creation** — there is only the `#[tokio::main]` runtime now; the offending `Runtime::new().unwrap()` sites are deleted (R3).
- **Overlay close button → `Command::Shutdown` IPC**, not `pkill`/`exit`. The GTK close handler sends `Command::Shutdown` via `ipc_helpers`; the daemon runs the existing graceful `shutdown()` path then exits. Removes the race with signal-based shutdown.
- **`SpaceManager::current_window()` bounds-safety:** return `windows.get(*current).cloned()` and clamp `current_index` on every mutating op. Add explicit invariant helper `clamp_current()` invoked by add/insert/remove/swap. Covered by AF-8 tests (R8).
- **Atomic `state.json` write:** write to `state.json.tmp` in the same directory, `fsync`, then `rename` over `state.json`. Same pattern for `config.json` saves.

### R7 — Module split + `lib.rs` restructure (AF-7)

- Split `daemon.rs` into `daemon/{mod,events,recovery,rematch,commands,visibility,lifecycle}.rs` per the module map.
- Split `overlay/manager.rs` into `overlay/{bar,model,settings_dialog,template_dialogs}.rs`; reuse existing `dialog_utils`, `theme`, `ui_components`, `window_utils`.
- **`lib.rs` restructure (required so pure-logic modules are library code and therefore unit-testable per AF-8).** The daemon logic moves out of `bin/daemon.rs` into the library; `bin/daemon.rs` becomes a ~120-line consumer of `space_manager::daemon`. New declaration set:
  ```rust
  // src/lib.rs
  pub mod daemon;        // NEW: daemon/{mod,events,recovery,rematch,commands,visibility,lifecycle}
  pub mod geometry;      // NEW: pure geometry (AF-8)
  pub mod hypr;          // NEW: typed hyprland-rs wrapper (AF-4)
  pub mod hypr_settings; // existing
  pub mod input;         // existing
  pub mod ipc;           // existing
  pub mod logging;       // NEW: file logging + panic hook (AF-5)
  pub mod manager;       // existing
  pub mod overlay;       // existing (internal split, exports OverlayHandle/OverlayMsg)
  pub mod process;       // existing
  pub mod types;         // existing
  ```
  `overlay/model.rs`, `geometry.rs`, `daemon/rematch.rs`, `daemon/visibility.rs` (pure `resolve_target_workspace`), and `ipc.rs` frame helpers must all be reachable from the library crate for `cargo test` to exercise them.
- **IPC protocol stays backward compatible.** `Command`/`Response` enums keep all existing variants and wire format (length-prefixed JSON). **Add `Command::Shutdown`** as a new variant (additive; older `spacectl` binaries are unaffected since they never send it).

### R9 — Pure `match_windows` re-match heuristic + tests (AF-1, AF-8)

The resync re-match (R1 step 2) is the single most correctness-critical new piece of logic, so it is a **pure, deterministic, unit-tested** function with no I/O:

```rust
// daemon/rematch.rs
pub struct MatchOutcome {
    pub matches: Vec<(ManagedWindowId, /* client addr */ String, /* pid */ Option<u32>)>,
    pub closed:  Vec<ManagedWindowId>,   // managed windows with no confident match
}
pub fn match_windows(managed: &[ManagedWindow], clients: &[ClientInfo]) -> MatchOutcome;
```

**Deterministic matching rules (strong key first, never guess):**
1. **PID is the strong key.** A managed window whose last-known `pid` is still present on a live client (and that pid's process is alive) re-matches to that client. PID match wins over everything.
2. **Title tiebreak.** For managed windows not matched by PID, among remaining unmatched clients of the same `class`, prefer an exact stored-`title` match.
3. **Oldest-unmatched-first.** If still ambiguous (multiple managed windows share a class with no PID/title distinction), assign remaining same-class clients to managed windows in a stable order: managed windows sorted by `id` ascending (ids are creation-timestamp based, so this is oldest-first), each taking the next available same-class client.
4. **Never guess.** Any managed window left without a client after steps 1–3 is placed in `closed` (marked closed), not speculatively bound. A client is assigned to at most one managed window.

This makes the two-browser-windows-same-class case (the review's ambiguous scenario) fully deterministic: PID first, then title, then oldest-first, then closed.

### R8 — Unit tests for pure logic (AF-8, AC-a; TDD golden rule)

Extract pure functions and test them **before** wiring (see Test Plan). Pure functions to create:

- `overlay::model::build_spaces(windows, current) -> (Vec<SpaceButton>, usize)`
- `geometry::in_edge_zone(win_rect, cursor, from_area, fraction, min_px) -> bool`
- `geometry::overlay_width(win_rect, overlay_size, from_area, fraction, min_px, offset_x) -> i32`
- `geometry::compute_anchor_margins(win_rect, monitor_rect, from_overlay, offset_x, offset_y, overlay_width, overlay_height) -> (Anchor, i32, i32)`
- `visibility::resolve_target_workspace(cached, visible_ws, window_ws_map, active_ws) -> i32` (refactored from `daemon.rs:739-772` to take data, no I/O)
- `rematch::match_windows(managed, clients) -> MatchOutcome` (R9 — resync re-match heuristic, pure, deterministic)
- `SpaceManager` index invariants (already synchronous logic over the vec; expose a testable inner type or test via the async API on a `current_thread` runtime).
- `ipc` frame encode/decode roundtrip (extract `encode_frame`/`decode_frame` helpers).
- config serde defaults (`ConfigFile` / `OverlayConfig` deserialization with missing fields).

---

## Test Plan

TDD: write these tests before/with the extraction of each pure function. Every function has positive and negative cases. Naming: `test_<action>_<scenario>`.

### Overlay model (`overlay/model.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_build_spaces_marks_current | Positive | 5 windows, current=2 → button[2].is_current, others false |
| test_build_spaces_custom_icon_used | Positive | window with `custom_icon=Some("🌐")` → label is the icon, not index |
| test_build_spaces_default_label_is_index | Positive | no custom icon → label is `(i+1).to_string()` |
| test_build_spaces_empty | Negative | 0 windows → empty vec, no panic |
| test_build_spaces_current_out_of_range | Negative | current=9 on 3 windows → clamped, no panic, no is_current set past end |

### Geometry (`geometry.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_in_edge_zone_left_hit | Positive | cursor within left fraction of window → true |
| test_in_edge_zone_left_miss | Negative | cursor in center → false |
| test_in_edge_zone_uses_min_px_floor | Positive | tiny window where fraction < min_px → min_px used |
| test_in_edge_zone_right_top_bottom | Positive | each `from_area` variant hits correctly |
| test_in_edge_zone_zero_size_window | Negative | width/height 0 → false, no divide/overflow |
| test_overlay_width_fixed_px | Positive | overlay_size="250" → 250 |
| test_overlay_width_change_area_x | Positive | computes zone minus 2*offset_x |
| test_overlay_width_invalid_string | Negative | unparseable overlay_size → falls back to 250 |
| test_compute_anchor_margins_bot_left | Positive | window at monitor-local (0,0) → BotLeft anchor, expected margins |
| test_compute_anchor_margins_top_right | Positive | correct anchor + margins from window rect |
| test_compute_anchor_margins_clamps_negative | Negative | offset pushing margin < 0 → clamped to 0 |
| test_compute_anchor_margins_multi_monitor_offset | Positive | window on secondary monitor → margins are monitor-local, not global |

### Visibility (`daemon/visibility.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_resolve_target_ws_prefers_cached_visible | Positive | cached ws present in visible set → returned |
| test_resolve_target_ws_falls_back_to_visible_managed | Positive | cache stale (not visible) → picks visible managed window's ws |
| test_resolve_target_ws_falls_back_to_cached_any | Positive | nothing visible → cached>0 returned |
| test_resolve_target_ws_defaults_to_active | Negative | no cache, no managed ws → active workspace |
| test_resolve_target_ws_ignores_nonpositive | Negative | ws values <=0 filtered out |

### Resync re-match heuristic (`daemon/rematch.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_match_windows_by_pid_alive | Positive | managed pid present on a live client → re-matched to that client's addr |
| test_match_windows_pid_wins_over_class | Positive | client with matching pid but different-looking class still matches by pid (strong key) |
| test_match_windows_title_tiebreak | Positive | two same-class managed windows, one client title matches stored title → that pairing chosen |
| test_match_windows_two_same_class_disambiguated_by_pid | Positive | two managed browser windows same class, distinct pids → each binds to its own pid'd client |
| test_match_windows_ambiguous_oldest_first | Positive | two same-class managed, no pid/title signal, two clients → assigned by ascending id (oldest first), stable |
| test_match_windows_pid_dead_marked_closed | Negative | managed window whose pid is gone and no other signal → in `closed`, never bound |
| test_match_windows_no_client_marked_closed | Negative | managed window with no matching client at all → `closed`, not guessed |
| test_match_windows_client_assigned_once | Negative | a single client is never assigned to two managed windows |
| test_match_windows_empty_inputs | Negative | empty managed and/or empty clients → empty matches, managed (if any) all closed, no panic |

### SpaceManager index invariants (`manager.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_current_window_empty_returns_none | Negative | empty list → None, no panic (regression for AF-6) |
| test_remove_at_shifts_current_down | Positive | remove index < current → current decremented |
| test_remove_last_clamps_current | Positive | remove tail while current==tail → current clamps to new last |
| test_insert_before_current_increments | Positive | insert at/before current → current +1 |
| test_swap_updates_current | Positive | swap involving current → current follows the window |
| test_next_wraps | Positive | next at last → wraps to 0 |
| test_prev_wraps | Positive | prev at 0 → wraps to last |
| test_switch_to_out_of_range | Negative | index >= len → None, current unchanged |
| test_remove_out_of_range | Negative | index >= len → None, no panic |

### Config serde (`manager.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_config_defaults_when_empty_object | Positive | `{}` → all defaults (side_mouse_binds true, overlay defaults) |
| test_config_partial_overlay_fills_defaults | Positive | overlay missing `from_area` → default "left" |
| test_config_preserves_templates | Positive | templates round-trip |
| test_config_rejects_malformed | Negative | invalid JSON → Err, daemon keeps defaults |
| test_config_tolerates_unknown_fields | Positive | additive future field ignored, no error |

### IPC framing (`ipc.rs`)

| Test | Type | Description |
|------|------|-------------|
| test_frame_roundtrip_command | Positive | encode(Command::Next) → decode → equal |
| test_frame_roundtrip_shutdown | Positive | new `Command::Shutdown` round-trips |
| test_frame_roundtrip_response_windows | Positive | `Response::Windows` round-trips |
| test_decode_truncated_frame | Negative | length prefix > payload → Err, no panic |
| test_decode_garbage_payload | Negative | valid length, invalid JSON → Err |
| test_backward_compat_legacy_command_json | Positive | JSON produced by current enum still decodes (guards protocol stability) |

### Integration tests (manual/scripted, documented in AC)

| Test | Type | Description |
|------|------|-------------|
| test_reconnect_after_socket_drop | Positive | kill Hyprland event socket connection → daemon reconnects < 5 s, resyncs (AC-b) |
| test_resync_survives_ipc_unavailable | Negative | resync() while Hyprland IPC momentarily down → logged Err, supervisor/triggers keep running, recovers on next trigger (AC-b/AC-c) |
| test_overlay_survives_dpms | Positive | `hyprctl dispatch dpms off; sleep 2; hyprctl dispatch dpms on` (software DPMS, socket stays up) → monitor-event trigger fires, overlay correct position, buttons work (AC-c) |
| test_overlay_survives_output_remove_add | Positive | `hyprctl keyword monitor <DP>,disable` then re-enable (simulates physical DP link loss → `monitorremoved`/`monitoradded`) → overlay re-anchors, buttons work (AC-c2) |
| test_overlay_recovers_via_periodic_check | Positive | force overlay/monitor drift with monitor events suppressed → 30 s consistency check schedules resync, overlay recovers (AC-c) |
| test_reset_overlay_position_reanchors | Positive | invoke `Command::ResetOverlayPosition` → recomputes margins from live geometry, emits Reposition+Show |
| test_show_hide_idempotent | Positive | repeated Show/Hide sequences never invert (AC-d) |
| test_logs_written | Positive | logs appear in `~/.space-manager/logs/` (AC-e) |
| test_release_build_clean | Positive | `cargo build --release` clean, `cargo test` passes (AC-a) |

---

## Acceptance Criteria

Definition of Done:

- **AC-a** — `cargo build --release` completes with no errors; `cargo test` passes all unit tests in the Test Plan.
- **AC-b** — Daemon survives a Hyprland event-socket drop: killing the event-socket connection causes reconnect within 5 s and a full state resync (managed windows re-matched, visibility re-asserted, overlay refreshed). A `resync()` that errors because IPC is momentarily unavailable is logged and retried; it never terminates the supervisor.
- **AC-c** — Overlay survives **software DPMS**: `hyprctl dispatch dpms off; sleep 2; hyprctl dispatch dpms on` (event socket stays connected) → the monitor-event trigger (Trigger A) fires, overlay re-anchors to correct position, buttons work. No manual restart.
- **AC-c2** — Overlay survives **physical DP link loss / output removal**: disabling and re-enabling the DP output (`hyprctl keyword monitor <name>,disable` → re-enable, which emits `monitorremoved`/`monitoradded`, the field-representative repro) → overlay re-anchors to correct position on the re-added monitor, buttons work. If neither a monitor event nor a socket drop is observed, the 30 s periodic consistency check (Trigger C) still recovers it.
- **AC-d** — Show/hide is idempotent; no pin-toggle inversion is possible (no pin state exists).
- **AC-e** — Logs are written to `~/.space-manager/logs/` with rotation, and panics are captured in the log.

Security / robustness checklist:

- [ ] No `unwrap()`/`expect()` on runtime creation or on Hyprland responses in steady-state paths (startup-only `expect` on signal handler acceptable, or replaced with logged error).
- [ ] `state.json` and `config.json` written atomically (temp + rename).
- [ ] No panic can occur from `current_index` vs. window-list races.
- [ ] Overlay never spawns a second `gtk4::Application`.
- [ ] No blocking `std::process::Command` on a tokio worker thread (excludes `process.rs`, which spawns user apps via async `tokio::process::Command`).
- [ ] Recovery supervisor and all trigger tasks are infinite/self-restarting; a failed `resync()` is logged and retried, never terminal.
- [ ] Recovery does not depend solely on event-socket reconnect (monitor-event + periodic triggers present).
- [ ] `match_windows` never speculatively binds an unmatched managed window (marks closed instead).
- [ ] Unix socket path unchanged; CLI wire protocol backward compatible.

---

## Migration / Rollout notes

1. **Dependencies:** add `gtk4-layer-shell` (crate `gtk4-layer-shell = "0.5"`, which resolves against gtk4 0.9 / glib 0.20; system lib `gtk4-layer-shell` 1.3.0 is installed) and `tracing-appender`. The developer may bump the gtk4 stack if a newer `gtk4-layer-shell` crate is required for compatibility — record the final versions in `Cargo.toml`. `async-channel` added for daemon→GTK messaging.
2. **Config compatibility:** `config.json` schema unchanged. All existing keys read the same. Any new key is additive with a serde default; missing keys never error (tested).
3. **State compatibility:** `state.json` format unchanged (windows + current id); existing user state loads without migration.
4. **CLI compatibility:** `spacectl` binary and its arguments are frozen. `Command::Shutdown` is added to the protocol but not exposed as a `spacectl` subcommand (used only by the overlay close button); older `spacectl` binaries keep working.
5. **Hyprland config cleanup (user-facing, optional):** because the overlay is now layer-shell, the previous `windowrule` entries for `com.spacermanager.overlay` become dead and can be removed from the user's Hyprland config. `setup_window_rules()` for the overlay class is dropped; rules for the *dialog* windows (settings/new-space/change-icon) remain since those are still normal GTK windows.
6. **Rollout order (Loop 2 / developer):**
   1. Land `logging.rs`, `hypr/mod.rs`, `geometry.rs`, `overlay/model.rs`, `daemon/rematch.rs` (pure `match_windows`) + their unit tests (no behavior change to overlay yet).
   2. Refactor daemon to route all Hyprland access through `hypr::*`; delete blocking subprocess calls.
   3. Add `daemon/recovery.rs` `resync()` + the three triggers (monitor/config events, reconnect supervisor, 30 s periodic check) with fault-tolerant error handling.
   4. Replace overlay with layer-shell bar + `OverlayMsg` channel; delete pin/movewindowpixel/title-scan paths; map `Command::ResetOverlayPosition` to recompute-and-Reposition.
   5. Module split (`daemon/*`, `overlay/*`), `lib.rs` restructure, add `Command::Shutdown`, wire close button.
   6. Run integration checks AC-b, AC-c, AC-c2, AC-d, AC-e.
7. **Version bump:** no Makefile version target detected in the repo; if one is added, use `make rev`/`make minor` per golden rules. Otherwise this is a single feature branch merged via PR to `main`.

---

## Product Owner Review Responses (round 1)

All eight action items from `REVIEW.md` are addressed in the sections above:

| # | Item | Resolution |
|---|------|-----------|
| 1 | Recovery must trigger on monitor add/remove/layout events, not only socket reconnect | R1 Trigger A (monitoradded/removed/configreloaded/activemonitorchanged, 150 ms debounced) + Trigger C (30 s periodic consistency check). AC-c (software DPMS) and new **AC-c2** (physical DP link loss via output disable/enable → monitorremoved/added) both added, with matching integration tests. |
| 2 | `resync()` errors logged-and-retried, never terminate the supervisor | R1 `resync()` step 1 + supervisor loop: errors are logged at `error!` and returned; the supervisor task and all trigger tasks continue; recovery happens on the next trigger / backoff. Covered by `test_resync_survives_ipc_unavailable`. |
| 3 | `Command::ResetOverlayPosition` behavior under layer-shell | R2: handler recomputes width + anchor/margins from live tracked-window geometry and emits `Reposition` + `Show`; doubles as a manual re-anchor/resync trigger; idempotent. |
| 4 | Pure `match_windows` + unit tests | New **R9** with deterministic rules (PID strong key → title tiebreak → oldest-id-first → else mark closed, never guess) and a 9-row test table including the two-same-class disambiguation and marked-closed negatives. |
| 5 | `process.rs` in module map | Added, marked UNCHANGED, with a note that it spawns user apps via async `tokio::process::Command` and is deliberately out of scope for AF-4. |
| 6 | `lib.rs` declarations | R7 now specifies the full new `lib.rs` declaration set (`daemon`, `geometry`, `hypr`, `logging` added) and that daemon logic moves into the library so pure modules are testable. |
| 7 | Latest-wins/coalescing channel intent | R3: documented latest-wins coalescing as the target failure mode for `UpdateSpaces`/`Reposition` (cap-64 `try_send` acceptable for first cut). |
| 8 | `from_overlay` vs `from_area` naming | R2: added an explicit note distinguishing the overlay-anchor corner (`from_overlay`) from the edge-zone hit-test side (`from_area`). |

**Response to Review Question 1 (does DPMS off/on alone reproduce the failure, or only physical DP loss?).**
Decision: the design is verified against **both** paths, and they are treated as distinct acceptance criteria. Software DPMS (`hyprctl dispatch dpms off/on`) is the CI-friendly check and may keep outputs "present" (possibly emitting no `monitorremoved`) — it exercises AC-c via `activemonitorchanged`/`configreloaded` and, as a backstop, the 30 s consistency check. Physical DP link loss removes and re-adds the output — it exercises AC-c2 via `monitorremoved`/`monitoradded`, which is the primary user-reported trigger. Because the three recovery triggers are independent and any one is sufficient, the design does not rely on which events a given wake happens to emit. Rationale: verifying only software DPMS risked passing CI while still failing in the field (exactly the reviewer's concern), so both are now first-class ACs with dedicated integration tests (`test_overlay_survives_dpms`, `test_overlay_survives_output_remove_add`, `test_overlay_recovers_via_periodic_check`).

---

## Open Questions

The redesign direction is mandated; these are the only genuinely undecidable implementation points that could affect the developer's approach. Recommendations are provided. **OQ-1 and OQ-2 are settled (accepted in review) and retained here for reference only — no rework.**

**OQ-1 — Event listener execution model.** `hyprland-rs` 0.4.0-beta.3 exposes both a blocking `EventListener` and an async `AsyncEventListener`. The reconnect supervisor (R1) is cleaner if the listener is async (runs as a tokio task, no dedicated thread). If the async listener has known issues on Hyprland 0.55.4 (event parsing gaps for newer events), fall back to the blocking listener on a dedicated `std::thread` that signals the tokio runtime to run `resync()`.
- *Recommendation:* use `AsyncEventListener` inside the single tokio runtime; keep the blocking-listener-on-thread as the documented fallback if an event variant fails to parse. Decision can be made at implementation time based on a quick smoke test; it does not change the public interface.

**OQ-2 — `follow_mouse` suppression relevance after layer-shell.** The `FollowMouseGuard` exists to stop focus-follows-mouse from stealing focus during overlay `movewindowpixel` sequences. With a layer-shell overlay (keyboard_interactivity None, not a client), overlay moves no longer touch focus, so the guard is unnecessary for overlay repositioning. However, `update_visibility()` (moving *managed browser windows* between workspaces) may still benefit from suppressing follow_mouse to avoid focus flicker during rapid switching.
- *Recommendation:* remove `FollowMouseGuard` from all overlay paths (mandated by AF-2); keep it only around `visibility::update_visibility()` for managed-window moves, migrated to use `hypr::get_option_int`/`hypr::keyword_set` instead of subprocess. If testing shows no flicker without it, drop it entirely in a follow-up. This does not block the redesign.
