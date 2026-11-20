//! The hub's part of the later design additions: the window's client id in `hub/hello`,
//! `enabled` in target-state notifications, projects and the new summary fields in the cache,
//! `mesh/usage` forwarding, the undelivered-message notice, and `hub/threads/find`.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_hub::cache::{CACHE_SCHEMA, Cache};
use yaac_protocol::attach::{self, ConnectionStatus};
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{
    Address, GitStatus, MeshMessage, MeshMessageKind, MessageId, Project, ProjectId, TargetId, TargetPath, ThreadId, Timestamp,
};

fn params(m: &yaac_protocol::rpc::Message) -> Option<&Value> {
    match m {
        yaac_protocol::rpc::Message::Notification { params, .. } => Some(params),
        _ => None,
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

fn project(id: &str, name: &str) -> Project {
    Project {
        id: ProjectId::new(id),
        name: name.into(),
        folder: TargetPath::new(format!("/work/{id}")),
        repo: None,
        cross_target_key: None,
        created_at: Timestamp::new("2026-01-01T00:00:00Z"),
    }
}

#[test]
fn hello_carries_the_client_id_and_target_state_carries_enabled() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    home.fakes.add(&alpha);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    let status = c.call("hub/status", json!({})).unwrap();
    let again = hub.client();
    let hello = again.call(attach::names::HELLO, json!({ "build": support::BUILD })).unwrap();
    assert!(hello["clientId"].is_string(), "{hello}");
    assert_eq!(hello["clientId"], status["clientId"]);
    c.call(attach::names::TARGETS_ENABLE, json!({ "id": a, "enabled": false })).unwrap();
    wait_until("a disabled state notification", || {
        c.drain().iter().any(|m| {
            m.method() == Some(attach::names::TARGET_STATE)
                && params(m).is_some_and(|p| p["enabled"] == json!(false) && p["state"]["status"] == "disabled")
        })
    });
}

#[test]
fn projects_and_the_new_summary_fields_land_in_the_cache() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    alpha.set_projects(vec![project("pj_1", "Billing")]);
    alpha.add_thread("t1");
    home.fakes.add(&alpha);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    wait_until("projects cached", || home.cache().projects(&a).unwrap().len() == 1);
    let listed = c.call("hub/projects/list", json!({ "id": a })).unwrap();
    assert_eq!(listed["projects"][0]["name"], "Billing");

    alpha.set_projects(vec![project("pj_1", "Billing"), project("pj_2", "Search")]);
    alpha.originate_projects_updated();
    wait_until("project/updated cached", || home.cache().projects(&a).unwrap().len() == 2);
    wait_until("project/updated forwarded", || {
        c.drain().iter().any(|m| params(m).is_some_and(|p| p["method"] == "project/updated"))
    });

    let mut s = support::fake_daemon::summary("t1");
    s.muted = true;
    s.pinned = true;
    s.agents = yaac_protocol::AgentsPolicy::Allow;
    s.git = Some(GitStatus { branch: Some("main".into()), added: 1, modified: 2, deleted: 3, ahead: 4, behind: 5, ..GitStatus::default() });
    alpha.originate_notification_thread_updated(&s);
    wait_until("summary cached with its new fields", || {
        home.cache().threads(&a).unwrap().iter().any(|t| t.muted && t.pinned && t.agents.allows() && t.git.as_ref().is_some_and(|g| g.behind == 5))
    });
    let cached = home.cache().threads(&a).unwrap();
    assert_eq!(cached[0].native_id.as_deref(), Some("native-t1"));
}

#[test]
fn a_cache_of_the_previous_schema_is_dropped_and_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE threads (x TEXT); PRAGMA user_version = 1;").unwrap();
    }
    assert_eq!(CACHE_SCHEMA, 2);
    let mut cache = Cache::open(&path).unwrap();
    let t = TargetId::new("x");
    cache.replace_projects(&t, &[project("pj_1", "P")]).unwrap();
    assert_eq!(cache.projects(&t).unwrap()[0].name, "P");
    assert_eq!(cache.target_row_count(&t).unwrap(), 1, "projects are a target's rows");
    cache.remove_target(&t).unwrap();
    assert!(cache.projects(&t).unwrap().is_empty());
}

#[test]
fn mesh_usage_is_forwarded_to_the_root_target_and_answered_back() {
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
    let root = Address::new(&b, &ThreadId::new("orchestrator"));

    let id = alpha.originate("mesh/usage", json!({ "root": root, "source": a, "agentTurns": 3, "threadsSpawned": 1 })).unwrap();
    assert_eq!(answer(&alpha, id).unwrap()["exhausted"], false);
    let reports = beta.usage_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].agent_turns, 3);
    assert_eq!(reports[0].source, a);

    beta.set_usage_exhausted(true);
    let id = alpha.originate("mesh/usage", json!({ "root": root, "source": a, "agentTurns": 4, "threadsSpawned": 1 })).unwrap();
    assert_eq!(answer(&alpha, id).unwrap()["exhausted"], true, "the reporter is told the root is paused");

    // A target cannot report for another.
    let id = alpha.originate("mesh/usage", json!({ "root": root, "source": b, "agentTurns": 1 })).unwrap();
    assert_eq!(answer(&alpha, id).unwrap_err().code, ErrorCode::PolicyDenied);
    // The root's target offline: the reporter keeps counting and reports again later.
    beta.set_offline(true);
    c.wait_status(&b, ConnectionStatus::Offline);
    let id = alpha.originate("mesh/usage", json!({ "root": root, "source": a, "agentTurns": 5 })).unwrap();
    assert_eq!(answer(&alpha, id).unwrap_err().code, ErrorCode::TargetOffline);
}

#[test]
fn a_message_undelivered_past_the_limit_earns_its_sender_one_passive_notice() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let mut cfg = home.config();
    cfg.undelivered_notice = Duration::from_millis(300);
    cfg.notice_sweep = Duration::from_millis(50);
    let hub = home.start_with(cfg);
    let c = hub.client();
    let a = c.add_target("alpha").id;
    let b = c.add_target("beta").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    beta.set_offline(true);
    c.wait_status(&b, ConnectionStatus::Offline);
    let m = MeshMessage {
        id: MessageId::new("m-late"),
        from: Address::new(&a, &ThreadId::new("sender")),
        to: Address::new(&b, &ThreadId::new("recipient")),
        kind: MeshMessageKind::Message,
        text: "are you there?".into(),
        in_reply_to: None,
        sent_at: Timestamp::new("2026-01-01T00:00:00Z"),
        seq: 1,
        root: None,
    };
    answer(&alpha, alpha.relay(&m).unwrap()).unwrap();
    wait_until("the notice reaches the sender's target", || alpha.mailbox().iter().any(|n| n.kind == MeshMessageKind::Notice));
    std::thread::sleep(Duration::from_millis(400));
    let notices: Vec<MeshMessage> = alpha.mailbox().into_iter().filter(|n| n.kind == MeshMessageKind::Notice).collect();
    assert_eq!(notices.len(), 1, "told once");
    let n = &notices[0];
    assert_eq!(n.to, m.from, "addressed to the sender");
    assert_eq!(n.from, m.to, "about the recipient");
    assert_eq!(n.in_reply_to.as_ref(), Some(&m.id));
    assert!(n.text.contains("still queued"));
    // The message itself is not cancelled: it is delivered when the recipient returns.
    beta.set_offline(false);
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("the late message delivered", || beta.mailbox().iter().any(|x| x.id == m.id));
    // The hub's own notice owes nobody a receipt.
    wait_until("nothing left in flight", || c.call("hub/status", json!({})).unwrap()["busy"]["messagesInFlight"] == 0);
}

#[test]
fn threads_are_found_across_targets_by_id_and_by_title_in_pages() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    for i in 0..12 {
        alpha.add_thread(&format!("t_alpha{i:02}"));
    }
    beta.add_thread("t_beta01");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    let b = c.add_target("beta").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("indexes cached", || home.cache().threads(&a).unwrap().len() == 12 && home.cache().threads(&b).unwrap().len() == 1);

    let by_native = c.call("hub/threads/find", json!({ "query": "native-t_beta01" })).unwrap();
    assert_eq!(by_native["results"][0]["target"], json!(b));
    assert_eq!(by_native["results"][0]["matched"], "nativeId");
    let prefix = c.call("hub/threads/find", json!({ "query": "t_alpha0" })).unwrap();
    assert_eq!(prefix["ambiguous"], true);
    assert_eq!(prefix["total"], 10);

    let first = c.call("hub/threads/find", json!({ "query": "thread", "limit": 50 })).unwrap();
    assert_eq!(first["results"].as_array().unwrap().len(), 10);
    assert_eq!(first["total"], 13);
    let next = c.call("hub/threads/find", json!({ "query": "", "cursor": first["cursor"] })).unwrap();
    assert_eq!(next["results"].as_array().unwrap().len(), 3);
    assert!(next["cursor"].is_null());
    let expired = c.call("hub/threads/find", json!({ "query": "", "cursor": first["cursor"] })).unwrap_err();
    assert_eq!(expired.code, ErrorCode::InvalidParams);
    let only_beta = c.call("hub/threads/find", json!({ "query": "thread", "target": b })).unwrap();
    assert_eq!(only_beta["total"], 1);
}
