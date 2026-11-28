//! External dialogs through the hub (docs/16-external-dialogs.md): a fake `yaacd` registers an
//! integrator channel and raises a dialog in its target inbox; the hub lists it to windows,
//! reminds of it like any request, routes the answer back over the target's one connection,
//! and clears it when the target reports it answered.

mod support;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{self, ConnectionStatus, InboxListResult, RequestFeed};
use yaac_protocol::rpc::Message;
use yaac_protocol::{
    Capability, DialogAnswer, DialogNode, DialogRequest, DialogSource, RequestBody, RequestId, RequestItem, RequestResponse,
    RequestState,
};

fn dialog(id: &str) -> RequestItem {
    let layout: DialogNode = serde_json::from_value(json!({ "type": "column", "children": [
        { "type": "text", "text": "Deploy to production?" },
        { "type": "textInput", "id": "token", "label": "Token", "secret": true, "required": true },
        { "type": "button", "id": "ok", "label": "Deploy", "kind": "primary" },
    ]}))
    .unwrap();
    RequestItem {
        request_id: RequestId::new(id),
        blocking: false,
        state: RequestState::Pending,
        resolution: None,
        body: RequestBody::Dialog(DialogRequest {
            title: "Deploy".into(),
            layout,
            source: DialogSource { channel_id: "ch1".into(), name: "deployer".into() },
            channel_open: true,
        }),
    }
}

fn inbox(c: &support::Client) -> InboxListResult {
    serde_json::from_value(c.call(attach::names::INBOX_LIST, json!({})).unwrap()).unwrap()
}

fn changed(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Notification { method, params } if method == attach::names::INBOX_CHANGED => Some(params.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_inbox_dialog_is_listed_reminded_answered_once_and_cleared() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    fake.set_features(vec![Capability::Channels]);
    home.fakes.add(&fake);
    let hub = home.start();
    let c = hub.client();
    let feed: RequestFeed = serde_json::from_value(c.call(attach::names::NOTIFICATIONS_SUBSCRIBE, json!({})).unwrap()).unwrap();
    assert!(feed.items.is_empty());
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);

    fake.register_channel("ch1", "deployer");
    fake.raise_inbox(dialog("d1"));
    wait_until("the inbox lists the dialog", || inbox(&c).targets.iter().any(|t| t.target == target && t.requests.len() == 1));
    let listed = inbox(&c);
    let t = listed.targets.iter().find(|t| t.target == target).unwrap();
    assert_eq!(t.channels.len(), 1);
    assert!(t.channels[0].connected);
    assert_eq!(t.channels[0].name, "deployer");

    // Windows were told, and the reminder feed counts it under "<integrator> on <target>".
    let mut seen = Vec::new();
    wait_until("the feed has the dialog", || {
        seen.extend(c.drain());
        seen.iter().any(|m| matches!(m, Message::Notification { method, params }
            if method == attach::names::NOTIFICATIONS_REQUESTS && params["items"].as_array().is_some_and(|i| i.len() == 1)))
    });
    assert!(!changed(&seen).is_empty(), "hub/inbox/changed was sent");
    let feed: RequestFeed = serde_json::from_value(c.call(attach::names::NOTIFICATIONS_SUBSCRIBE, json!({})).unwrap()).unwrap();
    let entry = &feed.items[0];
    assert!(entry.location.is_inbox());
    assert_eq!(entry.location.request_id, RequestId::new("d1"));
    assert_eq!(entry.kind, "Dialog");
    assert_eq!(entry.title, "Deploy");
    assert_eq!(entry.worker, "deployer on alpha");

    // The answer goes over the target's connection as `inbox/respond`, with its op id.
    let response = RequestResponse::Dialog(DialogAnswer {
        action: Some("ok".into()),
        values: serde_json::from_value(json!({ "token": "hunter2" })).unwrap(),
        cancelled: false,
    });
    c.routed(&target, "inbox/respond", json!({ "opId": "op-1", "requestId": "d1", "response": response })).expect("answered");
    assert_eq!(fake.inbox_answers().len(), 1);
    assert_eq!(fake.inbox_answers()[0].op_id.as_str(), "op-1");

    // A second answer is refused by the target, never re-sent by the hub.
    let again = c.routed(&target, "inbox/respond", json!({ "opId": "op-2", "requestId": "d1", "response": response }));
    assert!(again.is_err());
    assert_eq!(fake.inbox_answers().len(), 1);

    // Answered: gone from the inbox and from the reminder.
    wait_until("the inbox is empty", || inbox(&c).targets.iter().all(|t| t.requests.is_empty()));
    wait_until("the feed is empty", || {
        let feed: RequestFeed = serde_json::from_value(c.call(attach::names::NOTIFICATIONS_SUBSCRIBE, json!({})).unwrap()).unwrap();
        feed.items.is_empty()
    });

    // The integrator leaves: the registry says so.
    fake.originate_notification("channel/closed", json!({ "channelId": "ch1" }));
    wait_until("the channel is closed", || inbox(&c).targets.iter().any(|t| t.channels.iter().any(|ch| !ch.connected)));
}

#[test]
fn a_reconnect_rereads_the_inbox() {
    let home = Home::new();
    let fake = FakeDaemon::new("alpha");
    fake.set_features(vec![Capability::Channels]);
    home.fakes.add(&fake);
    fake.register_channel("ch1", "deployer");
    fake.raise_inbox(dialog("d1"));
    let hub = home.start();
    let c = hub.client();
    let target = c.add_target("alpha").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    wait_until("listed at connect", || inbox(&c).targets.iter().any(|t| t.requests.len() == 1 && t.channels.len() == 1));
    fake.kill_connections();
    fake.raise_inbox(dialog("d2"));
    wait_until("both after reconnect", || inbox(&c).targets.iter().any(|t| t.requests.len() == 2));
}
