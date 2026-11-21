//! Key chords and the user's bindings (D131): every command's keys are its defaults from the
//! command table ([`crate::commands`]) unless the user changed them in Settings → Keyboard; only
//! the differences are stored (a window preference, `keyBindings`).
//!
//! A chord is modifiers plus one key, or a mouse side button. Key names are the toolkit's own
//! logical names ("A", "F2", "Up", "Equals", "Space"), so the window maps them one to one; the
//! text shown to the user uses symbols where they read better ("Alt+←", "Ctrl+=").

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Value, json};

/// A mouse side button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseButton {
    /// The "back" side button.
    Back,
    /// The "forward" side button.
    Forward,
}

/// What is pressed with the modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChordKey {
    /// A keyboard key, by its canonical logical name ("A", "F2", "Up", "Equals").
    Key(String),
    /// A mouse side button.
    Mouse(MouseButton),
}

/// Modifiers plus a key. `ctrl` is the command modifier (Ctrl; Cmd on a Mac).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    /// Ctrl (Cmd on a Mac).
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt (Option).
    pub alt: bool,
    /// The key.
    pub key: ChordKey,
}

/// Canonical name and display symbol of keys whose symbol reads better than their name.
const SYMBOLS: &[(&str, &str)] = &[
    ("Up", "↑"),
    ("Down", "↓"),
    ("Left", "←"),
    ("Right", "→"),
    ("Equals", "="),
    ("Minus", "-"),
    ("Plus", "+"),
    ("Comma", ","),
    ("Period", "."),
    ("Slash", "/"),
    ("Backslash", "\\"),
    ("Semicolon", ";"),
    ("Colon", ":"),
    ("Quote", "'"),
    ("Backtick", "`"),
    ("OpenBracket", "["),
    ("CloseBracket", "]"),
];

/// Other spellings accepted when parsing.
const ALIASES: &[(&str, &str)] = &[
    ("ArrowUp", "Up"),
    ("ArrowDown", "Down"),
    ("ArrowLeft", "Left"),
    ("ArrowRight", "Right"),
    ("Esc", "Escape"),
    ("Del", "Delete"),
    ("Return", "Enter"),
    ("Ins", "Insert"),
    ("PgUp", "PageUp"),
    ("PgDn", "PageDown"),
];

const NAMED: &[&str] = &[
    "Escape", "Tab", "Backspace", "Enter", "Space", "Insert", "Delete", "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
];

/// The canonical name of a key, from a name, an alias, or a symbol; `None` if unknown.
pub fn canonical_key(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Some((name, _)) = SYMBOLS.iter().find(|(n, s)| *s == t || n.eq_ignore_ascii_case(t)) {
        return Some((*name).to_owned());
    }
    if let Some((_, name)) = ALIASES.iter().find(|(a, _)| a.eq_ignore_ascii_case(t)) {
        return Some((*name).to_owned());
    }
    if let Some(n) = NAMED.iter().find(|n| n.eq_ignore_ascii_case(t)) {
        return Some((*n).to_owned());
    }
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase().to_string());
    }
    if let Some(n) = t.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u8>().ok())
        && (1..=35).contains(&n)
    {
        return Some(format!("F{n}"));
    }
    None
}

impl Chord {
    /// A chord on a keyboard key (`name` in any accepted spelling; unknown names are kept).
    pub fn key(ctrl: bool, shift: bool, alt: bool, name: &str) -> Self {
        Self { ctrl, shift, alt, key: ChordKey::Key(canonical_key(name).unwrap_or_else(|| name.to_owned())) }
    }

    /// A mouse side button with no modifiers.
    pub fn mouse(b: MouseButton) -> Self {
        Self { ctrl: false, shift: false, alt: false, key: ChordKey::Mouse(b) }
    }

    /// Parses "Ctrl+Shift+N", "Alt+←", "Ctrl++", "F2", "Mouse back".
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let (mods, key) = if let Some(rest) = t.strip_suffix("++") {
            (rest, "+")
        } else if t == "+" {
            ("", "+")
        } else {
            match t.rsplit_once('+') {
                Some((m, k)) => (m, k),
                None => ("", t),
            }
        };
        let mut c = Self { ctrl: false, shift: false, alt: false, key: ChordKey::Key(String::new()) };
        for m in mods.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => c.ctrl = true,
                "shift" => c.shift = true,
                "alt" | "option" => c.alt = true,
                _ => return None,
            }
        }
        c.key = match key.trim().to_ascii_lowercase().as_str() {
            "mouse back" | "mouseback" | "mouse4" => ChordKey::Mouse(MouseButton::Back),
            "mouse forward" | "mouseforward" | "mouse5" => ChordKey::Mouse(MouseButton::Forward),
            _ => ChordKey::Key(canonical_key(key)?),
        };
        Some(c)
    }

    /// Whether it carries Ctrl or Alt — such a chord may fire while a text field has focus;
    /// a plain one (F2, Delete, Shift+X) only when focus is in the list or the thread view.
    pub fn works_in_text(&self) -> bool {
        self.ctrl || self.alt || matches!(self.key, ChordKey::Mouse(_))
    }

    /// The same chord without Shift: a symbol typed with Shift ("+" on "=") is tried again
    /// this way when nothing is bound to it exactly.
    pub fn without_shift(&self) -> Self {
        Self { shift: false, ..self.clone() }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        match &self.key {
            ChordKey::Mouse(MouseButton::Back) => f.write_str("Mouse back"),
            ChordKey::Mouse(MouseButton::Forward) => f.write_str("Mouse forward"),
            ChordKey::Key(k) => match SYMBOLS.iter().find(|(n, _)| n == k) {
                Some((_, s)) => f.write_str(s),
                None => f.write_str(k),
            },
        }
    }
}

/// "Ctrl+K", or "Alt+← / Mouse back" for several.
pub fn format_chords(chords: &[Chord]) -> String {
    chords.iter().map(ToString::to_string).collect::<Vec<_>>().join(" / ")
}

/// The user's changes to the default bindings, by command id: the chords that replace the
/// defaults (empty: the command has no key).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides(pub BTreeMap<String, Vec<Chord>>);

impl Overrides {
    /// As stored: `{"<command id>": ["Ctrl+X", …]}`.
    pub fn value(&self) -> Value {
        let m: serde_json::Map<String, Value> =
            self.0.iter().map(|(id, cs)| (id.clone(), json!(cs.iter().map(ToString::to_string).collect::<Vec<_>>()))).collect();
        Value::Object(m)
    }

    /// From a stored value; malformed entries are skipped.
    pub fn from_value(v: &Value) -> Self {
        let mut out = BTreeMap::new();
        if let Some(m) = v.as_object() {
            for (id, cs) in m {
                if let Some(list) = cs.as_array() {
                    let chords: Option<Vec<Chord>> = list.iter().map(|c| c.as_str().and_then(Chord::parse)).collect();
                    if let Some(chords) = chords {
                        out.insert(id.clone(), chords);
                    }
                }
            }
        }
        Self(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_and_format_round_trip() {
        for t in ["Ctrl+Space", "Ctrl+Shift+N", "Alt+←", "Alt+→", "Ctrl+↑", "Ctrl+=", "Ctrl+-", "Ctrl+0", "Ctrl+,", "F2", "Ctrl+Delete", "Mouse back", "Mouse forward", "Ctrl++", "Ctrl+Alt+Shift+F12"] {
            let c = Chord::parse(t).unwrap_or_else(|| panic!("{t} parses"));
            assert_eq!(c.to_string(), t);
            assert_eq!(Chord::parse(&c.to_string()), Some(c));
        }
        assert_eq!(Chord::parse("ctrl+shift+n"), Chord::parse("Ctrl+Shift+N"));
        assert_eq!(Chord::parse("Alt+ArrowLeft"), Chord::parse("Alt+Left"));
        assert_eq!(Chord::parse("Cmd+K"), Chord::parse("Ctrl+K"));
        assert_eq!(Chord::parse("Ctrl+Equals").map(|c| c.to_string()).as_deref(), Some("Ctrl+="));
        assert_eq!(Chord::parse("Hyper+K"), None);
        assert_eq!(Chord::parse("Ctrl+"), None);
        assert_eq!(Chord::parse("Ctrl+Nope"), None);
    }

    #[test]
    fn plain_keys_do_not_fire_in_text() {
        assert!(Chord::parse("Ctrl+L").is_some_and(|c| c.works_in_text()));
        assert!(Chord::parse("Alt+↑").is_some_and(|c| c.works_in_text()));
        assert!(Chord::parse("F2").is_some_and(|c| !c.works_in_text()));
        assert!(Chord::parse("Shift+Delete").is_some_and(|c| !c.works_in_text()));
    }

    #[test]
    fn overrides_store_only_what_they_hold() {
        let mut o = Overrides::default();
        o.0.insert("commander".into(), vec![Chord::parse("Ctrl+Shift+Space").unwrap()]);
        o.0.insert("rename".into(), vec![]);
        let v = o.value();
        assert_eq!(v, json!({"commander": ["Ctrl+Shift+Space"], "rename": []}));
        assert_eq!(Overrides::from_value(&v), o);
        assert_eq!(Overrides::from_value(&json!({"x": ["Nope+Q"], "y": 3})), Overrides::default());
    }
}
