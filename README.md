# Space Manager

A Wayland-native workspace manager for Hyprland that organizes multiple application instances as logical "spaces" with a beautiful GTK4 overlay interface. Perfect as a browser manager - replace your browser launcher keybind with Space Manager to seamlessly manage multiple browser profiles, development environments, or any application instances with ease.

Start it on-demand with a keybind, and it intelligently handles whether to launch or switch between existing spaces.

## Features

### Core Functionality
- 🪟 **Manage Multiple Instances**: Launch and organize unlimited application instances as separate "spaces"
- 🖱️ **Mouse Navigation**: Switch between spaces using side mouse buttons (back/forward)
- ⌨️ **Keyboard Shortcuts**: Full CLI control for power users
- 🎨 **Visual Overlay**: Beautiful GTK4 overlay showing current space with customizable icons
- 💾 **Session Persistence**: Automatically restore your spaces on restart
- 🎯 **Lazy Loading**: Spaces are restored only when you switch to them

### Advanced Features
- 📋 **Command Templates**: Save frequently used commands with variable substitution (e.g., `{{user-data}}`)
- 🎭 **Custom Icons**: Set emoji icons for each space for visual identification
- 🔄 **Reorder Spaces**: Move spaces left/right with right-click menu
- 📍 **Precise Spawning**: Insert new spaces at specific positions
- 🎚️ **Configurable Overlay**: Customize size, position, and appearance
- 🗑️ **Space Management**: Close applications and remove spaces via right-click menu
- 📜 **Scrollable Interface**: Smooth horizontal scrolling for many spaces

## Dependencies

### System Requirements
- **OS**: Linux with Wayland
- **Compositor**: Hyprland (tested with latest versions)
- **Rust**: 1.70 or later (for building)

### Runtime Dependencies
```bash
# Arch Linux
sudo pacman -S hyprland gtk4 libevdev

# Ubuntu/Debian
sudo apt install hyprland libgtk-4-1 libevdev2

# Fedora
sudo dnf install hyprland gtk4 libevdev
```

### Required Permissions
Space Manager needs access to input devices to monitor mouse buttons:

```bash
# Add your user to the input group
sudo usermod -aG input $USER

# Create udev rule for input device access (recommended)
sudo tee /etc/udev/rules.d/99-input.rules << 'EOF'
KERNEL=="event*", SUBSYSTEM=="input", MODE="0660", GROUP="input"
EOF

# Reload udev rules
sudo udevadm control --reload-rules
sudo udevadm trigger

# Log out and back in for group changes to take effect
```

**Note**: After adding yourself to the `input` group, you **must log out and log back in** for the changes to take effect.

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/spaceManager.git
cd spaceManager

# Build the project
cargo build --release

# Install binaries
sudo cp target/release/space-manager /usr/local/bin/
sudo cp target/release/spacectl /usr/local/bin/

# Make them executable
sudo chmod +x /usr/local/bin/space-manager
sudo chmod +x /usr/local/bin/spacectl
```

### Starting Space Manager

Space Manager is designed to start on-demand via a keybind, making it perfect for replacing your browser launcher or application shortcuts.

Add to your `~/.config/hypr/hyprland.conf`:

```conf
# Replace your browser keybind with space-manager
bind = SUPER, B, exec, space-manager

# Or use any keybind you prefer
bind = SUPER, S, exec, space-manager
```

**Recommended Usage**: Replace your existing browser launcher keybind with `space-manager`. When you press the keybind:
- If Space Manager is already running, it switches to the next space
- If not running, it starts and restores your last session

The daemon will run in the background and create configuration files at `~/.space-manager/`.

Alternatively, you can start it on boot with:
```conf
exec-once = space-manager
```

## Configuration

Space Manager creates two configuration files in `~/.space-manager/`:

### `config.json`
Settings for overlay appearance and behavior:

```json
{
  "overlay": {
    "enabled": true,
    "from_area": "left",
    "from_overlay": "bot_left",
    "offset_x": 8,
    "offset_y": 8,
    "overlay_size": "change_area_x"
  },
  "mouse": {
    "change_area_fraction": 0.0625,
    "min_pixels": 200
  },
  "side_mouse_binds": true,
  "command_templates": [
    {
      "name": "Brave Profile",
      "command": "brave --user-data-dir=\"$HOME/.config/{{profile}}\"",
      "variables": ["profile"]
    }
  ]
}
```

### `state.json`
Persistent state of your spaces (automatically managed):

```json
{
  "windows": [
    {
      "spawn_command": "brave --user-data-dir=\"~/.config/brave-personal\"",
      "class": "brave-browser",
      "addr": null,
      "icon": "🌐"
    }
  ],
  "current_index": 0
}
```

You can edit `config.json` manually or use the built-in settings dialog (click the hamburger menu in the overlay).

## Usage

### Interactive Overlay

The overlay appears at the bottom-left (by default) of your active window:

- **Click space numbers/icons**: Switch to that space
- **Hamburger menu (☰)**: Access settings and create new spaces
- **Close button (×)**: Close the overlay
- **Right-click on space**: Show context menu with:
  - Move Left/Right
  - Change Icon
  - Close Window

### Mouse Bindings

If `side_mouse_binds` is enabled (default):
- **Mouse Back Button**: Previous space
- **Mouse Forward Button**: Next space

The active area for mouse detection is configurable (default: left 6.25% of window or minimum 200px).

### CLI Commands

```bash
# Navigation
spacectl next          # Switch to next space
spacectl prev          # Switch to previous space
spacectl switch-to 3   # Switch to space at index 3 (0-based)

# Create spaces
spacectl spawn "brave --user-data-dir=~/.config/brave-work"
spacectl spawn-at 2 "firefox -P development"  # Insert at position 2

# Manage spaces
spacectl list          # List all spaces
spacectl cleanup       # Remove closed windows from state

# Templates
spacectl get-templates           # List available templates
spacectl add-template "Name" "command {{var}}" var1 var2

# Configuration
spacectl reload-config  # Reload configuration from disk

# Window management
spacectl swap-windows 0 2       # Swap positions of two spaces
spacectl set-window-icon 1 "🎮" # Set custom icon for space
```

### Creating Command Templates

Templates allow you to save commands with variables that you can fill in when spawning:

1. Click the **hamburger menu** in the overlay
2. Select **New Space**
3. Click **+ Add Template**
4. Enter a name and command with `{{variable}}` placeholders:
   ```
   Name: Brave Profile
   Command: brave --user-data-dir="$HOME/.config/{{profile}}"
   ```
5. Click **Save**

Now when you create a new space using this template, you'll be prompted to fill in the `profile` variable.

### Customizing the Overlay

1. Click the **hamburger menu** (☰)
2. Select **Settings**
3. Adjust:
   - **Overlay Position**: Top/Bottom, Left/Right
   - **Offset**: Distance from window edge (x and y)
   - **Change Area**: Mouse detection zone size
   - **Min Pixels**: Minimum mouse detection zone width
4. Click **Apply** to preview or **Save** to save and close

Changes take effect immediately without restarting.

## Examples

### Multi-Profile Browser Setup

```bash
# Create different browser profiles
spacectl spawn "brave --user-data-dir=~/.config/brave-personal"
spacectl spawn "brave --user-data-dir=~/.config/brave-work"
spacectl spawn "brave --user-data-dir=~/.config/brave-shopping"

# Set custom icons
spacectl set-window-icon 0 "🏠"
spacectl set-window-icon 1 "💼"
spacectl set-window-icon 2 "🛒"
```

### Development Environments

```bash
# Frontend development
spacectl spawn "code ~/projects/frontend"

# Backend development  
spacectl spawn "code ~/projects/backend"

# Documentation
spacectl spawn "firefox developer.mozilla.org"
```

### Using Templates

Create a template for development environments:

```
Name: VSCode Project
Command: code {{project-path}}
Variables: project-path
```

Then spawn with:
1. Click hamburger → New Space → VSCode Project
2. Fill in project path: `~/projects/my-app`
3. Choose position and icon
4. Click Spawn

## How It Works

1. **Space Manager** runs as a daemon and listens to Hyprland events via IPC
2. When you spawn a new instance, it tracks the process PID
3. When a new window opens, it matches it to the spawned process and manages it as a space
4. The GTK4 overlay shows current space and provides interactive controls
5. Mouse buttons and CLI commands switch focus between spaces
6. State is saved to `~/.space-manager/state.json` for session persistence
7. Closed windows are kept in state for lazy restoration

## Architecture

### Components

- **space-manager**: Long-running daemon
  - Listens to Hyprland IPC events (window open/close/workspace change)
  - Manages ordered list of spaces and tracks current index
  - Handles focus switching with Hyprland's `focuswindow` dispatch
  - Provides Unix socket IPC server for CLI commands
  - Monitors input devices for mouse button events
  - Manages GTK4 overlay window lifecycle

- **spacectl**: CLI tool
  - Sends commands to daemon via Unix socket (`/tmp/space-manager.sock`)
  - Parses command-line arguments and formats IPC messages

- **GTK4 Overlay**
  - Layer-shell window pinned to all workspaces
  - Hides when target window moves to different workspace
  - Shows/hides based on window visibility
  - Interactive controls for space management

### Key Files

- `src/bin/daemon.rs`: Main daemon entry point
- `src/bin/cli.rs`: CLI tool entry point
- `src/manager.rs`: Core SpaceManager logic
- `src/overlay/`: GTK4 overlay implementation
- `src/input.rs`: Mouse button event monitoring
- `src/ipc.rs`: IPC server/client
- `src/process.rs`: Process spawning and window matching

## Troubleshooting

### Mouse buttons not working

1. Check you're in the `input` group:
   ```bash
   groups | grep input
   ```

2. Verify permissions on input devices:
   ```bash
   ls -l /dev/input/event*
   ```
   Should show `crw-rw---- root input`

3. Check the daemon is detecting your mouse:
   ```bash
   # Look for "Found mouse device" in logs
   journalctl -f | grep space-manager
   ```

4. Try disabling `side_mouse_binds` and using keyboard shortcuts instead

### Overlay not showing

1. Check overlay is enabled in config:
   ```bash
   cat ~/.space-manager/config.json | grep enabled
   ```

2. Verify GTK4 is installed:
   ```bash
   pkg-config --modversion gtk4
   ```

3. Check daemon logs for GTK errors:
   ```bash
   journalctl -u space-manager
   ```

### Windows not spawning

1. Check the command works manually:
   ```bash
   brave --user-data-dir=~/.config/test-profile
   ```

2. Use absolute paths instead of `~`:
   ```bash
   spacectl spawn "brave --user-data-dir=\"$HOME/.config/brave-work\""
   ```

3. Check daemon logs for spawn errors

### Spaces not persisting

1. Verify state file exists:
   ```bash
   cat ~/.space-manager/state.json
   ```

2. Check file permissions:
   ```bash
   ls -l ~/.space-manager/
   ```

3. Look for write errors in logs

## Known Limitations

- Window matching relies on process PID and window class, which may not work for all applications
- GTK4 required for overlay (no fallback UI currently)

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## License

MIT

## Credits

Inspired by Arc Browser and Zen Browser's space/tab management concepts, adapted for Hyprland window management.

