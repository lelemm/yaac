//! Attaching over Tailscale (docs/02-architecture.md#attaching-over-tailscale): when the hub
//! setting `tailscaleAttach` ("Allow the Android client over Tailscale") is on, the hub also
//! accepts attaches over TCP on this host's Tailscale addresses, on [`TAILSCALE_ATTACH_PORT`].
//!
//! - It binds **only** the addresses Tailscale reports for this host (IPv4 and IPv6), never a
//!   wildcard address.
//! - It authorises each connection, before reading a byte from it, by asking Tailscale who the
//!   peer is (`tailscale whois`). Only this host's own Tailscale login is accepted; any other
//!   connection is closed at once. There is no pairing and nothing secret is stored.
//! - An accepted connection is an ordinary attach: the same frames, the same methods, and the
//!   same `hub/hello` build rule as the local socket.
//!
//! What Tailscale says is behind [`Tailnet`], so tests use a fake. The real one is
//! [`TailscaleCli`], which runs the `tailscale` command line.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use yaac_transport::BoxFuture;

use crate::core::CoreEvent;
use crate::protocol::TailscaleAttach;

/// The TCP port the hub listens on, on its Tailscale addresses only.
pub const TAILSCALE_ATTACH_PORT: u16 = 47_380;

/// How long one `tailscale` command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How often an unavailable Tailscale is asked again while the setting is on.
const RETRY: Duration = Duration::from_secs(60);
/// Connections being authorised at once; more are closed at once.
const MAX_PENDING: usize = 8;
/// TCP keepalive on both ends: a phone that sleeps or changes network is noticed within
/// about a minute (idle 30 s, then 3 probes 10 s apart).
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_PROBES: u32 = 3;

/// Turns on TCP keepalive for an attach over Tailscale (both the hub's and the client's end).
/// Where the platform cannot set the interval or the probe count, its defaults apply.
pub(crate) fn keep_alive(socket: &socket2::SockRef<'_>) -> std::io::Result<()> {
    let keepalive = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE);
    #[cfg(any(target_os = "android", target_os = "linux", target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "windows"))]
    let keepalive = keepalive.with_interval(KEEPALIVE_INTERVAL).with_retries(KEEPALIVE_PROBES);
    #[cfg(not(any(target_os = "android", target_os = "linux", target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "windows")))]
    let _ = (KEEPALIVE_INTERVAL, KEEPALIVE_PROBES);
    socket.set_tcp_keepalive(&keepalive)
}

/// Whether two Tailscale logins are the same. They are e-mail-like names, so case does not
/// matter.
pub(crate) fn same_login(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// This host on the tailnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThisNode {
    /// Its Tailscale addresses.
    pub addresses: Vec<IpAddr>,
    /// The Tailscale login it belongs to (`LoginName`).
    pub login: String,
}

/// What Tailscale knows about this host and its peers.
pub trait Tailnet: Send + Sync {
    /// This host's addresses and login (`tailscale status --json`). An error is a sentence for
    /// the user: Tailscale is missing, stopped, or logged out.
    fn this_node(&self) -> BoxFuture<'static, Result<ThisNode, String>>;
    /// The Tailscale login of the node at `peer` (`tailscale whois --json`).
    fn login_of(&self, peer: IpAddr) -> BoxFuture<'static, Result<String, String>>;
}

/// The real [`Tailnet`]: the `tailscale` command line.
#[derive(Clone, Debug, Default)]
pub struct TailscaleCli {
    /// The executable. `None`: `tailscale` on `PATH`, then where the installers put it.
    pub exe: Option<OsString>,
}

impl TailscaleCli {
    fn candidates(&self) -> Vec<OsString> {
        if let Some(exe) = &self.exe {
            return vec![exe.clone()];
        }
        let mut out: Vec<OsString> = vec!["tailscale".into()];
        if cfg!(windows) {
            out.push(r"C:\Program Files\Tailscale\tailscale.exe".into());
        }
        if cfg!(target_os = "macos") {
            out.push("/Applications/Tailscale.app/Contents/MacOS/Tailscale".into());
        }
        out
    }

    fn run(&self, args: Vec<String>) -> BoxFuture<'static, Result<Value, String>> {
        let candidates = self.candidates();
        Box::pin(async move {
            for exe in candidates {
                let mut command = tokio::process::Command::new(&exe);
                command.args(&args).stdin(std::process::Stdio::null()).kill_on_drop(true);
                #[cfg(windows)]
                {
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    command.creation_flags(CREATE_NO_WINDOW);
                }
                let output = match tokio::time::timeout(COMMAND_TIMEOUT, command.output()).await {
                    Err(_) => return Err("Tailscale did not answer in time.".to_owned()),
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Ok(Err(e)) => return Err(format!("Cannot run Tailscale: {e}.")),
                    Ok(Ok(output)) => output,
                };
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no message").trim().to_owned();
                    return Err(format!("Tailscale failed: {line}"));
                }
                return serde_json::from_slice(&output.stdout).map_err(|e| format!("Tailscale's answer is not JSON: {e}."));
            }
            Err("Tailscale is not installed on this host.".to_owned())
        })
    }
}

impl Tailnet for TailscaleCli {
    fn this_node(&self) -> BoxFuture<'static, Result<ThisNode, String>> {
        let run = self.run(vec!["status".into(), "--json".into()]);
        Box::pin(async move { parse_status(&run.await?) })
    }

    fn login_of(&self, peer: IpAddr) -> BoxFuture<'static, Result<String, String>> {
        let run = self.run(vec!["whois".into(), "--json".into(), peer.to_string()]);
        Box::pin(async move { parse_whois(&run.await?) })
    }
}

/// `tailscale status --json` → this node: `Self.TailscaleIPs`, and the login of
/// `User[Self.UserID]`.
pub fn parse_status(status: &Value) -> Result<ThisNode, String> {
    if let Some(state) = status.get("BackendState").and_then(Value::as_str)
        && state != "Running"
    {
        return Err(format!("Tailscale is not running on this host (state: {state})."));
    }
    let me = status.get("Self").ok_or("Tailscale did not describe this host.")?;
    let addresses: Vec<IpAddr> = me
        .get("TailscaleIPs")
        .and_then(Value::as_array)
        .map(|ips| ips.iter().filter_map(Value::as_str).filter_map(|ip| ip.parse().ok()).collect())
        .unwrap_or_default();
    if addresses.is_empty() {
        return Err("Tailscale has no address for this host.".to_owned());
    }
    let user = match me.get("UserID") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => return Err("Tailscale did not say which login this host belongs to.".to_owned()),
    };
    let login = status
        .get("User")
        .and_then(|users| users.get(&user))
        .and_then(|u| u.get("LoginName"))
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty())
        .ok_or("Tailscale did not say which login this host belongs to.")?;
    Ok(ThisNode { addresses, login: login.to_owned() })
}

/// `tailscale whois --json <ip>` → `UserProfile.LoginName`.
pub fn parse_whois(whois: &Value) -> Result<String, String> {
    whois
        .pointer("/UserProfile/LoginName")
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Tailscale does not know the peer's login.".to_owned())
}

/// Everything the listener needs from the hub.
pub(crate) struct Ctx {
    pub tailnet: Arc<dyn Tailnet>,
    pub port: u16,
    pub core: mpsc::UnboundedSender<CoreEvent>,
    pub writers: mpsc::UnboundedSender<JoinHandle<()>>,
}

/// Follows the setting until the hub stops: listens while it is on, reports each change of
/// state to the core. Never returns.
pub(crate) async fn run(mut enabled: watch::Receiver<bool>, ctx: Ctx) {
    let ctx = Arc::new(ctx);
    let report = |status: TailscaleAttach| {
        let _ = ctx.core.send(CoreEvent::Tailscale(status));
    };
    // A retry after `unavailable` keeps showing why until it succeeds (no flicker each minute).
    let mut retrying = false;
    loop {
        if !*enabled.borrow_and_update() {
            retrying = false;
            report(TailscaleAttach::Off);
            if enabled.changed().await.is_err() {
                return std::future::pending().await;
            }
            continue;
        }
        if !retrying {
            report(TailscaleAttach::Starting);
        }
        let bound = tokio::select! {
            bound = bind(&ctx) => bound,
            changed = enabled.changed() => {
                if changed.is_err() { return std::future::pending().await; }
                continue;
            }
        };
        match bound {
            Err(reason) => {
                crate::core::log(format!("not listening on Tailscale: {reason}"));
                report(TailscaleAttach::Unavailable { reason });
                tokio::select! {
                    changed = enabled.changed() => {
                        if changed.is_err() { return std::future::pending().await; }
                        retrying = false;
                    }
                    () = tokio::time::sleep(RETRY) => retrying = true,
                }
            }
            Ok((listeners, login)) => {
                retrying = false;
                let addresses = listeners.iter().filter_map(|l| l.local_addr().ok()).map(|a| a.to_string()).collect::<Vec<_>>();
                crate::core::log(format!("listening on Tailscale: {}", addresses.join(", ")));
                report(TailscaleAttach::Listening { addresses, login });
                // Turning the setting off also closes the connections it let in.
                let (stop_tx, stop_rx) = watch::channel(false);
                let pending = Arc::new(AtomicUsize::new(0));
                let tasks: Vec<JoinHandle<()>> = listeners
                    .into_iter()
                    .map(|listener| tokio::spawn(accept_loop(listener, ctx.clone(), stop_rx.clone(), pending.clone())))
                    .collect();
                let changed = enabled.changed().await;
                for task in tasks {
                    task.abort();
                }
                let _ = stop_tx.send(true);
                if changed.is_err() {
                    return std::future::pending().await;
                }
            }
        }
    }
}

/// Asks Tailscale for this host's addresses and binds each one. Succeeds when at least one
/// is bound.
async fn bind(ctx: &Ctx) -> Result<(Vec<TcpListener>, String), String> {
    let node = ctx.tailnet.this_node().await?;
    let mut listeners = Vec::new();
    let mut errors = Vec::new();
    for ip in node.addresses {
        if ip.is_unspecified() {
            continue; // never a wildcard address
        }
        match TcpListener::bind(SocketAddr::new(ip, ctx.port)).await {
            Ok(listener) => listeners.push(listener),
            Err(e) => errors.push(format!("{}: {e}", SocketAddr::new(ip, ctx.port))),
        }
    }
    if listeners.is_empty() {
        return Err(if errors.is_empty() { "Tailscale has no address for this host.".to_owned() } else { format!("Cannot listen on {}.", errors.join("; ")) });
    }
    Ok((listeners, node.login))
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>, stop: watch::Receiver<bool>, pending: Arc<AtomicUsize>) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            // Out of descriptors, or similar: do not spin.
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        if pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
            pending.fetch_sub(1, Ordering::SeqCst);
            crate::core::log(format!("refused a Tailscale attach from {}: too many waiting", peer.ip()));
            drop(stream);
            continue;
        }
        tokio::spawn(admit(stream, peer, ctx.clone(), stop.clone(), pending.clone()));
    }
}

/// Authorises one connection, then hands it to the attach server. Nothing is read from the
/// connection before Tailscale has named the peer.
async fn admit(stream: TcpStream, peer: SocketAddr, ctx: Arc<Ctx>, mut stop: watch::Receiver<bool>, pending: Arc<AtomicUsize>) {
    let verdict = authorise(ctx.tailnet.as_ref(), peer.ip()).await;
    pending.fetch_sub(1, Ordering::SeqCst);
    if let Err(reason) = verdict {
        crate::core::log(format!("refused a Tailscale attach from {}: {reason}", peer.ip()));
        drop(stream);
        return;
    }
    if *stop.borrow() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let _ = keep_alive(&socket2::SockRef::from(&stream));
    let (reader, writer) = stream.into_split();
    let stopped = async move {
        let _ = stop.wait_for(|stopped| *stopped).await;
    };
    crate::server::attach(reader, writer, &ctx.core, &ctx.writers, true, stopped);
}

/// Accepts the peer only when its login is this host's own.
pub(crate) async fn authorise(tailnet: &dyn Tailnet, peer: IpAddr) -> Result<(), String> {
    let checks = async { tokio::join!(tailnet.this_node(), tailnet.login_of(peer)) };
    let (me, them) = tokio::time::timeout(COMMAND_TIMEOUT, checks).await.map_err(|_| "Tailscale did not answer in time".to_owned())?;
    let (me, them) = (me?, them?);
    if !same_login(&me.login, &them) {
        return Err("another Tailscale login".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_names_this_node() {
        let status = json!({
            "BackendState": "Running",
            "Self": { "UserID": 6150179426259323_u64, "TailscaleIPs": ["100.91.112.95", "fd7a:115c:a1e0::37:7061"] },
            "User": { "6150179426259323": { "LoginName": "me@example.com" } }
        });
        let node = parse_status(&status).unwrap();
        assert_eq!(node.login, "me@example.com");
        assert_eq!(node.addresses, vec!["100.91.112.95".parse::<IpAddr>().unwrap(), "fd7a:115c:a1e0::37:7061".parse().unwrap()]);
    }

    #[test]
    fn status_refuses_a_stopped_or_anonymous_node() {
        assert!(parse_status(&json!({ "BackendState": "Stopped", "Self": {} })).unwrap_err().contains("Stopped"));
        assert!(parse_status(&json!({ "BackendState": "Running", "Self": { "TailscaleIPs": [] } })).is_err());
        assert!(parse_status(&json!({ "Self": { "UserID": 1, "TailscaleIPs": ["100.1.2.3"] }, "User": {} })).is_err());
    }

    #[test]
    fn logins_compare_without_case() {
        assert!(same_login("Me@Example.com", "me@example.COM"));
        assert!(!same_login("me@example.com", "you@example.com"));
        assert!(!same_login("", ""));
    }

    #[test]
    fn keepalive_is_set_on_a_tcp_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let sock = socket2::SockRef::from(&stream);
        keep_alive(&sock).unwrap();
        assert!(sock.keepalive().unwrap());
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            assert_eq!(sock.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
            assert_eq!(sock.tcp_keepalive_interval().unwrap(), KEEPALIVE_INTERVAL);
            assert_eq!(sock.tcp_keepalive_retries().unwrap(), KEEPALIVE_PROBES);
        }
    }

    #[test]
    fn whois_reads_the_login() {
        assert_eq!(parse_whois(&json!({ "Node": {}, "UserProfile": { "ID": 1, "LoginName": "me@example.com" } })).unwrap(), "me@example.com");
        assert!(parse_whois(&json!({ "Node": {} })).is_err());
        assert!(parse_whois(&json!({ "UserProfile": { "LoginName": "" } })).is_err());
    }
}
