//! SSH Setup, Verify, and Revoke through the hub (`hub/ssh/*`) against a throwaway local
//! `sshd` (`../yaac-transport/tests/support/sshd.rs`): its own host key, port, and
//! `authorized_keys` file, run as the current user. The real `~/.ssh` is never written (and no
//! real key or agent is offered: every session carries `IdentityAgent=none` and a missing
//! `IdentityFile`).
//!
//! The matrix: password setup, key-file (PEM) setup, verify, revoke removing only YAAC's
//! line, revoke again (idempotent), access already in place (nothing to revoke), a failed
//! proof removing the line again, and setup refused while the target is connected — then a
//! turn end to end: the real hub, ssh to the local sshd, the real daemon bundle installed by
//! stage 0, the fake provider.
//!
//! A non-root `sshd` cannot check passwords, so the password case runs `ssh` through a
//! stand-in (`fake-ssh`) that does what a password server makes `ssh` do — ask `SSH_ASKPASS`
//! and refuse a wrong answer — and then continues with a test key the server accepts.

#![cfg(unix)]
#![allow(clippy::needless_pass_by_value, reason = "test helpers take JSON by value like the support module")]

#[path = "../../yaac-transport/tests/support/sshd.rs"]
mod sshd;
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sshd::Sshd;
use support::{Home, RunningHub};
use yaac_hub::protocol::{SshRecordKind, SshRecordsResult, codes, names};
use yaac_hub::ssh::SshDefaults;
use yaac_protocol::attach::{self, ConnectionStatus};

const PASSWORD: &str = "correct-horse-battery-staple-7";

struct Rig {
    sshd: Sshd,
    home: Home,
    hub: Option<RunningHub>,
    c: support::Client,
    /// The server user's `YAAC_HOME` (YAAC's record on the server goes there).
    yaac_home: PathBuf,
    /// Lines in `authorized_keys` before anything ran.
    original: Vec<String>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.take() {
            drop(hub);
        }
        let _ = std::fs::remove_dir_all(&self.yaac_home);
    }
}

struct Opts {
    /// Offer the user's own key (in `authorized_keys`) by default: access already in place.
    own_key_works: bool,
    /// `ssh` is the password stand-in.
    password_server: bool,
    /// YAAC writes to a different `authorized_keys` than the server reads (the proof fails).
    wrong_authorized_keys: bool,
    /// The daemon bundle, for connecting.
    bundle: bool,
}

const NO: Opts = Opts { own_key_works: false, password_server: false, wrong_authorized_keys: false, bundle: false };

fn bundle() -> Option<yaac_transport::DaemonBundle> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../daemon/dist/yaacd.mjs");
    std::fs::read(path).ok().map(|b| yaac_transport::DaemonBundle::new("0.0.0-test", b))
}

/// `fake-ssh`: with a password sign-in (`PubkeyAuthentication=no`), asks `SSH_ASKPASS` as
/// `ssh` does, refuses a wrong password like a server, and otherwise goes on with the test
/// key; every other session is passed to the real `ssh` unchanged.
fn password_server(dir: &Path, key: &Path) -> PathBuf {
    let script = dir.join("fake-ssh");
    let log = dir.join("askpass-used");
    let text = format!(
        r#"#!/bin/sh
case " $* " in
  *" PubkeyAuthentication=no "*)
    pw=$("$SSH_ASKPASS" "tester@127.0.0.1's password: ") || {{ echo "tester@127.0.0.1: Permission denied (password)." >&2; exit 255; }}
    echo asked >>'{log}'
    [ "$pw" = '{PASSWORD}' ] || {{ echo "tester@127.0.0.1: Permission denied (password)." >&2; exit 255; }}
    n=$#
    for a in "$@"; do
      case $a in
        PubkeyAuthentication=no) a=PubkeyAuthentication=yes ;;
        PreferredAuthentications=*) a=PreferredAuthentications=publickey ;;
      esac
      set -- "$@" "$a"
    done
    shift "$n"
    exec ssh -o IdentityFile='{key}' "$@"
    ;;
esac
exec ssh "$@"
"#,
        log = log.display(),
        key = key.display()
    );
    std::fs::write(&script, text).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

fn rig(name: &str, opts: &Opts) -> Option<Rig> {
    if !Sshd::available() {
        eprintln!("SKIPPED: needs {} and ssh", sshd::SSHD);
        return None;
    }
    let sshd = Sshd::start(name, None);
    let original = sshd.authorized_lines();
    let home = Home::new();
    let yaac_home = std::env::temp_dir().join(format!("yaac-ssh-target-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&yaac_home);
    let paths = home.paths();
    let mut env = vec![("YAAC_HOME".to_owned(), yaac_home.display().to_string())];
    if opts.bundle {
        env.extend(
            [("YAAC_FAKE_PROVIDER", "1"), ("YAAC_CODEX_PROVIDER", "0"), ("YAAC_CLAUDE_PROVIDER", "0"), ("YAAC_IDLE_TIMEOUT_MS", "4000")]
                .map(|(k, v)| (k.to_owned(), v.to_owned())),
        );
    }
    let mut d = SshDefaults::new(&paths.data_dir, &paths.runtime_dir, if opts.bundle { bundle() } else { None }, env);
    d.askpass_exe = Some(PathBuf::from(env!("CARGO_BIN_EXE_yaac-askpass")));
    d.authorized_keys = Some(if opts.wrong_authorized_keys { sshd.dir.join("elsewhere") } else { sshd.authorized_keys.clone() }.display().to_string());
    d.runtime = None;
    d.machine = "test-host".into();
    d.options = vec!["IdentityAgent=none".into(), "GlobalKnownHostsFile=/dev/null".into()];
    if opts.own_key_works {
        d.options.push(format!("IdentityFile={}", sshd.client_key.display()));
    } else {
        d.options.push(format!("IdentityFile={}", sshd.dir.join("no-such-key").display()));
    }
    if opts.password_server {
        d.ssh_exe = password_server(&sshd.dir, &sshd.client_key).into();
    }
    let mut config = home.config();
    config.ssh = Some(Arc::new(d));
    let hub = home.start_with(config);
    let c = hub.client();
    Some(Rig { sshd, home, hub: Some(hub), c, yaac_home, original })
}

impl Rig {
    fn conn(&self) -> Value {
        json!({ "host": "127.0.0.1", "port": self.sshd.port })
    }

    fn with(&self, extra: Value) -> Value {
        let mut v = self.conn();
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, yaac_protocol::rpc::ErrorObject> {
        self.c.call(method, params)
    }

    /// Reach, check the fingerprint against `ssh-keygen -l`, confirm, probe.
    fn reach_and_probe(&self) -> bool {
        let key = self.call(names::SSH_REACH, self.conn()).expect("reach");
        let out = std::process::Command::new("ssh-keygen").arg("-lf").arg(self.sshd.dir.join("host_key.pub")).output().unwrap();
        let printed = String::from_utf8_lossy(&out.stdout);
        assert_eq!(key["algorithm"], "ED25519");
        assert!(printed.contains(key["fingerprint"].as_str().unwrap()), "{key} vs {printed}");
        let probed = self.call(names::SSH_PROBE, self.with(json!({ "fingerprint": key["fingerprint"] }))).expect("probe");
        probed["inPlace"].as_bool().unwrap()
    }

    fn record(&self) -> Option<yaac_hub::protocol::SshRecord> {
        let r: SshRecordsResult = serde_json::from_value(self.call(names::SSH_RECORDS, json!({})).unwrap()).unwrap();
        r.records.into_iter().find(|e| e.conn.port == Some(self.sshd.port)).map(|e| e.record)
    }

    fn yaac_keys(&self) -> usize {
        std::fs::read_dir(self.home.paths().data_dir.join("ssh/keys")).map_or(0, |d| d.count())
    }

    /// No file under the hub's profile or the server's YAAC home contains `needle`.
    fn nowhere_on_disk(&self, needle: &str) {
        for root in [self.home.dir.path(), self.yaac_home.as_path()] {
            for f in walk(root) {
                let bytes = std::fs::read(&f).unwrap_or_default();
                assert!(!String::from_utf8_lossy(&bytes).contains(needle), "{} contains the secret", f.display());
            }
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
        }
    }
    out
}

fn code(e: &yaac_protocol::rpc::ErrorObject) -> String {
    serde_json::to_value(&e.code).unwrap().as_str().unwrap_or_default().to_owned()
}

#[test]
fn password_setup_then_verify_then_revoke_removes_only_yaacs_line_and_is_idempotent() {
    let Some(r) = rig("password", &Opts { password_server: true, ..NO }) else { return };
    assert!(!r.reach_and_probe(), "no password-less access before setup");

    // A wrong password: refused, nothing changed on the server or here.
    let e = r.call(names::SSH_SIGN_IN, r.with(json!({ "password": "wrong" }))).unwrap_err();
    assert_eq!(code(&e), codes::SSH_FAILED, "{e:?}");
    assert!(e.message.contains("did not accept the password"), "{}", e.message);
    assert_eq!(r.sshd.authorized_lines(), r.original);
    assert_eq!(r.yaac_keys(), 0, "the key made for the failed attempt is gone");

    let installed = r.call(names::SSH_SIGN_IN, r.with(json!({ "password": PASSWORD }))).expect("sign in");
    let line = installed["line"].as_str().unwrap().to_owned();
    let comment = installed["comment"].as_str().unwrap().to_owned();
    assert!(comment.starts_with("yaac:test-host:") && line.starts_with("ssh-ed25519 ") && line.ends_with(&comment));
    assert!(std::fs::read_to_string(r.sshd.dir.join("askpass-used")).unwrap().lines().count() >= 2, "the password came through SSH_ASKPASS");
    let lines = r.sshd.authorized_lines();
    assert_eq!(lines.len(), r.original.len() + 1);
    assert_eq!(&lines[..r.original.len()], &r.original[..], "existing lines untouched");
    assert_eq!(lines.last().unwrap(), &line);
    assert_eq!(r.record().unwrap().kind, SshRecordKind::Installing);

    r.call(names::SSH_VERIFY, r.conn()).expect("verify: YAAC's key alone works");
    let rec = r.record().unwrap();
    assert_eq!((rec.kind, rec.line.as_deref()), (SshRecordKind::InstalledByYaac, Some(line.as_str())));
    let id = comment.rsplit(':').next().unwrap();
    assert_eq!(std::fs::read_to_string(r.yaac_home.join("ssh").join(id)).unwrap().trim(), line, "the record on the server");
    let key = r.home.paths().data_dir.join("ssh/keys");
    let key_file = walk(&key).into_iter().find(|p| p.file_name().is_some_and(|n| n == "id_ed25519")).unwrap();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&key_file).unwrap().permissions()) & 0o777, 0o600, "user-only key");
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(key_file.parent().unwrap()).unwrap().permissions()) & 0o777, 0o700);
    r.nowhere_on_disk(PASSWORD);

    // Verify on its own, later.
    r.call(names::SSH_VERIFY, r.conn()).expect("standalone verify");

    // Revoke: only YAAC's line goes, and YAAC's key.
    let revoked = r.call(names::SSH_REVOKE, r.conn()).expect("revoke");
    assert_eq!(revoked["removed"], true);
    assert_eq!(r.sshd.authorized_lines(), r.original, "exactly the original lines are left");
    assert_eq!(r.yaac_keys(), 0, "YAAC's key is deleted");
    assert!(!r.yaac_home.join("ssh").join(id).exists(), "the record on the server is removed");
    assert_eq!(r.record().unwrap().kind, SshRecordKind::Revoked);
    // Idempotent.
    assert_eq!(r.call(names::SSH_REVOKE, r.conn()).expect("revoke again")["removed"], false);
    assert_eq!(r.sshd.authorized_lines(), r.original);
    assert_eq!(r.record().unwrap().kind, SshRecordKind::Revoked);
    // And Verify now says password-less access is gone.
    assert!(r.call(names::SSH_VERIFY, r.conn()).is_err());
}

#[test]
fn key_file_setup_uses_the_pem_once_and_installs_yaacs_own_key() {
    let Some(r) = rig("pem", &NO) else { return };
    assert!(!r.reach_and_probe());
    let pem = std::fs::read_to_string(&r.sshd.client_key).unwrap();
    let installed = r.call(names::SSH_SIGN_IN, r.with(json!({ "keyFile": r.sshd.client_key }))).expect("sign in with the key file");
    r.call(names::SSH_VERIFY, r.conn()).expect("verify");
    assert_eq!(r.record().unwrap().kind, SshRecordKind::InstalledByYaac);
    let own = std::fs::read_to_string(r.sshd.client_key.with_extension("pub")).unwrap();
    assert!(!installed["line"].as_str().unwrap().contains(own.split_whitespace().nth(1).unwrap()), "YAAC installs its own key, not the PEM's");
    // The PEM was never copied.
    // (Lines 1-2 of every unencrypted ed25519 key are the same header; line 3 is its own.)
    r.nowhere_on_disk(pem.lines().nth(3).unwrap());
    // A missing key file is refused before anything happens.
    let e = r.call(names::SSH_SIGN_IN, r.with(json!({ "keyFile": "/no/such/key.pem" }))).unwrap_err();
    assert!(e.message.contains("already installed") || e.message.contains("does not exist"), "{}", e.message);
    assert_eq!(r.call(names::SSH_REVOKE, r.conn()).unwrap()["removed"], true);
    assert_eq!(r.sshd.authorized_lines(), r.original);
}

#[test]
fn access_already_in_place_installs_nothing_and_cannot_be_revoked() {
    let Some(r) = rig("inplace", &Opts { own_key_works: true, ..NO }) else { return };
    assert!(r.reach_and_probe(), "the user's own key works");
    assert_eq!(r.record().unwrap().kind, SshRecordKind::AlreadyInPlace);
    let e = r.call(names::SSH_REVOKE, r.conn()).unwrap_err();
    assert_eq!(code(&e), codes::NOTHING_TO_REVOKE);
    assert_eq!(r.sshd.authorized_lines(), r.original);
    assert_eq!(r.yaac_keys(), 0);
}

#[test]
fn a_failed_proof_removes_the_line_again() {
    let Some(r) = rig("proof", &Opts { wrong_authorized_keys: true, ..NO }) else { return };
    assert!(!r.reach_and_probe());
    r.call(names::SSH_SIGN_IN, r.with(json!({ "keyFile": r.sshd.client_key }))).expect("sign in");
    let elsewhere = r.sshd.dir.join("elsewhere");
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap().lines().count(), 1, "YAAC wrote where it was told");
    let e = r.call(names::SSH_VERIFY, r.conn()).unwrap_err();
    assert!(e.message.contains("did not accept YAAC's key"), "{}", e.message);
    r.call(names::SSH_REMOVE_KEY, r.conn()).expect("remove the line again");
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap().lines().count(), 0, "the line is gone");
    assert_eq!(r.record().unwrap().kind, SshRecordKind::NotSetUp);
    assert_eq!(r.yaac_keys(), 0);
    assert_eq!(r.sshd.authorized_lines(), r.original);
}

#[test]
fn an_unknown_fingerprint_is_refused_and_an_unreachable_server_says_so() {
    let Some(r) = rig("hostkey", &NO) else { return };
    let e = r.call(names::SSH_PROBE, r.with(json!({ "fingerprint": "SHA256:not-it" }))).unwrap_err();
    assert!(e.message.contains("not the host key"), "{}", e.message);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let e = r.call(names::SSH_REACH, json!({ "host": "127.0.0.1", "port": port })).unwrap_err();
    assert_eq!(code(&e), codes::SSH_UNREACHABLE, "{e:?}");
}

#[test]
fn end_to_end_a_turn_over_ssh_and_setup_is_refused_while_connected() {
    let Some(r) = rig("e2e", &Opts { bundle: true, ..NO }) else { return };
    if bundle().is_none() {
        return eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist");
    }
    assert!(!r.reach_and_probe());
    r.call(names::SSH_SIGN_IN, r.with(json!({ "keyFile": r.sshd.client_key }))).expect("sign in");
    r.call(names::SSH_VERIFY, r.conn()).expect("verify");

    let added = r
        .call(attach::names::TARGETS_ADD, json!({ "name": "devbox", "transport": { "kind": "ssh", "host": "127.0.0.1", "port": r.sshd.port }, "enabled": true }))
        .expect("add");
    let target: yaac_protocol::TargetId = serde_json::from_value(added["target"]["id"].clone()).unwrap();
    wait_connected(&r.c, &target);

    // Setup's short sessions never run beside the live connection.
    let e = r.call(names::SSH_REACH, r.conn()).unwrap_err();
    assert_eq!(code(&e), codes::TARGET_CONNECTED, "{e:?}");

    // A turn on the fake provider, through ssh, stage 0, and the real daemon bundle.
    let providers = r.c.routed(&target, "provider/list", json!({})).expect("provider/list");
    let instance = providers["providers"].as_array().and_then(|p| p.first()).and_then(|p| p["instance"].as_str()).expect("the fake provider").to_owned();
    let started = r
        .c
        .routed(&target, "thread/start", json!({
            "opId": "op-ssh-1",
            "instance": instance,
            "cwd": r.yaac_home.display().to_string(),
            "input": [{ "type": "text", "text": "/fake say hello over ssh" }]
        }))
        .expect("thread/start");
    let thread = started["threadId"].as_str().unwrap().to_owned();
    r.c.subscribe(&target, &thread);
    support::wait_until("the turn's text in the cache", || {
        r.home.cache().thread(&target, &yaac_protocol::ThreadId::new(&thread)).ok().flatten().is_some_and(|t| {
            t.items.iter().any(|i| matches!(&i.body, yaac_protocol::ItemBody::AgentMessage(m) if m.text.contains("hello over ssh")))
        })
    });
    assert_eq!(r.sshd.authorized_lines().len(), r.original.len() + 1);

    // Revoke closes the connection first, then removes YAAC's line; the target cannot sign
    // in afterwards and says so.
    assert_eq!(r.call(names::SSH_REVOKE, r.conn()).expect("revoke")["removed"], true);
    assert_eq!(r.sshd.authorized_lines(), r.original);
    support::wait_until("the target reports that it is not set up", || {
        r.c.targets().iter().any(|t| t.id == target && t.state.status == ConnectionStatus::Offline && t.state.detail.as_deref().is_some_and(|d| d.contains("not set up")))
    });
}

fn wait_connected(c: &support::Client, target: &yaac_protocol::TargetId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let t = c.targets().into_iter().find(|t| &t.id == target).unwrap();
        if t.state.status == ConnectionStatus::Connected {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "never connected: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(50));
    }
}
