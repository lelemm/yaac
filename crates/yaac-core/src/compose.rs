//! The composer's height and its Enter key (decision D89).
//!
//! Empty, the composer is two lines tall. It grows one line at a time with its text — wrapped
//! lines count as the lines they occupy — up to five; past five it stops growing and the text
//! scrolls inside it. Attachment chips and any strip above the text are not counted.

use crate::prefs::SendKey;

/// The fewest lines the composer shows.
pub const MIN_ROWS: usize = 2;
/// The most lines it grows to.
pub const MAX_ROWS: usize = 5;

/// How tall the composer is for a text that lays out in `lines` lines (wrapped lines
/// counted), and whether the text scrolls inside it.
pub fn rows(lines: usize) -> (usize, bool) {
    (lines.clamp(MIN_ROWS, MAX_ROWS), lines > MAX_ROWS)
}

/// What Enter does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterAction {
    /// Send the message; while a turn runs, queue it (D166).
    Send,
    /// Send it now: while a turn runs, steered into it or interrupting it (D166).
    SendNow,
    /// Insert a line break.
    Newline,
    /// Nothing (an input method is composing: Enter belongs to it).
    None,
}

/// Enter with these modifiers. With [`SendKey::Enter`], Enter sends (queues while a turn
/// runs), Ctrl+Enter sends now and Shift+Enter breaks the line; with [`SendKey::CtrlEnter`],
/// Ctrl+Enter sends, Ctrl+Shift+Enter sends now and Enter breaks the line (D166). Never sends
/// while an input method is composing.
pub fn enter(send_key: SendKey, shift: bool, command: bool, composing: bool) -> EnterAction {
    if composing {
        return EnterAction::None;
    }
    match send_key {
        SendKey::Enter if shift => EnterAction::Newline,
        SendKey::Enter if command => EnterAction::SendNow,
        SendKey::Enter => EnterAction::Send,
        SendKey::CtrlEnter if command && shift => EnterAction::SendNow,
        SendKey::CtrlEnter if command => EnterAction::Send,
        SendKey::CtrlEnter => EnterAction::Newline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_table() {
        for (lines, want) in [(0, (2, false)), (1, (2, false)), (2, (2, false)), (3, (3, false)), (4, (4, false)), (5, (5, false)), (6, (5, true)), (40, (5, true))] {
            assert_eq!(rows(lines), want, "{lines} lines");
        }
    }

    #[test]
    fn enter_table() {
        use EnterAction::*;
        use SendKey::*;
        let cases = [
            (Enter, false, false, false, Send),
            (Enter, true, false, false, Newline),
            (Enter, false, false, true, None),
            (Enter, true, false, true, None),
            (Enter, false, true, false, SendNow),
            (CtrlEnter, false, false, false, Newline),
            (CtrlEnter, false, true, false, Send),
            (CtrlEnter, true, true, false, SendNow),
            (CtrlEnter, true, false, false, Newline),
            (CtrlEnter, false, true, true, None),
        ];
        for (k, shift, cmd, ime, want) in cases {
            assert_eq!(enter(k, shift, cmd, ime), want, "{k:?} shift={shift} cmd={cmd} ime={ime}");
        }
    }
}
