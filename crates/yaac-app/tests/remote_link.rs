//! What the phone's link costs, measured (P393, docs/14-android.md): the Android window
//! controller (`view_protocol`, hub mode) attaches over TCP to a byte-counting proxy with a
//! mobile-data shape (75 ms each way, 1 Mbit/s each way) in front of a real hub on a temporary
//! socket, with the real `yaacd` bundle on its fake provider as the one target. Nothing listens
//! on a Tailscale address or on port 47380: the proxy is on loopback, the hub on a socket file.
//!
//! `#[ignore]`: a measurement with wall-clock numbers, run on demand:
//! `cargo test -p yaac-app --test remote_link -- --ignored --nocapture`.
//! It needs `daemon/dist/yaacd.mjs` and `node` (or `YAAC_RUNTIME`); without them it says so and
//! passes. It prints a report; the assertions are the correctness checks (typing shows without
//! the network, no draft traffic), never the timings.
#![cfg(all(unix, feature = "hub"))]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_app::view_protocol::{Mode, Options, ViewController};
use yaac_hub::client::HubClient;
use yaac_hub::{DefaultConnectors, HubConfig, HubPaths};
use yaac_transport::LocalConfig;

const ONE_WAY: Duration = Duration::from_millis(75);
const BYTES_PER_SEC: f64 = 125_000.0; // 1 Mbit/s
const OUTSIDE_THREADS: usize = 200;

fn seeded_thread(summary: &Value) -> bool {
    summary["title"].as_str().is_some_and(|title| title.starts_with("Conversation "))
}

/// Bytes the proxy carried, each way.
#[derive(Default)]
struct Counts {
    up: AtomicU64,
    down: AtomicU64,
}

impl Counts {
    fn get(&self) -> (u64, u64) {
        (self.up.load(Ordering::Relaxed), self.down.load(Ordering::Relaxed))
    }
}

/// One direction of the shaped link: read, hold for the one-way delay, pace to the rate.
fn pipe(mut from: impl Read + Send + 'static, mut to: impl Write + Send + 'static, count: Arc<Counts>, up: bool) {
    let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let c = if up { &count.up } else { &count.down };
                    c.fetch_add(n as u64, Ordering::Relaxed);
                    if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    std::thread::spawn(move || {
        let mut free = Instant::now();
        for (at, bytes) in rx {
            let start = (at + ONE_WAY).max(free);
            free = start + Duration::from_secs_f64(bytes.len() as f64 / BYTES_PER_SEC);
            let now = Instant::now();
            if free > now {
                std::thread::sleep(free - now);
            }
            if to.write_all(&bytes).is_err() {
                return;
            }
        }
    });
}

/// The proxy's open connections, to drop them as a phone changing network would.
type Open = Arc<std::sync::Mutex<Vec<std::net::TcpStream>>>;

fn proxy(socket: PathBuf, counts: Arc<Counts>, open: Open) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback proxy");
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let Ok(unix) = UnixStream::connect(&socket) else { continue };
            let _ = tcp.set_nodelay(true);
            open.lock().unwrap().push(tcp.try_clone().unwrap());
            pipe(tcp.try_clone().unwrap(), unix.try_clone().unwrap(), counts.clone(), true);
            pipe(unix, tcp, counts.clone(), false);
        }
    });
    address
}

fn bundle() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = LocalConfig::default_bundle(&root);
    bundle.is_file().then_some(bundle)
}

/// Measure index pages alone, excluding the hello and with a fresh TCP connection each time.
fn measure_index(socket: PathBuf, target: &str, first_budget: Option<usize>) -> (BTreeMap<String, usize>, usize) {
    let counts = Arc::new(Counts::default());
    let address = proxy(socket, counts.clone(), Arc::default());
    let client = HubClient::connect_tcp(address.parse().unwrap(), || {}).unwrap();
    let request = |method, params| client.request_blocking(method, params, Duration::from_secs(30)).unwrap();
    request("hub/hello", json!({ "build": yaac_app::BUILD }));
    let before = counts.get();
    let started = Instant::now();
    let mut pages = Vec::new();
    let mut all = 0;
    let mut seeded = BTreeMap::new();
    let mut token = String::new();
    let mut first = None;
    loop {
        let mut params = json!({ "token": token });
        if pages.is_empty() && let Some(bytes) = first_budget {
            params["maxBytes"] = json!(bytes);
        }
        let page = request("hub/request", json!({ "target": target, "method": "thread/sync", "params": params }));
        let changed = page["changed"].as_array().unwrap();
        if pages.is_empty() {
            assert_eq!(page["full"], true);
            first = Some((started.elapsed(), changed.iter().filter(|s| seeded_thread(s)).count()));
        }
        pages.push(page.to_string().len());
        all += changed.len();
        for summary in changed.iter().filter(|s| seeded_thread(s)) {
            let id = summary["id"].as_str().unwrap().to_owned();
            assert!(seeded.insert(id, summary.to_string().len()).is_none(), "duplicate index row");
        }
        if page["more"] != true {
            break;
        }
        let next = page["token"].as_str().unwrap();
        assert_ne!(next, token);
        next.clone_into(&mut token);
    }
    assert_eq!(seeded.len(), OUTSIDE_THREADS);
    let after = counts.get();
    let (first_time, first_rows) = first.unwrap();
    let label = if first_budget.is_some() { "24 KiB first budget" } else { "default page budget" };
    println!("thread/sync ({label}): {all} threads, pages {pages:?} JSON B, {} total JSON B, link up {} B / down {} B, first {first_time:?}, all {:?}",
        pages.iter().sum::<usize>(), after.0 - before.0, after.1 - before.1, started.elapsed());
    (seeded, first_rows)
}

/// Frames from the controller, each parsed, with the time it was taken.
struct Frames {
    controller: ViewController,
    composer: Value,
    seen: Vec<(Instant, usize, Value)>,
}

impl Frames {
    fn next(&mut self, wait: Duration) -> bool {
        let Some(frame) = self.controller.wait_frame(wait) else { return false };
        let v: Value = serde_json::from_str(&frame).unwrap_or(Value::Null);
        if let Some(c) = v.pointer("/views/composer") {
            self.composer = c.clone();
        }
        self.seen.push((Instant::now(), frame.len(), v));
        true
    }

    fn until(&mut self, what: &str, timeout: Duration, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.next(Duration::from_millis(50));
        }
    }

    fn drain_for(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            self.next(end.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
        }
    }

    #[allow(clippy::needless_pass_by_value)] // json!() at every call
    fn command(&self, command: &str, payload: Value) {
        self.controller.command(&json!({ "command": command, "payload": payload }).to_string()).expect("command");
    }
}

#[test]
#[ignore = "a measurement: run with --ignored --nocapture"]
fn the_phone_link_typing_and_streaming() {
    let Some(bundle) = bundle() else {
        eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist");
        return;
    };
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if std::process::Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: the runtime {runtime:?} does not run");
        return;
    }
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp").unwrap();
    let daemon_home = dir.path().join("target-home");
    std::fs::create_dir_all(&daemon_home).unwrap();
    let local = LocalConfig { runtime, bundle, env: vec![], inherit_stderr: false, build: None, install_files: vec![] }
        .with_env("YAAC_FAKE_PROVIDER", "1")
        .with_env("YAAC_CODEX_PROVIDER", "0")
        .with_env("YAAC_CLAUDE_PROVIDER", "0")
        .with_env("YAAC_HOME", daemon_home.join(".yaac"))
        .with_env("YAAC_SOCKET", dir.path().join("d.sock"))
        .with_env("YAAC_IDLE_TIMEOUT_MS", "3000")
        .with_env("HOME", &daemon_home);
    let paths = HubPaths::under(&dir.path().join("hub"));
    let socket = paths.socket();
    let mut config = HubConfig::new(yaac_app::BUILD, paths, Arc::new(DefaultConnectors { local, wsl: Default::default() }));
    config.seed_local_target = false;
    config.handle_signals = false;
    let hub = std::thread::spawn(move || yaac_hub::run_hub(config));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !socket.exists() {
        assert!(Instant::now() < deadline && !hub.is_finished(), "the hub did not start");
        std::thread::sleep(Duration::from_millis(10));
    }

    // A side client on the hub's machine sets up a target and a thread.
    let side = HubClient::connect(&socket).unwrap();
    let call = |m: &str, p: Value| side.request_blocking(m, p, Duration::from_secs(30)).unwrap_or_else(|e| panic!("{m}: {e:?}"));
    call("hub/hello", json!({ "build": yaac_app::BUILD }));
    let target = call("hub/targets/add", json!({ "name": "local", "transport": { "kind": "local" }, "enabled": true }))["target"]["id"].as_str().unwrap().to_owned();
    let routed = |m: &str, p: Value| side.request_blocking("hub/request",
        json!({ "target": target, "method": m, "params": p }), Duration::from_secs(60)).unwrap_or_else(|e| panic!("{m}: {e:?}"));
    let deadline = Instant::now() + Duration::from_secs(60);
    let instance = loop {
        if let Ok(p) = side.request_blocking("hub/request", json!({ "target": target, "method": "provider/list", "params": {} }), Duration::from_secs(5))
            && let Some(i) = p["providers"].as_array().and_then(|p| p.first()).and_then(|p| p["instance"].as_str())
        {
            break i.to_owned();
        }
        assert!(Instant::now() < deadline, "the real daemon never offered its fake provider");
        std::thread::sleep(Duration::from_millis(200));
    };
    let thread = routed("thread/start", json!({
        "opId": "op-link-1", "instance": instance, "cwd": daemon_home.display().to_string(),
        "input": [{ "type": "text", "text": "/fake say ready" }],
    }))["threadId"].as_str().unwrap().to_owned();
    let address = format!("{target}/{thread}");

    // The phone.
    let counts = Arc::new(Counts::default());
    let open: Open = Arc::default();
    let hub_address = proxy(socket.clone(), counts.clone(), open.clone());
    let phone_dir = dir.path().join("phone");
    let options = Options {
        mode: Mode::Hub { address: hub_address, data_dir: phone_dir.clone() },
        theme: "dark".into(),
        input: "touch".into(),
        requests: true,
        platform: "android".into(),
        browser_host: false,
    };
    let mut f = Frames { controller: ViewController::start(options.clone()).unwrap(), composer: Value::Null, seen: Vec::new() };
    f.command("shell.visible", json!({ "shown": true }));
    let t0 = Instant::now();
    f.until("the thread in the list", Duration::from_secs(60), |f| {
        f.seen.iter().any(|(_, _, v)| v.pointer("/views/sidebar").is_some_and(|s| s.to_string().contains(&thread)))
    });
    let attach_bytes = counts.get();
    let attach_time = t0.elapsed();
    f.drain_for(Duration::from_secs(2));
    let idle_start = counts.get();
    f.drain_for(Duration::from_secs(5));
    let idle_end = counts.get();

    f.command("sidebar.open", json!({ "id": address }));
    f.until("the thread's composer", Duration::from_secs(30), |f| f.composer["key"].as_str().is_some_and(|k| k.contains(&thread)));
    f.drain_for(Duration::from_secs(2));
    let key = f.composer["key"].as_str().unwrap().to_owned();

    // Typing: a fast typist, one key every 60 ms.
    let text = "Please refactor the parser so that errors carry their spans";
    let before = counts.get();
    let mut lags = Vec::new();
    let mut frame_bytes = Vec::new();
    for n in 1..=text.len() {
        let typed = &text[..n];
        let sent = Instant::now();
        let mark = f.seen.len();
        f.command("composer.edit", json!({ "key": key, "text": typed, "caret": n }));
        f.until("the composer's echo", Duration::from_secs(10), |f| f.composer["text"] == typed);
        lags.push(sent.elapsed());
        frame_bytes.extend(f.seen[mark..].iter().map(|(_, b, _)| *b));
        let rest = Duration::from_millis(60).saturating_sub(sent.elapsed());
        f.drain_for(rest);
    }
    let typed_end = counts.get();
    f.drain_for(Duration::from_secs(3));
    let settled = counts.get();
    assert!(std::fs::read_to_string(phone_dir.join("drafts.json")).is_ok_and(|s| s.contains(text)), "the draft is kept on the device");

    // Streaming in the open thread.
    let words: Vec<String> = (0..400).map(|i| format!("word{i}")).collect();
    let stream_before = counts.get();
    let mark = f.seen.len();
    routed("turn/start", json!({
        "opId": "op-link-2", "threadId": thread,
        "input": [{ "type": "text", "text": format!("/fake pace 10\n/fake say {}", words.join(" ")) }],
    }));
    f.until("the streamed answer", Duration::from_secs(60), |f| {
        f.seen.iter().rev().take(5).any(|(_, _, v)| v.to_string().contains("word399"))
    });
    f.drain_for(Duration::from_secs(1));
    let stream_after = counts.get();
    let stream_frames: Vec<usize> = f.seen[mark..].iter().map(|(_, b, _)| *b).collect();

    // A 20 MB tool output in a thread the phone has not opened, then the phone opens it.
    let unopened_before = counts.get();
    let big = routed("thread/start", json!({
        "opId": "op-link-3", "instance": instance, "cwd": daemon_home.display().to_string(),
        "input": [{ "type": "text", "text": "/fake big-output 20971520\n/fake say done" }],
    }))["threadId"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let read = routed("thread/read", json!({ "threadId": big }));
        if read.to_string().contains("\"done") { break; }
        assert!(Instant::now() < deadline, "the big output never finished");
        f.drain_for(Duration::from_millis(500));
    }
    f.drain_for(Duration::from_secs(2));
    let unopened_after = counts.get();
    f.command("sidebar.open", json!({ "id": format!("{target}/{big}") }));
    f.until("the big thread's composer", Duration::from_secs(60), |f| f.composer["key"].as_str().is_some_and(|k| k.contains(&big)));
    f.drain_for(Duration::from_secs(4));
    let opened_big = counts.get();
    println!("20 MB tool output in an unopened thread: link down {} B, up {} B", unopened_after.1 - unopened_before.1, unopened_after.0 - unopened_before.0);
    println!("opening that thread: link down {} B, up {} B", opened_big.1 - unopened_after.1, opened_big.0 - unopened_after.0);
    f.command("sidebar.open", json!({ "id": address }));
    f.until("back in the first thread", Duration::from_secs(30), |f| f.composer["key"].as_str() == Some(key.as_str()));
    f.drain_for(Duration::from_secs(1));

    // The phone changes network: the connection drops; typing goes on; the controller attaches
    // again by itself. The composer must never be emptied meanwhile.
    let mark = f.seen.len();
    let before_drop = counts.get();
    for s in open.lock().unwrap().drain(..) {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
    let dropped = Instant::now();
    let more = format!("{text}, and keep the old behaviour");
    for n in text.len() + 1..=more.len() {
        f.command("composer.edit", json!({ "key": key, "text": &more[..n], "caret": n }));
        f.drain_for(Duration::from_millis(60));
    }
    f.until("attached again", Duration::from_secs(60), |_| !open.lock().unwrap().is_empty());
    f.drain_for(Duration::from_secs(4));
    let reattach = dropped.elapsed();
    let after_reattach = counts.get();
    let emptied = f.seen[mark..].iter().any(|(_, _, v)| v.pointer("/views/composer/text").is_some_and(|t| t.as_str().is_some_and(|t| !more.starts_with(t) || t.len() < text.len())));
    let composer_now = f.composer["text"].as_str().unwrap_or_default().to_owned();

    lags.sort();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = |p: f64| lags[((lags.len() as f64 - 1.0) * p) as usize];
    let up_typing = typed_end.0 - before.0;
    let down_typing = typed_end.1 - before.1;
    println!("== the phone's link (75 ms each way, 1 Mbit/s) ==");
    println!("attach to a listed thread: {attach_time:?}, up {} B, down {} B", attach_bytes.0, attach_bytes.1);
    println!("idle 5 s: up {} B, down {} B", idle_end.0 - idle_start.0, idle_end.1 - idle_start.1);
    println!("typing {} keys: echo lag p50 {:?} p90 {:?} max {:?}", lags.len(), pct(0.5), pct(0.9), lags.last().unwrap());
    println!("typing: link up {up_typing} B, down {down_typing} B; 3 s after: up {} B, down {} B", settled.0 - typed_end.0, settled.1 - typed_end.1);
    println!("typing: controller frames {} B total, {} frames, max {} B", frame_bytes.iter().sum::<usize>(), frame_bytes.len(), frame_bytes.iter().max().unwrap_or(&0));
    println!("streaming {} words: link down {} B, up {} B; controller frames {} ({} B total, max {} B, mean {} B)",
        words.len(), stream_after.1 - stream_before.1, stream_after.0 - stream_before.0, stream_frames.len(),
        stream_frames.iter().sum::<usize>(), stream_frames.iter().max().unwrap_or(&0),
        stream_frames.iter().sum::<usize>() / stream_frames.len().max(1));
    println!("reconnect (typing on): back within {reattach:?}; link up {} B, down {} B; composer emptied or rolled back: {emptied}; text kept: {}",
        after_reattach.0 - before_drop.0, after_reattach.1 - before_drop.1, composer_now == more);
    assert!(!emptied, "a reconnection never empties or rolls back the composer");
    assert_eq!(composer_now, more, "composer after the reconnection: {}", f.composer);
    assert!(*lags.last().unwrap() < ONE_WAY * 2, "the echo of a key never waits for the network (max {:?})", lags.last().unwrap());

    f.controller.stop();

    // Seed actual outside conversations before measuring a fresh link to the same hub.
    // The fake provider rewrites its store for each seed, so discard the 20 MB fixture first.
    routed("thread/delete", json!({ "opId": "op-link-delete-big", "threadId": big, "alsoInHarness": true }));
    routed("discovery/caps", json!({ "opId": "op-link-caps", "update": { "enabled": false } }));
    let seed: String = (0..OUTSIDE_THREADS)
        .map(|i| format!("/fake outside-thread Conversation {i}: investigate the parser and keep the old behaviour\n"))
        .collect();
    routed("turn/start", json!({
        "opId": "op-link-seed", "threadId": thread, "input": [{ "type": "text", "text": seed }],
    }));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let mut token = String::new();
        let mut found = 0;
        loop {
            let page = routed("thread/sync", json!({ "token": token }));
            found += page["changed"].as_array().unwrap().iter().filter(|s| seeded_thread(s)).count();
            if page["more"] != true {
                break;
            }
            let next = page["token"].as_str().unwrap();
            assert_ne!(next, token);
            next.clone_into(&mut token);
        }
        if found == OUTSIDE_THREADS { break; }
        assert!(Instant::now() < deadline, "the fake provider created only {found} outside threads");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (baseline, baseline_first_rows) = measure_index(socket.clone(), &target, None);
    let (paged, paged_first_rows) = measure_index(socket.clone(), &target, Some(24 * 1024));
    assert_eq!(baseline, paged, "the smaller page budget delivers the same index");
    assert!(paged_first_rows < baseline_first_rows);
    let mean = paged.values().sum::<usize>() as f64 / paged.len() as f64;
    println!("thread/sync seeded summaries: {mean:.1} B mean, {} B max", paged.values().max().unwrap());

    let new_counts = Arc::new(Counts::default());
    let address = proxy(socket, new_counts.clone(), Arc::default());
    let options = Options { mode: Mode::Hub { address, data_dir: dir.path().join("new-phone") }, ..options };
    let started = Instant::now();
    let mut fresh = Frames { controller: ViewController::start(options).unwrap(), composer: Value::Null, seen: Vec::new() };
    fresh.command("shell.visible", json!({ "shown": true }));
    let seeded_rows = |frame: &Value| frame.pointer("/views/sidebar/sections").and_then(Value::as_array)
        .into_iter().flatten().filter_map(|section| section["rows"].as_array()).flatten()
        .filter(|row| seeded_thread(row)).count();
    fresh.until("the first seeded sidebar rows", Duration::from_secs(30), |f| f.seen.iter().any(|(_, _, v)| seeded_rows(v) > 0));
    let (first, first_rows) = fresh.seen.iter().map(|(time, _, v)| (time, seeded_rows(v))).find(|&(_, n)| n > 0).unwrap();
    assert_eq!(first_rows, paged_first_rows, "the sidebar paints the smaller page while more pages are in flight");
    println!("fresh phone sidebar: first {first_rows} seeded rows in {:?}", first.duration_since(started));
    fresh.until("all seeded sidebar rows", Duration::from_secs(30), |f| f.seen.iter().any(|(_, _, v)| seeded_rows(v) == OUTSIDE_THREADS));
    println!("fresh phone sidebar: all {OUTSIDE_THREADS} seeded rows in {:?}, link up {} B / down {} B", started.elapsed(), new_counts.get().0, new_counts.get().1);
    fresh.controller.stop();
    let _ = call("hub/shutdown", json!({}));
    let _ = hub.join();
}
