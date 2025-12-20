# Space Manager – Wayland App Instance Controller (Concept)
## Overview

Space Manager is a Wayland-native controller application designed for Hyprland.

It manages multiple external application instances (for example, Brave browser profiles) and presents them as a single logical application with spaces, inspired by Arc / Zen.

Space Manager does not embed or render other applications.
Instead, it launches, tracks, and focuses external windows using Hyprland IPC, while providing a minimal overlay UI.

## Goals

- Launch multiple isolated instances of any application
- Treat them as one logical group ("spaces")
- Switch between instances using Mouse Button 4 / 5
- Provide a textbox overlay to spawn new instances
- Display a small HUD overlay indicating the active space
- Be fully Wayland + Hyprland compatible

## Non-Goals

- No window embedding or reparenting
- No browser engine modifications
- No X11 hacks or compositor patches
- No Chromium forks

## High-Level Architecture

Space Manager runs as a controller daemon.

It communicates with Hyprland via IPC and controls focus, while apps remain normal Wayland clients.

Components:

- Hyprland IPC listener
- Process launcher
- Managed window registry
- Input-driven focus switcher
- Overlay UI (layer-shell)

## Core Concepts

### Managed Window

A managed window is an external application window launched or tracked by Space Manager.

Fields:

- Hyprland window address
- Window class
- Window title

Example structure (conceptual):

```
ManagedWindow
  address: string
  class: string
  title: string
```

### Manager State

Space Manager maintains an ordered list of windows and a current index.

Conceptually:

```
SpaceManager
  windows: list of ManagedWindow
  current_index: number
```

## Launching New Instances (Textbox Overlay)

### User Flow

1. User presses a keybind (example: SUPER + B)
2. A layer-shell textbox overlay appears at the bottom-left
3. User types a shell command, e.g.:
   ```
   brave --user-data-dir=~/.config/brave-work
   ```
4. User presses Enter
5. Space Manager spawns the process
6. When the window appears, it is registered
7. Focus switches to the new window

### Notes

- Commands are executed using `/bin/sh -c`
- No argument parsing is performed
- This keeps behavior flexible and user-controlled

## Mouse-Based Space Switching

### Input

- Mouse Button 4 → Previous space
- Mouse Button 5 → Next space

### Hyprland Integration

Hyprland binds mouse buttons to Space Manager commands:

```
bind = , mouse:275, exec, spacectl next
bind = , mouse:274, exec, spacectl prev
```

### Behavior

- `next` increments the index (wraps around)
- `prev` decrements the index (wraps around)
- Focus is applied using Hyprland dispatch by window address

## Overlay HUD (Space Indicator)

### Purpose

Provide feedback when switching spaces.

### Appearance

- Bottom-center overlay
- Small rounded rectangle
- Minimal text, e.g.:
  - `Space 2 / 4`
  - or `Brave Work`

### Behavior

- Appears on space change
- Appears after spawning a new instance
- Auto-hides after ~5 seconds
- Implemented using Wayland layer-shell

## Hyprland Integration

### IPC Events

Space Manager listens for:

- `windowopen`
- `windowclose`
- `windowfocus`

It updates internal state accordingly.

### Window Matching

When launching a process:

1. Track spawn time
2. Match newly opened windows by:
   - PID (preferred, if available)
   - Class + timing as fallback

## Technology Choices

### Language

**Rust**

Reasons:

- Wayland-friendly ecosystem
- Safe long-running daemon
- Strong async + IPC support
- Can be shipped as a single binary

### Suggested Libraries

- `hyprland` / `hyprland-rs` (IPC + events)
- `smithay-client-toolkit` (Wayland + layer-shell)
- `calloop` (event loop)
- `serde` (state handling)
- `nix` (process management)

### Process Model

- One daemon: `space-manager`
- One CLI tool: `spacectl`

`spacectl` commands:

- `spacectl next` - Switch to next space (requires mouse in left edge of window)
- `spacectl prev` - Switch to previous space (requires mouse in left edge of window)
- `spacectl spawn "<command>"` - Spawn a new managed window
- `spacectl list` - List all managed windows
- `spacectl cleanup` - Close all hidden windows in special:spaces workspace

The CLI communicates with the daemon via a local socket or IPC.

### Mouse Position Constraints

The `next` and `prev` commands check mouse position before switching:

- Mouse must be within **1/8 of window width** OR **150 pixels** from the left edge
- This allows Arc-like behavior where switching only works when hovering over the left edge
- Configurable in `src/bin/daemon.rs` (lines 29-30):
  - `max_distance_fraction: 0.125` (1/8 of window width)
  - `max_distance_pixels: 150` (150 pixels)

### Hyprland Configuration

Add these keybinds to `~/.config/hypr/hyprland.conf`:

```conf
bind = , mouse:275, exec, ~/RustroverProjects/browserSpaces/target/release/spacectl prev
bind = , mouse:276, exec, ~/RustroverProjects/browserSpaces/target/release/spacectl next
```

(Adjust mouse button numbers as needed - use `wev` to find your button codes)

## Constraints & Notes

- Windows remain independent Wayland clients
- Space Manager never draws inside application windows
- Overlays are separate layer-shell surfaces
- Behavior is compositor-cooperative

## Window Visibility Management

Space Manager hides inactive windows in a special Hyprland workspace:

- Only **one tracked window is visible** at a time
- Other tracked windows are moved to `special:spaces` workspace (hidden)
- When switching spaces, the target window is shown and all others are hidden
- Windows stay in the same regular workspace (e.g., workspace 4)

This creates an Arc-like experience where spaces feel like tabs within a single window.

## Persistent Session State

Space Manager automatically saves and restores your session:

### State Storage

- Saved to: `~/.space-manager/state.json`
- Contains: All tracked windows with their spawn commands

### On Shutdown (Ctrl+C or `pkill space-manager`)

1. Saves current state to disk
2. Gracefully closes all tracked windows
3. Windows close normally (browsers save tabs, etc.)

### On Startup

1. Loads saved state from `~/.space-manager/state.json`
2. Automatically respawns all windows using their original commands
3. Restores your exact session

### Example Workflow

```bash
# Start daemon
./target/release/space-manager &

# Spawn some windows
spacectl spawn "brave --app=https://tidal.com --user-data-dir=/tmp/brave-tidal"
spacectl spawn "brave --app=https://youtube.com --user-data-dir=/tmp/brave-youtube"

# Stop daemon (saves state automatically)
pkill space-manager

# Restart daemon - your windows are restored!
./target/release/space-manager &
```

## Cleanup Command

If you have leftover hidden windows from testing:

```bash
spacectl cleanup
```

This finds and closes all windows in the `special:spaces` workspace, useful for cleaning up orphaned test instances.

## Future Extensions (Out of Scope)

- Scroll-region based switching (bottom third of browser vertical tabs)
- Sidebar UI
- Named spaces with icons
- Per-space shortcuts

## Summary

Space Manager recreates an Arc-like space experience on Wayland by:

- orchestrating focus instead of embedding windows
- leveraging Hyprland IPC
- providing minimal overlays
- remaining application-agnostic

This approach is simple, robust, and aligns with Wayland's design.