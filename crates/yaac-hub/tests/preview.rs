//! Loopback preview listeners against an isolated yaacd and test TCP servers.

mod support;

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use support::{FakeDaemon, Home, wait_until};
use yaac_hub::protocol::names as ext;
use yaac_hub::{DefaultConnectors, HubConfig};
use yaac_protocol::TargetId;
use yaac_protocol::attach::{ConnectionStatus, names};
use yaac_protocol::methods;
use yaac_protocol::rpc::ErrorCode;
use yaac_transport::LocalConfig;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

fn socket(port: u16) -> io::Result<TcpStream> {
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    set_timeout(&stream)?;
    Ok(stream)
}

fn set_timeout(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))
}

fn local_port(result: &Value, key: &str) -> u16 {
    result[key].as_u64().unwrap().try_into().unwrap()
}

fn socks_connect(proxy: u16, address: &[u8], port: u16, expected: u8) -> TcpStream {
    let mut stream = socket(proxy).unwrap();
    stream.write_all(&[5, 2, 2, 0]).unwrap();
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    let mut request = vec![5, 1, 0];
    request.extend_from_slice(address);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).unwrap();
    let mut reply = [0; 10];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply[..4], [5, expected, 0, 1]);
    stream
}

fn assert_closed(mut stream: TcpStream) {
    let mut byte = [0; 1];
    match stream.read(&mut byte) {
        Ok(0) => {}
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("expected a closed preview socket, got {result:?}"),
    }
}

fn assert_listener_closed(port: u16) {
    wait_until("preview listener to stop accepting", || {
        socket(port).is_err_and(|err| err.kind() == io::ErrorKind::ConnectionRefused)
    });
}

fn round_trip(stream: &mut TcpStream, message: &[u8]) {
    stream.write_all(message).unwrap();
    let mut echo = vec![0; message.len()];
    stream.read_exact(&mut echo).unwrap();
    assert_eq!(echo, message);
}

fn list(client: &support::Client) -> Vec<Value> {
    client
        .call(names::PREVIEW_LIST, json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn connection_count(client: &support::Client, port: u16) -> u64 {
    list(client)
        .iter()
        .find(|p| local_port(p, "localPort") == port)
        .unwrap()["connections"]
        .as_u64()
        .unwrap()
}

fn real_config(home: &Home) -> Option<HubConfig> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = LocalConfig::default_bundle(&root);
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if !bundle.is_file() || std::process::Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: build daemon/dist/yaacd.mjs and put node on PATH (or set YAAC_RUNTIME)");
        return None;
    }
    let target_home = home.dir.path().join("target-home");
    std::fs::create_dir_all(&target_home).unwrap();
    let local = LocalConfig {
        runtime,
        bundle,
        env: vec![],
        inherit_stderr: false,
        build: None,
        install_files: vec![],
    }
    .with_env("YAAC_FAKE_PROVIDER", "1")
    .with_env("YAAC_CODEX_PROVIDER", "0")
    .with_env("YAAC_CLAUDE_PROVIDER", "0")
    .with_env("YAAC_HOME", target_home.join(".yaac"))
    .with_env("YAAC_SOCKET", home.dir.path().join("d.sock"))
    .with_env("YAAC_IDLE_TIMEOUT_MS", "3000")
    .with_env("HOME", &target_home);
    let mut config = home.config();
    config.connectors = Arc::new(DefaultConnectors {
        local,
        wsl: Default::default(),
    });
    // Leave an observable offline interval after the deliberate transport drop.
    config.backoff = yaac_hub::Backoff {
        initial: Duration::from_secs(3),
        max: Duration::from_secs(3),
    };
    Some(config)
}

fn start_echo() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        std::thread::scope(|scope| {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                scope.spawn(move || {
                    set_timeout(&stream).unwrap();
                    let mut reader = stream.try_clone().unwrap();
                    io::copy(&mut reader, &mut stream).unwrap();
                });
            }
        })
    });
    (port, worker)
}

fn test_long_stream(proxy: u16) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    // Transfer much more than initial credit in both directions at the same time on a
    // single connection. Its target closes as soon as it has written the final payload.
    let round_bytes = 1024 * 1024 + 13;
    let upstream: Vec<_> = (0..round_bytes).map(|i| (i % 251) as u8).collect();
    let downstream: Vec<_> = (0..round_bytes).map(|i| (250 - i % 251) as u8).collect();
    let worker = std::thread::spawn({
        let expected = upstream.clone();
        let sent = downstream.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            set_timeout(&stream).unwrap();
            let mut upgrade = [0; 16];
            stream.read_exact(&mut upgrade).unwrap();
            assert_eq!(&upgrade, b"upgrade channel\n");
            stream.write_all(b"ready\n").unwrap();
            std::thread::scope(|scope| {
                let mut writer = stream.try_clone().unwrap();
                scope.spawn(move || writer.write_all(&sent).unwrap());
                let mut data = vec![0; expected.len()];
                stream.read_exact(&mut data).unwrap();
                assert_eq!(data, expected);
            });
        }
    });
    let mut browser = socks_connect(proxy, b"\x03\x09localhost", port, 0);
    round_trip_handshake(&mut browser);
    std::thread::scope(|scope| {
        let mut writer = browser.try_clone().unwrap();
        scope.spawn(move || writer.write_all(&upstream).unwrap());
        let mut data = vec![0; downstream.len()];
        browser.read_exact(&mut data).unwrap();
        assert_eq!(data, downstream);
    });
    worker.join().unwrap();
    assert_closed(browser);
}

fn round_trip_handshake(browser: &mut TcpStream) {
    browser.write_all(b"upgrade channel\n").unwrap();
    let mut response = [0; 6];
    browser.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"ready\n");
}

#[test]
fn preview_tunnels_use_one_real_daemon_connection_and_follow_listener_lifetime() {
    let home = Home::new();
    let Some(config) = real_config(&home) else {
        return;
    };
    let hub = home.start_with(config);
    let client = hub.client();
    let target = client.add_target("preview").id;
    client.wait_status(&target, ConnectionStatus::Connected);
    let entry = client.targets().into_iter().find(|t| t.id == target).unwrap();
    assert!(
        entry.state.daemon.unwrap().port_forward,
        "build the real daemon with preview support"
    );

    let proxy_result = client.call(names::PREVIEW_PROXY, json!({ "target": target })).unwrap();
    let proxy = local_port(&proxy_result, "socksPort");
    assert_eq!(
        client.call(names::PREVIEW_PROXY, json!({ "target": target })).unwrap(),
        proxy_result
    );

    let (echo_port, echo_worker) = start_echo();
    // P323: any destination goes to the target, which resolves it. A name that cannot be a
    // host is refused by the hub (0x02) without a tunnel.
    assert_closed(socks_connect(proxy, &[3, 3, b'a', b' ', b'b'], echo_port, 2));
    // 127.0.0.2 is not loopback by the old rule; Linux routes all of 127/8 to this host.
    #[cfg(target_os = "linux")]
    {
        let other = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 2), 0)).unwrap();
        let other_port = other.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut server, _) = other.accept().unwrap();
                set_timeout(&server).unwrap();
                let mut bytes = [0; 5];
                server.read_exact(&mut bytes).unwrap();
                server.write_all(&bytes).unwrap();
            }
        });
        round_trip(&mut socks_connect(proxy, &[1, 127, 0, 0, 2], other_port, 0), b"addr4");
        // A domain-name CONNECT (address type 3) is passed on unresolved.
        let mut domain = vec![3, 9];
        domain.extend_from_slice(b"127.0.0.2");
        round_trip(&mut socks_connect(proxy, &domain, other_port, 0), b"named");
        server.join().unwrap();
    }

    let refused_port = {
        let unused = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        unused.local_addr().unwrap().port()
    };
    assert_closed(socks_connect(proxy, &[1, 127, 0, 0, 1], refused_port, 5));

    let mut browser = socks_connect(proxy, &[1, 127, 0, 0, 1], echo_port, 0);
    round_trip(&mut browser, b"SOCKS echo");
    test_long_stream(proxy);
    wait_until("the finished SOCKS tunnel to leave the list", || {
        connection_count(&client, proxy) == 1
    });

    let params = json!({ "target": target, "port": echo_port });
    let forward_result = client.call(names::PREVIEW_OPEN, params.clone()).unwrap();
    let forward = local_port(&forward_result, "localPort");
    assert_ne!(
        forward, echo_port,
        "the same port is in use by the target echo server on this test host"
    );
    assert_eq!(forward_result["url"], format!("http://127.0.0.1:{forward}/"));
    assert_eq!(
        client.call(names::PREVIEW_OPEN, params.clone()).unwrap(),
        forward_result
    );
    assert!(
        list(&client)
            .iter()
            .any(|p| p["kind"] == "forward" && p["port"] == echo_port)
    );
    let mut system_browser = socket(forward).unwrap();
    round_trip(&mut system_browser, b"forward echo");
    assert_eq!(connection_count(&client, forward), 1);

    client.call(names::PREVIEW_CLOSE, params.clone()).unwrap();
    assert_closed(system_browser);
    assert_listener_closed(forward);
    assert_eq!(list(&client).len(), 1);

    // A second listen address on the test host lets us exercise the free-port preference.
    if let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) {
        let port = listener.local_addr().unwrap().port();
        let ipv6_params = json!({ "target": target, "port": port, "host": "::1" });
        let local = client.call(names::PREVIEW_OPEN, ipv6_params.clone()).unwrap();
        assert_eq!(local_port(&local, "localPort"), port);
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut server, _) = listener.accept().unwrap();
                set_timeout(&server).unwrap();
                let mut bytes = [0; 4];
                server.read_exact(&mut bytes).unwrap();
                server.write_all(&bytes).unwrap();
            }
        });
        round_trip(&mut socket(port).unwrap(), b"IPv6");
        let ipv6_address = [4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        round_trip(&mut socks_connect(proxy, &ipv6_address, port, 0), b"IPv6");
        server.join().unwrap();
        client.call(names::PREVIEW_CLOSE, ipv6_params).unwrap();
    }

    let reopened = local_port(&client.call(names::PREVIEW_OPEN, params.clone()).unwrap(), "localPort");
    client
        .call(ext::DEBUG_DROP_CONNECTION, json!({ "id": target }))
        .unwrap();
    client.wait_status(&target, ConnectionStatus::Offline);
    assert_closed(browser);
    assert_closed(socket(proxy).unwrap());
    assert_closed(socket(reopened).unwrap());
    assert_eq!(list(&client).len(), 2);
    assert_eq!(connection_count(&client, proxy), 0);
    assert_eq!(
        client.call(names::PREVIEW_PROXY, json!({ "target": target })).unwrap(),
        proxy_result
    );
    assert_eq!(
        client
            .call(names::PREVIEW_OPEN, json!({ "target": target, "port": refused_port }))
            .unwrap_err()
            .code,
        ErrorCode::TargetOffline
    );

    client.wait_status(&target, ConnectionStatus::Connected);
    assert_eq!(
        local_port(
            &client.call(names::PREVIEW_PROXY, json!({ "target": target })).unwrap(),
            "socksPort"
        ),
        proxy
    );
    let mut reconnected = socks_connect(proxy, &[1, 127, 0, 0, 1], echo_port, 0);
    round_trip(&mut reconnected, b"reconnected");
    client
        .call(names::PREVIEW_PROXY_CLOSE, json!({ "target": target }))
        .unwrap();
    assert_closed(reconnected);
    assert_listener_closed(proxy);
    echo_worker.join().unwrap();

    client.call(names::TARGETS_REMOVE, json!({ "id": target })).unwrap();
    assert!(list(&client).is_empty());
    assert_listener_closed(reopened);
    hub.shutdown();
}

#[test]
fn preview_methods_report_a_target_without_port_forwarding() {
    let home = Home::new();
    home.fakes.add(&FakeDaemon::new("old"));
    let hub = home.start();
    let client = hub.client();
    let target: TargetId = client.add_target("old").id;
    client.wait_status(&target, ConnectionStatus::Connected);
    for (method, params) in [
        (names::PREVIEW_PROXY, json!({ "target": target })),
        (names::PREVIEW_OPEN, json!({ "target": target, "port": 5173 })),
    ] {
        let error = client.call(method, params).unwrap_err();
        assert_eq!(error.code, ErrorCode::CapabilityMissing);
        assert!(error.message.contains("portForward"));
    }
    let error = client
        .routed(&target, methods::names::PREVIEW_PORTS, json!({}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::CapabilityMissing);
    assert!(list(&client).is_empty());
    hub.shutdown();
}

/// P380: a tunnel carried over the attach connection itself (the Android client's way to a
/// target server): no hub listener, bytes both ways under credit, closed from either end.
#[test]
fn a_tunnel_over_the_attach_connection_reaches_a_target_server_and_closes() {
    let home = Home::new();
    let Some(config) = real_config(&home) else {
        return;
    };
    let hub = home.start_with(config);
    let client = hub.client();
    let target = client.add_target("preview").id;
    client.wait_status(&target, ConnectionStatus::Connected);
    let (echo_port, echo_worker) = start_echo();
    let open = |client: &support::Client| {
        let r = client
            .call(names::PREVIEW_TUNNEL, json!({ "target": target, "port": echo_port, "initialCredit": 65536 }))
            .unwrap();
        (yaac_protocol::StreamId(u32::try_from(r["stream"].as_u64().unwrap()).unwrap()), r["initialCredit"].as_u64().unwrap())
    };
    let (stream, credit) = open(&client);
    assert!(credit > 0, "the client may send at once");
    let message = b"over the attach link".to_vec();
    client.link.send_data(stream, message.clone()).unwrap();
    let mut echoed = Vec::new();
    while echoed.len() < message.len() {
        let (s, bytes) = client.link.recv_data_timeout(SOCKET_TIMEOUT).unwrap();
        assert_eq!(s, stream);
        echoed.extend(bytes);
    }
    assert_eq!(echoed, message);
    assert!(list(&client).is_empty(), "no hub listener is involved");

    // Closed by the window; a second tunnel then works the same way.
    client.call(names::PREVIEW_TUNNEL_CLOSE, json!({ "stream": stream })).unwrap();
    let (second, _) = open(&client);
    assert_ne!(second, stream);
    client.link.send_data(second, b"again".to_vec()).unwrap();
    let (s, bytes) = client.link.recv_data_timeout(SOCKET_TIMEOUT).unwrap();
    assert_eq!((s, bytes.as_slice()), (second, &b"again"[..]));
    client.call(names::PREVIEW_TUNNEL_CLOSE, json!({ "stream": second })).unwrap();
    // Nothing listens there: an ordinary error, and no stream.
    let refused_port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
    let refused = client.call(names::PREVIEW_TUNNEL, json!({ "target": target, "port": refused_port, "initialCredit": 65536 })).unwrap_err();
    assert_eq!(refused.code, ErrorCode::ConnectionRefused, "{}", refused.message);
    drop(echo_worker);
}
