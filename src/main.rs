use zellij_tile::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// Zellij emphasis color indices (theme-dependent)
//   0 = emphasis_0 (orange)
//   1 = emphasis_1 (cyan)
//   2 = emphasis_2 (green)
//   3 = emphasis_3 (magenta)
const COLOR_ORANGE: usize = 0;
const COLOR_CYAN: usize = 1;
const COLOR_GREEN: usize = 2;
const COLOR_MAGENTA: usize = 3;

const CMD_KEY: &str = "cmd";
const CMD_LOAD_AI: &str = "load_ai";

// --- Data Model ---

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AgentState {
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
struct TabNode {
    index: usize,   // tab position for switch_session_with_focus
    name: String,
    is_active: bool,
    pane_count: usize,
}

#[derive(Clone)]
struct SessionNode {
    name: String,
    is_current: bool,
    tabs: Vec<TabNode>,
}

enum TreeRow {
    Session(usize),        // sessions[si]
    Tab(usize, usize),     // sessions[si].tabs[ti]
}

// --- State ---

struct State {
    permissions_granted: bool,
    sessions: Vec<SessionNode>,
    expanded_sessions: BTreeSet<String>,
    cursor: usize,
    scroll_offset: usize,
    initial_load_complete: bool,
    is_focused: bool,
    session_layout: Option<String>,
    is_primary: bool,

    // Attention and AI state — keyed by session name, survive SessionUpdate
    attention_sessions: BTreeSet<String>,
    ai_states: BTreeMap<String, AgentState>,
    ai_state_since: BTreeMap<String, u64>,
    ai_last_duration: BTreeMap<String, u64>,
    ai_pane_count: BTreeMap<String, usize>,
    ai_agent_name: BTreeMap<String, String>,

    // Pills and progress — keyed by session name
    pills: BTreeMap<String, BTreeMap<String, String>>,
    progress: BTreeMap<String, u8>,
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

register_plugin!(State);

// --- Helpers ---

fn parse_session_agent(rest: &str) -> (&str, Option<&str>) {
    if let Some(idx) = rest.find("::") {
        let agent = &rest[idx + 2..];
        (&rest[..idx], if agent.is_empty() { None } else { Some(agent) })
    } else {
        (rest, None)
    }
}

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
        format!("{}/{}", home, rest)
    } else {
        path.to_string()
    }
}

// --- State Methods ---

impl State {
    fn rebuild_from_session_update(&mut self, sessions: &[SessionInfo]) {
        let current_name = sessions.iter()
            .find(|s| s.is_current_session)
            .map(|s| s.name.clone());

        self.sessions = sessions.iter().map(|s| {
            let tabs = s.tabs.iter().enumerate().map(|(i, t)| TabNode {
                index: i,
                name: t.name.clone(),
                is_active: t.active,
                pane_count: s.panes.panes.get(&t.position).map(|p| p.len()).unwrap_or(0),
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

    fn build_visible_rows(&self) -> Vec<TreeRow> {
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

    fn clamp_cursor(&mut self) {
        let len = self.build_visible_rows().len();
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(len - 1);
        }
    }

    fn ensure_cursor_visible(&mut self, visible_rows: usize) {
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
    fn cursor_session_index(&self) -> Option<usize> {
        let rows = self.build_visible_rows();
        match rows.get(self.cursor) {
            Some(TreeRow::Session(si)) => Some(*si),
            Some(TreeRow::Tab(si, _)) => Some(*si),
            None => None,
        }
    }

    fn setup_toggle_keybind(&self) {
        let plugin_id = get_plugin_ids().plugin_id;
        let config = format!(
            r#"
keybinds {{
    shared {{
        bind "Super o" {{
            MessagePluginId {plugin_id} {{
                name "toggle_sidebar"
            }}
        }}
        bind "Super t" {{
            MessagePluginId {plugin_id} {{
                name "new_tab_with_sidebar"
            }}
        }}
    }}
}}
"#,
        );
        reconfigure(config, false);
        eprintln!("Keybinds registered for plugin {}: Super+o (toggle), Super+t (new tab)", plugin_id);
    }

    fn create_tab_with_sidebar(&self) {
        if let Some(ref layout_path) = self.session_layout {
            new_tabs_with_layout_info(LayoutInfo::File(layout_path.clone(), Default::default()));
        } else {
            new_tabs_with_layout("layout { pane }");
        }
    }

    fn toggle_visibility(&mut self) {
        if self.is_focused {
            set_selectable(false);
            self.is_focused = false;
        } else {
            set_selectable(true);
            focus_plugin_pane(get_plugin_ids().plugin_id, false, false);
            self.is_focused = true;
        }
    }

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

    fn apply_ai_states_from_output(&mut self, stdout: &[u8]) {
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

    fn now_secs(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn format_elapsed(&self, session: &str) -> String {
        if let Some(&since) = self.ai_state_since.get(session) {
            let elapsed = self.now_secs().saturating_sub(since);
            Self::format_duration(elapsed)
        } else {
            String::new()
        }
    }

    fn format_last_duration(&self, session: &str) -> String {
        if let Some(&dur) = self.ai_last_duration.get(session) {
            Self::format_duration(dur)
        } else {
            String::new()
        }
    }

    fn format_duration(secs: u64) -> String {
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    }
}

// --- Plugin Lifecycle ---

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.session_layout = configuration.get("session_layout").map(|p| expand_tilde(p));
        self.is_primary = configuration.get("is_primary").map(|v| v != "false").unwrap_or(true);

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
        if !self.initial_load_complete {
            return;
        }

        let visible = self.build_visible_rows();
        if visible.is_empty() {
            return;
        }

        let content_rows = rows.saturating_sub(1); // reserve footer row
        self.ensure_cursor_visible(content_rows);

        let end = (self.scroll_offset + content_rows).min(visible.len());
        for (i, row_idx) in (self.scroll_offset..end).enumerate() {
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
                    let pane_suffix = if tab.pane_count > 1 {
                        format!(" [{}]", tab.pane_count)
                    } else {
                        String::new()
                    };
                    let line = format!("  {} {} {}{}", connector, dot, tab.name, pane_suffix);
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
        let footer_y = rows.saturating_sub(1);
        let hint = if !self.is_focused {
            " ⌘O to toggle"
        } else {
            " ↑↓:nav ↵:switch del:kill esc:exit"
        };
        let hint_text = Text::new(hint).color_all(COLOR_CYAN);
        print_text_with_coordinates(hint_text, 0, footer_y, Some(cols), None);
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            "toggle_sidebar" => {
                self.toggle_visibility();
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
