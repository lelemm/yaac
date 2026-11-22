//! Sending while a turn runs (D166): `turn/start` with `delivery`, and the queued messages'
//! `turn/queueEdit`, `turn/queueCancel` and `turn/queuePromote`. `yaacd` holds the queue and
//! decides how *now* reaches the running turn (steered, or interrupt and start); the window
//! only asks. Every request carries an operation id, and an uncertain outcome is said, never
//! sent again (invariant 10).

use serde_json::{Value, json};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::Delivery;

use super::{Call, Client};
use crate::attachments::DraftRef;
use crate::key::ThreadKey;
use crate::queue::QueueEdit;

/// What a queue request was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QueueOp {
    /// `turn/queueEdit`, with the edited text (given back if it cannot be saved).
    Edit(String),
    /// `turn/queueCancel`.
    Cancel,
    /// `turn/queuePromote` ("Send now" on a queued message).
    Promote,
}

/// The error code `yaacd` answers for a queued message that is gone (delivered or cancelled).
const ENTRY_GONE: &str = "queue_entry_not_found";

impl Client {
    /// Send the composer's content. With no turn running it starts one; while one runs it is
    /// **queued** (`delivery: queue`) and delivered when the turn ends. With a queued message
    /// open for editing, this saves the edit instead. Returns whether anything was sent.
    pub fn send(&mut self, key: &ThreadKey) -> bool {
        if self.state.queue_edits.contains_key(key) {
            return self.save_queue_edit(key);
        }
        self.send_with(key, &Delivery::Queue)
    }

    /// **Send now** (Ctrl+Enter): with a turn running, the message reaches it at once —
    /// steered where the thread can steer, otherwise the turn is interrupted and a new one
    /// starts with it; `yaacd` chooses by the same capability. While the thread waits on an
    /// approval or a question it is queued instead (answering is what moves the agent on).
    pub fn send_now(&mut self, key: &ThreadKey) -> bool {
        if self.state.queue_edits.contains_key(key) {
            return self.save_queue_edit(key);
        }
        let delivery = if crate::queue::awaiting_user(&self.state, key) { Delivery::Queue } else { Delivery::Now };
        self.send_with(key, &delivery)
    }

    fn send_with(&mut self, key: &ThreadKey, delivery: &Delivery) -> bool {
        if !self.state.may_act(key) {
            return false;
        }
        let Some(draft) = self.state.drafts.get(key).cloned() else { return false };
        let support = crate::attachments::thread_image_support(&self.state, key);
        let Some(input) = crate::attachments::input_parts(&draft.text, &draft.attachments, &support, Some(&key.target)) else { return false };
        let op = self.op_id();
        let next = self.state.next_settings.get(key).cloned().unwrap_or_default();
        self.state.drafts.remove(key);
        // `delivery` always goes: should a turn start just before this arrives (a queued
        // message delivered), the message waits or goes in as asked instead of being refused.
        let parts = input.clone();
        let mut params = json!({ "opId": op, "threadId": key.thread, "input": input, "delivery": delivery });
        if let Some(mdl) = next.model {
            params["model"] = json!(mdl);
        }
        if let Some(e) = next.effort {
            params["effort"] = json!(e);
        }
        if let Some(md) = next.mode {
            params["mode"] = json!(md);
        }
        if let Some(f) = next.fast {
            params["fast"] = json!(f);
        }
        if let Some(n) = self.state.next_settings.get_mut(key) {
            n.fast_note = None;
        }
        // The message shows at once, "Sending…" (D165); queued, it moves to the strip instead.
        self.echo(key, &op, parts, &draft, m::TURN_START, &params);
        self.routed(&key.target, m::TURN_START, params, Call::TurnStart { key: key.clone(), draft, op });
        true
    }

    /// Open a queued message in the composer for editing.
    pub fn begin_queue_edit(&mut self, key: &ThreadKey, queue_id: &str) {
        let Some(entry) = self.state.transcripts.get(key).and_then(|t| t.queued.iter().find(|q| q.queue_id == queue_id)) else { return };
        let edit = QueueEdit::of(entry);
        self.state.queue_edits.insert(key.clone(), edit);
    }

    /// Leave the edit without saving: the queued message stays as it was.
    pub fn end_queue_edit(&mut self, key: &ThreadKey) {
        self.state.queue_edits.remove(key);
    }

    /// Save the edit (`turn/queueEdit`). An edit that leaves nothing is not sent.
    pub fn save_queue_edit(&mut self, key: &ThreadKey) -> bool {
        if !self.state.may_act(key) {
            return false;
        }
        let Some(edit) = self.state.queue_edits.get(key).cloned() else { return false };
        let Some(input) = edit.input() else { return false };
        self.state.queue_edits.remove(key);
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "queueId": edit.queue_id, "input": input });
        self.queue_request(key, &edit.queue_id, m::TURN_QUEUE_EDIT, params, QueueOp::Edit(edit.text));
        true
    }

    /// Cancel a queued message (`turn/queueCancel`): it leaves the strip when `yaacd` says so.
    pub fn queue_cancel(&mut self, key: &ThreadKey, queue_id: &str) {
        if !self.state.may_act(key) {
            return;
        }
        if self.state.queue_edits.get(key).is_some_and(|e| e.queue_id == queue_id) {
            self.state.queue_edits.remove(key);
        }
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "queueId": queue_id });
        self.queue_request(key, queue_id, m::TURN_QUEUE_CANCEL, params, QueueOp::Cancel);
    }

    /// **Send now** on a queued message (`turn/queuePromote`): out of the queue, with the same
    /// mechanics as Ctrl+Enter. Not offered while the thread waits on the user.
    pub fn queue_promote(&mut self, key: &ThreadKey, queue_id: &str) {
        if !self.state.may_act(key) || crate::queue::awaiting_user(&self.state, key) {
            return;
        }
        let op = self.op_id();
        let params = json!({ "opId": op, "threadId": key.thread, "queueId": queue_id });
        self.queue_request(key, queue_id, m::TURN_QUEUE_PROMOTE, params, QueueOp::Promote);
    }

    fn queue_request(&mut self, key: &ThreadKey, queue_id: &str, method: &str, params: Value, op: QueueOp) {
        if let Some(t) = self.state.transcripts.get_mut(key) {
            // One request per queued message at a time: its buttons wait for the answer.
            if !t.queue_busy.insert(queue_id.to_owned()) {
                return;
            }
            t.revision += 1;
        }
        self.routed(&key.target, method, params, Call::Queue { key: key.clone(), queue_id: queue_id.to_owned(), op });
    }

    pub(super) fn on_queue_result(&mut self, key: &ThreadKey, queue_id: &str) {
        if let Some(t) = self.state.transcripts.get_mut(key) {
            t.queue_busy.remove(queue_id);
            t.revision += 1;
        }
    }

    pub(super) fn on_queue_error(&mut self, key: ThreadKey, queue_id: &str, op: QueueOp, error: &ErrorObject) {
        self.on_queue_result(&key, queue_id);
        let what = match op {
            QueueOp::Edit(_) => "saving your edit to the queued message",
            QueueOp::Cancel => "cancelling the queued message",
            QueueOp::Promote => "sending the queued message now",
        };
        if error.code == ErrorCode::OutcomeUnknown {
            self.error(Some(key), format!("YAAC lost contact while {what}. It may or may not have happened; it will not be asked again."));
            return;
        }
        let gone = error.code.as_str() == ENTRY_GONE;
        let text = match op {
            QueueOp::Edit(text) => {
                // The edited words are not lost: back in the message box when it is empty.
                let slot = self.state.drafts.entry(key.clone()).or_default();
                let back = slot.is_empty();
                if back {
                    slot.text = text;
                    self.dirty.insert(DraftRef::Thread(key.clone()));
                }
                let lead = if gone { "The queued message was delivered before your edit was saved.".to_owned() } else { format!("Your edit was not saved: {}", error.message) };
                if back { format!("{lead} Your edited text is back in the message box.") } else { lead }
            }
            QueueOp::Cancel if gone => "That queued message was already delivered.".to_owned(),
            QueueOp::Cancel => format!("Not cancelled: {}", error.message),
            QueueOp::Promote if gone => "That queued message was already delivered.".to_owned(),
            QueueOp::Promote => format!("Not sent now: {}", error.message),
        };
        self.error(Some(key), text);
    }
}
