//! Rate limits across targets (D161): the hub asks targets whose providers report rate limits,
//! groups the reports by account, passes changes to the windows, keeps an offline target's
//! last values marked stale, and holds refreshes.

mod support;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_protocol::attach::{self, ConnectionStatus, RateLimitsResult};
use yaac_protocol::rpc::Message;

fn provider(instance: &str, kind: &str) -> Value {
    json!({ "instance": instance, "kind": kind, "displayName": kind, "backend": "sdk", "store": { "key": null }, "status": "ready",
            "capabilities": ["threadList", "rateLimits"], "revision": 1 })
}

fn limits(instance: &str, kind: &str, key: &str, used: u32, at: &str) -> Value {
    json!({ "instance": instance, "kind": kind, "account": { "key": key, "label": "Plan" },
            "windows": [{ "id": "weekly", "label": "weekly", "usedPercent": used, "resetsAt": "2026-11-25T15:00:00Z", "windowMinutes": 10080 }],
            "at": at })
}

fn get(c: &support::Client, refresh: bool) -> RateLimitsResult {
    serde_json::from_value(c.call(attach::names::RATE_LIMITS_GET, json!({ "refresh": refresh })).expect("get")).expect("result")
}

fn changes(messages: &[Message]) -> Vec<RateLimitsResult> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Notification { method, params } if method == attach::names::RATE_LIMITS_CHANGED => serde_json::from_value(params.clone()).ok(),
            _ => None,
        })
        .collect()
}

#[test]
fn one_account_on_two_targets_is_one_entry_and_another_stays_apart() {
    let home = Home::new();
    let wsl = FakeDaemon::new("wsl");
    let devbox = FakeDaemon::new("devbox");
    for (f, used, at) in [(&wsl, 40, "2026-11-22T13:00:00Z"), (&devbox, 55, "2026-11-22T13:05:00Z")] {
        f.set_providers(vec![provider("pi_claude", "claude"), provider("pi_codex", "codex")]);
        f.set_rate_limits(vec![limits("pi_claude", "claude", "claude:aaaa", used, at)]);
        home.fakes.add(f);
    }
    devbox.set_rate_limits(vec![
        limits("pi_claude", "claude", "claude:aaaa", 55, "2026-11-22T13:05:00Z"),
        limits("pi_codex", "codex", "codex:bbbb", 70, "2026-11-22T13:05:00Z"),
    ]);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("wsl").id;
    let b = c.add_target("devbox").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("both reported", || get(&c, false).accounts.first().is_some_and(|x| x.targets.len() == 2));
    let r = get(&c, false);
    assert_eq!(r.accounts.len(), 2, "{r:?}");
    let claude = &r.accounts[0];
    assert_eq!(claude.key, "claude:aaaa");
    assert_eq!(claude.windows[0].used_percent, 55, "the newest report wins");
    assert!(!claude.stale);
    assert_eq!(r.accounts[1].key, "codex:bbbb");
    assert_eq!(r.accounts[1].targets, vec![b]);
    // Each target was asked once, without refresh, when its providers arrived.
    assert_eq!(wsl.rate_limit_asks(), vec![false]);

    // A rolling update reaches the window as hub/rateLimits/changed.
    c.drain();
    wsl.originate_notification("provider/rateLimits/updated", limits("pi_claude", "claude", "claude:aaaa", 90, "2026-11-22T13:10:00Z"));
    wait_until("the update", || {
        let all = c.drain();
        changes(&all).iter().any(|x| x.accounts[0].windows[0].used_percent == 90)
    });
}

#[test]
fn an_offline_target_keeps_its_values_marked_stale_also_after_a_hub_restart() {
    let home = Home::new();
    let devbox = FakeDaemon::new("devbox");
    devbox.set_providers(vec![provider("pi_codex", "codex")]);
    devbox.set_rate_limits(vec![limits("pi_codex", "codex", "codex:bbbb", 55, "2026-11-22T13:05:00Z")]);
    home.fakes.add(&devbox);
    let hub = home.start();
    let c = hub.client();
    let b = c.add_target("devbox").id;
    c.wait_status(&b, ConnectionStatus::Connected);
    wait_until("reported", || !get(&c, false).accounts.is_empty());
    devbox.set_offline(true);
    devbox.kill_connections();
    wait_until("stale", || get(&c, false).accounts.first().is_some_and(|x| x.stale));
    assert_eq!(get(&c, false).accounts[0].windows[0].used_percent, 55);
    drop(c);
    hub.shutdown();
    let hub = home.start();
    let c = hub.client();
    let r = get(&c, false);
    assert_eq!(r.accounts.len(), 1, "kept in the cache across the restart");
    assert!(r.accounts[0].stale);
}

#[test]
fn refreshes_are_asked_at_most_every_five_minutes_per_target() {
    let home = Home::new();
    let wsl = FakeDaemon::new("wsl");
    wsl.set_providers(vec![provider("pi_claude", "claude")]);
    wsl.set_rate_limits(vec![limits("pi_claude", "claude", "claude:aaaa", 1, "2026-11-22T13:00:00Z")]);
    home.fakes.add(&wsl);
    let hub = home.start();
    let c = hub.client();
    let a = c.add_target("wsl").id;
    c.wait_status(&a, ConnectionStatus::Connected);
    wait_until("asked", || !wsl.rate_limit_asks().is_empty());
    get(&c, true);
    get(&c, true);
    wait_until("refresh asked", || wsl.rate_limit_asks().len() == 2);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(wsl.rate_limit_asks(), vec![false, true]);
}
