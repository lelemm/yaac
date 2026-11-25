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

/// A file search token at a UTF-8 caret offset. File search stays separate from slash commands.
pub fn file_token(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let before = text.get(..caret)?;
    let after = text.get(caret..)?;
    let start = before.rfind(char::is_whitespace).map_or(0, |i| {
        i + before[i..].chars().next().map_or(0, char::len_utf8)
    });
    let query = before[start..].strip_prefix('@')?;
    if query.starts_with('"') {
        return None;
    }
    let end = caret + after.find(char::is_whitespace).unwrap_or(after.len());
    Some((start..end, query))
}

/// The absolute target path as a Markdown code span. Paths with spaces and backticks remain
/// intact, and the transcript displays exactly the path the agent can open.
pub fn file_reference(path: &str) -> String {
    // A code span normalizes line endings. Preserve unusual filenames as a JSON string.
    let escaped = path.contains(['\r', '\n']).then(|| serde_json::Value::String(path.to_owned()).to_string());
    let path = escaped.as_deref().unwrap_or(path);
    let mut longest = 0;
    let mut run = 0;
    for c in path.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let ticks = "`".repeat(longest + 1);
    if longest == 0 {
        format!("{ticks}{path}{ticks}")
    } else {
        format!("{ticks} {path} {ticks}")
    }
}

/// Whether the composer contains the manual compaction command.
pub fn is_compact_command(text: &str) -> bool {
    text.trim() == "/compact"
}

/// Whether a draft is in `!` mode (P344, P346): its text starts with `!`. The composer shows the
/// mode and hides the `!`; removing it (Backspace at the start of the field) leaves the mode.
pub fn is_shell_draft(text: &str) -> bool {
    text.starts_with('!')
}

/// The command a `!command` draft runs: the text after the leading `!`, trimmed. None when the
/// draft is not in `!` mode or has nothing to run.
pub fn shell_command(text: &str) -> Option<&str> {
    let command = text.strip_prefix('!')?.trim();
    (!command.is_empty()).then_some(command)
}

/// The field text a `!`-mode composer shows: the draft without its leading `!`.
pub fn shell_field(text: &str) -> &str {
    text.strip_prefix('!').unwrap_or(text)
}

/// What a front end typed into a composer field becomes in the draft (P346). `shell`: the
/// composer is in `!` mode, so `field` is the text after the absorbed `!`. A `!` typed at the
/// very start of a normal field enters the mode; the field then shows the rest.
pub fn draft_from_field(field: &str, shell: bool) -> String {
    if shell { format!("!{field}") } else { field.to_owned() }
}

/// This thread's earlier `!` commands, oldest first, for ↑ in the composer (P346): the
/// `!command` user messages, the user-run command items a harness keeps (Codex history), and
/// the queued ones. A command repeated back to back is listed once.
pub fn shell_history(state: &crate::state::AppState, key: &crate::key::ThreadKey) -> Vec<String> {
    use yaac_protocol::{InputPart, ItemBody, TextRole, ToolCallSource};
    let mut out: Vec<String> = Vec::new();
    let Some(t) = state.transcripts.get(key) else { return out };
    fn push(out: &mut Vec<String>, c: &str) {
        let c = c.trim();
        if !c.is_empty() && out.last().is_none_or(|l| l != c) {
            out.push(c.to_owned());
        }
    }
    for item in t.items() {
        match &item.body {
            ItemBody::UserMessage(m) => {
                if let Some(InputPart::Text(p)) = m.parts.first()
                    && p.role == Some(TextRole::Command)
                    && let Some(c) = p.text.strip_prefix('!')
                {
                    push(&mut out, c);
                }
            }
            ItemBody::ToolCall(c) if c.source == Some(ToolCallSource::UserShell) => {
                // Live, the `!command` message just before it already named it.
                push(&mut out, &c.title);
            }
            _ => {}
        }
    }
    for q in &t.queued {
        if let Some(c) = &q.shell_command {
            push(&mut out, c);
        }
    }
    out
}

/// Leave `!` mode, keeping the text (Backspace at the start of the field, the prompt row's ×,
/// or select-all and delete of the `!`).
pub fn leave_shell_mode(text: &str) -> String {
    shell_field(text).to_owned()
}

/// A deleting key in a `!`-mode field (P346).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    /// Backspace.
    Backspace,
    /// Delete (forward).
    Delete,
}

/// What a deleting key does to a `!`-mode field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKey {
    /// An ordinary edit: the field handles it.
    Pass,
    /// Leave the mode and keep the text (Backspace at the very start): the key is taken; the
    /// draft becomes [`leave_shell_mode`] of it.
    LeaveKeep,
    /// The whole field was selected and deleted: leave the mode with an empty draft.
    LeaveClear,
}

/// How a deleting key acts in a `!`-mode field, from the selection (`sel_start..sel_end`, in
/// characters, either order) and the field's length in characters. Backspace with the caret at
/// the very start — with or without text after it — removes the bang; deleting a selection of
/// the whole field removes the text and the bang.
pub fn shell_key(key: FieldKey, sel_start: usize, sel_end: usize, field_chars: usize) -> ShellKey {
    let (a, b) = (sel_start.min(sel_end), sel_start.max(sel_end));
    if a == 0 && b >= field_chars && field_chars > 0 {
        return ShellKey::LeaveClear;
    }
    if key == FieldKey::Backspace && a == 0 && b == 0 {
        return ShellKey::LeaveKeep;
    }
    ShellKey::Pass
}

/// The number of submitted message drafts the window remembers across conversations.
pub const MESSAGE_HISTORY_LIMIT: usize = 10;

/// ↑ and ↓ through a thread's earlier `!` commands in the composer (P346), newest first. The
/// text typed before the first ↑ comes back when ↓ goes past the newest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellHistory {
    /// How far back: 0 is the newest. None: not walking.
    pos: Option<usize>,
    /// The field before walking began.
    stash: String,
}

impl ShellHistory {
    /// One older command, or None at the oldest (or with no history). `history`: oldest first,
    /// as [`shell_history`] gives it; `current`: the field now.
    pub fn up(&mut self, history: &[String], current: &str) -> Option<String> {
        if history.is_empty() {
            return None;
        }
        let next = match self.pos {
            None => {
                self.stash = current.to_owned();
                0
            }
            Some(p) if p + 1 < history.len() => p + 1,
            Some(_) => return None,
        };
        self.pos = Some(next);
        Some(history[history.len() - 1 - next].clone())
    }

    /// One newer command; past the newest, the text typed before walking. None when not walking.
    pub fn down(&mut self, history: &[String]) -> Option<String> {
        match self.pos? {
            0 => {
                self.pos = None;
                Some(std::mem::take(&mut self.stash))
            }
            p => {
                self.pos = Some(p - 1);
                history.len().checked_sub(p).and_then(|i| history.get(i)).cloned()
            }
        }
    }

    /// Walking through the history.
    pub fn walking(&self) -> bool {
        self.pos.is_some()
    }

    /// Stop walking (the user edited the text, or sent it).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_bang_is_a_shell_command_and_nothing_else_is() {
        assert_eq!(shell_command("!npm test"), Some("npm test"));
        assert_eq!(shell_command("!  ls -la | head  \n"), Some("ls -la | head"));
        assert_eq!(shell_command("! printf '%s' \"a b\""), Some("printf '%s' \"a b\""));
        assert_eq!(shell_command("!"), None);
        assert_eq!(shell_command("!   "), None);
        assert_eq!(shell_command(" !ls"), None, "only a bang at the very start");
        assert_eq!(shell_command("run !ls"), None);
        assert!(is_shell_draft("!"));
        assert!(!is_shell_draft("ls"));
    }

    #[test]
    fn the_field_hides_the_bang_and_leaving_the_mode_keeps_the_text() {
        assert_eq!(shell_field("!npm test"), "npm test");
        assert_eq!(draft_from_field("npm test", true), "!npm test");
        assert_eq!(draft_from_field("npm test", false), "npm test");
        assert_eq!(leave_shell_mode("!npm test"), "npm test");
        assert_eq!(leave_shell_mode("!"), "");
        assert!(!is_shell_draft(&leave_shell_mode("!npm test")));
        // Typing `!` at the start again re-enters the mode.
        assert!(is_shell_draft(&draft_from_field("!npm test", false)));
    }

    #[test]
    fn backspace_at_the_start_or_deleting_everything_leaves_the_mode() {
        use FieldKey::{Backspace, Delete};
        // Caret at the start, with text after it, or in an empty field: the bang goes, the text stays.
        assert_eq!(shell_key(Backspace, 0, 0, 8), ShellKey::LeaveKeep);
        assert_eq!(shell_key(Backspace, 0, 0, 0), ShellKey::LeaveKeep);
        // Anywhere else Backspace edits the text.
        assert_eq!(shell_key(Backspace, 3, 3, 8), ShellKey::Pass);
        assert_eq!(shell_key(Delete, 0, 0, 8), ShellKey::Pass);
        assert_eq!(shell_key(Backspace, 0, 3, 8), ShellKey::Pass, "a part of the text selected");
        // Select all + delete, either key, either direction.
        assert_eq!(shell_key(Backspace, 0, 8, 8), ShellKey::LeaveClear);
        assert_eq!(shell_key(Delete, 8, 0, 8), ShellKey::LeaveClear);
        assert_eq!(leave_shell_mode("!npm test"), "npm test");
    }

    #[test]
    fn up_walks_back_through_earlier_commands_and_down_returns() {
        let h: Vec<String> = ["ls", "npm test", "git status"].iter().map(|s| (*s).to_owned()).collect();
        let mut w = ShellHistory::default();
        assert_eq!(w.down(&h), None, "not walking");
        assert_eq!(w.up(&h, "typed").as_deref(), Some("git status"));
        assert!(w.walking());
        assert_eq!(w.up(&h, "git status").as_deref(), Some("npm test"));
        assert_eq!(w.up(&h, "npm test").as_deref(), Some("ls"));
        assert_eq!(w.up(&h, "ls"), None, "at the oldest");
        assert_eq!(w.down(&h).as_deref(), Some("npm test"));
        assert_eq!(w.down(&h).as_deref(), Some("git status"));
        assert_eq!(w.down(&h).as_deref(), Some("typed"));
        assert!(!w.walking());
        assert_eq!(ShellHistory::default().up(&[], "x"), None);
    }

    #[test]
    fn file_tokens_and_references_preserve_surrounding_text() {
        let text = "éarlier @src/main later";
        let caret = text.find("/main").unwrap();
        let (range, query) = file_token(text, caret).unwrap();
        assert_eq!(query, "src");
        assert_eq!(&text[range], "@src/main");
        assert!(file_token("user@example.org", 16).is_none());
        assert!(file_token("é", 1).is_none());
        assert_eq!(file_reference(r"C:\work\design notes.md"), r"`C:\work\design notes.md`");
        assert_eq!(file_reference("/tmp/a`b"), "`` /tmp/a`b ``");
    }

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
