# yoinker

A terminal clipboard manager daemon for Linux (X11/Wayland).

## Features

- Daemon watches clipboard for changes, stores text and image history
- TUI picker with fuzzy search, relative timestamps, and pin support
- IPC over Unix domain socket (newline-delimited JSON)
- Neovim plugin with native libuv socket communication (no socat)
- Atomic persistence (crash-safe writes)
- Debounced saves to reduce disk I/O
- Auto-reconnect on clipboard connection loss
- Shell completions (bash, zsh, fish)
- Systemd service file for auto-start

## Install

```sh
cargo install --path yoinkerd
cargo install --path yoinker-cli
```

Or build both:

```sh
cargo build --workspace --release
# Binaries: target/release/yoinkerd, target/release/yoinker
```

## Quick Start

```sh
# Start the daemon
yoinker daemon start

# Copy some text in any app, then:
yoinker list          # TUI picker
yoinker get 0         # print most recent entry
yoinker list --json   # JSON output for scripting

# Manage daemon
yoinker daemon status
yoinker daemon stop
```

### Systemd (auto-start on login)

```sh
cp contrib/yoinkerd.service ~/.config/systemd/user/
systemctl --user enable --now yoinkerd
```

## Configuration

Place config at `~/.config/yoinker/config.toml`. See `config.example.toml` for all options.

```toml
max_history = 50
poll_interval_ms = 500
max_entry_bytes = 10485760  # 10 MB, 0 = unlimited
socket_path = "/run/user/1000/yoinker.sock"  # default: $XDG_RUNTIME_DIR/yoinker.sock
history_path = "/home/user/.local/share/yoinker/history.json"  # default: $XDG_DATA_HOME/yoinker/history.json
```

## CLI Reference

| Command | Description |
|---|---|
| `yoinker list` | TUI picker (Enter=select, Esc=cancel, Ctrl+X=delete, Ctrl+D/U=page) |
| `yoinker list --json` | Print history as JSON |
| `yoinker get <N>` | Print Nth entry to stdout |
| `yoinker pin <N>` | Pin entry (survives trim/clear) |
| `yoinker unpin <N>` | Unpin entry |
| `yoinker clear` | Clear unpinned entries |
| `yoinker store <TEXT>` | Store text directly |
| `yoinker daemon start` | Start daemon in background |
| `yoinker daemon stop` | Stop daemon (SIGTERM) |
| `yoinker daemon status` | Check daemon status |
| `yoinker completions <SHELL>` | Generate shell completions |

## GUI Picker

A native GUI picker (`yoinker-gui`) can be triggered with a global keybind. It auto-starts the daemon if needed.

```sh
cargo install --path yoinker-gui
```

Bind `yoinker-gui` (or `contrib/yoinker-popup.sh`) to a global shortcut (e.g. Super+V) in your DE.

**Keyboard:** Enter=select, Esc=close, Tab/arrows=navigate, Ctrl+D/U=page, Ctrl+P=pin, Ctrl+T=tag, Ctrl+X=delete, Ctrl+L=search

## Neovim Plugin

Add to your plugin manager (lazy.nvim example):

```lua
{
  dir = "~/path/to/yoinker/nvim",
  config = function()
    require("yoinker").setup({
      -- socket_path = nil,  -- auto-detect
      -- keymap_prefix = "<leader>y",
    })
  end,
}
```

### Keymaps

| Key | Mode | Action |
|---|---|---|
| `<leader>yy` | Visual | Store selection |
| `<leader>yp` | Visual | Pin selection |
| `<leader>yl` | Normal | Open floating picker |
| `<leader>y1` | Normal | Paste most recent |

The floating picker supports typing to filter, Ctrl+N/P or arrow keys to navigate, Enter to paste, Ctrl+X to delete, and Esc to cancel.

## Architecture

```
yoinkerd (daemon)
  ├── watcher: polls system clipboard, adds to history
  ├── history: in-memory store with debounced atomic persistence
  └── socket: Unix socket server handling JSON requests

yoinker (CLI)
  ├── ipc: Unix socket client
  ├── tui: ratatui-based picker with fuzzy search
  └── daemon: start/stop/status management

nvim plugin
  └── libuv Unix socket IPC, floating window picker
```
