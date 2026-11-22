//! Queue or send now while a turn runs (D166, docs/07-ui.md#composer).
//!
//! The queue itself is `yaacd`'s: durable, in order, delivered when the running turn ends. The
//! window keeps a copy per thread ([`crate::transcript::Transcript::queued`]) from the
//! subscription's snapshot and the `queueChanged` events, and draws it as a strip above the
//! composer. Whether **Send now** steers or interrupts comes from the thread's `steer`
//! capability, never from the harness's name; the daemon makes the same choice when it acts.

use yaac_protocol::{Capability, InputPart, QueuedMessage, has_capability};

use crate::ThreadKey;
use crate::attention::{Attention, own_attention};
use crate::state::AppState;

/// "Send now" where the harness can steer: the message joins the running turn.
pub const SEND_NOW: &str = "Send now";
/// "Send now" where it cannot: the running turn is interrupted and a new one starts.
pub const SEND_NOW_INTERRUPTS: &str = "Send now — interrupts";

/// A queued message being edited in the composer: its text there, and what the text box
/// cannot show (attachments, labelled text such as a paste) kept as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEdit {
    /// The queued message.
    pub queue_id: String,
    /// Its editable text.
    pub text: String,
    /// Its other parts, sent back unchanged.
    pub keep: Vec<InputPart>,
}

impl QueueEdit {
    /// Start editing a queued message.
    pub fn of(entry: &QueuedMessage) -> Self {
        let mut text = Vec::new();
        let mut keep = Vec::new();
        for part in &entry.input {
            match part {
                InputPart::Text(t) if t.role.is_none() => text.push(t.text.clone()),
                other => keep.push(other.clone()),
            }
        }
        Self { queue_id: entry.queue_id.clone(), text: text.join("\n"), keep }
    }

    /// The message to save, or `None` when nothing would be left.
    pub fn input(&self) -> Option<Vec<InputPart>> {
        let mut parts = Vec::new();
        if !self.text.trim().is_empty() {
            parts.push(InputPart::text(self.text.clone()));
        }
        parts.extend(self.keep.iter().cloned());
        (!parts.is_empty()).then_some(parts)
    }
}

/// One row of the queue strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedView {
    /// The queued message.
    pub queue_id: String,
    /// Its first line ("2 attachments" when it has no text).
    pub preview: String,
    /// "+ 2 attachments" beside the text, when it has some.
    pub extra: Option<String>,
    /// An edit, cancel or Send now of this window is in flight: its buttons wait.
    pub busy: bool,
    /// It is open in the composer for editing.
    pub editing: bool,
}

/// A turn is running on the thread.
pub fn running(state: &AppState, key: &ThreadKey) -> bool {
    state.transcripts.get(key).is_some_and(|t| t.active_turn.is_some())
}

/// The thread waits on an approval or a question for the user: a message can only be queued
/// (answering the card is what moves the agent on).
pub fn awaiting_user(state: &AppState, key: &ThreadKey) -> bool {
    matches!(own_attention(state, key), Some(Attention::Request(_) | Attention::Unspecified))
}

fn has(state: &AppState, key: &ThreadKey, cap: &Capability) -> bool {
    state.transcripts.get(key).and_then(|t| t.effective.as_ref()).is_some_and(|e| has_capability(&e.capabilities, cap))
}

/// What the Send now button says: whether it steers or interrupts, from the `steer` capability.
pub fn now_label(steer: bool) -> &'static str {
    if steer { SEND_NOW } else { SEND_NOW_INTERRUPTS }
}

/// The Send now button of a running thread, when it is offered: not while the thread waits on
/// the user, and only where the message can reach the running turn (steered, or by stopping it).
pub fn send_now(state: &AppState, key: &ThreadKey) -> Option<&'static str> {
    if !running(state, key) || awaiting_user(state, key) {
        return None;
    }
    let steer = has(state, key, &Capability::Steer);
    (steer || has(state, key, &Capability::Interrupt)).then(|| now_label(steer))
}

/// The first line of a queued message, and its attachments counted.
pub fn preview(input: &[InputPart]) -> (String, Option<String>) {
    let text: String = input
        .iter()
        .filter_map(|p| match p {
            InputPart::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .flat_map(str::lines)
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned();
    let files = input.iter().filter(|p| matches!(p, InputPart::Image(_) | InputPart::File(_))).count();
    let count = match files {
        0 => None,
        1 => Some("1 attachment".to_owned()),
        n => Some(format!("{n} attachments")),
    };
    if text.is_empty() { (count.unwrap_or_default(), None) } else { (text, count.map(|c| format!("+ {c}"))) }
}

/// The queue strip above the composer, in delivery order; empty when nothing waits.
pub fn strip(state: &AppState, key: &ThreadKey) -> Vec<QueuedView> {
    let Some(t) = state.transcripts.get(key) else { return Vec::new() };
    let editing = state.queue_edits.get(key).map(|e| e.queue_id.as_str());
    t.queued
        .iter()
        .map(|q| {
            let (preview, extra) = preview(&q.input);
            QueuedView { queue_id: q.queue_id.clone(), preview, extra, busy: t.queue_busy.contains(&q.queue_id), editing: editing == Some(q.queue_id.as_str()) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use yaac_protocol::{FilePart, TextRole, Timestamp};

    use super::*;

    fn entry(id: &str, input: Vec<InputPart>) -> QueuedMessage {
        QueuedMessage { queue_id: id.into(), input, queued_at: Timestamp::from("2026-11-22T10:00:00Z"), model: None, effort: None, mode: None, fast: None }
    }

    fn file(name: &str) -> InputPart {
        InputPart::File(FilePart { upload_id: name.into(), name: Some(name.into()), media_type: None })
    }

    #[test]
    fn the_label_says_whether_it_steers_or_interrupts() {
        assert_eq!(now_label(true), "Send now");
        assert_eq!(now_label(false), "Send now — interrupts");
    }

    #[test]
    fn a_preview_is_the_first_line_and_the_attachments_counted() {
        assert_eq!(preview(&[InputPart::text("\n  Also run the slow tests\nand then lint")]), ("Also run the slow tests".to_owned(), None));
        assert_eq!(preview(&[InputPart::text("Look at these"), file("a.log"), file("b.log")]), ("Look at these".to_owned(), Some("+ 2 attachments".to_owned())));
        assert_eq!(preview(&[file("a.log")]), ("1 attachment".to_owned(), None));
    }

    #[test]
    fn an_edit_changes_the_text_and_keeps_what_the_text_box_cannot_show() {
        let paste = InputPart::text_with_role("big paste", TextRole::new("paste"), Some("Pasted text"));
        let e = entry("q1", vec![InputPart::text("first"), paste.clone(), file("a.log")]);
        let mut edit = QueueEdit::of(&e);
        assert_eq!(edit.text, "first");
        assert_eq!(edit.keep, vec![paste.clone(), file("a.log")]);
        edit.text = "first, but better".into();
        assert_eq!(edit.input(), Some(vec![InputPart::text("first, but better"), paste, file("a.log")]));
        let mut only_text = QueueEdit::of(&entry("q2", vec![InputPart::text("x")]));
        only_text.text = "  ".into();
        assert_eq!(only_text.input(), None, "nothing left to send");
    }
}
