//! Window-owned custom themes, stored with the theme selection as one preference.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_BYTES: usize = 16 * 1024;
const MAX_THEMES: usize = 32;
pub const TOKENS: &[&str] = &[
    "bg",
    "panel",
    "raised",
    "faint",
    "border",
    "text",
    "text2",
    "primary",
    "on_primary",
    "selected",
    "needs",
    "done",
    "failed",
    "user_bg",
    "fast",
    "ultrafast",
    "shell",
];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub base: Base,
    pub colors: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Base {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Theme JSON exceeds 16 KiB.".into());
        }
        let theme: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        theme.validate()?;
        Ok(theme)
    }
    fn validate(&self) -> Result<(), String> {
        let name = &self.name;
        if name.trim() != name
            || name.is_empty()
            || name.len() > 64
            || name.chars().any(char::is_control)
            || ["system", "light", "dark", "custom"]
                .iter()
                .any(|s| name.eq_ignore_ascii_case(s))
        {
            return Err("Choose a non-reserved theme name of 1–64 bytes without surrounding whitespace or control characters.".into());
        }
        for (key, value) in &self.colors {
            if !TOKENS.contains(&key.as_str()) {
                return Err(format!("Unknown colour {key}."));
            }
            if value.len() != 7
                || !value.starts_with('#')
                || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err(format!("{key} must be #RRGGBB."));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub selected: String,
    pub entries: BTreeMap<String, Theme>,
}
impl Catalog {
    pub fn valid(&self) -> bool {
        self.entries.len() <= MAX_THEMES
            && self.current().is_some()
            && self
                .entries
                .iter()
                .all(|(k, v)| k == &v.name && v.validate().is_ok())
    }
    pub fn current(&self) -> Option<&Theme> {
        self.entries.get(&self.selected)
    }
    pub fn import(&mut self, theme: Theme, overwrite: bool) -> Result<(), String> {
        theme.validate()?;
        if self.entries.contains_key(&theme.name) {
            if !overwrite {
                return Err(
                    "A theme with this name already exists. Explicit overwrite is required.".into(),
                );
            }
        } else if self.entries.len() >= MAX_THEMES {
            return Err("The library holds up to 32 themes.".into());
        }
        if self.entries.is_empty() {
            self.selected.clone_from(&theme.name);
        }
        self.entries.insert(theme.name.clone(), theme);
        Ok(())
    }
}
