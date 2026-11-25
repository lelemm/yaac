//! WPF key names and physical modifier bits; the shared emulator encodes the bytes.
use serde_json::Value;
use yaac_term::{Key, KeypadKey, Mods, MouseAction, MouseButton};

pub(super) fn mods(data: &Value) -> Mods {
    let mask = data["mods"].as_u64().unwrap_or(0);
    Mods {
        shift: mask & 1 != 0,
        alt: mask & 2 != 0,
        ctrl: mask & 4 != 0,
        meta: mask & 8 != 0,
    }
}

pub(super) fn key(name: &str) -> Option<Key> {
    Some(match name {
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Home" => Key::Home,
        "End" => Key::End,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Insert" => Key::Insert,
        "Delete" => Key::Delete,
        "Add" => Key::Keypad(KeypadKey::Add),
        "Subtract" => Key::Keypad(KeypadKey::Subtract),
        "Decimal" => Key::Keypad(KeypadKey::Decimal),
        "Multiply" => Key::Keypad(KeypadKey::Multiply),
        "Divide" => Key::Keypad(KeypadKey::Divide),
        _ if name.chars().count() == 1 => Key::Char(name.chars().next()?),
        _ if name.starts_with("NumPad") => Key::Keypad(KeypadKey::Digit(name[6..].parse().ok()?)),
        _ if name.starts_with('F') => Key::F(name[1..].parse().ok()?),
        _ => return None,
    })
}

pub(super) fn mouse(data: &Value) -> Option<(MouseButton, MouseAction)> {
    let button = match data["button"].as_str()? {
        "Left" => MouseButton::Left,
        "Right" => MouseButton::Right,
        "Middle" => MouseButton::Middle,
        "Down" => MouseButton::WheelDown,
        "Up" => MouseButton::WheelUp,
        "None" => MouseButton::None,
        _ => return None,
    };
    let action = match data["action"].as_str()? {
        "down" | "wheel" => MouseAction::Press,
        "up" => MouseAction::Release,
        "move" => MouseAction::Motion,
        _ => return None,
    };
    Some((button, action))
}

/// Input for the shell, or a local change to the shared grid. Selection and search
/// never leave the window, including while the target is disconnected.
pub(super) fn handle(
    term: &yaac_term::Terminal,
    frame: &yaac_term::Frame,
    command: &str,
    data: &Value,
) -> Option<Vec<u8>> {
    let mods = mods(data);
    match command {
        "terminal.key" => term.key(key(data["key"].as_str()?)?, mods),
        "terminal.text" => {
            let text = data["text"].as_str()?;
            if mods.alt && !mods.ctrl && text.chars().count() == 1 {
                term.key(Key::Char(text.chars().next()?), mods)
            } else {
                Some(term.text(text))
            }
        }
        "terminal.focus" => term.focus(data["focused"] == true),
        "terminal.clear" => {
            term.clear();
            None
        }
        "terminal.search" => {
            match data["query"].as_str()? {
                "" => term.search_close(),
                query => term.search(query),
            }
            None
        }
        "terminal.next" => {
            term.search_next(data["backward"] == true);
            None
        }
        "terminal.mouse" => {
            let (button, action) = mouse(data)?;
            let row = usize::try_from(data["row"].as_u64()?)
                .ok()?
                .min(frame.rows.saturating_sub(1));
            let col = usize::try_from(data["col"].as_u64()?)
                .ok()?
                .min(frame.cols.saturating_sub(1));
            let side = if data["right"] == true {
                yaac_term::CellSide::Right
            } else {
                yaac_term::CellSide::Left
            };
            if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown) {
                return term.wheel(
                    if button == MouseButton::WheelUp {
                        3
                    } else {
                        -3
                    },
                    row,
                    col,
                    mods,
                );
            }
            if action == MouseAction::Press
                && button == MouseButton::Left
                && mods.ctrl
                && let Some(link) = frame.link_at(row, col)
            {
                crate::browser::open(&link.uri);
                return None;
            }
            if !mods.shift && term.modes().mouse() {
                return term.mouse(button, action, row, col, mods);
            }
            if button == MouseButton::Left {
                use yaac_term::SelectKind;
                match action {
                    MouseAction::Press if mods.shift && frame.selection.is_some() => {
                        term.select_to(row, col, side)
                    }
                    MouseAction::Press => {
                        let kind = match data["clicks"].as_u64() {
                            Some(2) => SelectKind::Word,
                            Some(3..) => SelectKind::Line,
                            _ if mods.alt => SelectKind::Block,
                            _ => SelectKind::Simple,
                        };
                        term.select_start(row, col, side, kind);
                    }
                    MouseAction::Motion => term.select_to(row, col, side),
                    MouseAction::Release => {}
                }
            }
            None
        }
        _ => None,
    }
}
