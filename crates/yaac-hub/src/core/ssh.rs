//! `hub/ssh/*` (docs/02-architecture.md#setting-up-an-ssh-connection): the core's side of SSH
//! Setup, Verify, and Revoke. Each request takes the server's setup token from the
//! [`crate::ssh::Gate`] at once — refused with `target_connected` while the server has a live
//! connection — and runs its sessions on a task, which answers with [`CoreEvent::Reply`] and
//! reports record changes with [`SshEvent::Record`] (always before the answer).
//!
//! The install session of `hub/ssh/signIn` stays open after YAAC's line is appended; it is a
//! small task fed by `hub/ssh/verify` (prove the key works, then keep the line) and
//! `hub/ssh/removeKey` (remove it again). Left alone for ten minutes, or if the hub stops, the
//! session ends and the server removes the line by itself.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use yaac_protocol::attach::{ConnectionStatus, TargetTransport};
use yaac_protocol::rpc::{ErrorObject, RpcId};
use yaac_protocol::TargetId;
use zeroize::Zeroizing;

use super::{ClientKey, Core, CoreEvent, err, hub_err, now_ms, parse, state, to_value};
use crate::protocol::{
    self as ext, SshConnParams, SshHostKey, SshInstalledKey, SshProbeParams, SshProbeResult, SshRecord, SshRecordEntry, SshRecordKind,
    SshRecordsResult, SshRevokeResult, SshSignInParams, codes,
};
use crate::ssh::setup::{self, Credential, Fail, Scanned};
use crate::ssh::{Ident, SetupToken, SshDefaults};

/// How long an install session waits for `verify` or `removeKey`.
const DECIDE_WITHIN: Duration = Duration::from_secs(600);

/// What a setup task tells the core.
#[derive(Debug)]
pub(crate) enum SshEvent {
    /// A host key was read; keep it for `probe`.
    Scanned { ident: Ident, key: Scanned },
    /// What YAAC did on a server changed.
    Record { ident: Ident, record: SshRecord },
    /// An install session is waiting for its decision.
    Installing { ident: Ident, tx: mpsc::UnboundedSender<Decide> },
    /// That session ended.
    InstallDone { ident: Ident },
}

/// A decision for an install session, and who to answer.
#[derive(Debug)]
pub(crate) enum Decide {
    /// `hub/ssh/verify`.
    Verify { client: ClientKey, id: RpcId },
    /// `hub/ssh/removeKey`.
    Remove { client: ClientKey, id: RpcId },
}

/// The core's per-server setup state.
#[derive(Debug, Default)]
pub(crate) struct SshState {
    /// Host keys read by `reach`, waiting for the user's confirmation.
    scanned: std::collections::HashMap<Ident, Scanned>,
    /// Open install sessions.
    installing: std::collections::HashMap<Ident, mpsc::UnboundedSender<Decide>>,
}

fn connected(ident: &Ident) -> ErrorObject {
    hub_err(
        codes::TARGET_CONNECTED,
        format!("{} is connected (or being set up): SSH setup runs only while it has no live connection", ident.label()),
    )
}

fn fail(f: Fail) -> ErrorObject {
    hub_err(f.code, f.message)
}

fn record(kind: SshRecordKind, key: Option<&SshInstalledKey>) -> SshRecord {
    SshRecord { kind, line: key.map(|k| k.line.clone()), comment: key.map(|k| k.comment.clone()), at_ms: now_ms() }
}

/// Runs `job` on a task with the setup token, answering `client` with its outcome.
struct Task {
    core: mpsc::UnboundedSender<CoreEvent>,
    defaults: Arc<SshDefaults>,
    ident: Ident,
    client: ClientKey,
    id: RpcId,
}

impl Task {
    fn record(&self, record: SshRecord) {
        let _ = self.core.send(CoreEvent::Ssh(SshEvent::Record { ident: self.ident.clone(), record }));
    }

    fn reply(&self, outcome: Result<Value, ErrorObject>) {
        let _ = self.core.send(CoreEvent::Reply { client: self.client, id: self.id.clone(), outcome });
    }
}

impl Core {
    fn ssh_defaults(&self) -> Result<Arc<SshDefaults>, ErrorObject> {
        self.cfg.ssh.clone().ok_or_else(|| hub_err(codes::SSH_FAILED, "SSH is not available in this hub"))
    }

    fn ssh_record(&self, ident: &Ident) -> Option<&SshRecord> {
        self.settings.ssh.iter().find(|e| Ident::from_params(&e.conn) == *ident).map(|e| &e.record)
    }

    /// The targets that reach this server.
    fn ssh_targets(&self, ident: &Ident) -> Vec<TargetId> {
        self.targets
            .iter()
            .filter(|(_, t)| matches!(&t.def.transport, TargetTransport::Ssh(s) if Ident::from_transport(s) == *ident))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn ssh_token(defaults: &SshDefaults, ident: &Ident) -> Result<SetupToken, ErrorObject> {
        defaults.gate.setup(ident).ok_or_else(|| connected(ident))
    }

    fn task(&self, defaults: &Arc<SshDefaults>, ident: &Ident, client: ClientKey, id: &RpcId) -> Task {
        Task { core: self.tx.clone(), defaults: defaults.clone(), ident: ident.clone(), client, id: id.clone() }
    }

    /// A `hub/ssh/*` request. `None`: answered later by a task.
    pub(super) fn ssh_request(&mut self, client: ClientKey, id: &RpcId, method: &str, params: Value) -> Option<Result<Value, ErrorObject>> {
        if method == ext::names::SSH_RECORDS {
            let records = self.settings.ssh.clone();
            return Some(Ok(to_value(&SshRecordsResult { records })));
        }
        let defaults = match self.ssh_defaults() {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let outcome = match method {
            ext::names::SSH_REACH => parse::<SshConnParams>(params).and_then(|p| self.ssh_reach(&defaults, client, id, &p)),
            ext::names::SSH_PROBE => parse::<SshProbeParams>(params).and_then(|p| self.ssh_probe(&defaults, client, id, &p)),
            ext::names::SSH_SIGN_IN => parse::<SshSignInParams>(params).and_then(|p| self.ssh_sign_in(&defaults, client, id, p)),
            ext::names::SSH_VERIFY => parse::<SshConnParams>(params).and_then(|p| self.ssh_verify(&defaults, client, id, &p)),
            ext::names::SSH_REMOVE_KEY => parse::<SshConnParams>(params).and_then(|p| self.ssh_remove_key(client, id, &p)),
            ext::names::SSH_REVOKE => parse::<SshConnParams>(params).and_then(|p| self.ssh_revoke(&defaults, client, id, &p)),
            ext::names::SSH_REMOVED_BY_HAND => parse::<SshConnParams>(params).and_then(|p| self.ssh_removed_by_hand(&defaults, &p)),
            _ => Err(err(yaac_protocol::rpc::ErrorCode::MethodNotFound, format!("unknown hub method {method}"))),
        };
        match outcome {
            Ok(Some(v)) => Some(Ok(v)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }

    fn ident(p: &SshConnParams) -> Result<Ident, ErrorObject> {
        let ident = Ident::from_params(p);
        if ident.valid() { Ok(ident) } else { Err(hub_err(codes::SSH_FAILED, "invalid host or user")) }
    }

    fn ssh_reach(&mut self, d: &Arc<SshDefaults>, client: ClientKey, id: &RpcId, p: &SshConnParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(p)?;
        let token = Self::ssh_token(d, &ident)?;
        let task = self.task(d, &ident, client, id);
        tokio::spawn(async move {
            let result = setup::reach(&task.defaults, &task.ident).await;
            drop(token);
            match result {
                Ok(key) => {
                    let answer = SshHostKey { algorithm: key.algorithm.clone(), fingerprint: key.fingerprint.clone() };
                    let _ = task.core.send(CoreEvent::Ssh(SshEvent::Scanned { ident: task.ident.clone(), key }));
                    task.reply(Ok(to_value(&answer)));
                }
                Err(f) => task.reply(Err(fail(f))),
            }
        });
        Ok(None)
    }

    fn ssh_probe(&mut self, d: &Arc<SshDefaults>, client: ClientKey, id: &RpcId, p: &SshProbeParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(&p.conn)?;
        let Some(key) = self.ssh.scanned.get(&ident).filter(|k| k.fingerprint == p.fingerprint).cloned() else {
            return Err(hub_err(codes::SSH_FAILED, "that is not the host key YAAC read from this server: reach it again"));
        };
        let token = Self::ssh_token(d, &ident)?;
        setup::trust(d, &key).map_err(fail)?;
        self.ssh.scanned.remove(&ident);
        let has_key = self.ssh_record(&ident).is_some_and(SshRecord::has_key);
        let task = self.task(d, &ident, client, id);
        tokio::spawn(async move {
            let yaac_key = task.defaults.key(&task.ident);
            let mut result = Ok(false);
            if has_key && yaac_key.is_file() {
                result = setup::probe(&task.defaults, &task.ident, Some(&yaac_key)).await;
            }
            let by_yaac = matches!(result, Ok(true));
            if !by_yaac {
                result = setup::probe(&task.defaults, &task.ident, None).await;
            }
            drop(token);
            match result {
                Ok(in_place) => {
                    // Access that worked before YAAC did anything is recorded so Revoke stays
                    // disabled; YAAC's own working key keeps its record.
                    if in_place && !by_yaac {
                        task.record(record(SshRecordKind::AlreadyInPlace, None));
                    }
                    task.reply(Ok(to_value(&SshProbeResult { in_place })));
                }
                Err(f) => task.reply(Err(fail(f))),
            }
        });
        Ok(None)
    }

    fn ssh_sign_in(&mut self, d: &Arc<SshDefaults>, client: ClientKey, id: &RpcId, p: SshSignInParams) -> Result<Option<Value>, ErrorObject> {
        let SshSignInParams { conn, password, key_file, passphrase } = p;
        // The secrets go into wiped buffers at once; nothing below copies them.
        let credential = Credential {
            password: password.map(Zeroizing::new),
            key_file: key_file.filter(|k| !k.trim().is_empty()).map(std::path::PathBuf::from),
            passphrase: passphrase.filter(|s| !s.is_empty()).map(Zeroizing::new),
        };
        let ident = Self::ident(&conn)?;
        if self.ssh_record(&ident).is_some_and(|r| r.kind == SshRecordKind::InstalledByYaac) {
            return Err(hub_err(codes::SSH_FAILED, "YAAC's key is already installed on this server: Revoke it first to set it up again"));
        }
        // A session still waiting from an earlier attempt ends now (and removes its line).
        self.ssh.installing.remove(&ident);
        // Its token is released as its task ends: then wait for it, below; a live connection
        // is refused at once.
        let token = d.gate.setup(&ident);
        if token.is_none() && d.gate.is_live(&ident) {
            return Err(connected(&ident));
        }
        let task = self.task(d, &ident, client, id);
        tokio::spawn(async move {
            let token = match token {
                Some(t) => t,
                None => match task.defaults.gate.setup_within(&task.ident, Duration::from_secs(10)).await {
                    Some(t) => t,
                    None => return task.reply(Err(hub_err(codes::TARGET_CONNECTED, "another setup step is still running for this server"))),
                },
            };
            let installing = match setup::sign_in(&task.defaults, &task.ident, credential).await {
                Ok(i) => i,
                Err(f) => return task.reply(Err(fail(f))),
            };
            let key = SshInstalledKey { line: installing.line.clone(), comment: installing.comment.clone() };
            task.record(record(SshRecordKind::Installing, Some(&key)));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let _ = task.core.send(CoreEvent::Ssh(SshEvent::Installing { ident: task.ident.clone(), tx }));
            task.reply(Ok(to_value(&key)));
            install_session(&task, installing, &key, &mut rx).await;
            drop(token);
            let _ = task.core.send(CoreEvent::Ssh(SshEvent::InstallDone { ident: task.ident.clone() }));
        });
        Ok(None)
    }

    fn ssh_verify(&mut self, d: &Arc<SshDefaults>, client: ClientKey, id: &RpcId, p: &SshConnParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(p)?;
        if let Some(tx) = self.ssh.installing.get(&ident) {
            return match tx.send(Decide::Verify { client, id: id.clone() }) {
                Ok(()) => Ok(None),
                Err(_) => Err(hub_err(codes::SSH_FAILED, "the setup session has ended: set the server up again")),
            };
        }
        // On its own: does password-less access still work, the way connections use it?
        let token = Self::ssh_token(d, &ident)?;
        let has_key = self.ssh_record(&ident).is_some_and(|r| r.kind == SshRecordKind::InstalledByYaac);
        let task = self.task(d, &ident, client, id);
        tokio::spawn(async move {
            let key = task.defaults.key(&task.ident);
            let result = setup::probe(&task.defaults, &task.ident, (has_key && key.is_file()).then_some(key.as_path())).await;
            drop(token);
            task.reply(match result {
                Ok(true) => Ok(json!({})),
                Ok(false) => Err(hub_err(codes::SSH_FAILED, "the server refused the sign-in: password-less access is not working")),
                Err(f) => Err(fail(f)),
            });
        });
        Ok(None)
    }

    fn ssh_remove_key(&mut self, client: ClientKey, id: &RpcId, p: &SshConnParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(p)?;
        match self.ssh.installing.get(&ident) {
            Some(tx) if tx.send(Decide::Remove { client, id: id.clone() }).is_ok() => Ok(None),
            _ => Err(hub_err(codes::SSH_FAILED, "no setup session is waiting for this server (its line was removed when it ended)")),
        }
    }

    fn ssh_revoke(&mut self, d: &Arc<SshDefaults>, client: ClientKey, id: &RpcId, p: &SshConnParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(p)?;
        let Some(rec) = self.ssh_record(&ident).cloned() else {
            return Ok(Some(to_value(&SshRevokeResult { removed: false })));
        };
        match rec.kind {
            SshRecordKind::AlreadyInPlace => {
                return Err(hub_err(codes::NOTHING_TO_REVOKE, "YAAC installed nothing on this server: the access it uses is your own"));
            }
            SshRecordKind::Revoked | SshRecordKind::RemovedByHand | SshRecordKind::NotSetUp => {
                return Ok(Some(to_value(&SshRevokeResult { removed: false })));
            }
            SshRecordKind::InstalledByYaac | SshRecordKind::Installing => {}
        }
        let (Some(line), Some(comment)) = (rec.line.clone(), rec.comment.clone()) else {
            return Err(hub_err(codes::SSH_FAILED, "YAAC's record of the installed line is missing"));
        };
        // 1. Close the live connection; the target reconnects (without YAAC's key) afterwards.
        for t in self.ssh_targets(&ident) {
            if self.targets.get(&t).is_some_and(|x| x.def.enabled) {
                self.stop_target(&t, state(ConnectionStatus::Connecting, Some("revoking YAAC's key".into())));
            }
        }
        self.ssh.installing.remove(&ident);
        let task = self.task(d, &ident, client, id);
        tokio::spawn(async move {
            let Some(token) = task.defaults.gate.setup_within(&task.ident, Duration::from_secs(15)).await else {
                task.record(rec);
                return task.reply(Err(hub_err(codes::TARGET_CONNECTED, "the connection did not close in time; try again")));
            };
            // 2. Remove exactly the line holding YAAC's key; 3. delete the key.
            let result = setup::revoke(&task.defaults, &task.ident, &line, &comment).await;
            drop(token);
            match result {
                Ok(removed) => {
                    task.record(record(SshRecordKind::Revoked, None));
                    task.reply(Ok(to_value(&SshRevokeResult { removed })));
                }
                Err(f) => {
                    // Reconnect as before: the key is kept until the line is gone.
                    task.record(rec);
                    task.reply(Err(fail(f)));
                }
            }
        });
        Ok(None)
    }

    fn ssh_removed_by_hand(&mut self, d: &Arc<SshDefaults>, p: &SshConnParams) -> Result<Option<Value>, ErrorObject> {
        let ident = Self::ident(p)?;
        if self.ssh_record(&ident).is_some_and(|r| r.kind == SshRecordKind::AlreadyInPlace) {
            return Err(hub_err(codes::NOTHING_TO_REVOKE, "YAAC installed nothing on this server"));
        }
        self.ssh.installing.remove(&ident);
        setup::forget_key(d, &ident);
        self.on_ssh(SshEvent::Record { ident, record: record(SshRecordKind::RemovedByHand, None) });
        Ok(Some(json!({})))
    }

    /// A setup task's report.
    pub(super) fn on_ssh(&mut self, event: SshEvent) {
        match event {
            SshEvent::Scanned { ident, key } => {
                self.ssh.scanned.insert(ident, key);
            }
            SshEvent::Installing { ident, tx } => {
                self.ssh.installing.insert(ident, tx);
            }
            SshEvent::InstallDone { ident } => {
                if self.ssh.installing.get(&ident).is_some_and(mpsc::UnboundedSender::is_closed) {
                    self.ssh.installing.remove(&ident);
                }
            }
            SshEvent::Record { ident, record } => {
                let entry = SshRecordEntry { conn: ident.params(), record };
                let before = self.settings.ssh.clone();
                match self.settings.ssh.iter_mut().find(|e| Ident::from_params(&e.conn) == ident) {
                    Some(e) => *e = entry,
                    None => self.settings.ssh.push(entry),
                }
                if let Err(e) = self.save_settings() {
                    super::log(format!("ssh: cannot save the setup record: {}", e.message));
                    self.settings.ssh = before;
                }
                self.notify_all(ext::names::SSH_CHANGED, json!({}));
                // A target of this server picks up (or drops) YAAC's key on its next attempt.
                for t in self.ssh_targets(&ident) {
                    let reconnect = self.targets.get(&t).is_some_and(|x| x.def.enabled && x.conn.is_none());
                    if reconnect {
                        self.stop_target(&t, state(ConnectionStatus::Connecting, None));
                        self.start_target(&t);
                    }
                }
            }
        }
    }
}

/// The open install session: answer `verify` (prove YAAC's key works, then keep the line) and
/// `removeKey` (remove it again) until one of them ends it, or nobody decides in time.
async fn install_session(task: &Task, installing: setup::Installing, key: &SshInstalledKey, rx: &mut mpsc::UnboundedReceiver<Decide>) {
    let d = &task.defaults;
    let reply = |client: ClientKey, id: RpcId, outcome: Result<Value, ErrorObject>| {
        let _ = task.core.send(CoreEvent::Reply { client, id, outcome });
    };
    loop {
        match tokio::time::timeout(DECIDE_WITHIN, rx.recv()).await {
            Ok(Some(Decide::Verify { client, id })) => {
                let yaac_key = d.key(&task.ident);
                match setup::probe(d, &task.ident, Some(&yaac_key)).await {
                    Ok(true) => {
                        let kept = installing.finish(true).await;
                        match kept {
                            Ok(()) => {
                                task.record(record(SshRecordKind::InstalledByYaac, Some(key)));
                                reply(client, id, Ok(json!({})));
                            }
                            Err(f) => {
                                setup::forget_key(d, &task.ident);
                                task.record(record(SshRecordKind::NotSetUp, None));
                                reply(client, id, Err(fail(f)));
                            }
                        }
                        return;
                    }
                    Ok(false) => reply(
                        client,
                        id,
                        Err(hub_err(codes::SSH_FAILED, "the server did not accept YAAC's key (it may read authorized keys from somewhere else)")),
                    ),
                    Err(f) => reply(client, id, Err(fail(f))),
                }
            }
            Ok(Some(Decide::Remove { client, id })) => {
                let removed = installing.finish(false).await;
                task.record(record(SshRecordKind::NotSetUp, None));
                reply(client, id, removed.map(|()| json!({})).map_err(fail));
                return;
            }
            // Nobody decided (or the hub is stopping): remove the line again.
            Ok(None) | Err(_) => {
                let _ = installing.finish(false).await;
                task.record(record(SshRecordKind::NotSetUp, None));
                return;
            }
        }
    }
}
