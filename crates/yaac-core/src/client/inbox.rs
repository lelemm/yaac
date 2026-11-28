//! Target inboxes (docs/16-external-dialogs.md, P418): the dialogs integrators raised without a
//! thread, as the hub lists them (`hub/inbox/list`, `hub/inbox/changed`), and answering one
//! (`inbox/respond`, routed to the target over its one connection). An answer is sent once with
//! an operation id; a dropped connection shows "outcome unknown" and is never re-sent
//! (invariant 10). **Check latest status** asks the target itself.

use serde_json::{Value, json};
use yaac_protocol::attach::{self, HubChannel, TargetInbox};
use yaac_protocol::methods::names as m;
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{DialogAnswer, RequestId, RequestState, TargetId};

use super::{Call, Client};
use crate::dialogs::{self, InboxAnswer};

impl Client {
    /// Ask the hub for every inbox (after hello).
    pub(super) fn load_inboxes(&mut self) {
        self.request(attach::names::INBOX_LIST, json!({}), Call::Inboxes);
    }

    pub(super) fn on_inboxes(&mut self, result: Value) {
        let Ok(list) = serde_json::from_value::<attach::InboxListResult>(result) else { return };
        let known: Vec<TargetId> = self.state.inbox.targets.keys().cloned().collect();
        for target in known {
            if !list.targets.iter().any(|t| t.target == target) {
                self.state.inbox.replace(TargetInbox { target, ..TargetInbox::default() });
            }
        }
        for inbox in list.targets {
            self.state.inbox.replace(inbox);
        }
    }

    pub(super) fn on_inbox_changed(&mut self, params: Value) {
        if let Ok(inbox) = serde_json::from_value::<TargetInbox>(params) {
            self.state.inbox.replace(inbox);
        }
    }

    /// Answer (or, with [`dialogs::cancel`], dismiss) a dialog of `target`'s inbox. The entry
    /// shows "submitted" at once and leaves Needs you; it is never re-sent.
    pub fn respond_inbox(&mut self, target: &TargetId, request_id: &RequestId, answer: DialogAnswer) {
        let Some(request) = self.state.inbox.request(target, request_id) else { return };
        if request.state != RequestState::Pending {
            return;
        }
        let Some(dialog) = dialogs::of(&request) else { return };
        if !answer.cancelled && !dialogs::answerable(dialog) {
            return;
        }
        let summary = dialogs::summary(dialog, &answer);
        self.state.inbox.answers.insert((target.clone(), request_id.clone()), InboxAnswer { state: RequestState::Submitted, summary });
        let op = self.op_id();
        let params = json!({ "opId": op, "requestId": request_id, "response": dialogs::response(answer) });
        self.routed(target, m::INBOX_RESPOND, params, Call::InboxRespond { target: target.clone(), request_id: request_id.clone() });
    }

    /// **Check latest status** of an inbox dialog whose answer may not have arrived: asks the
    /// target for its inbox. Still open there: the answer did not arrive, and the user may
    /// answer again (a new decision, not a re-send). Gone: it was decided.
    pub fn check_inbox(&mut self, target: &TargetId) {
        self.routed(target, m::INBOX_LIST, json!({}), Call::InboxCheck(target.clone()));
    }

    pub(super) fn on_inbox_check(&mut self, target: &TargetId, result: Value) {
        let Ok(list) = serde_json::from_value::<yaac_protocol::InboxListResult>(result) else { return };
        // What the target holds open now; an unknown answer to one of them did not arrive.
        let open: Vec<RequestId> = list.requests.iter().filter(|r| r.state == RequestState::Pending).map(|r| r.request_id.clone()).collect();
        self.state.inbox.answers.retain(|(t, id), a| !(t == target && a.state == RequestState::Unknown && open.contains(id)));
        let mut requests = list.requests;
        requests.retain(|r| matches!(r.state, RequestState::Pending | RequestState::Submitted | RequestState::Unknown));
        let channels = list
            .channels
            .into_iter()
            .map(|c| HubChannel { channel_id: c.channel_id, name: c.name, description: c.description, connected: true })
            .collect();
        self.state.inbox.replace(TargetInbox { target: target.clone(), requests, channels });
    }

    pub(super) fn on_inbox_respond_error(&mut self, target: &TargetId, request_id: &RequestId, error: &ErrorObject) {
        let key = (target.clone(), request_id.clone());
        match error.code {
            ErrorCode::OutcomeUnknown => {
                if let Some(a) = self.state.inbox.answers.get_mut(&key) {
                    a.state = RequestState::Unknown;
                }
            }
            // Answered elsewhere first, or withdrawn: the inbox update will say.
            ErrorCode::RequestNotPending => {}
            _ => {
                self.state.inbox.answers.remove(&key);
                self.error(None, format!("Your answer was not sent: {}", error.message));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};
    use yaac_protocol::{ClientId, RequestId, RequestState, TargetId};

    use super::super::{Call, Client};
    use crate::attention::{Attention, AttentionMap};
    use crate::dialogs;
    use crate::hints::RequestKindWord;
    use crate::key::ThreadKey;

    fn client() -> (Client, TargetId) {
        let mut client = Client::new(Box::new(crate::link::DetachedLink), "test", ClientId::new("window"));
        let target = TargetId::new("devbox");
        client.state.set_targets(serde_json::from_value(json!([
            {"id": target, "name": "devbox", "enabled": true, "transport": {"kind": "local"}, "state": {"status": "connected"}}
        ])).unwrap());
        (client, target)
    }

    fn dialog(id: &str, open: bool) -> serde_json::Value {
        json!({ "requestId": id, "state": "pending", "requestKind": "dialog", "title": "Deploy",
            "source": { "channelId": "c1", "name": "deployer" }, "channelOpen": open,
            "layout": { "type": "column", "children": [
                { "type": "textInput", "id": "token", "label": "Token", "secret": true, "required": true },
                { "type": "button", "id": "go", "label": "Deploy", "kind": "primary" } ] } })
    }

    #[allow(clippy::needless_pass_by_value, reason = "test helper")]
    fn changed(client: &mut Client, target: &TargetId, requests: Vec<serde_json::Value>) {
        client.on_notification(yaac_protocol::attach::names::INBOX_CHANGED, json!({
            "target": target, "requests": requests,
            "channels": [{ "channelId": "c1", "name": "deployer", "connected": true }] }));
    }

    fn pending_respond(client: &Client) -> Vec<yaac_protocol::rpc::RpcId> {
        client.pending.iter().filter(|(_, c)| matches!(c, Call::InboxRespond { .. })).map(|(id, _)| id.clone()).collect()
    }

    fn needs(client: &Client) -> Vec<crate::sidebar::NeedsYouEntry> {
        crate::sidebar::needs_you(&client.state, &AttentionMap::compute(&client.state), 0)
    }

    fn answer() -> yaac_protocol::DialogAnswer {
        let layout = dialogs::of(&serde_json::from_value(dialog("x", true)).unwrap()).unwrap().layout.clone();
        let mut draft = dialogs::DialogDraft::new(&layout);
        draft.set("token", dialogs::DraftValue::Text("hunter2".into()));
        dialogs::answer(&layout, &draft, "go").unwrap()
    }

    #[test]
    fn an_inbox_dialog_needs_you_under_its_target_and_is_answered_once() {
        let (mut c, target) = client();
        changed(&mut c, &target, vec![dialog("d1", true)]);
        let list = needs(&c);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].dialog, Some(RequestId::new("d1")));
        assert_eq!(list[0].target_name, "devbox");
        assert_eq!(list[0].title, "Deploy");
        assert_eq!(list[0].attention, Attention::Request(RequestKindWord::Dialog));
        assert_eq!(list[0].key.thread.as_str(), yaac_protocol::attach::INBOX_THREAD);

        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        assert_eq!(pending_respond(&c).len(), 1, "answered once, never twice");
        let shown = c.state.inbox.request(&target, &RequestId::new("d1")).unwrap();
        assert_eq!(shown.state, RequestState::Submitted);
        let summary = c.state.inbox.local_summary(&target, &RequestId::new("d1")).unwrap();
        assert!(!summary.contains("hunter2"), "the secret is never shown: {summary}");
        assert!(needs(&c).is_empty(), "a submitted answer waits on the target, not the user");

        // The target confirms by dropping it from the inbox: gone everywhere.
        let id = pending_respond(&c).remove(0);
        c.handle(Message::Response { id, result: json!({}) });
        changed(&mut c, &target, vec![]);
        assert!(c.state.inbox.request(&target, &RequestId::new("d1")).is_none());
        assert!(c.state.inbox.answers.is_empty());
        assert!(needs(&c).is_empty());
    }

    #[test]
    fn a_lost_answer_is_outcome_unknown_until_the_target_says() {
        let (mut c, target) = client();
        changed(&mut c, &target, vec![dialog("d1", true)]);
        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        let id = pending_respond(&c).remove(0);
        c.handle(Message::Error { id: Some(id), error: ErrorObject::new(ErrorCode::OutcomeUnknown, "dropped") });
        assert_eq!(c.state.inbox.request(&target, &RequestId::new("d1")).unwrap().state, RequestState::Unknown);
        assert_eq!(needs(&c).len(), 1, "the user must check it");
        // Never re-sent, and not answerable again while unknown.
        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        assert!(pending_respond(&c).is_empty());
        // Check latest status: the target still holds it open, so the answer did not arrive.
        c.check_inbox(&target);
        let check = c.pending.iter().find(|(_, call)| matches!(call, Call::InboxCheck(_))).unwrap().0.clone();
        c.handle(Message::Response { id: check, result: json!({ "requests": [dialog("d1", true)], "channels": [] }) });
        assert_eq!(c.state.inbox.request(&target, &RequestId::new("d1")).unwrap().state, RequestState::Pending);
    }

    #[test]
    fn a_disconnected_integrator_still_gets_its_answer() {
        let (mut c, target) = client();
        changed(&mut c, &target, vec![dialog("d1", false)]);
        let r = c.state.inbox.request(&target, &RequestId::new("d1")).unwrap();
        let d = dialogs::of(&r).unwrap();
        assert_eq!(dialogs::gone_line(d).as_deref(), Some("deployer is not connected · your answer will be delivered when it reconnects"));
        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        assert_eq!(pending_respond(&c).len(), 1, "recorded by yaacd, delivered when the integrator resumes");
        // Dismissing (closing it) is a cancellation, also allowed.
        changed(&mut c, &target, vec![dialog("d2", false)]);
        c.respond_inbox(&target, &RequestId::new("d2"), dialogs::cancel());
        assert_eq!(c.state.inbox.local_summary(&target, &RequestId::new("d2")), Some("Dismissed"));
    }

    #[test]
    fn a_refused_answer_goes_back_to_pending_with_the_reason() {
        let (mut c, target) = client();
        changed(&mut c, &target, vec![dialog("d1", true)]);
        c.respond_inbox(&target, &RequestId::new("d1"), answer());
        let id = pending_respond(&c).remove(0);
        c.handle(Message::Error { id: Some(id), error: ErrorObject::new(ErrorCode::InvalidParams, "Input \"token\" is required.") });
        assert_eq!(c.state.inbox.request(&target, &RequestId::new("d1")).unwrap().state, RequestState::Pending);
        assert!(c.errors.iter().any(|e| e.text.contains("token")));
    }

    #[test]
    fn a_dialog_in_a_thread_raises_the_threads_attention() {
        let (mut c, target) = client();
        let summary = json!({ "id": "t1", "title": "Ship it", "instance": "fake", "cwd": "/work", "state": "needsAttention",
            "control": "free", "originKind": "yaac", "createdAt": "2026-11-27T00:00:00Z", "updatedAt": "2026-11-27T00:00:00Z" });
        c.state.upsert_summary(&target, serde_json::from_value(summary).unwrap());
        let key = ThreadKey::new(target.clone(), "t1");
        let mut item = dialog("d1", true);
        for (k, v) in [("id", json!("i1")), ("turnId", json!("u1")), ("status", json!("inProgress")), ("kind", json!("request"))] {
            item[k] = v;
        }
        c.on_notification(yaac_protocol::attach::names::NOTIFICATION, json!({ "target": target, "method": "thread/event", "params": {
            "threadId": "t1", "epoch": 1, "seq": 1, "event": { "type": "itemStarted", "item": item } } }));
        let map = AttentionMap::compute(&c.state);
        let list = crate::sidebar::needs_you(&c.state, &map, 0);
        assert!(list.iter().any(|e| e.key == key && e.dialog.is_none()), "{list:?}");
        assert_eq!(map.of(&key), Some(Attention::Request(RequestKindWord::Dialog)), "the dialog is why it needs you");
    }
}
