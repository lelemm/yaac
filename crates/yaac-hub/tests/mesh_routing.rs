//! The hub's own part of agents' mesh queries (docs/10-agent-mesh.md#listing-is-scoped-and-bounded):
//! a caller must belong to the target its query came from; `findThreads` and `listTargets` are
//! answered by the hub from its index, with no fan-out to the daemons; other queries are routed.

mod support;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{Address, ThreadId};

fn answer(fake: &FakeDaemon, id: i64) -> Result<Value, ErrorObject> {
    let mut out = None;
    wait_until("the hub's answer", || {
        out = fake.peer_answer(id);
        out.is_some()
    });
    out.unwrap()
}

#[test]
fn mesh_queries_are_checked_and_fleet_questions_are_answered_from_the_index() {
    let home = Home::new();
    let alpha = FakeDaemon::new("alpha");
    let beta = FakeDaemon::new("beta");
    beta.add_thread("t_ci_integration_0001");
    beta.add_thread("t_other_0002");
    home.fakes.add(&alpha);
    home.fakes.add(&beta);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("alpha").id;
    let b = c.add_target("beta").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("beta's index", || home.cache().threads(&b).map(|t| t.len() == 2).unwrap_or(false));
    let caller = Address::new(&a, &ThreadId::new("t_caller"));

    // A caller that is not on the reporting target is refused.
    let forged = Address::new(&b, &ThreadId::new("t_other_0002"));
    let id = alpha.originate("mesh/query", json!({ "caller": forged, "target": b, "query": { "tool": "listModels" } })).unwrap();
    assert_eq!(answer(&alpha, id).unwrap_err().code, ErrorCode::PolicyDenied);

    // find by id prefix, across targets, from the hub's own index.
    let id = alpha.originate("mesh/query", json!({ "caller": caller, "target": a, "query": { "tool": "findThreads", "query": "t_ci_int" } })).unwrap();
    let r = answer(&alpha, id).unwrap();
    assert_eq!(r["tool"], "findThreads");
    assert_eq!(r["total"], 1);
    assert_eq!(r["results"][0]["thread"]["peer"]["address"], Address::new(&b, &ThreadId::new("t_ci_integration_0001")).as_str());
    assert_eq!(r["results"][0]["thread"]["peer"]["targetName"], "beta");
    assert_eq!(r["results"][0]["matched"], "id");
    // find by fuzzy title ("thread t_ci_integration_0001" is the fake's title).
    let id = alpha.originate("mesh/query", json!({ "caller": caller, "target": a, "query": { "tool": "findThreads", "query": "thred other" } })).unwrap();
    let r = answer(&alpha, id).unwrap();
    assert_eq!(r["total"], 1, "{r}");
    assert_eq!(r["results"][0]["matched"], "title");
    // Neither question reached a daemon as a query.
    assert_eq!(beta.queries() + alpha.queries(), 0, "no fan-out to the daemons");

    // listTargets: names, reachability, counts.
    let id = alpha.originate("mesh/query", json!({ "caller": caller, "target": a, "query": { "tool": "listTargets" } })).unwrap();
    let r = answer(&alpha, id).unwrap();
    let targets = r["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|t| t["reachable"] == true && t["activeThreads"] == 0));
    assert!(targets.iter().any(|t| t["name"] == "beta"));
}
