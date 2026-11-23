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
    /// Theme.
    pub theme: ThemePref,
    /// The Muted section is expanded.
    pub muted_open: bool,
    /// Base UI font size in points.
    pub font_size: f32,
    /// The send key.
    pub send_key: SendKey,
    /// The sidebar's width, and whether it is collapsed to the strip.
    pub sidebar: crate::strip::SidebarSize,
    /// The docked agents panel's width (D105), between [`AGENTS_MIN_WIDTH`] and
    /// [`AGENTS_MAX_WIDTH`].
    pub agents_width: f32,
    /// D127: closing the window hides it to the system tray (on by default).
    pub tray_on_close: bool,
    /// The user's key bindings, as differences from the defaults (D131).
    pub bindings: crate::keys::Overrides,
    /// The Commander's recently used commands, most recent first (D130).
    pub recent: Vec<String>,
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
        Self { grouping: Grouping::default(), collapsed_groups: BTreeSet::new(), ordering: Ordering::default(), docked: false, theme: ThemePref::System, muted_open: false, font_size: 13.5, send_key: SendKey::Enter, sidebar: crate::strip::SidebarSize::default(), agents_width: AGENTS_DEFAULT_WIDTH, tray_on_close: true, bindings: crate::keys::Overrides::default(), recent: Vec::new() }
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
    pub const FONT_SIZE: &str = "fontSize";
    /// Send key.
    pub const SEND_KEY: &str = "sendKey";
    /// The sidebar's width and collapsed state.
    pub const SIDEBAR: &str = "sidebar";
    /// The docked agents panel's width.
    pub const AGENTS_WIDTH: &str = "agentsPanelWidth";
    /// The sidebar's ordering (D116).
    pub const ORDERING: &str = "ordering";
    /// Closing the window hides it to the tray (D127).
    pub const TRAY_ON_CLOSE: &str = "trayOnClose";
    /// The key bindings that differ from the defaults (D131).
    pub const KEY_BINDINGS: &str = "keyBindings";
    /// The Commander's recently used commands (D130).
    pub const COMMANDER_RECENT: &str = "commanderRecent";
    /// Every key.
    pub const ALL: [&str; 13] = [GROUPING, AGENTS_DOCKED, THEME, COLLAPSED, COLLAPSED_GROUPS, FONT_SIZE, SEND_KEY, SIDEBAR, AGENTS_WIDTH, ORDERING, TRAY_ON_CLOSE, KEY_BINDINGS, COMMANDER_RECENT];
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
            keys::THEME => json!(match self.theme {
                ThemePref::System => "system",
                ThemePref::Dark => "dark",
                ThemePref::Light => "light",
            }),
            keys::COLLAPSED => json!({ "muted": !self.muted_open }),
            keys::COLLAPSED_GROUPS => json!(self.collapsed_groups),
            keys::FONT_SIZE => json!(self.font_size),
            keys::SIDEBAR => json!({ "width": self.sidebar.width, "collapsed": self.sidebar.collapsed }),
            keys::AGENTS_WIDTH => json!(self.agents_width),
            keys::TRAY_ON_CLOSE => json!(self.tray_on_close),
            keys::KEY_BINDINGS => self.bindings.value(),
            keys::COMMANDER_RECENT => json!(self.recent),
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
            keys::KEY_BINDINGS => {
                if v.is_object() {
                    self.bindings = crate::keys::Overrides::from_value(v);
                }
            }
            keys::COMMANDER_RECENT => {
                if let Some(list) = v.as_array() {
                    self.recent = list.iter().filter_map(Value::as_str).filter(|id| crate::commands::by_id(id).is_some()).take(crate::commands::RECENT_MAX).map(str::to_owned).collect();
                }
            }
            keys::THEME => {
                self.theme = match v.as_str() {
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
