//! The mesh relay: store before acknowledging, deduplicate by message id, hold while the
//! destination is offline (across a hub restart), deliver exactly once, hand the receipt
//! back, keep rosters current, and route `mesh/query`.

mod support;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::rpc::ErrorObject;
use yaac_protocol::{Address, MeshMessage, MeshMessageKind, MessageId, ReceiptStatus, TargetId, ThreadId, Timestamp};

fn message(id: &str, from: &TargetId, to: &TargetId) -> MeshMessage {
    MeshMessage {
        id: MessageId::new(id),
        from: Address::new(from, &ThreadId::new("sender")),
        to: Address::new(to, &ThreadId::new("recipient")),
        kind: MeshMessageKind::Message,
        text: "please review the decoder".into(),
        in_reply_to: None,
        sent_at: Timestamp::new("2026-01-01T00:00:00Z"),
        seq: 1,
        root: None,
        delivery: None,
    }
}

fn answer(fake: &FakeDaemon, id: i64) -> Result<Value, ErrorObject> {
    let mut out = None;
    wait_until("the hub's answer", || {
        out = fake.peer_answer(id);
        out.is_some()
    });
    out.unwrap()
}

fn receipt_status(v: &Value) -> String {
    v["receipt"]["status"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn a_relayed_message_survives_the_destination_being_offline_and_is_delivered_exactly_once() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let (a, b) = {
        let c = hub.client();
        let a = c.add_target("alpha").id;
        let b = c.add_target("beta").id;
        c.wait_status(&a, ConnectionStatus::Connected);
        c.wait_status(&b, ConnectionStatus::Connected);
        beta.set_offline(true);
        c.wait_status(&b, ConnectionStatus::Offline);
        (a, b)
    };
    let m = message("m1", &a, &b);

    // Stored before it is acknowledged; queued because the destination is offline.
    let first = answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    assert_eq!(receipt_status(&first), "queued");
    assert_eq!(first["receipt"]["reason"], "targetOffline");
    // The source's outbox retries until acknowledged: a duplicate is harmless.
    let dup = answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    assert_eq!(receipt_status(&dup), "queued");

    // Once acknowledged, the relay queue is the only copy: it survives the hub restarting.
    hub.shutdown();
    let relay = yaac_hub::relay::RelayQueue::open(&home.paths().relay_file()).unwrap();
    assert_eq!(relay.state(&m.id).unwrap(), Some(("queued".to_owned(), false)));
    drop(relay);
    let hub = home.start();
    let c = hub.client();
    c.wait_status(&a, ConnectionStatus::Connected);
    let again = answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    assert_eq!(receipt_status(&again), "queued");
    assert!(beta.deliver_calls().is_empty());

    // The destination comes back: delivered once, and the source gets the receipt.
    beta.set_offline(false);
    c.call(yaac_hub::protocol::names::TARGETS_RECONNECT, json!({ "id": b })).unwrap();
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("delivered", || beta.mailbox().len() == 1);
    wait_until("receipt handed back", || alpha.receipts().iter().any(|r| r.message_id == m.id));
    let receipt = alpha.receipts().into_iter().find(|r| r.message_id == m.id).unwrap();
    assert_eq!(receipt.receipt.status, ReceiptStatus::Delivered);
    assert_eq!(beta.mailbox()[0].text, m.text);

    // A late duplicate after delivery is answered from the queue and not delivered again.
    let late = answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    assert_eq!(receipt_status(&late), "delivered");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(beta.deliver_calls().len(), 1, "exactly one mesh/deliver");
    wait_until("nothing in flight", || {
        c.call(yaac_hub::protocol::names::STATUS, json!({})).unwrap()["busy"]["messagesInFlight"] == 0
    });
}

#[test]
fn a_message_to_an_online_destination_is_delivered_at_once_and_failures_are_receipted() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    let b = c.add_target("beta").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);

    let m = message("m2", &a, &b);
    let r = answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    assert_eq!(receipt_status(&r), "queued");
    assert!(r["receipt"].get("reason").is_none(), "the destination is online");
    wait_until("delivered", || beta.mailbox().iter().any(|x| x.id == m.id));
    wait_until("receipt", || alpha.receipts().iter().any(|x| x.message_id == m.id));

    // An unknown destination fails at once, in the answer itself.
    let unknown = message("m3", &a, &TargetId::new("tnowhere"));
    let r = answer(&alpha, alpha.relay(&unknown).unwrap()).unwrap();
    assert_eq!(receipt_status(&r), "failed");
    assert_eq!(r["receipt"]["reason"], "unknownAddress");

    // A sender address on another target is refused: the hub only relays a target's own.
    let forged = message("m4", &b, &a);
    let e = answer(&alpha, alpha.relay(&forged).unwrap()).unwrap_err();
    assert_eq!(e.code.as_str(), "policy_denied");

    // A transient delivery failure keeps the message queued; it is delivered on reconnect.
    beta.set_deliver_fails(true);
    let m5 = message("m5", &a, &b);
    answer(&alpha, alpha.relay(&m5).unwrap()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!beta.mailbox().iter().any(|x| x.id == m5.id));
    beta.set_deliver_fails(false);
    beta.kill_connections();
    wait_until("delivered after reconnect", || beta.mailbox().iter().any(|x| x.id == m5.id));
}

#[test]
fn rosters_are_kept_current_and_queries_are_routed() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    wait_until("first roster", || alpha.rosters().last().is_some_and(|r| r.self_id == a && r.targets.len() == 1));
    let b = c.add_target("beta").id;
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("roster lists beta reachable", || {
        alpha.rosters().last().is_some_and(|r| r.targets.iter().any(|t| t.id == b && t.reachable))
    });
    assert!(beta.rosters().last().is_some_and(|r| r.self_id == b && r.self_name.as_str() == "beta"));

    // mesh/query from alpha, answered by beta.
    let caller = Address::new(&a, &ThreadId::new("sender"));
    let q = json!({ "caller": caller, "target": b, "query": { "tool": "listThreads" } });
    let r = answer(&alpha, alpha.originate("mesh/query", q.clone()).unwrap()).unwrap();
    assert_eq!(r["tool"], "listThreads");
    assert_eq!(beta.queries(), 1);

    // beta goes offline: the roster says so, and queries fail clearly.
    beta.set_offline(true);
    c.wait_status(&b, ConnectionStatus::Offline);
    wait_until("roster marks beta unreachable", || {
        alpha.rosters().last().is_some_and(|r| r.targets.iter().any(|t| t.id == b && !t.reachable))
    });
    let e = answer(&alpha, alpha.originate("mesh/query", q).unwrap()).unwrap_err();
    assert_eq!(e.code.as_str(), "target_offline");
}
