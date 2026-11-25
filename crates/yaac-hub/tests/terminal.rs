//! End-to-end terminal budgets. The baseline uses the very same Node stdio/socket
//! bridge with a raw echo/file server on its isolated Unix socket.
#![cfg(unix)]
mod support;

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use support::{Client, Home};
use yaac_hub::DefaultConnectors;
use yaac_protocol::attach::{ConnectionStatus, names};
use yaac_protocol::rpc::Message;
use yaac_protocol::{StreamId, TargetId};
use yaac_transport::LocalConfig;

const FILE_BYTES: u64 = 100_000_000;
const SAMPLES: usize = 600;
const TIMEOUT: Duration = Duration::from_secs(15);
const CREDIT: u64 = 1 << 20;

fn distribution(mut echo: impl FnMut(u8)) -> (Duration, Duration) {
    let mut samples = Vec::new();
    for i in 0..(SAMPLES + 32) {
        let start = Instant::now();
        echo(b'a' + u8::try_from(i % 26).unwrap());
        if i >= 32 {
            samples.push(start.elapsed());
        }
    }
    samples.sort_unstable();
    (samples[SAMPLES / 2 - 1], samples[SAMPLES * 99 / 100 - 1])
}

fn raw(home: &Path, local: &LocalConfig, file: &Path) -> ((Duration, Duration), Duration) {
    assert!(local.bundle.is_file(), "build daemon/dist/yaacd.mjs with npm run build in daemon/");
    let socket = home.join("raw.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut bridge = Command::new(&local.runtime)
        .arg(&local.bundle)
        .arg("connect")
        .envs(local.env.iter().cloned())
        .env("YAAC_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let file = file.to_owned();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        loop {
            let mut key = [0];
            s.read_exact(&mut key).unwrap();
            if key == [0] {
                break;
            }
            s.write_all(&key).unwrap();
        }
        assert_eq!(io::copy(&mut File::open(file).unwrap(), &mut s).unwrap(), FILE_BYTES);
    });
    let mut input = bridge.stdin.take().unwrap();
    let mut output = bridge.stdout.take().unwrap();
    let samples = distribution(|key| {
        input.write_all(&[key]).unwrap();
        let mut echo = [0];
        output.read_exact(&mut echo).unwrap();
        assert_eq!(echo, [key]);
    });
    let start = Instant::now();
    input.write_all(&[0]).unwrap();
    assert_eq!(io::copy(&mut output.take(FILE_BYTES), &mut io::sink()).unwrap(), FILE_BYTES);
    let copy = start.elapsed();
    server.join().unwrap();
    drop(input);
    assert!(bridge.wait().unwrap().success());
    (samples, copy)
}

fn pty_floor(dir: &Path, root: &Path, local: &LocalConfig) -> Duration {
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/terminal-floor.cjs");
    let result = Command::new(&local.runtime)
        .arg(probe)
        .arg(dir)
        .arg(FILE_BYTES.to_string())
        .envs(local.env.iter().cloned())
        .current_dir(root.join("daemon"))
        .output()
        .unwrap();
    assert!(result.status.success(), "PTY baseline: {}", String::from_utf8_lossy(&result.stderr));
    Duration::from_nanos(String::from_utf8(result.stdout).unwrap().trim().parse().unwrap())
}

fn credit(client: &Client, stream: StreamId, bytes: u64) {
    client
        .link
        .send(Message::notification("stream/credit", json!({ "stream": stream, "bytes": bytes })))
        .unwrap();
}

fn receive(client: &Client, stream: StreamId) -> Vec<u8> {
    let (s, bytes) = client.link.recv_data_timeout(TIMEOUT).expect("terminal output");
    assert_eq!(s, stream);
    bytes
}

fn program(client: &Client, target: &TargetId, cwd: &Path, cmd: &str) -> (String, StreamId) {
    let r = client
        .call(
            names::TERMINAL_OPEN,
            json!({"target":target,"cwd":cwd,"cols":80,"rows":24,"shell":"/bin/sh"}),
        )
        .unwrap();
    let stream = serde_json::from_value(r["stream"].clone()).unwrap();
    credit(client, stream, CREDIT);
    let setup = format!("stty raw -echo; printf '\\036'; {cmd}\n");
    client.link.send_data(stream, setup.into_bytes()).unwrap();
    loop {
        let chunk = receive(client, stream);
        credit(client, stream, chunk.len() as u64);
        if chunk.last() == Some(&30) {
            break;
        }
    }
    (r["terminal"].as_str().unwrap().to_owned(), stream)
}

fn latency(client: &Client, stream: StreamId) -> (Duration, Duration) {
    distribution(|key| {
        client.link.send_data(stream, vec![key]).unwrap();
        assert_eq!(receive(client, stream), [key]);
    })
}

fn terminal_copy(c: &Client, target: &TargetId, dir: &Path) -> Duration {
    let (_, stream) = program(c, target, dir, "dd bs=1 count=1 of=/dev/null 2>/dev/null; exec cat payload");
    let start = Instant::now();
    c.link.send_data(stream, vec![1]).unwrap();
    let mut count = 0;
    let mut consumed = 0;
    while count < FILE_BYTES {
        let (s, chunk) = c
            .link
            .recv_data_timeout(TIMEOUT)
            .unwrap_or_else(|e| panic!("after {count} bytes: {e}, events {:?}", c.drain()));
        assert_eq!(stream, s);
        count += chunk.len() as u64;
        consumed += chunk.len() as u64;
        if consumed >= CREDIT / 4 {
            credit(c, stream, consumed);
            consumed = 0;
        }
    }
    let copy = start.elapsed();
    assert_eq!(count, FILE_BYTES);
    copy
}

fn isolated_config(dir: &Path) -> LocalConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    LocalConfig::from_env(&root)
        .with_env("HOME", dir)
        .with_env("YAAC_HOME", dir.join("daemon"))
        .with_env("YAAC_SOCKET", dir.join("d.sock"))
        .with_env("YAAC_IDLE_TIMEOUT_MS", "1000")
        .with_env("YAAC_FAKE_PROVIDER", "1")
        .with_env("YAAC_CODEX_PROVIDER", "0")
        .with_env("YAAC_CLAUDE_PROVIDER", "0")
}

#[test]
fn real_terminal_budgets() {
    let home = Home::new();
    let dir = home.dir.path();
    let file = dir.join("payload");
    File::create(&file).unwrap().set_len(FILE_BYTES).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let local = isolated_config(dir);
    let (baseline, raw_copy) = raw(dir, &local, &file);
    eprintln!("raw echo {baseline:?}, raw 100 MB {raw_copy:?}");
    let mut config = home.config();
    config.connectors = Arc::new(DefaultConnectors {
        local: local.clone(),
        wsl: Default::default(),
    });
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("local").id;
    c.wait_status(&target, ConnectionStatus::Connected);
    let (term, stream) = program(&c, &target, dir, "exec cat");
    let quiet = latency(&c, stream);
    eprintln!("idle echo {quiet:?}");
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let forward = c.call(names::PREVIEW_OPEN, json!({"target":target,"port":port})).unwrap();
    let counter = Arc::new(AtomicU64::new(0));
    let transferred = counter.clone();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = vec![0; 64 * 1024];
        loop {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            transferred.fetch_add(n as u64, Ordering::Relaxed);
        }
    });
    let writer = std::thread::spawn(move || {
        let port = u16::try_from(forward["localPort"].as_u64().unwrap()).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let data = vec![1; 65536];
        while s.write_all(&data).is_ok() {}
    });
    support::wait_until("bulk tunnel flowing", || counter.load(Ordering::Relaxed) >= CREDIT);
    let previous = counter.load(Ordering::Relaxed);
    let loaded = latency(&c, stream);
    eprintln!("loaded echo {loaded:?}");
    let concurrent = counter.load(Ordering::Relaxed) - previous;
    c.call(names::PREVIEW_CLOSE, json!({"target":target,"port":port})).unwrap();
    server.join().unwrap();
    writer.join().unwrap();
    c.call(names::TERMINAL_CLOSE, json!({"target":target,"terminal":term})).unwrap();
    // Pair adjacent samples and take the median of three independent transfers to
    // reduce interference from competing CI tasks while keeping the specified limits.
    let mut copies = [Duration::ZERO; 3];
    let mut ratios = [0.0; 3];
    for (copy, ratio) in copies.iter_mut().zip(&mut ratios) {
        let floor = pty_floor(dir, &root, &local);
        *copy = terminal_copy(&c, &target, dir);
        *ratio = copy.as_secs_f64() / floor.as_secs_f64();
        eprintln!("100 MB direct node-pty {floor:?}, full chain {copy:?}, {:.2}×", *ratio);
    }
    copies.sort_unstable();
    ratios.sort_by(f64::total_cmp);
    let copy = copies[1];
    hub.shutdown();
    eprintln!(
        "echo p50/p99 over {SAMPLES} keys: raw {baseline:?}, terminal {quiet:?}, with {concurrent} bulk bytes {loaded:?}; 100 MB raw {raw_copy:?}, terminal {copy:?} ({:.2}x)",
        copy.as_secs_f64() / raw_copy.as_secs_f64()
    );
    assert!(concurrent >= CREDIT);
    assert!(quiet.1 <= baseline.1 + Duration::from_millis(5), "idle echo p99 budget");
    assert!(loaded.1 <= baseline.1 + Duration::from_millis(5), "bulk echo p99 budget");
    eprintln!("PTY-relative {:.2}×, {:.1} MB/s", ratios[1], 100.0 / copy.as_secs_f64());
    assert!(ratios[1] <= 1.5, "terminal PTY-relative copy budget");
    assert!(copy <= Duration::from_secs(2), "terminal minimum throughput");
}

fn replay(client: &Client, stream: StreamId, suffix: &[u8]) {
    let mut buf = Vec::new();
    while !buf.ends_with(suffix) {
        buf.extend(receive(client, stream));
        assert!(buf.len() as u64 <= 2 * CREDIT);
    }
}

fn event(client: &Client, method: &str, target: &TargetId, terminal: &str) {
    support::wait_until(method, || {
        client.drain().iter().any(|m| matches!(
            m, Message::Notification { method: m, params } if m == method && params["target"] == target.as_str() && params["terminal"] == terminal
        ))
    });
}

#[test]
fn window_and_target_disconnect_preserve_shell_and_replay() {
    let home = Home::new();
    let mut config = home.config();
    config.connectors = Arc::new(DefaultConnectors {
        local: isolated_config(home.dir.path()),
        wsl: Default::default(),
    });
    config.backoff.initial = Duration::from_millis(180);
    let hub = home.start_with(config);
    let first = hub.client();
    let target = first.add_target("local").id;
    first.wait_status(&target, ConnectionStatus::Connected);
    let (term, a) = program(&first, &target, home.dir.path(), "exec cat");
    let second = hub.client();
    let observer = hub.client();
    let attach = || {
        let result = second
            .call(
                names::TERMINAL_ATTACH,
                json!({"target": target, "terminal": term, "cols":90,"rows":30}),
            )
            .unwrap();
        serde_json::from_value(result["stream"].clone()).unwrap()
    };
    let b = attach();
    let title = b"\x1b]2;relay test\x07";
    first.link.send_data(a, title.to_vec()).unwrap();
    replay(&first, a, title);
    event(&first, names::TERMINAL_TITLE, &target, &term);
    event(&second, names::TERMINAL_TITLE, &target, &term);
    assert!(
        observer
            .drain()
            .iter()
            .all(|m| !matches!(m,Message::Notification {method,..} if method==names::TERMINAL_TITLE))
    );
    assert!(second.link.try_recv_data().is_none(), "output before credit");
    credit(&second, b, CREDIT);
    replay(&second, b, title);
    drop(first);
    second.link.send_data(b, vec![0x80, 0, 0xff]).unwrap();
    replay(&second, b, &[0x80, 0, 0xff]);
    second
        .call(yaac_hub::protocol::names::DEBUG_DROP_CONNECTION, json!({"id":target}))
        .unwrap();
    second.wait_status(&target, ConnectionStatus::Offline);
    second.wait_status(&target, ConnectionStatus::Connected);
    let current = attach();
    assert_ne!(current, b);
    credit(&second, current, CREDIT);
    replay(&second, current, &[0x80, 0, 0xff]);
    second
        .call(
            names::TERMINAL_RESIZE,
            json!({"target":target,"terminal":term,"rows":40,"cols":100}),
        )
        .unwrap();
    let list = second.call(names::TERMINAL_LIST, json!({"target":target})).unwrap();
    assert_eq!(list["terminals"][0]["terminal"], term);
    assert!(list["terminals"][0].get("name").is_none());
    // A name the user gives (P343) reaches every attached window and the list; empty clears it.
    second.call(names::TERMINAL_RENAME, json!({"target":target,"terminal":term,"name":" build watch "})).unwrap();
    event(&second, names::TERMINAL_RENAMED, &target, &term);
    let list = second.call(names::TERMINAL_LIST, json!({"target":target})).unwrap();
    assert_eq!(list["terminals"][0]["name"], "build watch");
    second.call(names::TERMINAL_RENAME, json!({"target":target,"terminal":term,"name":""})).unwrap();
    let list = second.call(names::TERMINAL_LIST, json!({"target":target})).unwrap();
    assert!(list["terminals"][0].get("name").is_none());
    let shells = second.call(names::TERMINAL_SHELLS, json!({"target":target})).unwrap();
    assert!(!shells["shells"].as_array().unwrap().is_empty());
    second
        .call(names::TERMINAL_CLOSE, json!({"target":target,"terminal":term}))
        .unwrap();
    event(&second, names::TERMINAL_EXITED, &target, &term);
    hub.shutdown();
}
