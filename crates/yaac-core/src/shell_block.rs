//! A `!command` the user ran, as one block in the transcript (P344, P346): not a chat bubble.
//!
//! Three shapes reach a front end, and all become the same block:
//! - live (both harnesses): a user message whose only part is the command text `!cmd` (role
//!   `command`), then the user-run `toolCall` (`source: userShell`) with the output;
//! - Codex history: the user-run `toolCall` alone;
//! - Claude Code history: the `!cmd` message with context parts labelled "Shell output",
//!   "Shell error output" and "Shell exit code", in the same message or in the next one.
//!
//! [`absorbed`] names the items a `!cmd` message takes over (its toolCall, a context-only
//! message with its output), so they are not drawn again; [`block`] reads the block.

use yaac_protocol::{InputPart, Item, ItemBody, ItemId, ItemStatus, TextRole, ToolCallItem, ToolCallSource, TurnId, UserMessageItem};

use crate::transcript::Transcript;

/// About this many output lines show before "Show all N lines".
pub const FOLD_LINES: usize = 12;

/// The context labels Claude Code's history gives a `!command`'s result.
pub const OUTPUT_LABEL: &str = "Shell output";
/// See [`OUTPUT_LABEL`].
pub const ERROR_LABEL: &str = "Shell error output";
/// See [`OUTPUT_LABEL`].
pub const EXIT_LABEL: &str = "Shell exit code";

/// One `!command` and how it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandBlock {
    /// The command line, without the `!`.
    pub command: String,
    /// Where it ran, when known.
    pub cwd: Option<String>,
    /// Still running (Stop is offered).
    pub running: bool,
    /// Its exit status, once known.
    pub exit_code: Option<i32>,
    /// Its result is recorded (a toolCall, or Claude Code's output parts): not merely sent.
    pub recorded: bool,
    /// It failed: a non-zero exit, or the item failed.
    pub failed: bool,
    /// How long it ran.
    pub duration_ms: Option<u64>,
    /// What it printed (stdout, then stderr for Claude history).
    pub output: String,
    /// The output is a head of a longer one.
    pub truncated: bool,
    /// The user-run toolCall, when there is one.
    pub call: Option<ItemId>,
    /// The turn it belongs to.
    pub turn: TurnId,
}

impl CommandBlock {
    /// The finished badge: "exit 0 · 1.2s". None while running, or when nothing is known.
    pub fn badge(&self) -> Option<String> {
        if self.running {
            return None;
        }
        let code = self.exit_code.map(|c| format!("exit {c}"));
        let code = code.or_else(|| self.failed.then(|| "failed".to_owned()));
        match (code, self.duration_ms.map(duration)) {
            (Some(c), Some(d)) => Some(format!("{c} · {d}")),
            (Some(c), None) => Some(c),
            (None, Some(d)) => Some(d),
            (None, None) => None,
        }
    }

    /// The block finished well (✓) — as opposed to ✗, or no mark while running or unknown.
    pub fn ok(&self) -> Option<bool> {
        if self.running || !self.recorded {
            None
        } else {
            Some(!self.failed && self.exit_code.is_none_or(|c| c == 0))
        }
    }
}

/// "0.4s", "12.3s", "2m 05s".
pub fn duration(ms: u64) -> String {
    if ms < 60_000 {
        #[allow(clippy::cast_precision_loss, reason = "a rounded duration for people")]
        let s = ms as f64 / 1000.0;
        format!("{s:.1}s")
    } else {
        let s = ms / 1000;
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// The command of a `!cmd` user message (the first visible part is a command starting `!`).
pub fn bang_message(u: &UserMessageItem) -> Option<&str> {
    let first = u.parts.iter().find_map(|p| match p {
        InputPart::Text(t) if t.role != Some(TextRole::Context) && !t.text.trim().is_empty() => Some(t),
        _ => None,
    })?;
    if first.role != Some(TextRole::Command) {
        return None;
    }
    let c = first.text.trim().strip_prefix('!')?.trim();
    (!c.is_empty()).then_some(c)
}

/// A user-run command item.
pub fn user_shell(item: &Item) -> Option<&ToolCallItem> {
    match &item.body {
        ItemBody::ToolCall(c) if c.source == Some(ToolCallSource::UserShell) => Some(c),
        _ => None,
    }
}

fn shell_label(label: Option<&str>) -> bool {
    matches!(label, Some(OUTPUT_LABEL | ERROR_LABEL | EXIT_LABEL))
}

/// A message holding only Claude Code's record of a `!command`'s result.
fn shell_context_only(u: &UserMessageItem) -> bool {
    !u.parts.is_empty()
        && u.parts.iter().all(|p| matches!(p, InputPart::Text(t) if t.role == Some(TextRole::Context) && shell_label(t.label.as_deref())))
}

/// What a `!cmd` message at `index` takes over: its user-run toolCall (the first one after it in
/// its turn, before another message), and the next message when it is only the shell output.
fn partners(items: &[Item], index: usize) -> (Option<usize>, Option<usize>) {
    let turn = &items[index].turn_id;
    let mut call = None;
    let mut context = None;
    for (j, item) in items.iter().enumerate().skip(index + 1) {
        if &item.turn_id != turn {
            break;
        }
        match &item.body {
            ItemBody::UserMessage(u) if context.is_none() && call.is_none() && j == index + 1 && shell_context_only(u) => context = Some(j),
            ItemBody::UserMessage(_) | ItemBody::AgentMessage(_) => break,
            _ if call.is_none() && user_shell(item).is_some() => {
                call = Some(j);
                break;
            }
            _ => {}
        }
    }
    (call, context)
}

/// Which items are part of an earlier `!cmd` message's block and must not be drawn on their
/// own: `out[i]` is true for item `i`.
pub fn absorbed(items: &[Item]) -> Vec<bool> {
    let mut out = vec![false; items.len()];
    for (i, item) in items.iter().enumerate() {
        if let ItemBody::UserMessage(u) = &item.body
            && bang_message(u).is_some()
        {
            let (call, context) = partners(items, i);
            for j in [call, context].into_iter().flatten() {
                out[j] = true;
            }
        }
    }
    out
}

/// The command block an item starts: a `!cmd` user message, or a user-run command item no such
/// message took over. None for anything else.
pub fn block(t: &Transcript, index: usize) -> Option<CommandBlock> {
    of_items(t.items(), index, t.active_turn.as_ref())
}

/// [`block`] over a plain list of items, `active` being the running turn.
pub fn of_items(items: &[Item], index: usize, active: Option<&TurnId>) -> Option<CommandBlock> {
    let item = items.get(index)?;
    match &item.body {
        ItemBody::UserMessage(u) => {
            let command = bang_message(u)?.to_owned();
            let (call, context) = partners(items, index);
            if let Some(j) = call {
                let c = user_shell(&items[j]).expect("partner is a user-run call");
                return Some(from_call(&items[j], c, Some(command)));
            }
            // Claude Code history: the result as context parts, here or in the next message.
            let mut parts: Vec<(&str, &str)> = Vec::new();
            shell_parts(u, &mut parts);
            if let Some(j) = context
                && let ItemBody::UserMessage(next) = &items[j].body
            {
                shell_parts(next, &mut parts);
            }
            let mut output = String::new();
            let mut exit_code = None;
            for (label, text) in &parts {
                match *label {
                    EXIT_LABEL => exit_code = text.trim().parse::<i32>().ok(),
                    _ => {
                        let text = text.trim_end_matches('\n');
                        if text.is_empty() {
                            continue;
                        }
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(text);
                    }
                }
            }
            let recorded = !parts.is_empty();
            // Before the toolCall arrives, a `!cmd` in the running turn is running.
            let running = !recorded && active == Some(&item.turn_id);
            Some(CommandBlock {
                command,
                cwd: None,
                running,
                exit_code,
                recorded,
                failed: exit_code.is_some_and(|c| c != 0),
                duration_ms: None,
                output,
                truncated: false,
                call: None,
                turn: item.turn_id.clone(),
            })
        }
        ItemBody::ToolCall(c) if c.source == Some(ToolCallSource::UserShell) => {
            if absorbed_by_message(items, index) {
                return None;
            }
            Some(from_call(item, c, None))
        }
        _ => None,
    }
}

fn shell_parts<'a>(u: &'a UserMessageItem, out: &mut Vec<(&'a str, &'a str)>) {
    for p in &u.parts {
        if let InputPart::Text(t) = p
            && t.role == Some(TextRole::Context)
            && let Some(l) = t.label.as_deref().filter(|l| shell_label(Some(l)))
        {
            out.push((l, t.text.as_str()));
        }
    }
}

fn absorbed_by_message(items: &[Item], index: usize) -> bool {
    let turn = &items[index].turn_id;
    items[..index].iter().enumerate().rev().take_while(|(_, i)| &i.turn_id == turn).any(|(j, i)| {
        matches!(&i.body, ItemBody::UserMessage(u) if bang_message(u).is_some()) && partners(items, j).0 == Some(index)
    })
}

fn from_call(item: &Item, c: &ToolCallItem, command: Option<String>) -> CommandBlock {
    let running = item.status == ItemStatus::InProgress;
    CommandBlock {
        command: command.unwrap_or_else(|| c.title.trim().trim_start_matches('!').trim().to_owned()),
        cwd: c.cwd.as_ref().map(ToString::to_string),
        running,
        exit_code: c.exit_code,
        recorded: true,
        failed: !running && (item.status == ItemStatus::Failed || c.exit_code.is_some_and(|x| x != 0)),
        duration_ms: c.duration_ms,
        output: plain_output(&c.output),
        truncated: c.output_truncated,
        call: Some(item.id.clone()),
        turn: item.turn_id.clone(),
    }
}

/// A command's output as plain text: ANSI escapes removed, a line rewritten with `\r` (a
/// progress bar) kept as it ended, trailing newlines dropped.
pub fn plain_output(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let mut lines: Vec<&str> = Vec::new();
    for line in out.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        lines.push(line.rsplit('\r').next().unwrap_or(line));
    }
    lines.join("\n").trim_end_matches('\n').to_owned()
}

/// The output as shown: all of it when open (or short), else its first [`FOLD_LINES`] lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold<'a> {
    /// The text to draw.
    pub shown: &'a str,
    /// Lines in the whole output.
    pub total: usize,
    /// Lines not shown ("Show all N lines" when non-zero).
    pub hidden: usize,
}

/// Fold an output to about [`FOLD_LINES`] lines; one or two lines more are shown rather than
/// hidden behind the button.
pub fn fold(output: &str, open: bool) -> Fold<'_> {
    let total = if output.is_empty() { 0 } else { output.lines().count() };
    if open || total <= FOLD_LINES + 2 {
        return Fold { shown: output, total, hidden: 0 };
    }
    let end = output.match_indices('\n').nth(FOLD_LINES - 1).map_or(output.len(), |(i, _)| i);
    Fold { shown: &output[..end], total, hidden: total - FOLD_LINES }
}

/// "Show all 40 lines".
pub fn show_all_label(total: usize) -> String {
    format!("Show all {total} lines")
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_protocol::{TextPart, ToolKind};

    fn item(id: &str, turn: &str, status: ItemStatus, body: ItemBody) -> Item {
        Item { id: ItemId::new(id), turn_id: TurnId::new(turn), status, started_at: None, completed_at: None, body }
    }

    fn text(t: &str, role: Option<TextRole>, label: Option<&str>) -> InputPart {
        InputPart::Text(TextPart { text: t.into(), role, label: label.map(Into::into) })
    }

    fn user(id: &str, turn: &str, parts: Vec<InputPart>) -> Item {
        item(id, turn, ItemStatus::Completed, ItemBody::UserMessage(UserMessageItem { parts, op_id: None }))
    }

    fn call(id: &str, turn: &str, status: ItemStatus, title: &str, output: &str, exit: Option<i32>) -> Item {
        item(
            id,
            turn,
            status,
            ItemBody::ToolCall(ToolCallItem {
                tool: ToolKind::Command,
                title: title.into(),
                input: None,
                output: output.into(),
                output_truncated: false,
                output_bytes: None,
                exit_code: exit,
                cwd: None,
                agent_call: None,
                skill: None,
                images: Vec::new(),
                source: Some(ToolCallSource::UserShell),
                duration_ms: Some(1234),
            }),
        )
    }

    fn agent(id: &str, turn: &str) -> Item {
        item(id, turn, ItemStatus::Completed, ItemBody::AgentMessage(yaac_protocol::AgentMessageItem { text: "ok".into() }))
    }

    #[test]
    fn a_live_command_pairs_its_message_with_its_call() {
        let items = vec![
            user("u", "t1", vec![text("!npm test", Some(TextRole::Command), None)]),
            call("c", "t1", ItemStatus::InProgress, "npm test", "PASS\n", None),
            agent("a", "t1"),
        ];
        assert_eq!(absorbed(&items), vec![false, true, false]);
        let b = of_items(&items, 0, Some(&TurnId::new("t1"))).unwrap();
        assert_eq!(b.command, "npm test");
        assert!(b.running);
        assert_eq!(b.output, "PASS");
        assert_eq!(b.badge(), None);
        assert_eq!(b.call, Some(ItemId::new("c")));
        // The call is not a block of its own.
        assert_eq!(of_items(&items, 1, None), None);
        assert_eq!(of_items(&items, 2, None), None);
    }

    #[test]
    fn a_finished_command_shows_its_exit_and_duration() {
        let items = vec![user("u", "t1", vec![text("!false", Some(TextRole::Command), None)]), call("c", "t1", ItemStatus::Completed, "false", "", Some(1))];
        let b = of_items(&items, 0, None).unwrap();
        assert!(!b.running && b.failed);
        assert_eq!(b.ok(), Some(false));
        assert_eq!(b.badge().as_deref(), Some("exit 1 · 1.2s"));
        let items = vec![user("u", "t1", vec![text("!true", Some(TextRole::Command), None)]), call("c", "t1", ItemStatus::Completed, "true", "", Some(0))];
        assert_eq!(of_items(&items, 0, None).unwrap().ok(), Some(true));
    }

    #[test]
    fn before_its_call_arrives_a_command_in_the_running_turn_runs() {
        let items = vec![user("u", "t1", vec![text("!ls", Some(TextRole::Command), None)])];
        assert!(of_items(&items, 0, Some(&TurnId::new("t1"))).unwrap().running);
        assert!(!of_items(&items, 0, None).unwrap().running);
    }

    #[test]
    fn codex_history_gives_a_lone_call_its_own_block() {
        let items = vec![call("c", "t1", ItemStatus::Completed, "cargo test", "ok\n", Some(0)), agent("a", "t1")];
        assert_eq!(absorbed(&items), vec![false, false]);
        let b = of_items(&items, 0, None).unwrap();
        assert_eq!(b.command, "cargo test");
        assert_eq!(b.badge().as_deref(), Some("exit 0 · 1.2s"));
    }

    #[test]
    fn claude_history_reads_the_shell_context_parts() {
        // In the same message.
        let items = vec![
            user(
                "u",
                "t1",
                vec![
                    text("!git status", Some(TextRole::Command), None),
                    text("On branch main\n", Some(TextRole::Context), Some(OUTPUT_LABEL)),
                    text("", Some(TextRole::Context), Some(ERROR_LABEL)),
                ],
            ),
            agent("a", "t1"),
        ];
        let b = of_items(&items, 0, None).unwrap();
        assert_eq!(b.output, "On branch main");
        assert!(!b.running);
        // In the next message, with an exit code.
        let items = vec![
            user("u", "t1", vec![text("!false", Some(TextRole::Command), None)]),
            user("o", "t1", vec![text("boom", Some(TextRole::Context), Some(ERROR_LABEL)), text("2", Some(TextRole::Context), Some(EXIT_LABEL))]),
            agent("a", "t1"),
        ];
        assert_eq!(absorbed(&items), vec![false, true, false]);
        let b = of_items(&items, 0, Some(&TurnId::new("t1"))).unwrap();
        assert!(!b.running, "a recorded result is finished");
        assert_eq!(b.output, "boom");
        assert_eq!(b.exit_code, Some(2));
        assert_eq!(b.badge().as_deref(), Some("exit 2"));
    }

    #[test]
    fn only_a_bang_command_is_a_block() {
        let items = vec![
            user("u", "t1", vec![text("/model opus", Some(TextRole::Command), None)]),
            user("v", "t2", vec![text("!not a command", None, None)]),
        ];
        assert_eq!(of_items(&items, 0, None), None);
        assert_eq!(of_items(&items, 1, None), None);
        // A call in another turn, or after the answer, is not taken over.
        let items = vec![
            user("u", "t1", vec![text("!ls", Some(TextRole::Command), None)]),
            agent("a", "t1"),
            call("c", "t1", ItemStatus::Completed, "pwd", "", Some(0)),
            call("d", "t2", ItemStatus::Completed, "pwd", "", Some(0)),
        ];
        assert_eq!(absorbed(&items), vec![false, false, false, false]);
        assert!(of_items(&items, 2, None).is_some());
    }

    #[test]
    fn long_output_folds_to_twelve_lines() {
        let long: String = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let f = fold(&long, false);
        assert_eq!((f.total, f.hidden), (40, 28));
        assert_eq!(f.shown.lines().count(), FOLD_LINES);
        assert!(f.shown.ends_with("line 12"));
        assert_eq!(fold(&long, true).hidden, 0);
        assert_eq!(show_all_label(40), "Show all 40 lines");
        // A little over the limit is shown whole.
        let short: String = (1..=14).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(fold(&short, false).hidden, 0);
        assert_eq!(fold("", false).total, 0);
    }

    #[test]
    fn output_is_plain_text() {
        assert_eq!(plain_output("\u{1b}[1;32mok\u{1b}[0m\r\n"), "ok");
        assert_eq!(plain_output("10%\r50%\r100%\ndone\n\n"), "100%\ndone");
        assert_eq!(plain_output("\u{1b}]633;A\u{7}$ ls\n"), "$ ls");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(400), "0.4s");
        assert_eq!(duration(12_340), "12.3s");
        assert_eq!(duration(125_000), "2m 05s");
    }
}
