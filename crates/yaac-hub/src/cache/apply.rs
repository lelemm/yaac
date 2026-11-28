//! Applying journal events to cached items — pure functions over the protocol types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use yaac_protocol::{
    ContextUsage, Control, DeltaField, Item, ItemBody, QueueChange, QueueChangedEvent, QueuedMessage, QuestionAnswer, RequestBody,
    RequestResponse, ThreadEffective, ThreadHold, ThreadSubscribeResult, TokenUsage, TurnId, TurnStatus,
};

/// Thread-level state the events carry besides items: control, the running turn, effective
/// capabilities, usage. Stored with the thread's position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    /// Control, relative to this hub (the target's one client on this host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<Control>,
    /// Where it is in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_holder: Option<String>,
    /// The running turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<TurnId>,
    /// Effective capabilities and applied settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<ThreadEffective>,
    /// Cumulative token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    /// Context window usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
    /// The last turn that ended, and how.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<(TurnId, TurnStatus)>,
    /// User messages queued behind the running turn, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<QueuedMessage>,
    /// Set while the thread is stopped and held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHold>,
}

/// Applies a `queueChanged` event to a queue.
pub fn apply_queue(queue: &mut Vec<QueuedMessage>, e: &QueueChangedEvent) {
    let i = queue.iter().position(|q| q.queue_id == e.entry.queue_id);
    match (&e.change, i) {
        (QueueChange::Added, None) => queue.push(e.entry.clone()),
        (QueueChange::Added | QueueChange::Updated, Some(i)) => queue[i] = e.entry.clone(),
        (QueueChange::Removed | QueueChange::Delivered, Some(i)) => {
            queue.remove(i);
        }
        _ => {}
    }
}

impl LiveState {
    /// The state a subscription result describes.
    pub fn from_subscribed(subscribed: &ThreadSubscribeResult) -> Self {
        Self {
            control: Some(subscribed.control.clone()),
            control_holder: subscribed.control_holder.clone(),
            active_turn: subscribed.active_turn.clone(),
            effective: subscribed.effective.clone(),
            queued: subscribed.queued.clone().unwrap_or_default(),
            hold: subscribed.hold.clone(),
            ..Self::default()
        }
    }
}

/// Removes answers to `secret` questions from a request item. They are never written to the
/// host.
pub fn redact_secrets(item: &mut Item) {
    let ItemBody::Request(request) = &mut item.body else { return };
    if let RequestBody::Dialog(dialog) = &request.body {
        // A dialog's secret inputs (P418): their values are dropped, not masked.
        if let Some(resolution) = request.resolution.as_mut()
            && let Some(RequestResponse::Dialog(answer)) = resolution.answer.as_mut()
        {
            yaac_protocol::dialog_tree::redact(&dialog.layout, &mut answer.values);
        }
        return;
    }
    let RequestBody::Question(question) = &request.body else { return };
    let secret: BTreeSet<_> = question.questions.iter().filter(|q| q.secret).map(|q| q.id.clone()).collect();
    if secret.is_empty() {
        return;
    }
    if let Some(resolution) = request.resolution.as_mut()
        && let Some(RequestResponse::Question(answers)) = resolution.answer.as_mut()
    {
        for (id, answer) in &mut answers.answers {
            if secret.contains(id) {
                *answer = QuestionAnswer { selected: Vec::new(), other: None, redacted: true };
            }
        }
    }
}

/// Appends a delta to the field it names. Returns whether the item changed.
pub fn apply_delta(item: &mut Item, field: &DeltaField, delta: &str) -> bool {
    match (&mut item.body, field) {
        (ItemBody::AgentMessage(m), DeltaField::Text) => m.text.push_str(delta),
        (ItemBody::Reasoning(r), DeltaField::Reasoning) => r.reasoning.push_str(delta),
        (ItemBody::ToolCall(t), DeltaField::Output) => t.output.push_str(delta),
        _ => return false,
    }
    true
}
