//! Window-side choices that persist: grouping, the docked agents panel, theme, font size, send
//! key, collapsed sections, keeping running in the tray, key bindings and the Commander's recent
//! commands. Attached to a hub, each change is
//! sent as `hub/uiState/set` (the hub is the cache's only writer) and read back from the cache
//! file's `ui_state` table at start; in demo mode they stay in memory. (The renderer choice is
//! not here: it is needed before the window exists, so the window keeps it in its own display
//! file, [`crate::display::DisplayFile`].)

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::sidebar::{Grouping, Order, Ordering};

/// Theme preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePref {
    /// Follow the system.
    #[default]
    System,
    /// Dark.
    Dark,
    /// Light.
    Light,
    /// Use the selected imported palette.
    Custom,
}

impl ThemePref {
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Custom => "custom",
        }
    }
}

/// Which key sends a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SendKey {
    /// Enter sends, Shift+Enter is a newline.
    #[default]
    Enter,
    /// Ctrl+Enter sends, Enter is a newline.
    CtrlEnter,
}

/// Empty selections use the fonts bundled with YAAC.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FontPrefs {
    pub normal: String,
    pub code: String,
}

/// The persisted choices.
#[derive(Clone, Debug, PartialEq)]
pub struct UiPrefs {
    /// The sidebar grouping checkboxes and "Show dismissed".
    pub grouping: Grouping,
    /// Sidebar group IDs whose rows are hidden.
    pub collapsed_groups: BTreeSet<String>,
    /// The sidebar's ordering inside each group (D116).
    pub ordering: Ordering,
    /// The agents panel is docked.
    pub docked: bool,
    /// Default layout for file diff tabs.
    pub diff_side_by_side: bool,
    /// Theme.
    pub theme: ThemePref,
    pub themes: std::sync::Arc<crate::themes::Catalog>,
    /// The Muted section is expanded.
    pub muted_open: bool,
    /// Base UI font size in points.
    pub font_size: f32,
    pub fonts: FontPrefs,
    /// The send key.
    pub send_key: SendKey,
    /// The sidebar's width, and whether it is collapsed to the strip.
    pub sidebar: crate::strip::SidebarSize,
    /// The docked agents panel's width (D105), between [`AGENTS_MIN_WIDTH`] and
    /// [`AGENTS_MAX_WIDTH`].
    pub agents_width: f32,
    /// Fraction of the compact pill’s vertical travel, from top to bottom.
    pub pill_position: f32,
    /// D127: closing the window hides it to the system tray (on by default).
    pub tray_on_close: bool,
    /// The user's key bindings, as differences from the defaults (D131).
    pub bindings: crate::keys::Overrides,
    /// "Works in the terminal" per command, where it differs from the default (P338).
    pub terminal_commands: std::collections::BTreeMap<String, bool>,
    /// The Commander's recently used commands, most recent first (D130).
    pub recent: Vec<String>,
    /// Recent composer messages, newest first.
    pub composer_history: std::sync::Arc<[String]>,
    /// File cards dismissed in this window profile, scoped to the conversation showing them.
    pub dismissed_artifacts: BTreeSet<String>,
    /// The first-use desktop voice call explanation was accepted.
    pub desktop_call_consent: bool,
    /// Settings → Terminal (P339) and the terminal panel's height (P338).
    pub terminal: TerminalPrefs,
}

/// Settings → Terminal (docs/13-terminal.md#clipboard-and-selection) and the panel's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPrefs {
    /// Selecting text copies it (off by default).
    pub copy_on_select: bool,
    /// Ctrl+V goes to the shell (bash's literal-next) instead of pasting.
    pub ctrl_v_to_shell: bool,
    /// Ask before pasting several lines into a shell without bracketed paste ("Don't ask
    /// again" turns it off).
    pub confirm_multiline_paste: bool,
    /// The terminal panel's height as a share of the chat column.
    pub panel_share: f32,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self { copy_on_select: false, ctrl_v_to_shell: false, confirm_multiline_paste: true, panel_share: 1.0 / 3.0 }
    }
}

/// The narrowest docked agents panel.
pub const AGENTS_MIN_WIDTH: f32 = 240.0;
/// The widest.
pub const AGENTS_MAX_WIDTH: f32 = 520.0;
/// Its default width.
pub const AGENTS_DEFAULT_WIDTH: f32 = 300.0;

/// The agents panel's width after its edge was dragged so it would be `to` points wide.
pub fn agents_width(to: f32) -> f32 {
    to.clamp(AGENTS_MIN_WIDTH, AGENTS_MAX_WIDTH)
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self { diff_side_by_side: false, grouping: Grouping::default(), collapsed_groups: BTreeSet::new(), ordering: Ordering::default(), docked: false, theme: ThemePref::System, themes: Default::default(), muted_open: false, font_size: 13.5, fonts: FontPrefs::default(), send_key: SendKey::Enter, sidebar: crate::strip::SidebarSize::default(), agents_width: AGENTS_DEFAULT_WIDTH, pill_position: 0.5, tray_on_close: true, bindings: crate::keys::Overrides::default(), terminal_commands: std::collections::BTreeMap::new(), recent: Vec::new(), composer_history: Default::default(), dismissed_artifacts: BTreeSet::new(), desktop_call_consent: false, terminal: TerminalPrefs::default() }
    }
}

/// The `ui_state` keys.
pub mod keys {
    /// Grouping.
    pub const GROUPING: &str = "grouping";
    /// Docked agents panel.
    pub const AGENTS_DOCKED: &str = "agentsPanelDocked";
    /// Theme.
    pub const THEME: &str = "theme";
    /// Collapsed sections.
    pub const COLLAPSED: &str = "collapsed";
    /// Collapsed sidebar groups.
    pub const COLLAPSED_GROUPS: &str = "collapsedGroups";
    /// Font size.
    pub const FONTS: &str = "fonts";
    pub const FONT_SIZE: &str = "fontSize";
    /// Send key.
    pub const SEND_KEY: &str = "sendKey";
    /// The sidebar's width and collapsed state.
    pub const SIDEBAR: &str = "sidebar";
    /// The docked agents panel's width.
    pub const AGENTS_WIDTH: &str = "agentsPanelWidth";
    /// Vertical position of the floating pill.
    pub const PILL_POSITION: &str = "agentsPillPosition";
    /// The sidebar's ordering (D116).
    pub const ORDERING: &str = "ordering";
    /// Closing the window hides it to the tray (D127).
    pub const TRAY_ON_CLOSE: &str = "trayOnClose";
    /// The key bindings that differ from the defaults (D131).
    pub const KEY_BINDINGS: &str = "keyBindings";
    /// The Commander's recently used commands (D130).
    pub const COMMANDER_RECENT: &str = "commanderRecent";
    /// Recently submitted message drafts.
    pub use yaac_protocol::attach::COMPOSER_HISTORY;
    /// Dismissed generated-file cards.
    pub const DISMISSED_ARTIFACTS: &str = "dismissedArtifacts";
    /// First-use consent for desktop voice calls.
    pub const DESKTOP_CALL_CONSENT: &str = "desktopCallConsent";
    /// Side-by-side layout in diff tabs.
    pub const DIFF_SIDE_BY_SIDE: &str = "diffSideBySide";
    /// Settings → Terminal and the panel's height (P338, P339).
    pub const TERMINAL: &str = "terminal";
    /// Commands whose "Works in the terminal" differs from the default (P338).
    pub const TERMINAL_COMMANDS: &str = "terminalCommands";
    /// Every key.
    pub const ALL: [&str; 21] = [GROUPING, AGENTS_DOCKED, THEME, COLLAPSED, COLLAPSED_GROUPS, FONT_SIZE, SEND_KEY, SIDEBAR, AGENTS_WIDTH, PILL_POSITION, ORDERING, TRAY_ON_CLOSE, KEY_BINDINGS, COMMANDER_RECENT, DIFF_SIDE_BY_SIDE, TERMINAL, TERMINAL_COMMANDS, COMPOSER_HISTORY, DISMISSED_ARTIFACTS, FONTS, DESKTOP_CALL_CONSENT];
}

impl UiPrefs {
    /// The value stored under `key`.
    pub fn value(&self, key: &str) -> Value {
        match key {
            keys::GROUPING => json!({
                "machine": self.grouping.machine,
                "project": self.grouping.project,
                "worktree": self.grouping.worktree,
                "showDismissed": self.grouping.show_dismissed,
            }),
            keys::ORDERING => json!({
                "order": match self.ordering.order {
                    Order::LastMessage => "lastMessage",
                    Order::AToZ => "aToZ",
                    Order::ZToA => "zToA",
                },
                "unseenFirst": self.ordering.unseen_first,
                "needsFirst": self.ordering.needs_first,
                "runningOnly": self.ordering.running_only,
            }),
            keys::AGENTS_DOCKED => json!(self.docked),
            keys::THEME if !self.themes.entries.is_empty() => json!({"mode": self.theme.id(), "catalog": &*self.themes}),
            keys::THEME => json!(self.theme.id()),
            keys::COLLAPSED => json!({ "muted": !self.muted_open }),
            keys::COLLAPSED_GROUPS => json!(self.collapsed_groups),
            keys::FONT_SIZE => json!(self.font_size),
            keys::FONTS => json!(self.fonts),
            keys::SIDEBAR => json!({ "width": self.sidebar.width, "collapsed": self.sidebar.collapsed }),
            keys::PILL_POSITION => json!(self.pill_position),
            keys::AGENTS_WIDTH => json!(self.agents_width),
            keys::TRAY_ON_CLOSE => json!(self.tray_on_close),
            keys::KEY_BINDINGS => self.bindings.value(),
            keys::TERMINAL_COMMANDS => json!(self.terminal_commands),
            keys::DIFF_SIDE_BY_SIDE => json!(self.diff_side_by_side),
            keys::TERMINAL => json!({
                "copyOnSelect": self.terminal.copy_on_select,
                "ctrlVToShell": self.terminal.ctrl_v_to_shell,
                "confirmMultilinePaste": self.terminal.confirm_multiline_paste,
                "panelShare": self.terminal.panel_share,
            }),
            keys::COMMANDER_RECENT => json!(self.recent),
            keys::DISMISSED_ARTIFACTS => json!(self.dismissed_artifacts),
            keys::DESKTOP_CALL_CONSENT => json!(self.desktop_call_consent),
            keys::COMPOSER_HISTORY => json!(self.composer_history.iter()
                .filter(|text| yaac_protocol::attach::history_entry(text))
                .take(yaac_protocol::attach::COMPOSER_HISTORY_LIMIT).collect::<Vec<_>>()),
            keys::SEND_KEY => json!(match self.send_key {
                SendKey::Enter => "enter",
                SendKey::CtrlEnter => "ctrlEnter",
            }),
            _ => Value::Null,
        }
    }

    /// Apply a stored value; unknown keys and malformed values are ignored (the default stays).
    pub fn load(&mut self, key: &str, v: &Value) {
        let b = |field: &str, cur: bool| v.get(field).and_then(Value::as_bool).unwrap_or(cur);
        match key {
            keys::GROUPING => {
                let g = &mut self.grouping;
                g.machine = b("machine", g.machine);
                g.project = b("project", g.project);
                g.worktree = b("worktree", g.worktree);
                g.show_dismissed = b("showDismissed", g.show_dismissed);
            }
            keys::ORDERING => {
                let o = &mut self.ordering;
                o.order = match v.get("order").and_then(Value::as_str) {
                    Some("lastMessage") => Order::LastMessage,
                    Some("aToZ") => Order::AToZ,
                    Some("zToA") => Order::ZToA,
                    _ => o.order,
                };
                o.unseen_first = b("unseenFirst", o.unseen_first);
                o.needs_first = b("needsFirst", o.needs_first);
                o.running_only = b("runningOnly", o.running_only);
            }
            keys::AGENTS_DOCKED => self.docked = v.as_bool().unwrap_or(self.docked),
            keys::TRAY_ON_CLOSE => self.tray_on_close = v.as_bool().unwrap_or(self.tray_on_close),
            keys::DESKTOP_CALL_CONSENT => self.desktop_call_consent = v.as_bool().unwrap_or(self.desktop_call_consent),
            keys::KEY_BINDINGS => {
                if v.is_object() {
                    self.bindings = crate::keys::Overrides::from_value(v);
                }
            }
            keys::DIFF_SIDE_BY_SIDE => self.diff_side_by_side = v.as_bool().unwrap_or(self.diff_side_by_side),
            keys::TERMINAL_COMMANDS => {
                if let Some(m) = v.as_object() {
                    self.terminal_commands = m.iter().filter(|(id, _)| crate::commands::by_id(id).is_some()).filter_map(|(id, on)| Some((id.clone(), on.as_bool()?))).collect();
                }
            }
            keys::TERMINAL => {
                let t = &mut self.terminal;
                t.copy_on_select = b("copyOnSelect", t.copy_on_select);
                t.ctrl_v_to_shell = b("ctrlVToShell", t.ctrl_v_to_shell);
                t.confirm_multiline_paste = b("confirmMultilinePaste", t.confirm_multiline_paste);
                #[allow(clippy::cast_possible_truncation)]
                let share = v.get("panelShare").and_then(Value::as_f64).map(|f| f as f32);
                t.panel_share = share.filter(|s| (0.05..=0.95).contains(s)).unwrap_or(t.panel_share);
            }
            keys::COMMANDER_RECENT => {
                if let Some(list) = v.as_array() {
                    self.recent = list.iter().filter_map(Value::as_str).filter(|id| crate::commands::by_id(id).is_some()).take(crate::commands::RECENT_MAX).map(str::to_owned).collect();
                }
            }
            keys::COMPOSER_HISTORY => {
                if v.is_array() {
                    self.composer_history = yaac_protocol::attach::history_entries(v)
                        .map(str::to_owned).collect::<Vec<_>>().into();
                }
            }
            keys::DISMISSED_ARTIFACTS => {
                if let Ok(dismissed) = serde_json::from_value(v.clone()) {
                    self.dismissed_artifacts = dismissed;
                }
            }
            keys::THEME => {
                if let Some(catalog) = v.get("catalog") {
                    let Ok(parsed) = serde_json::from_value::<crate::themes::Catalog>(catalog.clone()) else { return };
                    if !parsed.valid() { return; }
                    self.themes = std::sync::Arc::new(parsed);
                }
                self.theme = match v.get("mode").unwrap_or(v).as_str() {
                    Some("custom") if self.themes.current().is_some() => ThemePref::Custom,
                    Some("dark") => ThemePref::Dark,
                    Some("light") => ThemePref::Light,
                    Some("system") => ThemePref::System,
                    _ => self.theme,
                }
            }
            keys::COLLAPSED => self.muted_open = !b("muted", !self.muted_open),
            keys::COLLAPSED_GROUPS => {
                if let Ok(groups) = serde_json::from_value(v.clone()) {
                    self.collapsed_groups = groups;
                }
            }
            #[allow(clippy::cast_possible_truncation, reason = "font sizes are small")]
            keys::FONTS => {
                if let Ok(fonts) = serde_json::from_value(v.clone()) { self.fonts = fonts; }
            }
            keys::FONT_SIZE => {
                if let Some(f) = v.as_f64().filter(|f| (9.0..=24.0).contains(f)) {
                    self.font_size = f as f32;
                }
            }
            #[allow(clippy::cast_possible_truncation, reason = "widths are small")]
            keys::SIDEBAR => {
                if let Some(w) = v.get("width").and_then(Value::as_f64) {
                    self.sidebar.width = (w as f32).clamp(crate::strip::MIN_WIDTH, crate::strip::MAX_WIDTH);
                }
                self.sidebar.collapsed = b("collapsed", self.sidebar.collapsed);
            }
            #[allow(clippy::cast_possible_truncation, reason = "fraction clamped to unit range")]
            keys::PILL_POSITION => {
                if let Some(n) = v.as_f64() {
                    self.pill_position = n.clamp(0.0, 1.0) as f32;
                }
            }
            #[allow(clippy::cast_possible_truncation, reason = "widths are small")]
            keys::AGENTS_WIDTH => {
                if let Some(w) = v.as_f64() {
                    self.agents_width = agents_width(w as f32);
                }
            }
            keys::SEND_KEY => {
                self.send_key = match v.as_str() {
                    Some("ctrlEnter") => SendKey::CtrlEnter,
                    Some("enter") => SendKey::Enter,
                    _ => self.send_key,
                }
            }
            _ => {}
        }
    }

    /// The keys whose values differ between two sets of choices.
    pub fn changed(&self, before: &UiPrefs) -> Vec<&'static str> {
        keys::ALL.into_iter().filter(|k| self.value(k) != before.value(k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agents_panel_width_is_clamped_and_remembered() {
        assert_eq!(agents_width(100.0), AGENTS_MIN_WIDTH);
        assert_eq!(agents_width(900.0), AGENTS_MAX_WIDTH);
        assert_eq!(agents_width(360.0), 360.0);
        let mut p = UiPrefs { agents_width: 410.0, ..UiPrefs::default() };
        let stored = p.value(keys::AGENTS_WIDTH);
        assert_eq!(p.changed(&UiPrefs::default()), [keys::AGENTS_WIDTH]);
        p = UiPrefs::default();
        p.load(keys::AGENTS_WIDTH, &stored);
        assert_eq!(p.agents_width, 410.0);
        p.load(keys::AGENTS_WIDTH, &json!(5000));
        assert_eq!(p.agents_width, AGENTS_MAX_WIDTH);
    }

    #[test]
    fn pill_position_round_trips_and_clamps() {
        let mut prefs = UiPrefs::default();
        assert_eq!(prefs.pill_position, 0.5);
        for (stored, expected) in [(0.375, 0.375), (-1.0, 0.0), (1.5, 1.0)] {
            prefs.load(keys::PILL_POSITION, &json!(stored));
            assert_eq!(prefs.value(keys::PILL_POSITION), json!(expected));
            assert_eq!(prefs.changed(&UiPrefs::default()), [keys::PILL_POSITION]);
        }
    }

    #[test]
    fn keeping_running_in_the_tray_is_on_by_default_and_stored() {
        assert!(UiPrefs::default().tray_on_close);
        let off = UiPrefs { tray_on_close: false, ..UiPrefs::default() };
        assert_eq!(off.changed(&UiPrefs::default()), [keys::TRAY_ON_CLOSE]);
        let mut p = UiPrefs::default();
        p.load(keys::TRAY_ON_CLOSE, &off.value(keys::TRAY_ON_CLOSE));
        assert!(!p.tray_on_close);
        p.load(keys::TRAY_ON_CLOSE, &json!("yes"));
        assert!(!p.tray_on_close, "a malformed value keeps what was there");
    }
}
