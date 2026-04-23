# zellij-session-sidebar — Rewrite Plan

## Context

Replacing `zellij-project-sidebar` with a session-first tree sidebar.
Repo: https://github.com/nuTTeLLo/zellij-session-sidebar
Old repo (`zellij-project-sidebar`) is untouched.

**Why:** Mac stays as primary machine running cmux + zellij. iPhone/iPad SSH in to attach to
zellij sessions. The sidebar needs to be navigable over SSH and show agent status inline.

**Reference:** https://github.com/laperlej/zellij-choose-tree (tree structure, expand/collapse model)

---

## What Changes vs Old Sidebar

| Old | New |
|-----|-----|
| Directory scan → project list | `SessionUpdate` is sole source of truth |
| Flat card list | Collapsible tree: sessions → tabs |
| `Project` / `ProjectMetadata` structs | `SessionNode` / `TabNode` structs |
| `selected_index` (flat) | `cursor` (index into visible tree rows) |
| `browse_mode` / `search_query` | Expand/collapse replaces this |
| `scan_dirs`, `CMD_SCAN_DIR`, git polling | Removed |
| Card box-drawing borders | Tree connectors (`▼ ▶ ├ └ ● ○`) |

**Kept as-is:** pipe message protocol, AI state maps, attention system, pills, progress,
`load_ai_states()` shell script, `now_secs()`, duration formatting, toggle keybind, `reconfigure()`.

---

## Target Tree Appearance

```
▼* ▶ my-session          claude · 2m !
  ├ ● main
  └ ○ research            [3]
▶  · other-session
▶  ■ done-session         claude · 45s✓
```

- `▼`/`▶` = expanded/collapsed
- `*` = current session
- `·`/`▶`/`■`/`!` = agent status icon (none / active / idle / waiting+attention)
- `●`/`○` = active tab dot
- `[3]` = pane count when > 1
- Agent badge, pills, progress bar inline on session row

---

## New Data Model

```rust
struct TabNode {
    index: usize,      // tab position for switch_session_with_focus
    name: String,
    is_active: bool,
    pane_count: usize,
}

struct SessionNode {
    name: String,
    is_current: bool,
    tabs: Vec<TabNode>,
}

enum TreeRow {
    Session(usize),        // sessions[si]
    Tab(usize, usize),     // sessions[si].tabs[ti]
}
```

Agent state, attention, pills, progress remain as separate `BTreeMap<String, _>` keyed by session name (same as before — outlive SessionUpdate).

---

## Phase Plan

### Phase 1 — Data model + SessionUpdate rebuild
- [ ] Replace `Project`/`ProjectMetadata`/`SessionStatus` with `SessionNode`/`TabNode`
- [ ] Replace `projects: Vec<Project>` with `sessions: Vec<SessionNode>`
- [ ] Replace discovery/scan fields with `expanded_sessions: BTreeSet<String>`
- [ ] Implement `rebuild_from_session_update()` — sort current-first then alpha, auto-expand current
- [ ] Implement `build_visible_rows() -> Vec<TreeRow>`
- [ ] Implement `clamp_cursor()` and `ensure_cursor_visible()`
- [ ] Wire `SessionUpdate` event to call rebuild, then clamp cursor
- [ ] Remove `RunCommandResult` handling for scan/git (keep CMD_LOAD_AI)
- [ ] Build and confirm it compiles

### Phase 2 — Tree rendering
- [ ] Implement `render_session_line(si, is_selected, cols) -> Text`
  - Expand icon + current marker + status icon + name + agent badge + pills + progress
  - Color: icon color by agent state, name green if current, badge green if active
- [ ] Implement `render_tab_line(si, ti, is_selected, cols) -> Text`
  - Indent + connector (├/└) + active dot (●/○) + tab name + pane count
- [ ] Update `render()` to iterate visible rows, call per-type renderer
- [ ] Footer hint line
- [ ] Test with 1-3 sessions, various tab counts

### Phase 3 — Navigation
- [ ] Key handler: `↑`/`↓` move cursor through visible rows
- [ ] Key handler: `→`/`l` expands session, `←`/`h` collapses + jumps cursor to parent
- [ ] Key handler: `Enter` — switch session or switch tab via `switch_session_with_focus`
- [ ] Key handler: `Delete` — kill non-current session
- [ ] Key handler: `Esc` — blur sidebar (`set_selectable(false)`)
- [ ] Mouse: scroll up/down moves cursor, single click sets cursor, double click activates
- [ ] Auto-track cursor to current session when sidebar is not focused

### Phase 4 — Agent overlay + notifications
- [ ] Re-attach pipe system (mostly unchanged from old sidebar)
  - `sidebar::attention::`, `sidebar::clear::`
  - `sidebar::ai-active::`, `sidebar::ai-idle::`, `sidebar::ai-waiting::`
  - `sidebar::pill`, `sidebar::pill-clear`
  - `sidebar::progress`, `sidebar::progress-clear`
- [ ] Wire `load_ai_states()` + `apply_ai_states_from_output()` (same shell script)
- [ ] Render agent badge inline on session row
- [ ] Render attention `!` icon
- [ ] Confirm pipe messages still work with existing Claude Code hooks

### Phase 5 — Polish + config
- [ ] Toggle keybind via `reconfigure()` (Super+O)
- [ ] `session_layout` config for new session creation (for future worktree work)
- [ ] `is_primary` config (secondary instances skip keybind registration)
- [ ] Update `zellij.kdl` layout config
- [ ] Update README

---

## Future Work (not in this rewrite)

- **cmux-zellij-bridge daemon** — reconciles cmux workspaces ↔ zellij sessions (Option B, loose coupling)
- **iOS push notifications** — extend Claude Code hooks to curl ntfy.sh/Pushover endpoint
- **Tab-level agent status** — show agent badge per tab (needs pane-level data from PaneManifest)
- **Worktree integration** — workspace creation wizard that runs `git worktree add` and spawns zellij session at worktree path
- **Zellij tab rename for attention** — rename tab with `!` prefix when attention flagged (SSH-visible)
- **cmux notification bridge** — sidebar pipe → cmux native notification center

---

## Key API Reference (zellij-tile 0.44)

```rust
// Session switching
switch_session(Some(&name))
switch_session_with_focus(&session_name, Some(tab_index), None)
kill_sessions(&[name])

// Plugin focus
set_selectable(bool)
show_self(bool)

// Keybinds
get_plugin_ids().plugin_id
reconfigure(config_str, false)

// Timer
set_timeout(secs_f64)

// Rendering
print_text_with_coordinates(text, x, y, Some(cols), None)
Text::new(&str).selected().color_range(color, range).color_all(color)

// Permissions
request_permission(&[PermissionType::ReadApplicationState, ...])
subscribe(&[EventType::SessionUpdate, EventType::Key, ...])

// Shell commands (for load_ai_states)
run_command_with_env_variables_and_cwd(&["sh", "-c", script], env, cwd, ctx)
```

---

## Files to Change

| File | Change |
|------|--------|
| `src/main.rs` | Full rewrite (phases 1–5) |
| `Cargo.toml` | Rename package to `zellij-session-sidebar` |
| `zellij.kdl` | Update plugin path/config |
| `README.md` | Rewrite for new feature set |
| `scripts/` | Keep `sidebar-status.sh` (hooks unchanged), remove scan scripts |
