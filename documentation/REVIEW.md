# Space Manager Backend Redesign Refinement Review

**Reviewer:** Tech Lead
**Date:** 2026-07-10
**Status:** Changes Requested

---

## Overview

This is a high-quality, audit-grounded refinement. Root causes are enumerated with IDs (AF-1..AF-8), every requirement traces back to a finding and forward to an acceptance criterion, and the test plan carries positive+negative cases per the TDD golden rule. I verified the audit against the live code and the evidence checks out. The changes below are focused: the recovery-trigger model is under-specified for the exact regression the user mandated (DP monitor sleep/wake), and two existing behaviors are not carried through the redesign.

---

## Action Items

### Critical (Must Fix Before Implementation)

1. **Recovery is triggered only by event-socket drop; the DP-wake regression may not drop the socket (AF-1/AF-2, AC-c).**
   R1's supervisor calls `resync()` "on every (re)connect," and R2 states "after a wake, `resync()` (R1) re-sends `Reposition`." That chain assumes the DPMS/DP-sleep event *causes the event socket to drop*. On Hyprland, a DisplayPort monitor losing signal typically emits `monitorremoved`/`monitoradded` (and layer surfaces get torn down / re-mapped) **without necessarily disconnecting the IPC event socket**. If the socket stays up, the reconnect loop never fires, `resync()` never runs, and the overlay is not re-anchored — reproducing the exact user-reported failure. The current code already has a monitor-event handler (`daemon.rs:1362`) precisely because monitor events, not socket drops, are the DP-wake signal.
   Define an explicit, event-driven recovery trigger independent of socket reconnect: `monitoradded`, `monitorremoved`, and monitor-layout-change events must each schedule a `resync()` (or at minimum a `Reposition`/`Show`). State this in R1 and R2, and add it to the resync trigger list. Without this, AC-c is not actually satisfied by the design.

2. **Define `resync()` fault-tolerance under a not-yet-ready compositor.**
   The R1 loop runs `resync()` before `run_listener()`. If Hyprland IPC is momentarily unavailable (mid-wake, mid-reload), `resync()` will error. The refinement doesn't say whether a failed `resync()` propagates (killing the loop) or is logged-and-retried. Specify that `resync()` errors are logged and the loop continues to backoff+retry — a failed resync must never terminate the supervisor. This is a direct dependency of AC-b/AC-c.

### High Priority (Should Fix)

3. **`Command::ResetOverlayPosition` has no defined behavior in the new design.**
   This existing IPC command (types.rs:78, wired to the hamburger menu per recent commit `eb1246d`) forced a reposition to the configured location and interacted with the now-deleted `saved_position` logic. R7 freezes the protocol and keeps all variants, but neither R2 nor the OverlayMsg section says how `ResetOverlayPosition` maps to the layer-shell world. Specify that its handler recomputes margins from current tracked-window geometry and emits `OverlayMsg::Reposition` (+ `Show`). Otherwise a shipped, user-facing menu action silently breaks.

4. **No unit test for the resync window re-match heuristic (AF-1, AF-8).**
   R1.2 re-matches managed windows "by `ManagedWindow.id` heuristics (pid still alive + class)" — this is the single most correctness-critical new piece of logic, and class alone is ambiguous when two managed windows share a class (two browser windows). Only the end-to-end `test_reconnect_after_socket_drop` integration test covers it, and integration tests are manual per the plan. Extract a pure function, e.g. `resync::match_windows(managed: &[ManagedWindow], clients: &[ClientInfo]) -> Vec<Match>`, and unit-test it: pid-alive re-match (positive), pid-dead → marked closed (negative), and two-windows-same-class disambiguated by pid (edge). State pid is the strong key and class is only a tiebreaker/validator.

### Medium Priority (Should Address)

5. **`process.rs` is absent from the target module map.**
   The module map (lines 67-100) enumerates every `src/` file but omits `process.rs` (`ProcessLauncher`, actively used). Add it to the map marked UNCHANGED so the developer doesn't assume it was folded in or dropped. Note it uses `tokio::process::Command` (async) to spawn *user* apps, so it is correctly out of scope for the AF-4 "no blocking subprocess" rule — worth stating to preempt confusion.

6. **`lib.rs` restructure is implied but not stated.**
   Moving daemon logic from `bin/daemon.rs` into `src/daemon/*.rs` (and adding `hypr/`, `logging.rs`, `geometry.rs`) means `lib.rs` must gain `pub mod daemon; pub mod hypr; pub mod logging; pub mod geometry;` and the bin becomes a thin consumer of the lib. Current `lib.rs` declares none of these. State the new `lib.rs` declaration set explicitly in R7 so the ~120-line `daemon.rs` target is achievable (pure-logic modules must be library code to be unit-testable per AF-8).

### Low Priority (Nice to Have)

7. **Bounded channel drop semantics for state messages.**
   R3 uses a bounded `async_channel` (cap 64) with `try_send` + `warn!` on full. For `UpdateSpaces`/`Reposition`, dropping the *newest* message leaves a stale overlay. If the GTK thread ever stalls, latest-wins (coalescing) is safer than dropping. Not blocking at cap 64, but note it as the intended failure mode.

8. **`from_overlay` vs `from_area` naming.**
   R2 and the `Reposition` struct use `from_overlay` for the corner anchor while `from_area` is the edge-zone hit-test side. They are legitimately different config fields, but the near-identical names invite bugs. A one-line note distinguishing them in R2 would help the implementer.

---

## Summary Checklist

| # | Priority | Item | Status |
|---|----------|------|--------|
| 1 | Critical | Recovery must trigger on monitor add/remove/layout events, not only socket reconnect (AC-c) | [ ] |
| 2 | Critical | Specify `resync()` errors are logged-and-retried, never terminate the supervisor | [ ] |
| 3 | High | Define `Command::ResetOverlayPosition` behavior under layer-shell | [ ] |
| 4 | High | Add pure `match_windows` function + unit tests for resync re-match | [ ] |
| 5 | Medium | Add `process.rs` to the module map (UNCHANGED) | [ ] |
| 6 | Medium | State new `lib.rs` module declarations for the daemon/hypr/logging/geometry split | [ ] |
| 7 | Low | Note latest-wins/coalescing intent for `UpdateSpaces`/`Reposition` channel | [ ] |
| 8 | Low | Clarify `from_overlay` vs `from_area` naming | [ ] |

---

## Questions for Discussion

1. **Does DPMS off/on alone reproduce the failure, or only physical DP signal loss?**
   The audit and AC-c use `hyprctl dispatch dpms off/on` as the repro, but the user's report is a *physical* DP monitor sleeping. These can differ: software DPMS may keep monitors "present" (no `monitorremoved`), while a real DP link drop removes and re-adds the output. If the design is validated only against software DPMS, it may pass AC-c yet still fail in the field.
   *Recommendation:* keep AC-c's `hyprctl dpms` test as the CI-friendly check, but add a second acceptance path documenting the physical-unplug/replug (or `hyprctl output` remove/add) scenario, since that is what exercises the Item-1 monitor-event trigger. Confirm which one the resync design is being verified against.

---

## Positive Notes

- Excellent traceability: every AF finding maps to an R requirement and an AC, and the audit line numbers are accurate (I confirmed AF-1 at `daemon.rs:1373-1376` and the second `Application::builder()` at `manager.rs:540`).
- The layer-shell rewrite (R2) structurally eliminates the pin-toggle inversion rather than papering over it — "no toggle state to desync" is the right framing for AC-d.
- Data-access-layer golden rule is honored cleanly: the `hypr` module (R4) with local `ClientInfo`/`MonitorInfo` structs keeps `hyprland-rs` types from leaking across the codebase.
- Pure-function extraction for geometry/model/visibility/ipc (R8) is exactly the right seam for TDD, and the test tables carry genuine negative cases (zero-size window, out-of-range index, truncated frame, malformed config).
- Scope is disciplined — explicitly reliability/organization only, CLI and config schemas frozen, `Command::Shutdown` additive and backward compatible, device-hotplug rescan correctly deferred as a stretch. No scope creep.
- Atomic `state.json`/`config.json` writes (R6) and the panic hook + file logging (R5) close real durability/observability gaps.
- The rollout order (land pure modules + tests first, then route through `hypr`, then reconnect, then layer-shell, then split) is safe and reviewable in increments.

---

## Cross-Component Coordination Required

None external — this is a single-binary/single-repo redesign. The only "cross-component" surface is the `spacectl` ↔ daemon IPC protocol, which R7 keeps backward compatible (additive `Command::Shutdown`). No change-request files needed.

---

## Next Steps

1. Address Critical items 1-2 (monitor-event-driven recovery trigger; resync fault-tolerance) — these are load-bearing for the user's core mandate.
2. Address High items 3-4 (`ResetOverlayPosition` mapping; resync re-match unit tests).
3. Fold in Medium items 5-6 (module map completeness; `lib.rs` declarations).
4. Answer Question 1 and reconcile AC-c with the physical-DP-wake scenario.
5. Low items 7-8 are optional polish for this round.
6. Re-submit for re-review; the redesign direction and both accepted Open Questions (OQ-1 AsyncEventListener + fallback, OQ-2 FollowMouseGuard scoped to managed-window moves) are settled and need no rework.

---

## Developer Responses

The refinement agent added a "Product Owner Review Responses (round 1)" section (REFINEMENT.md:474-490) mapping every action item to its resolution and answering the discussion question.

### Question 1: DPMS off/on vs. physical DP link loss
**Decision:** Verify against both, as distinct acceptance criteria. Software DPMS may keep outputs "present" (possibly no `monitorremoved`) and is the CI-friendly check (AC-c, backed by `activemonitorchanged`/`configreloaded` and the periodic check). Physical DP link loss removes/re-adds the output (AC-c2, `monitorremoved`/`monitoradded`) and is the primary field repro. Because the three recovery triggers are independent and any one suffices, correctness does not depend on which events a given wake emits.
This directly resolves the reviewer's concern about passing CI while failing in the field.

---

## Re-Review (Tech Lead)

**Date:** 2026-07-10
**Status:** Approved

All eight action items and the discussion question are fully resolved. I re-read the revised REFINEMENT.md end to end and cross-checked the load-bearing changes against the codebase.

### Verification of each item

| # | Priority | Item | Status |
|---|----------|------|--------|
| 1 | Critical | Monitor/layout-event recovery trigger | Resolved — R1 Trigger A (`monitoradded`/`_v2`/`monitorremoved`/`configreloaded`/`activemonitorchanged`, 150 ms debounced) + Trigger C (30 s consistency check), independent of socket reconnect. New AC-c2 + `test_overlay_survives_output_remove_add` + `test_overlay_recovers_via_periodic_check`. |
| 2 | Critical | `resync()` fault tolerance | Resolved — R1 step 1 and supervisor loop log-and-return on error; supervisor and all trigger tasks never exit; `test_resync_survives_ipc_unavailable` added. |
| 3 | High | `ResetOverlayPosition` under layer-shell | Resolved — R2 handler recomputes width + anchor/margins from live geometry, emits `Reposition`+`Show`, idempotent, doubles as manual re-anchor. Test added. |
| 4 | High | Pure `match_windows` + tests | Resolved — new R9 with deterministic PID → title → oldest-id-first → mark-closed rules and a 9-row positive/negative table covering the two-same-class case and never-guess negatives. |
| 5 | Medium | `process.rs` in module map | Resolved — added, UNCHANGED, with the async/user-app out-of-scope note; also reflected in the AC checklist. |
| 6 | Medium | `lib.rs` restructure | Resolved — R7 specifies the full new declaration set and that daemon logic moves into the library for testability. |
| 7 | Low | Channel coalescing intent | Resolved — R3 documents latest-wins as the target failure mode. |
| 8 | Low | `from_overlay` vs `from_area` naming | Resolved — R2 note distinguishes the two fields and their target functions. |

### Notes confirmed during re-review

- The R9 oldest-id-first tiebreak is sound: `ManagedWindow::new` (types.rs:21-27) mints ids as `win_<millis-since-epoch>`, which sort lexically in creation order given fixed digit width — the "oldest first" claim holds.
- The mutex-coalesced single `resync()` with three feeders is the right shape: it prevents overlapping reconciliations while guaranteeing at least one re-run for any trigger that arrives mid-resync.
- AC coverage now maps cleanly to the user mandate: AC-b (socket drop), AC-c (software DPMS), AC-c2 (physical DP loss), AC-d (idempotent show/hide), AC-e (logging), all with corresponding tests.
- No scope creep introduced by the revision; `input.rs` restructure was correctly declined, and the periodic check is scoped to a cheap drift probe rather than an unconditional resync.

---

## Final Approval

**Date:** 2026-07-10
**Reviewer:** Tech Lead
**Status:** Approved

The refinement is complete, traceable (AF → R → AC → tests), unambiguous, and implementable by a developer who has not seen the audit. The two accepted Open Questions (OQ-1, OQ-2) are settled and need no further discussion. Recovery no longer hinges on a single trigger, the DP-wake regression is covered by two distinct acceptance paths plus a safety net, and every pure function carries positive and negative unit tests per the TDD golden rule.

**Recommendations for implementation:**
1. Follow the rollout order in Migration note 6: land the pure modules first (`logging`, `hypr`, `geometry`, `overlay/model`, `daemon/rematch`) with their unit tests before touching runtime behavior — this front-loads AF-8 and de-risks the rest.
2. Make OQ-1's async-vs-blocking listener call early with a smoke test on Hyprland 0.55.4; it does not change the public interface, so it won't ripple.
3. Watch the debounce/coalesce interaction (Trigger A 150 ms debounce feeding the mutex-guarded resync) during AC-c2 — a DP wake emits a remove+add burst; confirm it collapses to one resync, not a thrash.
4. Deferrable: R3's full latest-wins coalescing slot may ship as cap-64 `try_send` in the first cut (as stated), and OQ-2's "drop FollowMouseGuard entirely" is a legitimate follow-up if no flicker is observed.

**Go build it.**
