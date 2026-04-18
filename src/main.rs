use zellij_tile::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// Zellij emphasis color indices (theme-dependent)
//   0 = emphasis_0 (orange)
//   1 = emphasis_1 (cyan)
//   2 = emphasis_2 (green)
//   3 = emphasis_3 (magenta)
#[allow(dead_code)]
const COLOR_ORANGE: usize = 0;
const COLOR_CYAN: usize = 1;
const COLOR_GREEN: usize = 2;
#[allow(dead_code)]
const COLOR_MAGENTA: usize = 3;

const CMD_KEY: &str = "cmd";
const CMD_LOAD_AI: &str = "load_ai";

// --- Data Model ---

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Active,
    Idle,
    Waiting,
    Unknown,
}

impl Default for AgentState {
    fn default() -> Self {
        AgentState::Unknown
    }
}

#[derive(Clone)]
pub struct TabNode {
    pub index: usize,   // tab position for switch_session_with_focus
    pub name: String,
    pub is_active: bool,
}

#[derive(Clone)]
pub struct SessionNode {
    pub name: String,
    pub is_current: bool,
    pub tabs: Vec<TabNode>,
}

pub enum TreeRow {
    Session(usize),        // sessions[si]
    Tab(usize, usize),     // sessions[si].tabs[ti]
}

// --- State ---

pub struct State {
    pub permissions_granted: bool,
    pub sessions: Vec<SessionNode>,
    pub expanded_sessions: BTreeSet<String>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub initial_load_complete: bool,
    pub is_focused: bool,
    pub session_layout: Option<String>,
    pub is_primary: bool,
    pub toggle_key: String,    // e.g. "Ctrl o" or "Super o"
    pub new_tab_key: String,   // e.g. "Ctrl t"
    pub hint: Option<String>,  // custom footer hint when unfocused
    pub is_hidden: bool,    // true while sidebar is hidden
    pub last_cols: usize,   // last known cols from render()

    // Attention and AI state — keyed by session name, survive SessionUpdate
    pub attention_sessions: BTreeSet<String>,
    pub ai_states: BTreeMap<String, AgentState>,
    pub ai_state_since: BTreeMap<String, u64>,
    pub ai_last_duration: BTreeMap<String, u64>,
    #[allow(dead_code)]
    pub ai_pane_count: BTreeMap<String, usize>,
    pub ai_agent_name: BTreeMap<String, String>,

    // Pills and progress — keyed by session name
    pub pills: BTreeMap<String, BTreeMap<String, String>>,
    pub progress: BTreeMap<String, u8>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            permissions_granted: false,
            sessions: Vec::new(),
            expanded_sessions: BTreeSet::new(),
            cursor: 0,
            scroll_offset: 0,
            initial_load_complete: false,
            is_focused: false,
            session_layout: None,
            is_primary: true,
            toggle_key: "o".to_string(),
            new_tab_key: "Ctrl t".to_string(),
            hint: None,
            is_hidden: false,
            last_cols: 0,
            attention_sessions: BTreeSet::new(),
            ai_states: BTreeMap::new(),
            ai_state_since: BTreeMap::new(),
            ai_last_duration: BTreeMap::new(),
            ai_pane_count: BTreeMap::new(),
            ai_agent_name: BTreeMap::new(),
            pills: BTreeMap::new(),
            progress: BTreeMap::new(),
        }
    }
}

// --- Helpers ---

/// Parse hint string into (key, label) pairs.
/// Input format: "^O,o sidebar  ^O,w sessions  ^O,f favs"
/// Output: [("o", "sidebar"), ("w", "sessions"), ("f", "favs")]
pub fn parse_hint_items(hint: &str) -> Vec<(String, String)> {
    // TODO: make the separator configurable instead of hardcoding two spaces
    let separator = "  ";
    let max_items = 99; // arbitrary cap to prevent runaway parsing
    hint.split(separator)
        .take(max_items)
        .filter_map(|item| {
            let item = item.trim();
            let rest = item.strip_prefix("^O,").unwrap_or(item);
            let mut parts = rest.splitn(2, ' ');
            let key = parts.next().filter(|s| !s.is_empty())?.to_string();
            let label = parts.next().unwrap_or("").trim().to_string();
            Some((key.clone(), label.clone()))
        })
        .collect()
}

pub fn parse_session_agent(rest: &str) -> (&str, Option<&str>) {
    if let Some(idx) = rest.find("::") {
        let agent = &rest[idx + 2..];
        (&rest[..idx], if agent.is_empty() { None } else { Some(agent) })
    } else {
        (rest, None)
    }
}

pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
        format!("{}/{}", home, rest)
    } else {
        path.to_string()
    }
}

// --- Pure State Methods (always compiled, tested) ---

impl State {
    pub fn rebuild_from_session_update(&mut self, sessions: &[SessionInfo]) {
        let current_name = sessions.iter()
            .find(|s| s.is_current_session)
            .map(|s| s.name.clone());

        self.sessions = sessions.iter().map(|s| {
            let tabs = s.tabs.iter().enumerate().map(|(i, t)| TabNode {
                index: i,
                name: t.name.clone(),
                is_active: t.active,
            }).collect();
            SessionNode {
                name: s.name.clone(),
                is_current: s.is_current_session,
                tabs,
            }
        }).collect();

        // Sort: current session first, then alphabetical
        self.sessions.sort_by(|a, b| {
            b.is_current.cmp(&a.is_current)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        // Auto-expand current session
        if let Some(name) = &current_name {
            self.expanded_sessions.insert(name.clone());
        }

        // Prune expanded_sessions for gone sessions
        let session_names: BTreeSet<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
        self.expanded_sessions.retain(|name| session_names.contains(name));
        // Don't prune ai_states — stale entries for gone sessions are harmless,
        // and SessionUpdate fires on every switch which could wipe pipe-delivered state.
    }

    pub fn build_visible_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for (si, session) in self.sessions.iter().enumerate() {
            rows.push(TreeRow::Session(si));
            if self.expanded_sessions.contains(&session.name) {
                for ti in 0..session.tabs.len() {
                    rows.push(TreeRow::Tab(si, ti));
                }
            }
        }
        rows
    }

    pub fn clamp_cursor(&mut self) {
        let len = self.build_visible_rows().len();
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(len - 1);
        }
    }

    pub fn ensure_cursor_visible(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
        if self.cursor >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.cursor.saturating_sub(visible_rows - 1);
        }
    }

    /// Returns the session index the cursor is on (whether on a Session or Tab row).
    pub fn cursor_session_index(&self) -> Option<usize> {
        let rows = self.build_visible_rows();
        match rows.get(self.cursor) {
            Some(TreeRow::Session(si)) => Some(*si),
            Some(TreeRow::Tab(si, _)) => Some(*si),
            None => None,
        }
    }

    pub fn apply_ai_states_from_output(&mut self, stdout: &[u8]) {
        // Each line: "SESSION STATE TIMESTAMP DURATION [AGENT]"
        let output = String::from_utf8_lossy(stdout);
        for line in output.lines() {
            let parts: Vec<&str> = line.trim().split(' ').collect();
            if parts.len() < 2 {
                continue;
            }
            let session = parts[0];
            let state = match parts.get(1).copied() {
                Some("active") => AgentState::Active,
                Some("idle") => AgentState::Idle,
                Some("waiting") => AgentState::Waiting,
                _ => continue,
            };
            let ts = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let dur = parts.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let agent = parts.get(4).copied().unwrap_or("").trim();

            self.ai_states.insert(session.to_string(), state);
            if ts > 0 {
                self.ai_state_since.insert(session.to_string(), ts);
            }
            if dur > 0 {
                self.ai_last_duration.insert(session.to_string(), dur);
            }
            if !agent.is_empty() {
                self.ai_agent_name.insert(session.to_string(), agent.to_string());
            }
        }
    }

    pub fn now_secs(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub fn format_elapsed(&self, session: &str) -> String {
        if let Some(&since) = self.ai_state_since.get(session) {
            let elapsed = self.now_secs().saturating_sub(since);
            Self::format_duration(elapsed)
        } else {
            String::new()
        }
    }

    pub fn format_last_duration(&self, session: &str) -> String {
        if let Some(&dur) = self.ai_last_duration.get(session) {
            Self::format_duration(dur)
        } else {
            String::new()
        }
    }

    pub fn format_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    }
}

// --- Plugin-only State Methods (excluded from test builds to avoid unresolved shim symbols) ---

#[cfg(not(test))]
impl State {
    fn load_ai_states(&mut self) {
        let script = r#"
dir=/tmp/sidebar-ai
[ -d "$dir" ] || exit 0
now=$(date +%s)
stale=300
for session_dir in "$dir"/*/; do
  [ -d "$session_dir" ] || continue
  session=$(basename "$session_dir")
  best_rank=0; best_state=""; best_ts=0; best_dur=0; best_agent=""
  for f in "$session_dir"*; do
    [ -f "$f" ] || continue
    read -r line < "$f" 2>/dev/null || continue
    state=$(echo "$line" | awk '{print $1}')
    ts=$(echo "$line" | awk '{print $2}')
    dur=$(echo "$line" | awk '{print $3}')
    agent=$(echo "$line" | awk '{print $4}')
    if [ "$state" != "idle" ]; then
      mtime=$(stat -f %m "$f" 2>/dev/null || stat -c %Y "$f" 2>/dev/null || echo 0)
      age=$((now - mtime))
      [ "$age" -gt "$stale" ] && continue
    fi
    case "$state" in
      active) rank=3 ;;
      waiting) rank=2 ;;
      idle) rank=1 ;;
      *) rank=0 ;;
    esac
    if [ "$rank" -gt "$best_rank" ]; then
      best_rank=$rank; best_state=$state; best_ts=$ts; best_dur=$dur; best_agent=$agent
    fi
  done
  [ -n "$best_state" ] && echo "$session $best_state $best_ts $best_dur $best_agent"
done
"#;
        let mut ctx = BTreeMap::new();
        ctx.insert(CMD_KEY.to_string(), CMD_LOAD_AI.to_string());
        run_command_with_env_variables_and_cwd(
            &["sh", "-c", script],
            BTreeMap::new(),
            PathBuf::from("/"),
            ctx,
        );
    }

    fn setup_toggle_keybind(&self) {
        let plugin_id = get_plugin_ids().plugin_id;
        let toggle_key = &self.toggle_key;
        let new_tab_key = &self.new_tab_key;
        // Toggle focus is bound in session mode (Ctrl+O → key).
        // Hide/show is bound globally so it works from any mode.
        // New tab is bound in shared mode as it's a creation action.
        let config = format!(
            r#"
keybinds {{
    session {{
        bind "{toggle_key}" {{
            MessagePluginId {plugin_id} {{
                name "toggle_sidebar"
            }}
            SwitchToMode "Normal"
        }}
    }}
    shared_except "locked" {{
        bind "Ctrl /" {{
            MessagePluginId {plugin_id} {{
                name "hide_sidebar"
            }}
        }}
    }}
    shared {{
        bind "{new_tab_key}" {{
            MessagePluginId {plugin_id} {{
                name "new_tab_with_sidebar"
            }}
        }}
    }}
}}
"#,
        );
        reconfigure(config, false);
        eprintln!("Keybinds registered for plugin {}: Ctrl+O,{} (toggle), {} (new tab)", plugin_id, toggle_key, new_tab_key);
    }

    fn create_tab_with_sidebar(&self) {
        if let Some(ref layout_path) = self.session_layout {
            new_tabs_with_layout_info(LayoutInfo::File(layout_path.clone(), Default::default()));
        } else {
            new_tabs_with_layout("layout { pane }");
        }
    }

    fn toggle_focus(&mut self) {
        if self.is_focused {
            set_selectable(false);
            self.is_focused = false;
        } else if !self.is_hidden {
            set_selectable(true);
            focus_plugin_pane(get_plugin_ids().plugin_id, false, false);
            self.is_focused = true;
        }
    }


    fn toggle_hide(&mut self) {
        if self.is_focused {
            set_selectable(false);
            self.is_focused = false;
        }
        if self.is_hidden {
            if let Some(ref layout_path) = self.session_layout {
                // First unsuppress ourselves so override_layout can match us to the sidebar slot
                show_self(false);
                // Then re-apply the session layout to fix positioning at 10% left.
                // retain_plugins=true: existing tab-bar, status-bar, and sidebar are reused
                // (no duplicates since we're unsuppressed and matchable).
                eprintln!("Showing sidebar via show_self + override_layout: {}", layout_path);
                override_layout(
                    LayoutInfo::File(layout_path.clone(), Default::default()),
                    true,  // retain_existing_terminal_panes
                    true,  // retain_existing_plugin_panes
                    true,  // apply_only_to_active_tab
                    BTreeMap::new(),
                );
                // Ensure sidebar is not focusable/navigable as a regular pane
                set_selectable(false);
            }
            self.is_hidden = false;
        } else {
            // Suppress the sidebar pane — it vanishes, other panes expand to fill.
            // Tab bar, status bar, and terminal panes are completely untouched.
            hide_self();
            self.is_hidden = true;
        }
    }
}

// --- Plugin Lifecycle (excluded from test builds) ---

#[cfg(not(test))]
register_plugin!(State);

#[cfg(not(test))]
impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.session_layout = configuration.get("session_layout").map(|p| expand_tilde(p));
        self.is_primary = configuration.get("is_primary").map(|v| v != "false").unwrap_or(true);
        if let Some(k) = configuration.get("toggle_key") { self.toggle_key = k.clone(); }
        if let Some(k) = configuration.get("new_tab_key") { self.new_tab_key = k.clone(); }
        self.hint = configuration.get("hint").cloned();

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::Reconfigure,
            PermissionType::RunCommands,
        ]);

        subscribe(&[
            EventType::SessionUpdate,
            EventType::PermissionRequestResult,
            EventType::Key,
            EventType::Mouse,
            EventType::Timer,
            EventType::RunCommandResult,
        ]);

        set_selectable(true);
        self.load_ai_states();
        eprintln!("Plugin loaded, requesting permissions");
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.permissions_granted = true;
                set_selectable(false);
                if self.is_primary {
                    self.setup_toggle_keybind();
                }
                set_timeout(2.0);
                eprintln!("Permissions granted");
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                eprintln!("Permissions denied — plugin cannot function");
                false
            }
            Event::RunCommandResult(_, stdout, _, context) => {
                if context.get(CMD_KEY).map(|s| s.as_str()) == Some(CMD_LOAD_AI) {
                    self.apply_ai_states_from_output(&stdout);
                    true
                } else {
                    false
                }
            }
            Event::SessionUpdate(sessions, _resurrectable) => {
                self.rebuild_from_session_update(&sessions);
                self.load_ai_states();
                // Re-register keybind on every SessionUpdate — each session's plugin has a
                // unique plugin_id, so switching sessions would otherwise point the bind at
                // the wrong instance.
                if self.is_primary && self.permissions_granted {
                    self.setup_toggle_keybind();
                }

                // Auto-track cursor to current session when sidebar is not focused
                if !self.is_focused {
                    let rows = self.build_visible_rows();
                    if let Some(pos) = rows.iter().position(|r| {
                        matches!(r, TreeRow::Session(si) if self.sessions[*si].is_current)
                    }) {
                        self.cursor = pos;
                    }
                }

                self.clamp_cursor();
                self.initial_load_complete = true;
                true
            }
            Event::Timer(_) => {
                self.load_ai_states();
                set_timeout(10.0);
                true
            }
            Event::Key(key) => match key.bare_key {
                BareKey::Down if key.has_no_modifiers() => {
                    let len = self.build_visible_rows().len();
                    if len > 0 {
                        self.cursor = (self.cursor + 1).min(len - 1);
                    }
                    true
                }
                BareKey::Up if key.has_no_modifiers() => {
                    self.cursor = self.cursor.saturating_sub(1);
                    true
                }
                BareKey::Enter if key.has_no_modifiers() => {
                    let rows = self.build_visible_rows();
                    match rows.get(self.cursor) {
                        Some(TreeRow::Session(si)) => {
                            let name = self.sessions[*si].name.clone();
                            self.attention_sessions.remove(&name);
                            switch_session(Some(&name));
                            set_selectable(false);
                            self.is_focused = false;
                        }
                        Some(TreeRow::Tab(si, ti)) => {
                            let name = self.sessions[*si].name.clone();
                            let tab_idx = self.sessions[*si].tabs[*ti].index;
                            self.attention_sessions.remove(&name);
                            switch_session_with_focus(&name, Some(tab_idx), None);
                            set_selectable(false);
                            self.is_focused = false;
                        }
                        None => {}
                    }
                    true
                }
                BareKey::Delete if key.has_no_modifiers() => {
                    if let Some(si) = self.cursor_session_index() {
                        let session = &self.sessions[si];
                        if !session.is_current {
                            kill_sessions(&[session.name.clone()]);
                        }
                    }
                    true
                }
                BareKey::Right if key.has_no_modifiers() => {
                    let si = match self.build_visible_rows().get(self.cursor) {
                        Some(TreeRow::Session(si)) => Some(*si),
                        _ => None,
                    };
                    if let Some(si) = si {
                        let name = self.sessions[si].name.clone();
                        self.expanded_sessions.insert(name);
                    }
                    true
                }
                BareKey::Left if key.has_no_modifiers() => {
                    let target = match self.build_visible_rows().get(self.cursor) {
                        Some(TreeRow::Session(si)) => Some((*si, false)),
                        Some(TreeRow::Tab(si, _)) => Some((*si, true)),
                        None => None,
                    };
                    if let Some((si, is_tab)) = target {
                        let name = self.sessions[si].name.clone();
                        self.expanded_sessions.remove(&name);
                        if is_tab {
                            // Move cursor up to the parent session row
                            let new_rows = self.build_visible_rows();
                            if let Some(pos) = new_rows.iter().position(|r| matches!(r, TreeRow::Session(i) if *i == si)) {
                                self.cursor = pos;
                            }
                        }
                    }
                    true
                }
                BareKey::Esc if key.has_no_modifiers() => {
                    set_selectable(false);
                    self.is_focused = false;
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => match mouse {
                Mouse::ScrollUp(_) => {
                    self.cursor = self.cursor.saturating_sub(1);
                    true
                }
                Mouse::ScrollDown(_) => {
                    let len = self.build_visible_rows().len();
                    if len > 0 {
                        self.cursor = (self.cursor + 1).min(len - 1);
                    }
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.last_cols = cols;

        if self.is_hidden {
            return;
        }

        if !self.initial_load_complete {
            return;
        }

        let visible = self.build_visible_rows();
        if visible.is_empty() {
            return;
        }

        // Footer height: stacked ^O + |-key lines when unfocused with hint, else 1
        let footer_items: Vec<(String, String)> = if !self.is_focused {
            let raw = self.hint.as_deref().unwrap_or("^O,o to toggle");
            parse_hint_items(raw)
        } else {
            vec![]
        };
        let footer_height = if !self.is_focused {
            1 + footer_items.len()
        } else {
            1
        };
        // Title row + content + footer
        let title_height = 1;
        let content_rows = rows.saturating_sub(footer_height + title_height);
        self.ensure_cursor_visible(content_rows);

        // Title row (replaces border name since pane is borderless)
        let title = "Sessions";
        let title_clipped: String = title.chars().take(cols).collect();
        let title_text = Text::new(&title_clipped).color_all(COLOR_CYAN);
        print_text_with_coordinates(title_text, 0, 0, Some(cols), None);

        let end = (self.scroll_offset + content_rows).min(visible.len());
        for (i, row_idx) in (self.scroll_offset..end).enumerate() {
            let i = i + title_height; // offset below title row
            let is_selected = row_idx == self.cursor;
            let text = match &visible[row_idx] {
                TreeRow::Session(si) => {
                    let s = &self.sessions[*si];
                    let expand = if self.expanded_sessions.contains(&s.name) { "▼" } else { "▶" };
                    let current_marker = if s.is_current { "*" } else { " " };
                    let line = format!("{}{} {}", expand, current_marker, s.name);
                    let line_clipped: String = line.chars().take(cols).collect();
                    let mut t = Text::new(&line_clipped);
                    if is_selected {
                        t = t.selected();
                    }
                    if s.is_current {
                        t = t.color_range(COLOR_GREEN, 0..line_clipped.chars().count());
                    }
                    t
                }
                TreeRow::Tab(si, ti) => {
                    let s = &self.sessions[*si];
                    let tab = &s.tabs[*ti];
                    let connector = if *ti == s.tabs.len() - 1 { "└" } else { "├" };
                    let dot = if tab.is_active { "●" } else { "○" };
                    let line = format!("  {} {} {}", connector, dot, tab.name);
                    let line_clipped: String = line.chars().take(cols).collect();
                    let mut t = Text::new(&line_clipped);
                    if is_selected {
                        t = t.selected();
                    }
                    if tab.is_active {
                        // Color the active dot green (char at index 4)
                        t = t.color_range(COLOR_GREEN, 4..5);
                    }
                    t
                }
            };
            print_text_with_coordinates(text, 0, i, Some(cols), None);
        }

        // Footer hint — pinned to bottom
        let footer_start = rows.saturating_sub(footer_height);
        if !self.is_focused {
            // First line: "^O"
            let header = Text::new(" ^O").color_all(COLOR_CYAN);
            print_text_with_coordinates(header, 0, footer_start, Some(cols), None);
            // Subsequent lines: " |-key label"
            for (i, (key, label)) in footer_items.iter().enumerate() {
                let line = format!(" |-{} {}", key, label);
                let line_clipped: String = line.chars().take(cols).collect();
                let t = Text::new(&line_clipped).color_all(COLOR_CYAN);
                print_text_with_coordinates(t, 0, footer_start + 1 + i, Some(cols), None);
            }
        } else {
            let hint_text = Text::new(" ↑↓:nav ↵:switch del:kill esc:exit").color_all(COLOR_CYAN);
            print_text_with_coordinates(hint_text, 0, footer_start, Some(cols), None);
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            "toggle_sidebar" => {
                self.toggle_focus();
                true
            }
            "hide_sidebar" => {
                self.toggle_hide();
                true
            }
            "new_tab_with_sidebar" => {
                self.create_tab_with_sidebar();
                true
            }
            name if name.starts_with("sidebar::attention::") => {
                let session_name = name.strip_prefix("sidebar::attention::").unwrap_or("").to_string();
                if !session_name.is_empty() {
                    eprintln!("Attention flagged: {}", session_name);
                    self.attention_sessions.insert(session_name);
                }
                true
            }
            name if name.starts_with("sidebar::clear::") => {
                let session_name = name.strip_prefix("sidebar::clear::").unwrap_or("");
                self.attention_sessions.remove(session_name);
                eprintln!("Attention cleared: {}", session_name);
                true
            }
            "focus_sidebar" => {
                set_selectable(true);
                show_self(false);
                self.is_focused = true;
                eprintln!("Sidebar activated via pipe (legacy focus_sidebar)");
                true
            }
            name if name.starts_with("sidebar::ai-active::") => {
                let rest = name.strip_prefix("sidebar::ai-active::").unwrap_or("");
                let (session, agent) = parse_session_agent(rest);
                if !session.is_empty() {
                    if !matches!(self.ai_states.get(session), Some(AgentState::Active)) {
                        self.ai_state_since.insert(session.to_string(), self.now_secs());
                    }
                    self.ai_states.insert(session.to_string(), AgentState::Active);
                    if let Some(a) = agent {
                        self.ai_agent_name.insert(session.to_string(), a.to_string());
                    }
                }
                true
            }
            name if name.starts_with("sidebar::ai-idle::") => {
                let rest = name.strip_prefix("sidebar::ai-idle::").unwrap_or("");
                let (session, agent) = parse_session_agent(rest);
                if !session.is_empty() {
                    self.ai_state_since.insert(session.to_string(), self.now_secs());
                    self.ai_states.insert(session.to_string(), AgentState::Idle);
                    if let Some(a) = agent {
                        self.ai_agent_name.insert(session.to_string(), a.to_string());
                    }
                }
                true
            }
            name if name.starts_with("sidebar::ai-waiting::") => {
                let rest = name.strip_prefix("sidebar::ai-waiting::").unwrap_or("");
                let (session, agent) = parse_session_agent(rest);
                if !session.is_empty() {
                    self.ai_state_since.insert(session.to_string(), self.now_secs());
                    self.ai_states.insert(session.to_string(), AgentState::Waiting);
                    if let Some(a) = agent {
                        self.ai_agent_name.insert(session.to_string(), a.to_string());
                    }
                }
                true
            }
            "sidebar::pill" => {
                let session = pipe_message.args.get("session").cloned();
                let key = pipe_message.args.get("key").cloned();
                let value = pipe_message.args.get("value").cloned();
                if let (Some(session), Some(key), Some(value)) = (session, key, value) {
                    self.pills.entry(session.clone()).or_default().insert(key.clone(), value.clone());
                    eprintln!("Pill set: {}={} for {}", key, value, session);
                    true
                } else {
                    false
                }
            }
            "sidebar::pill-clear" => {
                if let Some(session) = pipe_message.args.get("session").cloned() {
                    if let Some(key) = pipe_message.args.get("key") {
                        self.pills.entry(session.clone()).or_default().remove(key);
                        eprintln!("Pill cleared: {} for {}", key, session);
                    } else {
                        self.pills.remove(&session);
                        eprintln!("All pills cleared for {}", session);
                    }
                    true
                } else {
                    false
                }
            }
            "sidebar::progress" => {
                let session = pipe_message.args.get("session").cloned();
                let pct_str = pipe_message.args.get("pct").cloned();
                if let (Some(session), Some(pct_str)) = (session, pct_str) {
                    if let Ok(pct) = pct_str.parse::<u8>() {
                        if pct == 0 {
                            self.progress.remove(&session);
                        } else {
                            self.progress.insert(session.clone(), pct.min(100));
                        }
                        eprintln!("Progress set: {}% for {}", pct, session);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            "sidebar::progress-clear" => {
                if let Some(session) = pipe_message.args.get("session").cloned() {
                    self.progress.remove(&session);
                    eprintln!("Progress cleared for {}", session);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a minimal SessionInfo with just name + is_current + tabs
    fn make_session(name: &str, is_current: bool, tabs: Vec<TabInfo>) -> SessionInfo {
        SessionInfo {
            name: name.to_string(),
            is_current_session: is_current,
            tabs,
            ..Default::default()
        }
    }

    fn make_tab(position: usize, name: &str, active: bool) -> TabInfo {
        TabInfo {
            position,
            name: name.to_string(),
            active,
            ..Default::default()
        }
    }

    fn make_state_with_sessions(sessions: &[(&str, bool, &[&str])]) -> State {
        // sessions: (name, is_current, tab_names)
        let mut state = State::default();
        state.sessions = sessions.iter().map(|(name, is_current, tab_names)| {
            SessionNode {
                name: name.to_string(),
                is_current: *is_current,
                tabs: tab_names.iter().enumerate().map(|(ti, tname)| TabNode {
                    index: ti,
                    name: tname.to_string(),
                    is_active: ti == 0,
                }).collect(),
            }
        }).collect();
        state
    }

    // --- parse_session_agent ---

    #[test]
    fn test_parse_session_agent_no_agent() {
        let (session, agent) = parse_session_agent("my-session");
        assert_eq!(session, "my-session");
        assert_eq!(agent, None);
    }

    #[test]
    fn test_parse_session_agent_with_agent() {
        let (session, agent) = parse_session_agent("my-session::claude");
        assert_eq!(session, "my-session");
        assert_eq!(agent, Some("claude"));
    }

    #[test]
    fn test_parse_session_agent_empty_agent() {
        let (session, agent) = parse_session_agent("my-session::");
        assert_eq!(session, "my-session");
        assert_eq!(agent, None);
    }

    // --- expand_tilde ---

    #[test]
    fn test_expand_tilde_absolute() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
    }

    #[test]
    fn test_expand_tilde_tilde_only() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn test_expand_tilde_tilde_prefix() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand_tilde("~/projects"), format!("{}/projects", home));
    }

    // --- format_duration ---

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(State::format_duration(0), "0s");
        assert_eq!(State::format_duration(1), "1s");
        assert_eq!(State::format_duration(59), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(State::format_duration(60), "1m");
        assert_eq!(State::format_duration(90), "1m");
        assert_eq!(State::format_duration(3599), "59m");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(State::format_duration(3600), "1h");
        assert_eq!(State::format_duration(7200), "2h");
    }

    // --- rebuild_from_session_update ---

    #[test]
    fn test_rebuild_current_session_sorts_first() {
        let mut state = State::default();
        let sessions = vec![
            make_session("zebra", false, vec![]),
            make_session("alpha", false, vec![]),
            make_session("current", true, vec![]),
        ];
        state.rebuild_from_session_update(&sessions);
        assert_eq!(state.sessions[0].name, "current");
    }

    #[test]
    fn test_rebuild_non_current_sorted_alpha() {
        let mut state = State::default();
        let sessions = vec![
            make_session("zebra", false, vec![]),
            make_session("alpha", false, vec![]),
            make_session("mango", false, vec![]),
        ];
        state.rebuild_from_session_update(&sessions);
        assert_eq!(state.sessions[0].name, "alpha");
        assert_eq!(state.sessions[1].name, "mango");
        assert_eq!(state.sessions[2].name, "zebra");
    }

    #[test]
    fn test_rebuild_auto_expands_current() {
        let mut state = State::default();
        let sessions = vec![
            make_session("other", false, vec![]),
            make_session("current", true, vec![]),
        ];
        state.rebuild_from_session_update(&sessions);
        assert!(state.expanded_sessions.contains("current"));
        assert!(!state.expanded_sessions.contains("other"));
    }

    #[test]
    fn test_rebuild_prunes_expanded_for_gone_sessions() {
        let mut state = State::default();
        state.expanded_sessions.insert("gone-session".to_string());
        let sessions = vec![make_session("alive", false, vec![])];
        state.rebuild_from_session_update(&sessions);
        assert!(!state.expanded_sessions.contains("gone-session"));
    }

    #[test]
    fn test_rebuild_keeps_expanded_for_existing_sessions() {
        let mut state = State::default();
        state.expanded_sessions.insert("still-here".to_string());
        let sessions = vec![make_session("still-here", false, vec![])];
        state.rebuild_from_session_update(&sessions);
        assert!(state.expanded_sessions.contains("still-here"));
    }

    #[test]
    fn test_rebuild_extracts_tab_data() {
        let mut state = State::default();
        let sessions = vec![make_session("s", false, vec![
            make_tab(0, "main", true),
            make_tab(1, "logs", false),
        ])];
        state.rebuild_from_session_update(&sessions);
        assert_eq!(state.sessions[0].tabs.len(), 2);
        assert_eq!(state.sessions[0].tabs[0].name, "main");
        assert!(state.sessions[0].tabs[0].is_active);
        assert_eq!(state.sessions[0].tabs[1].name, "logs");
        assert!(!state.sessions[0].tabs[1].is_active);
    }

    // --- build_visible_rows ---

    #[test]
    fn test_visible_rows_all_collapsed() {
        let mut state = make_state_with_sessions(&[
            ("session-a", false, &["main"]),
            ("session-b", false, &["main"]),
        ]);
        state.expanded_sessions.clear();
        let rows = state.build_visible_rows();
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], TreeRow::Session(0)));
        assert!(matches!(rows[1], TreeRow::Session(1)));
    }

    #[test]
    fn test_visible_rows_one_expanded() {
        let mut state = make_state_with_sessions(&[
            ("session-a", false, &["main", "logs"]),
            ("session-b", false, &["main"]),
        ]);
        state.expanded_sessions.insert("session-a".to_string());
        let rows = state.build_visible_rows();
        // session-a + 2 tabs + session-b
        assert_eq!(rows.len(), 4);
        assert!(matches!(rows[0], TreeRow::Session(0)));
        assert!(matches!(rows[1], TreeRow::Tab(0, 0)));
        assert!(matches!(rows[2], TreeRow::Tab(0, 1)));
        assert!(matches!(rows[3], TreeRow::Session(1)));
    }

    #[test]
    fn test_visible_rows_empty() {
        let state = State::default();
        let rows = state.build_visible_rows();
        assert!(rows.is_empty());
    }

    // --- clamp_cursor ---

    #[test]
    fn test_clamp_cursor_within_bounds() {
        let mut state = make_state_with_sessions(&[
            ("a", false, &[]),
            ("b", false, &[]),
        ]);
        state.cursor = 1;
        state.clamp_cursor();
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn test_clamp_cursor_beyond_end() {
        let mut state = make_state_with_sessions(&[
            ("a", false, &[]),
            ("b", false, &[]),
        ]);
        state.cursor = 99;
        state.clamp_cursor();
        assert_eq!(state.cursor, 1); // 2 sessions, max index = 1
    }

    #[test]
    fn test_clamp_cursor_empty_state() {
        let mut state = State::default();
        state.cursor = 5;
        state.clamp_cursor();
        assert_eq!(state.cursor, 0);
    }

    // --- ensure_cursor_visible ---

    #[test]
    fn test_ensure_cursor_visible_already_visible() {
        let mut state = State::default();
        state.cursor = 3;
        state.scroll_offset = 0;
        state.ensure_cursor_visible(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_ensure_cursor_visible_above_viewport() {
        let mut state = State::default();
        state.cursor = 2;
        state.scroll_offset = 5; // cursor is above
        state.ensure_cursor_visible(10);
        assert_eq!(state.scroll_offset, 2);
    }

    #[test]
    fn test_ensure_cursor_visible_below_viewport() {
        let mut state = State::default();
        state.cursor = 15;
        state.scroll_offset = 0;
        state.ensure_cursor_visible(10); // viewport shows rows 0..9
        assert_eq!(state.scroll_offset, 6); // 15 - (10 - 1)
    }

    #[test]
    fn test_ensure_cursor_visible_zero_rows() {
        let mut state = State::default();
        state.cursor = 5;
        state.scroll_offset = 0;
        state.ensure_cursor_visible(0); // no-op
        assert_eq!(state.scroll_offset, 0);
    }

    // --- cursor_session_index ---

    #[test]
    fn test_cursor_session_index_on_session_row() {
        let mut state = make_state_with_sessions(&[
            ("a", false, &[]),
            ("b", false, &[]),
        ]);
        state.cursor = 0;
        assert_eq!(state.cursor_session_index(), Some(0));
        state.cursor = 1;
        assert_eq!(state.cursor_session_index(), Some(1));
    }

    #[test]
    fn test_cursor_session_index_on_tab_row() {
        let mut state = make_state_with_sessions(&[
            ("a", false, &["main", "logs"]),
            ("b", false, &[]),
        ]);
        state.expanded_sessions.insert("a".to_string());
        // rows: Session(0), Tab(0,0), Tab(0,1), Session(1)
        state.cursor = 1; // Tab(0,0)
        assert_eq!(state.cursor_session_index(), Some(0));
        state.cursor = 2; // Tab(0,1)
        assert_eq!(state.cursor_session_index(), Some(0));
        state.cursor = 3; // Session(1)
        assert_eq!(state.cursor_session_index(), Some(1));
    }

    #[test]
    fn test_cursor_session_index_empty() {
        let state = State::default();
        assert_eq!(state.cursor_session_index(), None);
    }

    // --- apply_ai_states_from_output ---

    #[test]
    fn test_apply_ai_states_active() {
        let mut state = State::default();
        let output = b"my-session active 1700000000 0 claude\n";
        state.apply_ai_states_from_output(output);
        assert!(matches!(state.ai_states.get("my-session"), Some(AgentState::Active)));
        assert_eq!(state.ai_state_since.get("my-session"), Some(&1700000000));
        assert_eq!(state.ai_agent_name.get("my-session").map(|s| s.as_str()), Some("claude"));
    }

    #[test]
    fn test_apply_ai_states_idle_with_duration() {
        let mut state = State::default();
        let output = b"work idle 1700000100 42 opencode\n";
        state.apply_ai_states_from_output(output);
        assert!(matches!(state.ai_states.get("work"), Some(AgentState::Idle)));
        assert_eq!(state.ai_last_duration.get("work"), Some(&42));
        assert_eq!(state.ai_agent_name.get("work").map(|s| s.as_str()), Some("opencode"));
    }

    #[test]
    fn test_apply_ai_states_waiting() {
        let mut state = State::default();
        let output = b"session-x waiting 1700000200 0\n";
        state.apply_ai_states_from_output(output);
        assert!(matches!(state.ai_states.get("session-x"), Some(AgentState::Waiting)));
    }

    #[test]
    fn test_apply_ai_states_unknown_skipped() {
        let mut state = State::default();
        let output = b"session-x unknown 0 0\n";
        state.apply_ai_states_from_output(output);
        assert!(state.ai_states.get("session-x").is_none());
    }

    #[test]
    fn test_apply_ai_states_malformed_skipped() {
        let mut state = State::default();
        let output = b"only-one-field\n\n   \n";
        state.apply_ai_states_from_output(output);
        assert!(state.ai_states.is_empty());
    }

    #[test]
    fn test_apply_ai_states_multiple_sessions() {
        let mut state = State::default();
        let output = b"sess-a active 100 0 claude\nsess-b idle 200 30\n";
        state.apply_ai_states_from_output(output);
        assert!(matches!(state.ai_states.get("sess-a"), Some(AgentState::Active)));
        assert!(matches!(state.ai_states.get("sess-b"), Some(AgentState::Idle)));
        assert_eq!(state.ai_last_duration.get("sess-b"), Some(&30));
    }
}
