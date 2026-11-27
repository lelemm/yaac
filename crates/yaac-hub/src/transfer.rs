//! Moving the hub to another machine (docs/15-hub-transfer.md, P408–P410): what the hub needs
//! from the `yaac` binary, and the rewritten `hub.json` the new hub starts with.
//!
//! The orchestration is in the core (`core/transfer.rs`): `hub/transfer/candidates` and
//! `hub/transfer/start`, over the chosen target's one existing `yaacd` connection
//! (invariant 1). Finding the release asset of this hub's build and turning this computer's
//! start-at-login off are a [`TransferHost`], which the `yaac` binary supplies (the updater's
//! release code); tests supply a fake. A hub without one cannot move.

use std::path::{Path, PathBuf};

use serde_json::Value;
use yaac_protocol::TargetId;
use yaac_protocol::attach::{LocalTransport, TargetTransport};

use crate::protocol::SshRecordKind;
use crate::settings::Settings;

/// The hub binary for one platform, as the release `manifest.json` of this hub's build lists
/// it (`yaac-hub-linux-<arch>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubAsset {
    /// Where `yaacd` on the new machine downloads it from.
    pub url: String,
    /// Its SHA-256, lower-case hex.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
}

/// What the hub asks of the `yaac` binary to move. Every method blocks; the hub calls them on
/// a thread of its own, never on its I/O thread.
pub trait TransferHost: Send + Sync {
    /// The Linux hub binary of exactly `build` for `arch` (`x86_64`, `aarch64`). An error is a
    /// sentence for the user.
    fn hub_asset(&self, build: &str, arch: &str) -> Result<HubAsset, String>;
    /// Turns this computer's start-at-login of the hub off (the hub has moved).
    fn stop_autostart(&self) -> Result<(), String>;
}

/// The name of the release asset of the Linux hub for `arch`.
pub fn hub_asset_name(arch: &str) -> String {
    format!("yaac-hub-linux-{arch}")
}

/// Architectures a Linux hub is published for.
pub const HUB_ARCHES: &[&str] = &["x86_64"];

/// `extra` field of a target the new hub cannot reach (P410): why, for the user.
pub const UNREACHABLE_FIELD: &str = "movedUnreachable";
/// `extra` field of an SSH target that used the user's own key or agent (P410): it needs a
/// sign-in on the new hub.
pub const SIGN_IN_FIELD: &str = "signInAgain";

/// A file of YAAC's that moves with the hub, beside `hub.json` and the stores: YAAC's own SSH
/// key for a server (invariant 12: only the keys YAAC generated), and its confirmed host keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedFile {
    /// Where it is on this computer.
    pub path: PathBuf,
    /// Its name in the staging directory on the new machine (`data/ssh/...`).
    pub name: String,
}

/// The new hub's settings (P410), and YAAC's own files to copy with them.
///
/// - `dest` becomes the new hub's local target, with the same id (its threads keep their keys).
/// - Other SSH targets stay. One YAAC set up with its own key (and the key is here) keeps it:
///   the key is copied. One that used the user's own key or agent is marked
///   [`SIGN_IN_FIELD`]; the user's keys are never copied.
/// - WSL targets and this computer's local target cannot be reached from the new machine: they
///   stay, disabled, marked [`UNREACHABLE_FIELD`].
/// - `hubId` and `clientId` are kept; attaching over Tailscale is turned on (every client is
///   remote now).
pub fn rewrite_settings(settings: &Settings, dest: &TargetId, this_host: &str, ssh_dir: Option<&Path>) -> (Settings, Vec<MovedFile>) {
    let mut out = settings.clone();
    let mut files = Vec::new();
    for t in &mut out.targets {
        if &t.id == dest {
            t.transport = TargetTransport::Local(LocalTransport {});
            t.extra.remove(SIGN_IN_FIELD);
            t.extra.remove(UNREACHABLE_FIELD);
            continue;
        }
        match &t.transport {
            TargetTransport::Ssh(s) => {
                let ident = crate::ssh::Ident::from_transport(s);
                let own = settings.ssh.iter().find(|e| crate::ssh::Ident::from_params(&e.conn) == ident).is_some_and(|e| e.record.kind == SshRecordKind::InstalledByYaac);
                let key = ssh_dir.map(|d| d.join("keys").join(ident.slug()).join("id_ed25519"));
                match key.filter(|k| own && k.is_file()) {
                    Some(key) => {
                        let name = format!("data/ssh/keys/{}/id_ed25519", ident.slug());
                        let public = key.with_extension("pub");
                        if public.is_file() {
                            files.push(MovedFile { path: public, name: format!("{name}.pub") });
                        }
                        files.push(MovedFile { path: key, name });
                    }
                    None => {
                        t.extra.insert(SIGN_IN_FIELD.into(), Value::String("Sign in again on the new hub: YAAC never copies your own SSH keys.".into()));
                    }
                }
            }
            TargetTransport::Wsl(_) | TargetTransport::Local(_) => {
                t.enabled = false;
                t.extra.insert(UNREACHABLE_FIELD.into(), Value::String(format!("On {this_host}: reachable only by a hub on that PC.")));
            }
            TargetTransport::Unknown(_) => {}
        }
    }
    if let Some(known) = ssh_dir.map(|d| d.join("known_hosts")).filter(|k| k.is_file()) {
        files.push(MovedFile { path: known, name: "data/ssh/known_hosts".into() });
    }
    out.hub.tailscale_attach = true;
    (out, files)
}

/// A consistent copy of an open SQLite database into `dest` (`VACUUM INTO`): what the
/// transfer stages on the new machine.
pub(crate) fn vacuum_into(conn: &rusqlite::Connection, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(dest);
    let text = dest.to_str().ok_or("the staging path is not valid text")?;
    conn.execute("VACUUM INTO ?1", [text]).map(|_| ()).map_err(|e| format!("cannot copy into {}: {e}", dest.display()))
}

/// Whether an operation id can name a staging directory on the new machine (`yaacd` takes
/// only these).
pub fn op_id_ok(op: &str) -> bool {
    !op.is_empty() && op.len() <= 128 && op.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{SshConnParams, SshRecord, SshRecordEntry};
    use crate::settings::TargetDef;
    use yaac_protocol::TargetName;

    fn def(id: &str, transport: TargetTransport) -> TargetDef {
        TargetDef { id: TargetId::new(id), name: TargetName::new(id), transport, enabled: true, extra: Default::default() }
    }

    fn ssh(host: &str) -> TargetTransport {
        serde_json::from_value(serde_json::json!({ "kind": "ssh", "host": host })).unwrap()
    }

    #[test]
    fn the_new_hub_gets_the_destination_as_local_and_only_yaacs_own_keys() {
        let dir = tempfile::tempdir().unwrap();
        let ssh_dir = dir.path().join("ssh");
        let mut settings = Settings::first_run(false, true);
        settings.hub_id = Some("h1".into());
        settings.targets = vec![
            def("local", TargetTransport::Local(LocalTransport {})),
            def("wsl", serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Ubuntu" })).unwrap()),
            def("x", ssh("box")),
            def("own", ssh("own-key")),
            def("user", ssh("user-key")),
        ];
        for host in ["own-key", "user-key"] {
            settings.ssh.push(SshRecordEntry {
                conn: serde_json::from_value::<SshConnParams>(serde_json::json!({ "host": host })).unwrap(),
                record: SshRecord { kind: if host == "own-key" { SshRecordKind::InstalledByYaac } else { SshRecordKind::AlreadyInPlace }, line: None, comment: None, at_ms: 0 },
            });
        }
        let ident = crate::ssh::Ident::from_transport(match &settings.targets[3].transport { TargetTransport::Ssh(s) => s, _ => unreachable!() });
        let key_dir = ssh_dir.join("keys").join(ident.slug());
        std::fs::create_dir_all(&key_dir).unwrap();
        std::fs::write(key_dir.join("id_ed25519"), "private").unwrap();
        std::fs::write(key_dir.join("id_ed25519.pub"), "public").unwrap();
        std::fs::write(ssh_dir.join("known_hosts"), "hosts").unwrap();

        let (new, files) = rewrite_settings(&settings, &TargetId::new("x"), "my-pc", Some(&ssh_dir));
        assert_eq!(new.hub_id.as_deref(), Some("h1"));
        assert!(new.hub.tailscale_attach);
        let t = |id: &str| new.targets.iter().find(|t| t.id.as_str() == id).unwrap();
        assert!(matches!(t("x").transport, TargetTransport::Local(_)));
        assert!(t("x").enabled);
        for id in ["local", "wsl"] {
            assert!(!t(id).enabled, "{id}");
            assert_eq!(t(id).extra[UNREACHABLE_FIELD], "On my-pc: reachable only by a hub on that PC.");
        }
        assert!(t("own").enabled && !t("own").extra.contains_key(SIGN_IN_FIELD));
        assert!(t("user").enabled && t("user").extra.contains_key(SIGN_IN_FIELD));
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        let slug = ident.slug();
        assert_eq!(names, [format!("data/ssh/keys/{slug}/id_ed25519.pub"), format!("data/ssh/keys/{slug}/id_ed25519"), "data/ssh/known_hosts".into()]);
    }

    #[test]
    fn op_ids_name_a_directory() {
        assert!(op_id_ok("op-1_A"));
        for bad in ["", "a/b", "..", "a b", &"x".repeat(129)] {
            assert!(!op_id_ok(bad), "{bad}");
        }
    }
}
