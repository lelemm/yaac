//! Keys, mouse and paste to the bytes each terminal mode expects (xterm encodings, and ConPTY's
//! win32-input-mode for PowerShell targets).
use crate::frame::Modes;

/// A key, renderer-neutral. Printable text normally arrives as text (`Terminal::text`);
/// [`Key::Char`] is for a character typed with Ctrl or Alt, or where the renderer has no text
/// event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// The character the key produces without Ctrl or Alt (Shift applied), e.g. `'c'` for Ctrl+C.
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1–F20.
    F(u8),
    /// A key on the numeric keypad (application keypad mode changes these).
    Keypad(KeypadKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeypadKey {
    Digit(u8),
    Decimal,
    Divide,
    Multiply,
    Subtract,
    Add,
    Enter,
    Equal,
}

/// Modifier keys held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The logo / Command key.
    pub meta: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, ctrl: false, alt: false, meta: false };
    pub const SHIFT: Mods = Mods { shift: true, ..Self::NONE };
    pub const CTRL: Mods = Mods { ctrl: true, ..Self::NONE };
    pub const ALT: Mods = Mods { alt: true, ..Self::NONE };

    fn any(self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }
    /// The xterm modifier parameter (`CSI 1 ; m X`).
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl) + 8 * u8::from(self.meta)
    }
}

/// The bytes for a key press, or `None` for a key that sends nothing.
pub fn encode_key(key: Key, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    if modes.win32_input
        && let Some(seq) = win32(key, mods)
    {
        return Some(seq);
    }
    let esc = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    let csi_mod = |fin: u8| -> Vec<u8> {
        if mods.any() {
            format!("\x1b[1;{}{}", mods.param(), fin as char).into_bytes()
        } else if modes.app_cursor {
            vec![0x1b, b'O', fin]
        } else {
            vec![0x1b, b'[', fin]
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if mods.any() { format!("\x1b[{n};{}~", mods.param()).into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    Some(match key {
        Key::Char(c) => {
            if mods.ctrl {
                let b = ctrl_byte(c).map_or_else(|| c.to_string().into_bytes(), |b| vec![b]);
                esc(b)
            } else {
                esc(c.to_string().into_bytes())
            }
        }
        Key::Enter => esc(if modes.line_feed_new_line { b"\r\n".to_vec() } else { b"\r".to_vec() }),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc(b"\t".to_vec()),
        Key::Backspace => esc(vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Escape => esc(vec![0x1b]),
        Key::Up => csi_mod(b'A'),
        Key::Down => csi_mod(b'B'),
        Key::Right => csi_mod(b'C'),
        Key::Left => csi_mod(b'D'),
        Key::Home => csi_mod(b'H'),
        Key::End => csi_mod(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let fin = b'P' + (n - 1);
            if mods.any() { format!("\x1b[1;{}{}", mods.param(), fin as char).into_bytes() } else { vec![0x1b, b'O', fin] }
        }
        Key::F(n) => tilde(match n {
            5 => 15,
            6 => 17,
            7 => 18,
            8 => 19,
            9 => 20,
            10 => 21,
            11 => 23,
            12 => 24,
            13 => 25,
            14 => 26,
            15 => 28,
            16 => 29,
            17 => 31,
            18 => 32,
            19 => 33,
            20 => 34,
            _ => return None,
        }),
        Key::Keypad(k) => {
            if modes.app_keypad && !mods.any() {
                let fin = match k {
                    KeypadKey::Digit(d) => b'p' + d.min(9),
                    KeypadKey::Decimal => b'n',
                    KeypadKey::Divide => b'o',
                    KeypadKey::Multiply => b'j',
                    KeypadKey::Subtract => b'm',
                    KeypadKey::Add => b'k',
                    KeypadKey::Enter => b'M',
                    KeypadKey::Equal => b'X',
                };
                vec![0x1b, b'O', fin]
            } else {
                let c = match k {
                    KeypadKey::Digit(d) => char::from(b'0' + d.min(9)),
                    KeypadKey::Decimal => '.',
                    KeypadKey::Divide => '/',
                    KeypadKey::Multiply => '*',
                    KeypadKey::Subtract => '-',
                    KeypadKey::Add => '+',
                    KeypadKey::Enter => return encode_key(Key::Enter, mods, modes),
                    KeypadKey::Equal => '=',
                };
                return encode_key(Key::Char(c), mods, modes);
            }
        }
    })
}

/// Ctrl plus a character, as xterm sends it.
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => (c as u8) & 0x1f,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// A win32-input-mode key event pair (down, up): `CSI Vk;Sc;Uc;Kd;Cs;Rc _`. Only for keys that
/// VT cannot carry exactly (anything with Ctrl or Alt, and the non-text keys); plain text still
/// goes as UTF-8, which ConPTY maps to key events itself.
fn win32(key: Key, mods: Mods) -> Option<Vec<u8>> {
    const ENHANCED: u32 = 0x100;
    let (vk, sc, uc, enhanced): (u32, u32, u32, bool) = match key {
        Key::Char(c) => {
            if !(mods.ctrl || mods.alt) {
                return None;
            }
            let (vk, sc) = char_vk(c)?;
            let uc = if mods.ctrl && !mods.alt { ctrl_byte(c).map_or(u32::from(c), u32::from) } else { u32::from(c) };
            let uc = if c == ' ' { 0x20 } else { uc };
            (vk, sc, uc, false)
        }
        Key::Enter => (0x0D, 28, if mods.ctrl { 10 } else { 13 }, false),
        Key::Tab => (0x09, 15, 9, false),
        Key::Backspace => (0x08, 14, if mods.ctrl { 0x7f } else { 8 }, false),
        Key::Escape => (0x1B, 1, 27, false),
        Key::Up => (0x26, 72, 0, true),
        Key::Down => (0x28, 80, 0, true),
        Key::Left => (0x25, 75, 0, true),
        Key::Right => (0x27, 77, 0, true),
        Key::Home => (0x24, 71, 0, true),
        Key::End => (0x23, 79, 0, true),
        Key::PageUp => (0x21, 73, 0, true),
        Key::PageDown => (0x22, 81, 0, true),
        Key::Insert => (0x2D, 82, 0, true),
        Key::Delete => (0x2E, 83, 0, true),
        Key::F(n @ 1..=10) => (0x70 + u32::from(n) - 1, 59 + u32::from(n) - 1, 0, false),
        Key::F(11) => (0x7A, 87, 0, false),
        Key::F(12) => (0x7B, 88, 0, false),
        Key::F(n @ 13..=20) => (0x7C + u32::from(n) - 13, 0, 0, false),
        Key::F(_) | Key::Keypad(_) => return None,
    };
    let mut cs = 0u32;
    if mods.shift {
        cs |= 0x10;
    }
    if mods.ctrl {
        cs |= 0x08;
    }
    if mods.alt {
        cs |= 0x02;
    }
    if enhanced {
        cs |= ENHANCED;
    }
    Some(format!("\x1b[{vk};{sc};{uc};1;{cs};1_\x1b[{vk};{sc};{uc};0;{cs};1_").into_bytes())
}

/// Virtual-key and scan code of a character on a US layout.
fn char_vk(c: char) -> Option<(u32, u32)> {
    const ROW_Q: &str = "qwertyuiop";
    const ROW_A: &str = "asdfghjkl";
    const ROW_Z: &str = "zxcvbnm";
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        let vk = u32::from(lower.to_ascii_uppercase());
        let sc = [(ROW_Q, 16), (ROW_A, 30), (ROW_Z, 44)]
            .iter()
            .find_map(|(row, base)| row.find(lower).map(|i| base + u32::try_from(i).unwrap_or(0)))?;
        return Some((vk, sc));
    }
    Some(match c {
        '1'..='9' => (u32::from(c), 2 + u32::from(c) - u32::from('1')),
        '0' => (0x30, 11),
        ' ' => (0x20, 57),
        '-' | '_' => (0xBD, 12),
        '=' | '+' => (0xBB, 13),
        '[' | '{' => (0xDB, 26),
        ']' | '}' => (0xDD, 27),
        ';' | ':' => (0xBA, 39),
        '\'' | '"' => (0xDE, 40),
        '`' | '~' => (0xC0, 41),
        '\\' | '|' => (0xDC, 43),
        ',' | '<' => (0xBC, 51),
        '.' | '>' => (0xBE, 52),
        '/' | '?' => (0xBF, 53),
        '!' => (0x31, 2),
        '@' => (0x32, 3),
        '#' => (0x33, 4),
        '$' => (0x34, 5),
        '%' => (0x35, 6),
        '^' => (0x36, 7),
        '&' => (0x37, 8),
        '*' => (0x38, 9),
        '(' => (0x39, 10),
        ')' => (0x30, 11),
        _ => return None,
    })
}

/// A mouse button or wheel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Motion with no button held.
    None,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseAction {
    Press,
    Release,
    /// Motion, with `button` held (or [`MouseButton::None`]).
    Motion,
}

/// The report for a mouse event at a viewport cell, if the program asked for this kind of
/// event. Callers keep Shift+click for local selection.
pub fn encode_mouse(button: MouseButton, action: MouseAction, row: usize, col: usize, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    if !modes.mouse() {
        return None;
    }
    let wheel = matches!(button, MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::WheelLeft | MouseButton::WheelRight);
    match action {
        MouseAction::Motion if button == MouseButton::None && !modes.mouse_motion => return None,
        MouseAction::Motion if !(modes.mouse_motion || modes.mouse_drag) => return None,
        MouseAction::Release if wheel => return None,
        _ => {}
    }
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt || mods.meta {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    let (x, y) = (u32::try_from(col).ok()? + 1, u32::try_from(row).ok()? + 1);
    if modes.sgr_mouse {
        let fin = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{fin}").into_bytes());
    }
    if action == MouseAction::Release {
        code = 3 + (code & !3 & !32 & !64);
    }
    let mut out = b"\x1b[M".to_vec();
    for v in [code, x, y] {
        let v = v + 32;
        if modes.utf8_mouse {
            out.extend_from_slice(char::from_u32(v).filter(|_| v < 2048)?.to_string().as_bytes());
        } else {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

/// What a paste becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paste {
    pub bytes: Vec<u8>,
    /// Several lines going to a shell that did not ask for bracketed paste, so each line would
    /// run as it arrives: ask the user first ("Paste 12 lines? They will run.").
    pub confirm: bool,
    /// Lines in the paste (for that question).
    pub lines: usize,
}

/// Paste text (P339): bracketed when the program asked for it (with any ESC removed so the
/// text cannot end the bracket early); `\r\n` and `\n` become `\r`, what a PTY expects.
pub fn encode_paste(text: &str, modes: &Modes) -> Paste {
    let body = text.trim_end_matches(['\r', '\n']);
    let lines = body.split(['\n', '\r']).filter(|l| !l.is_empty()).count().max(1);
    let confirm = lines > 1 && !modes.bracketed_paste && !modes.alt_screen;
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    let bytes = if modes.bracketed_paste {
        let mut v = b"\x1b[200~".to_vec();
        v.extend(normalized.bytes().filter(|b| *b != 0x1b));
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        normalized.into_bytes()
    };
    Paste { bytes, confirm, lines }
}

/// What a clipboard chord does in a focused terminal (P339).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// Copy the selection (and clear it, for Ctrl+C).
    Copy,
    /// Paste the clipboard.
    Paste,
    /// Not a clipboard chord here: encode the key for the shell.
    Send,
}

/// Ctrl+C copies when there is a selection, otherwise it interrupts; Ctrl+Shift+C always
/// copies; Ctrl+V and Ctrl+Shift+V paste (Ctrl+V goes to the shell with `ctrl_v_to_shell`);
/// Ctrl+Insert copies and Shift+Insert pastes.
pub fn shortcut(key: Key, mods: Mods, has_selection: bool, ctrl_v_to_shell: bool) -> Shortcut {
    if mods.alt || mods.meta {
        return Shortcut::Send;
    }
    match (key, mods.ctrl, mods.shift) {
        (Key::Char('c' | 'C'), true, true) => Shortcut::Copy,
        (Key::Char('c' | 'C'), true, false) if has_selection => Shortcut::Copy,
        (Key::Char('v' | 'V'), true, true) => Shortcut::Paste,
        (Key::Char('v' | 'V'), true, false) if !ctrl_v_to_shell => Shortcut::Paste,
        (Key::Insert, true, false) if has_selection => Shortcut::Copy,
        (Key::Insert, false, true) => Shortcut::Paste,
        _ => Shortcut::Send,
    }
}

/// Focus in / out report (DECSET 1004).
pub fn encode_focus(focused: bool, modes: &Modes) -> Option<&'static [u8]> {
    modes.focus_events.then_some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}
