//! Window-side choices that persist: grouping, the docked agents panel, theme, font size, send
//! key, and collapsed sections. Attached to a hub, each change is sent as `hub/uiState/set`
//! (the hub is the cache's only writer) and read back from the cache file's `ui_state` table at
//! start; in demo mode they stay in memory.

use serde_json::{Value, json};

use crate::sidebar::Grouping;

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiPrefs {
    /// The sidebar grouping checkboxes and "Show dismissed".
    pub grouping: Grouping,
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
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self { grouping: Grouping::default(), docked: false, theme: ThemePref::System, muted_open: false, font_size: 13.5, send_key: SendKey::Enter }
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
    /// Font size.
    pub const FONT_SIZE: &str = "fontSize";
    /// Send key.
    pub const SEND_KEY: &str = "sendKey";
    /// Every key.
    pub const ALL: [&str; 6] = [GROUPING, AGENTS_DOCKED, THEME, COLLAPSED, FONT_SIZE, SEND_KEY];
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
            keys::AGENTS_DOCKED => json!(self.docked),
            keys::THEME => json!(match self.theme {
                ThemePref::System => "system",
                ThemePref::Dark => "dark",
                ThemePref::Light => "light",
            }),
            keys::COLLAPSED => json!({ "muted": !self.muted_open }),
            keys::FONT_SIZE => json!(self.font_size),
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
            keys::AGENTS_DOCKED => self.docked = v.as_bool().unwrap_or(self.docked),
            keys::THEME => {
                self.theme = match v.as_str() {
                    Some("dark") => ThemePref::Dark,
                    Some("light") => ThemePref::Light,
                    Some("system") => ThemePref::System,
                    _ => self.theme,
                }
            }
            keys::COLLAPSED => self.muted_open = !b("muted", !self.muted_open),
            #[allow(clippy::cast_possible_truncation, reason = "font sizes are small")]
            keys::FONT_SIZE => {
                if let Some(f) = v.as_f64().filter(|f| (9.0..=24.0).contains(f)) {
                    self.font_size = f as f32;
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
