# Space Manager

A Wayland-native controller application for Hyprland that manages multiple application instances as logical "spaces", inspired by Arc/Zen browser.

## Features

- Launch multiple isolated instances of any application
- Treat them as one logical group ("spaces")
- Switch between instances using mouse buttons or CLI commands
- Minimal HUD overlay for feedback
- Full Wayland + Hyprland compatibility

## Building

```bash
cargo build --release
```

The binaries will be in `target/release/`:
- `space-manager` - The daemon
- `spacectl` - The CLI tool

## Installation

```bash
# Build the project
cargo build --release

# Copy binaries to your PATH
sudo cp target/release/space-manager /usr/local/bin/
sudo cp target/release/spacectl /usr/local/bin/
```

## Usage

### 1. Start the Daemon

```bash
space-manager
```

Or add it to your Hyprland config to start automatically:

```conf
exec-once = space-manager
```

### 2. Configure Hyprland Keybindings

Add these to your `~/.config/hypr/hyprland.conf`:

```conf
# Mouse button bindings for space switching
bind = , mouse:275, exec, spacectl next
bind = , mouse:274, exec, spacectl prev

# Optional: keyboard shortcuts
bind = SUPER, bracketright, exec, spacectl next
bind = SUPER, bracketleft, exec, spacectl prev

# Spawn new instances
bind = SUPER, B, exec, spacectl spawn "brave --user-data-dir=~/.config/brave-personal"
bind = SUPER SHIFT, B, exec, spacectl spawn "brave --user-data-dir=~/.config/brave-work"
```

### 3. CLI Commands

```bash
# Switch to next space
spacectl next

# Switch to previous space
spacectl prev

# Spawn a new application instance
spacectl spawn "brave --user-data-dir=~/.config/brave-work"
spacectl spawn "firefox -P work"

# List all managed windows
spacectl list
```

## How It Works

1. **Space Manager** runs as a daemon and listens to Hyprland window events
2. When you spawn a new instance using `spacectl spawn`, it tracks the process
3. When a new window opens, it matches it to the spawned process and adds it to the managed windows list
4. Mouse buttons (or keybinds) cycle through managed windows and focus them using Hyprland IPC
5. A notification shows the current space number when switching

## Architecture

- **space-manager**: Long-running daemon that:
  - Listens to Hyprland IPC events (window open/close)
  - Manages the list of windows
  - Handles focus switching
  - Provides IPC server for CLI commands

- **spacectl**: CLI tool that sends commands to the daemon via Unix socket

- **Core Components**:
  - `SpaceManager`: Maintains ordered list of windows and current index
  - `ProcessLauncher`: Spawns processes and matches them to new windows
  - `IpcServer/IpcClient`: Communication between daemon and CLI
  - `OverlayManager`: Shows HUD notifications (currently uses notify-send)

## Known Limitations

- Layer-shell overlays (textbox and custom HUD) are not yet fully implemented
- Currently uses `notify-send` for notifications instead of custom overlays
- Window matching is based on timing and process class name

## Future Enhancements

See `spec.md` for planned features:
- Custom Wayland layer-shell overlays
- Interactive textbox for spawning commands
- Named spaces with icons
- Persistent session restore
- Per-space shortcuts

## License

MIT
