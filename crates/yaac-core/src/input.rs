//! How the user points at things: a mouse and keyboard, or a touch screen (the Android
//! client, docs/design/android-screens.md). The few texts that name an input mechanic
//! ("Ctrl+click rows") say the touch equivalent instead, per the mapping table there. This is
//! about the input only: it never depends on the provider. [`Platform`] is the same idea for
//! the few settings whose meaning differs on Android (the tray, the renderer).

/// The window's input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    /// A mouse and keyboard (the desktop windows).
    #[default]
    Pointer,
    /// A touch screen without a pointer (the Android client).
    Touch,
}

impl InputMode {
    /// From the view protocol's `shell.inputMode` (`"pointer"` or `"touch"`).
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "pointer" => Some(Self::Pointer),
            "touch" => Some(Self::Touch),
            _ => None,
        }
    }

    /// `pointer` on a mouse and keyboard, `touch` on a touch screen.
    pub fn pick(self, pointer: &'static str, touch: &'static str) -> &'static str {
        match self {
            Self::Pointer => pointer,
            Self::Touch => touch,
        }
    }
}

/// What the window runs on, where that changes what a setting means: the desktop, or the
/// Android app (where the tray is an ongoing notification and Android draws the window).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Platform {
    #[default]
    Desktop,
    Android,
}

impl Platform {
    /// From the view protocol's `shell.platform` (`"desktop"` or `"android"`).
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "desktop" => Some(Self::Desktop),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InputMode;

    #[test]
    fn ids_and_picks() {
        assert_eq!(InputMode::from_id("touch"), Some(InputMode::Touch));
        assert_eq!(InputMode::from_id("pointer"), Some(InputMode::Pointer));
        assert_eq!(InputMode::from_id("pen"), None);
        assert_eq!(InputMode::default().pick("a", "b"), "a");
        assert_eq!(InputMode::Touch.pick("a", "b"), "b");
    }
}
