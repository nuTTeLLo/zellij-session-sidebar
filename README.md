# zellij-session-sidebar

A persistent sidebar plugin for [Zellij](https://zellij.dev) that shows all your active sessions and their tabs in a collapsible tree. Navigate sessions, switch tabs, kill sessions, and get AI agent visibility — all without leaving your current pane.

> **Forked from** [zellij-project-sidebar](https://github.com/AndrewBeniston/zellij-project-sidebar) by Andrew Beniston. The original plugin is directory/project-scan based with fuzzy search and browse mode. This fork pivots to a session-first model — it shows whatever Zellij sessions exist, with their tabs, rather than scanning a directory. The pipe API, AI state machinery, attention system, and hook integration patterns are all inherited and extended from the original. Thank you to Andrew for the solid foundation.

---

## Current status — Phase 1

Phase 1 is the working foundation. The sidebar is functional and stable for daily use.

**What works today:**

- **Session tree**: all running Zellij sessions listed with their tabs, expandable/collapsible with `▶`/`▼`
- **Tab navigation**: expand a session to see its tabs; navigate and jump directly to a specific tab
- **Current session highlighted**: green text shows where you are
- **Active tab indicator**: `●` (green) / `○` marks the active tab within each session
- **Cursor auto-tracks**: when unfocused, the cursor follows the current session
- **Hide/show toggle**: `Ctrl+/` completely suppresses the sidebar pane — other panes expand to fill — and restores it at its original position via layout override
- **Toggle focus**: configurable key to focus/unfocus the sidebar (`Ctrl+O, o` by default)
- **New tab with sidebar**: configurable key creates a new tab using the configured layout
- **Kill session**: `Delete` on a non-current session kills it
- **Mouse support**: scroll wheel navigates the list
- **Attention system**: sessions can be flagged with a pipe message; cleared when you switch to them
- **AI state storage**: pipe messages for `active`/`idle`/`waiting` states are received and stored per session (not yet rendered — Phase 2)
- **Pills and progress**: arbitrary key/value pills and 0–100% progress values can be pushed via pipe (not yet rendered — Phase 2)
- **Stacked hint footer**: when unfocused, the bottom shows a compact key reference parsed from the `hint` config option

---

## Planned phases

### Phase 2 — AI and status rendering
Render the state that is already being stored:
- AI agent indicators next to session names (`active` / `idle` / `waiting` with duration)
- Progress bars inline with session rows
- Pills (small key=value badges) per session
- Make the hide key configurable (currently hardcoded to `Ctrl+/`)

### Phase 3 — Polish and UX
- Sessionizer integration (fuzzy-launch a new session from a directory)
- Favourites pinning
- Configurable column layout (compact / full)
- Per-session tab count badge

---

## Build

Requires Rust with the `wasm32-wasip1` target:

```bash
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
```

The plugin binary is at `target/wasm32-wasip1/release/zellij-session-sidebar.wasm`.

---

## Layout setup

Add the sidebar to your Zellij layout. The `session_layout` path should point back at the same layout file — the sidebar uses it to restore its position when toggled back on.

```kdl
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size="10%" borderless=true {
                plugin location="file:~/.config/zellij/plugins/zellij-session-sidebar.wasm" {
                    is_primary         true
                    session_layout     "~/.config/zellij/layouts/default.kdl"
                    hint               "^O,o sidebar  ^O,w sessions  ^O,r sessionizer  ^O,t picker  ^O,f favs"
                }
            }
            pane {
                children
            }
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }
}
```

### Configuration options

| Option | Default | Description |
|--------|---------|-------------|
| `session_layout` | — | Path to the layout file used when restoring the sidebar and creating new tabs. Supports `~`. |
| `is_primary` | `true` | Only the primary instance registers global keybinds. Set `false` on secondary instances. |
| `toggle_key` | `o` | Key pressed after `Ctrl+O` to toggle sidebar focus. |
| `new_tab_key` | `Ctrl t` | Key that opens a new tab using `session_layout`. |
| `hint` | — | Footer hint string shown when sidebar is unfocused. Format: `^O,<key> <label>  ^O,<key> <label>` |

---

## Keybindings

### Global (registered by the plugin)

| Key | Action |
|-----|--------|
| `Ctrl+/` | Toggle sidebar visibility (hide/show) |
| `Ctrl+O, <toggle_key>` | Toggle sidebar focus |
| `<new_tab_key>` | New tab with sidebar layout |

> The hide key (`Ctrl+/`) is currently hardcoded. Configurability is planned for Phase 2.

### When sidebar is focused

| Key | Action |
|-----|--------|
| `↑` / `↓` | Navigate sessions and tabs |
| `→` | Expand session to show tabs |
| `←` | Collapse session (or jump to parent if on a tab) |
| `Enter` | Switch to session / jump to tab |
| `Delete` | Kill selected session (no-op on current session) |
| `Esc` | Deactivate sidebar |
| Scroll | Navigate list |

---

## Attention system

Flag a session as needing your attention via Zellij pipe:

```bash
# Flag
zellij pipe --name "sidebar::attention::my-session"

# Clear
zellij pipe --name "sidebar::clear::my-session"
```

Attention is automatically cleared when you switch to the session via the sidebar.

---

## Pipe API

All pipe messages are sent with `zellij pipe --name "<message>"`.

### AI state (stored now, rendered in Phase 2)

```bash
zellij pipe --name "sidebar::ai-active::my-session"    # agent is working
zellij pipe --name "sidebar::ai-idle::my-session"      # agent finished
zellij pipe --name "sidebar::ai-waiting::my-session"   # agent needs input

# Optionally tag the agent name with ::agent-name suffix
zellij pipe --name "sidebar::ai-active::my-session::claude"
```

### Pills (stored now, rendered in Phase 2)

```bash
zellij pipe --name "sidebar::pill" \
    --args "session=my-session,key=branch,value=main"

zellij pipe --name "sidebar::pill-clear" \
    --args "session=my-session,key=branch"   # clear one
zellij pipe --name "sidebar::pill-clear" \
    --args "session=my-session"              # clear all
```

### Progress (stored now, rendered in Phase 2)

```bash
zellij pipe --name "sidebar::progress" \
    --args "session=my-session,pct=42"

zellij pipe --name "sidebar::progress-clear" \
    --args "session=my-session"
```

---

## Claude Code hook integration

AI state is shared across sessions via files in `$TMPDIR/zellij-$(id -u)/sidebar-ai/`. The sidebar polls this directory every 10 seconds for cross-session visibility, and pipe messages provide instant updates in the current session.

Register a hook script in `~/.claude/settings.json` to push state automatically:

```json
{
  "hooks": {
    "PostToolUse": [{ "hooks": [{ "type": "command", "command": "$HOME/.claude/hooks/sidebar-status.sh", "async": true }] }],
    "Stop":        [{ "hooks": [{ "type": "command", "command": "$HOME/.claude/hooks/sidebar-status.sh", "async": true }] }],
    "Notification":[{ "hooks": [{ "type": "command", "command": "$HOME/.claude/hooks/sidebar-status.sh", "async": true }] }],
    "SessionStart":[{ "hooks": [{ "type": "command", "command": "$HOME/.claude/hooks/sidebar-status.sh", "async": true }] }]
  }
}
```

The hook script should write the state file and send the appropriate pipe message. See the original [zellij-project-sidebar](https://github.com/AndrewBeniston/zellij-project-sidebar) for a reference hook script.

---

## Requirements

- Zellij 0.44.x+
- Rust with `wasm32-wasip1` target (`rustup target add wasm32-wasip1`)

---

## Licence

MIT
