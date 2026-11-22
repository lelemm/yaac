//! A user message as the transcript shows it (D141): what the user typed, pasted and ran, in
//! order, with the context the harness added for the model kept apart. `yaacd` has already
//! split the harness's markup into parts with a role; this is the window's reading of them.

use yaac_protocol::{InputPart, TextRole, UserMessageItem};

/// One visible piece of a user message, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Text the user typed (markdown).
    Typed(&'a str),
    /// Text the user pasted: a quiet block, collapsed to "Pasted text · N lines".
    Pasted {
        /// Its index among the message's parts (the key of its expanded state).
        part: usize,
        text: &'a str,
        lines: usize,
        /// The paste is itself fenced code: shown as code; otherwise as plain text.
        code: bool,
    },
    /// A command the user ran ("/model opus", "!ls"): a compact chip.
    Command(&'a str),
}

/// Context the harness added: not shown in the bubble, available behind "Context".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context<'a> {
    pub label: &'a str,
    pub text: &'a str,
}

fn role(p: &InputPart) -> Option<(&str, Option<&TextRole>, Option<&str>)> {
    match p {
        InputPart::Text(t) => Some((t.text.as_str(), t.role.as_ref(), t.label.as_deref())),
        _ => None,
    }
}

/// The visible pieces, in order. A role this build does not know is shown as typed text:
/// unrecognised content is never hidden.
pub fn pieces(u: &UserMessageItem) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    for (i, p) in u.parts.iter().enumerate() {
        let Some((text, role, _)) = role(p) else { continue };
        match role {
            Some(TextRole::Context) => {}
            Some(TextRole::Pasted) => out.push(Piece::Pasted { part: i, text, lines: line_count(text), code: is_fenced(text) }),
            Some(TextRole::Command) => out.push(Piece::Command(text.trim())),
            _ if text.trim().is_empty() => {}
            _ => out.push(Piece::Typed(text)),
        }
    }
    out
}

/// The context parts, in order.
pub fn context(u: &UserMessageItem) -> Vec<Context<'_>> {
    u.parts
        .iter()
        .filter_map(role)
        .filter(|(_, r, _)| matches!(r, Some(TextRole::Context)))
        .map(|(text, _, label)| Context { label: label.unwrap_or("Context"), text })
        .collect()
}

/// Nothing to show in a bubble: only context (a reminder the harness wrote on its own).
pub fn context_only(u: &UserMessageItem) -> bool {
    pieces(u).is_empty() && !u.parts.iter().any(|p| matches!(p, InputPart::Image(_) | InputPart::File(_))) && !context(u).is_empty()
}

/// What "Copy message" copies: the typed text, the pasted text and the commands, in order,
/// without the harness's context.
pub fn copy_text(u: &UserMessageItem) -> String {
    pieces(u)
        .iter()
        .map(|p| match p {
            Piece::Typed(t) => *t,
            Piece::Pasted { text, .. } => text.trim_end_matches('\n'),
            Piece::Command(c) => *c,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Lines of a text, as a reader counts them.
pub fn line_count(text: &str) -> usize {
    let t = text.trim_end_matches('\n');
    if t.is_empty() { 0 } else { t.split('\n').count() }
}

/// "Pasted text · 12 lines".
pub fn pasted_label(lines: usize) -> String {
    format!("Pasted text · {lines} {}", if lines == 1 { "line" } else { "lines" })
}

/// "Context · 2" / "Context": the disclosure under a message with context.
pub fn context_label(n: usize) -> String {
    if n > 1 { format!("Context · {n}") } else { "Context".to_owned() }
}

/// Starts with a code fence and ends with its closing fence.
fn is_fenced(text: &str) -> bool {
    let t = text.trim();
    let open = t.lines().next().unwrap_or("");
    let fence: String = open.chars().take_while(|c| *c == '`' || *c == '~').collect();
    fence.len() >= 3 && t.lines().count() >= 2 && t.lines().last().is_some_and(|l| l.trim() == fence)
}

/// One line: line breaks, tabs and other control characters become single spaces, runs of
/// spaces one. For every single-line rendering (a notice row, an elided title): a newline in
/// one-line text would otherwise draw as a missing glyph.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_control() || c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(parts: Vec<InputPart>) -> UserMessageItem {
        UserMessageItem { parts, op_id: None }
    }

    #[test]
    fn pieces_keep_order_and_hide_context() {
        let u = msg(vec![
            InputPart::text_as("be brief", TextRole::Context, Some("System reminder")),
            InputPart::text("Look at this:"),
            InputPart::text_as("a\nb\nc\n", TextRole::Pasted, None),
            InputPart::text_as("/model opus", TextRole::Command, None),
        ]);
        assert_eq!(
            pieces(&u),
            vec![Piece::Typed("Look at this:"), Piece::Pasted { part: 2, text: "a\nb\nc\n", lines: 3, code: false }, Piece::Command("/model opus")]
        );
        assert_eq!(context(&u), vec![Context { label: "System reminder", text: "be brief" }]);
        assert!(!context_only(&u));
        assert_eq!(copy_text(&u), "Look at this:\n\na\nb\nc\n\n/model opus");
    }

    #[test]
    fn a_fenced_paste_is_code_and_an_unknown_role_is_shown() {
        let u = msg(vec![
            InputPart::text_as("```rust\nfn f() {}\n```", TextRole::Pasted, None),
            InputPart::Text(yaac_protocol::TextPart { text: "from the future".into(), role: Some(TextRole::Unknown("excerpt".into())), label: None }),
        ]);
        assert!(matches!(pieces(&u)[0], Piece::Pasted { code: true, lines: 3, .. }));
        assert_eq!(pieces(&u)[1], Piece::Typed("from the future"));
    }

    #[test]
    fn context_only_messages() {
        let u = msg(vec![InputPart::text_as("r", TextRole::Context, None)]);
        assert!(context_only(&u));
        assert_eq!(context(&u)[0].label, "Context");
        assert_eq!(copy_text(&u), "");
        assert!(!context_only(&msg(vec![InputPart::text("")])));
    }

    #[test]
    fn labels_and_lines() {
        assert_eq!(pasted_label(1), "Pasted text · 1 line");
        assert_eq!(pasted_label(12), "Pasted text · 12 lines");
        assert_eq!(context_label(1), "Context");
        assert_eq!(context_label(3), "Context · 3");
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a\n\nb"), 3);
    }

    #[test]
    fn one_line_collapses_breaks_and_controls() {
        assert_eq!(one_line("Another Claude session sent a message:\n<x>\r\n\tbody\u{2028}end  "), "Another Claude session sent a message: <x> body end");
        assert_eq!(one_line("\n\nlead"), "lead");
        assert_eq!(one_line("a\u{7}b"), "a b");
    }
}
