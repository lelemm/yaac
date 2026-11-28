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
use yaac_protocol::attach::{LocalTransport, TargetTransport, WindowsInteropTransport};
use yaac_protocol::{OpId, TargetId};

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
    /// This PC's `yaac.exe` as a hub inside one of its WSL distros sees it
    /// (`/mnt/c/…/yaac.exe`), when this hub can move into its own WSL (a Windows hub). `None`:
    /// WSL targets cannot take the hub.
    fn interop_exe(&self) -> Option<String> {
        None
    }
    /// The hub moves into this PC's WSL (docs/15-hub-transfer.md#into-this-pcs-wsl): starts the
    /// keeper (`yaac wsl-hub`, [`keeper_args`]), which waits for this hub to exit and for
    /// `yaacd` to place the files before it starts the new hub, and keeps the distro running.
    fn start_wsl_keeper(&self, keeper: &WslKeeper) -> Result<(), String> {
        let _ = keeper;
        Err("This build of YAAC cannot keep a hub running in WSL.".into())
    }
    /// Replaces this computer's start-at-login of the hub by the keeper's
    /// (`yaac wsl-hub --distro …`, without the transfer).
    fn keeper_autostart(&self, keeper: &WslKeeper) -> Result<(), String> {
        let _ = keeper;
        Err("This build of YAAC cannot keep a hub running in WSL.".into())
    }
}

/// A hub in one of this PC's WSL distros, kept running by `yaac wsl-hub` on Windows (WSL stops
/// a distro that nothing runs in, and `yaacd` there neither starts the hub nor installs a
/// start-at-login entry for it: `hubHost/start {startHub: false}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslKeeper {
    /// The distro, as the WSL target names it.
    pub distro: String,
    /// The Linux user, when not the distro's default.
    pub user: Option<String>,
    /// The transfer whose files the first start waits for (`<data>/transfer-<op>.result`
    /// saying `placed`). `None`: a later start, at login.
    pub op: Option<OpId>,
}

/// `yaac wsl-hub` (the keeper's subcommand).
pub const KEEPER_COMMAND: &str = "wsl-hub";

/// The keeper's arguments after the executable: `wsl-hub --distro <d> [--user <u>]
/// [--after-transfer <op>]`.
pub fn keeper_args(keeper: &WslKeeper) -> Vec<String> {
    let mut out = vec![KEEPER_COMMAND.to_owned(), "--distro".into(), keeper.distro.clone()];
    if let Some(user) = &keeper.user {
        out.extend(["--user".into(), user.clone()]);
    }
    if let Some(op) = &keeper.op {
        out.extend(["--after-transfer".into(), op.as_str().to_owned()]);
    }
    out
}

/// Reads [`keeper_args`] back (after `wsl-hub`). An error is the usage line.
pub fn parse_keeper_args(args: &[String]) -> Result<WslKeeper, String> {
    const USAGE: &str = "usage: yaac wsl-hub --distro <name> [--user <name>] [--after-transfer <opId>]";
    let mut keeper = WslKeeper { distro: String::new(), user: None, op: None };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().filter(|v| !v.is_empty()).ok_or(USAGE)?;
        match flag.as_str() {
            "--distro" => keeper.distro.clone_from(value),
            "--user" => keeper.user = Some(value.clone()),
            "--after-transfer" if op_id_ok(value) => keeper.op = Some(OpId::new(value.clone())),
            _ => return Err(USAGE.into()),
        }
    }
    if keeper.distro.is_empty() {
        return Err(USAGE.into());
    }
    Ok(keeper)
}

/// The hub's data directory on a Linux machine (a WSL distro, an SSH server), as `yaacd` there
/// finds it (`YAAC_DATA_DIR`, else `$XDG_DATA_HOME/yaac`, else `~/.local/share/yaac`), in `sh`.
const DISTRO_DATA_DIR: &str = r#"d="${YAAC_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yaac}""#;

/// The `sh` script that runs the moved hub's `yaac hub-bridge` on its machine: the PC's window
/// reaches a hub that moved into WSL or onto an SSH server through it, without Tailscale.
pub fn bridge_script() -> String {
    format!(r#"{DISTRO_DATA_DIR}; exec "$d/bin/yaac" {}"#, crate::interop::HUB_BRIDGE_COMMAND)
}

/// What the PC's window runs after `wsl.exe` to reach a hub in that distro: `-d <distro> [-u
/// <user>] --exec sh -c <bridge_script>`.
pub fn wsl_bridge_argv(distro: &str, user: Option<&str>) -> Vec<String> {
    let mut out = vec!["-d".to_owned(), distro.to_owned()];
    if let Some(user) = user {
        out.extend(["-u".into(), user.to_owned()]);
    }
    out.extend(["--exec".into(), "sh".into(), "-c".into(), bridge_script()]);
    out
}

/// The remote command (one string, for the server's shell) that runs [`bridge_script`] over
/// `ssh`.
pub fn ssh_bridge_remote() -> String {
    format!("exec sh -c '{}'", bridge_script().replace('\'', r"'\''"))
}

/// How long the first start waits for `yaacd` to place the moved hub's files, in seconds.
pub const PLACED_WAIT_SECS: u32 = 120;

/// What the keeper runs in the distro, after `wsl.exe`: `-d <distro> [-u <user>] --exec sh -c
/// <script> sh [<op>]`. With a transfer, the script first waits (at most
/// [`PLACED_WAIT_SECS`]) for `<data>/transfer-<op>.result` and starts the hub only when it says
/// `placed` (the binary itself is promoted with the other files). Then it `exec`s
/// `<data>/bin/yaac hub`, so the keeper's `wsl.exe` lives exactly as long as the hub.
pub fn keeper_wsl_argv(keeper: &WslKeeper) -> Vec<String> {
    let mut out = vec!["-d".to_owned(), keeper.distro.clone()];
    if let Some(user) = &keeper.user {
        out.extend(["-u".into(), user.clone()]);
    }
    let script = format!(
        r#"{DISTRO_DATA_DIR}
if [ -n "$1" ]; then
  r="$d/transfer-$1.result"; i=0
  while [ ! -f "$r" ]; do
    i=$((i+1)); if [ "$i" -gt {PLACED_WAIT_SECS} ]; then echo "yaac wsl-hub: the moved hub's files were never placed ($r)" >&2; exit 1; fi
    sleep 1
  done
  if ! grep -Eq '"status"[[:space:]]*:[[:space:]]*"placed"' "$r"; then echo "yaac wsl-hub: the moved hub's files were not placed:" >&2; cat "$r" >&2; exit 1; fi
fi
exec "$d/bin/yaac" hub"#
    );
    out.extend(["--exec".into(), "sh".into(), "-c".into(), script, "sh".into()]);
    if let Some(op) = &keeper.op {
        out.push(op.as_str().to_owned());
    }
    out
}

/// What the keeper runs in the distro to stop the hub there (`yaac quit`: `hub/shutdown`,
/// stores checkpointed).
pub fn keeper_quit_argv(keeper: &WslKeeper) -> Vec<String> {
    let mut out = vec!["-d".to_owned(), keeper.distro.clone()];
    if let Some(user) = &keeper.user {
        out.extend(["-u".into(), user.clone()]);
    }
    out.extend(["--exec".into(), "sh".into(), "-c".into(), format!(r#"{DISTRO_DATA_DIR}; exec "$d/bin/yaac" quit"#)]);
    out
}

/// Whether this process runs inside WSL: `WSL_DISTRO_NAME` is set, or the kernel has WSL's
/// interop handler (`/proc/sys/fs/binfmt_misc/WSLInterop`).
pub fn running_in_wsl() -> bool {
    in_wsl(std::env::var_os("WSL_DISTRO_NAME"), Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists() || Path::new("/proc/sys/fs/binfmt_misc/WSLInterop-late").exists())
}

/// [`running_in_wsl`] from its two facts (testable).
pub fn in_wsl(distro_name: Option<std::ffi::OsString>, interop_marker: bool) -> bool {
    cfg!(target_os = "linux") && (distro_name.is_some_and(|d| !d.is_empty()) || interop_marker)
}

/// A Windows path as WSL mounts it (`C:\Users\me\yaac.exe` → `/mnt/c/Users/me/yaac.exe`;
/// WSL's default `automount.root`). `None`: not a drive path.
pub fn windows_to_wsl_path(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let rest = chars.as_str().strip_prefix(':')?;
    if !(rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/')) {
        return None;
    }
    Some(format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest.replace('\\', "/")))
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

/// Where the hub goes, for [`rewrite_settings`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination<'a> {
    /// Another machine (an SSH server): this PC's WSL and Windows targets are unreachable there.
    Remote,
    /// One of this PC's WSL distros; this PC's `yaac.exe` as the distro sees it.
    IntoWsl { interop_exe: &'a str },
    /// This PC's Windows (a `windowsInterop` target, or a Windows target with YAAC installed).
    /// `from_wsl`: this hub runs in that PC's WSL distro, which its new hub reaches as `wsl`.
    ToWindows { from_wsl: Option<&'a str> },
}

/// The new hub's settings (P410), and YAAC's own files to copy with them.
///
/// - `dest` becomes the new hub's local target, with the same id (its threads keep their keys).
/// - [`Destination::IntoWsl`]: the other WSL targets stay as they are (`wsl.exe` runs from
///   inside WSL), and this PC's local target becomes [`TargetTransport::WindowsInterop`]
///   (`yaac.exe local-daemon`, through interop).
/// - [`Destination::ToWindows`]: WSL targets stay, this hub's own local target becomes the
///   `wsl` target of its distro (when this hub runs in WSL; else it is unreachable).
/// - Other SSH targets stay. One YAAC set up with its own key (and the key is here) keeps it:
///   the key is copied. One that used the user's own key or agent is marked
///   [`SIGN_IN_FIELD`]; the user's keys are never copied.
/// - WSL targets and this computer's local target cannot be reached from the new machine: they
///   stay, disabled, marked [`UNREACHABLE_FIELD`].
/// - `hubId` and `clientId` are kept; attaching over Tailscale is turned on (every client is
///   remote now).
pub fn rewrite_settings(settings: &Settings, dest: &TargetId, this_host: &str, ssh_dir: Option<&Path>, to: &Destination<'_>) -> (Settings, Vec<MovedFile>) {
    let interop_exe = match to {
        Destination::IntoWsl { interop_exe } => Some(*interop_exe),
        _ => None,
    };
    let to_windows = matches!(to, Destination::ToWindows { .. });
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
            TargetTransport::Wsl(_) | TargetTransport::WindowsInterop(_) if interop_exe.is_some() => {}
            TargetTransport::Local(_) if let Some(exe) = interop_exe => {
                t.transport = TargetTransport::WindowsInterop(WindowsInteropTransport { exe: exe.to_owned() });
            }
            TargetTransport::Wsl(_) if to_windows => {}
            TargetTransport::Local(_) if let Destination::ToWindows { from_wsl: Some(distro) } = to => {
                t.transport = TargetTransport::Wsl(yaac_protocol::attach::WslTransport { distro: (*distro).to_owned(), user: None });
            }
            TargetTransport::Wsl(_) | TargetTransport::Local(_) | TargetTransport::WindowsInterop(_) => {
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

        let (new, files) = rewrite_settings(&settings, &TargetId::new("x"), "my-pc", Some(&ssh_dir), &Destination::Remote);
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
    fn into_this_pcs_wsl_the_other_distros_and_windows_stay_reachable() {
        let mut settings = Settings::first_run(false, true);
        settings.targets = vec![
            def("local", TargetTransport::Local(LocalTransport {})),
            def("ubuntu", serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Ubuntu" })).unwrap()),
            def("debian", serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Debian", "user": "dev" })).unwrap()),
            def("box", ssh("box")),
        ];
        let exe = "/mnt/c/Users/me/AppData/Local/Programs/YAAC/yaac.exe";
        let (new, files) = rewrite_settings(&settings, &TargetId::new("ubuntu"), "my-pc", None, &Destination::IntoWsl { interop_exe: exe });
        let t = |id: &str| new.targets.iter().find(|t| t.id.as_str() == id).unwrap();
        assert!(matches!(t("ubuntu").transport, TargetTransport::Local(_)), "the destination is the new hub's own");
        assert_eq!(t("local").transport, TargetTransport::WindowsInterop(WindowsInteropTransport { exe: exe.into() }));
        assert_eq!(t("debian").transport, settings.targets[2].transport, "wsl.exe works from inside WSL");
        for id in ["local", "ubuntu", "debian", "box"] {
            assert!(t(id).enabled, "{id}");
            assert!(!t(id).extra.contains_key(UNREACHABLE_FIELD), "{id}");
        }
        assert!(t("box").extra.contains_key(SIGN_IN_FIELD), "SSH is unchanged: no key of YAAC's here");
        assert!(files.is_empty());
        assert!(new.hub.tailscale_attach);
    }

    #[test]
    fn back_to_this_pcs_windows_the_distro_becomes_a_wsl_target_again() {
        let mut settings = Settings::first_run(false, true);
        settings.targets = vec![
            def("ubuntu", TargetTransport::Local(LocalTransport {})),
            def("windows", TargetTransport::WindowsInterop(WindowsInteropTransport { exe: "/mnt/c/YAAC/yaac.exe".into() })),
            def("debian", serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Debian" })).unwrap()),
            def("box", ssh("box")),
        ];
        let (new, _) = rewrite_settings(&settings, &TargetId::new("windows"), "my-pc", None, &Destination::ToWindows { from_wsl: Some("Ubuntu") });
        let t = |id: &str| new.targets.iter().find(|t| t.id.as_str() == id).unwrap();
        assert!(matches!(t("windows").transport, TargetTransport::Local(_)), "Windows is the new hub's own");
        assert_eq!(t("ubuntu").transport, TargetTransport::Wsl(yaac_protocol::attach::WslTransport { distro: "Ubuntu".into(), user: None }));
        assert_eq!(t("debian").transport, settings.targets[2].transport);
        for id in ["ubuntu", "windows", "debian", "box"] {
            assert!(t(id).enabled && !t(id).extra.contains_key(UNREACHABLE_FIELD), "{id}");
        }
        // From an SSH server: its own local target is not reachable from Windows.
        let (new, _) = rewrite_settings(&settings, &TargetId::new("windows"), "box", None, &Destination::ToWindows { from_wsl: None });
        assert!(!new.targets[0].enabled && new.targets[0].extra.contains_key(UNREACHABLE_FIELD));
    }

    #[test]
    fn the_pcs_window_reaches_a_moved_hub_through_its_bridge() {
        let script = bridge_script();
        assert!(script.ends_with(r#"exec "$d/bin/yaac" hub-bridge"#), "{script}");
        assert_eq!(wsl_bridge_argv("Ubuntu", Some("dev"))[..7], ["-d", "Ubuntu", "-u", "dev", "--exec", "sh", "-c"]);
        assert_eq!(wsl_bridge_argv("Ubuntu", None).last().unwrap(), &script);
        // Over ssh the remote shell gets one string; the script survives its quoting.
        let remote = ssh_bridge_remote();
        let out = std::process::Command::new("sh").arg("-c").arg(remote.replace(r#"exec "$d/bin/yaac" hub-bridge"#, r#"echo "$d""#)).env("YAAC_DATA_DIR", "/x y").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/x y");
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", remote.strip_prefix("exec sh -c ").unwrap())).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), script, "single-quoted whole");
    }

    #[test]
    fn the_keeper_runs_the_moved_hub_in_its_distro() {
        let keeper = WslKeeper { distro: "Ubuntu".into(), user: Some("me".into()), op: Some(OpId::new("op-1")) };
        let args = keeper_args(&keeper);
        assert_eq!(args, ["wsl-hub", "--distro", "Ubuntu", "--user", "me", "--after-transfer", "op-1"]);
        assert_eq!(parse_keeper_args(&args[1..]).unwrap(), keeper);
        let login = WslKeeper { user: None, op: None, ..keeper.clone() };
        assert_eq!(keeper_args(&login), ["wsl-hub", "--distro", "Ubuntu"]);
        assert_eq!(parse_keeper_args(&keeper_args(&login)[1..]).unwrap(), login);
        for bad in [&[][..], &["--distro".to_owned()][..], &["--user".to_owned(), "me".to_owned()][..], &["--distro".to_owned(), "U".to_owned(), "--after-transfer".to_owned(), "../x".to_owned()][..]] {
            assert!(parse_keeper_args(bad).is_err(), "{bad:?}");
        }

        let argv = keeper_wsl_argv(&keeper);
        assert_eq!(&argv[..6], ["-d", "Ubuntu", "-u", "me", "--exec", "sh"]);
        assert_eq!(argv[6], "-c");
        assert_eq!(&argv[8..], ["sh", "op-1"], "the transfer is $1");
        let script = &argv[7];
        assert!(script.contains(r#"d="${YAAC_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yaac}""#), "yaacd's own data dir");
        assert!(script.contains(r#"r="$d/transfer-$1.result""#));
        assert!(script.contains(&format!(r#"-gt {PLACED_WAIT_SECS} ]"#)));
        assert!(script.contains(r#""status"[[:space:]]*:[[:space:]]*"placed""#));
        assert!(script.trim_end().ends_with(r#"exec "$d/bin/yaac" hub"#), "wsl.exe lives as long as the hub");
        // At login there is no transfer to wait for.
        let argv = keeper_wsl_argv(&login);
        assert_eq!(argv.last().unwrap(), "sh");
        assert_eq!(&argv[..2], ["-d", "Ubuntu"]);
        let quit = keeper_quit_argv(&keeper);
        assert_eq!(&quit[..7], ["-d", "Ubuntu", "-u", "me", "--exec", "sh", "-c"]);
        assert!(quit[7].ends_with(r#"exec "$d/bin/yaac" quit"#));
    }

    #[test]
    fn the_shell_script_waits_for_placed_then_execs_the_hub() {
        // Run the script with a fake data dir: `placed` → the hub is exec'd; `failed` → not.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let yaac = bin.join("yaac");
        std::fs::write(&yaac, "#!/bin/sh\necho \"ran $*\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&yaac, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = |op: &str| {
            let argv = keeper_wsl_argv(&WslKeeper { distro: "U".into(), user: None, op: Some(OpId::new(op)) });
            let out = std::process::Command::new(&argv[3]).args(&argv[4..]).env("YAAC_DATA_DIR", dir.path()).output().unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
        };
        std::fs::write(dir.path().join("transfer-ok.result"), r#"{ "opId": "ok", "status": "placed" }"#).unwrap();
        let (ok, out, _) = run("ok");
        assert!(ok && out.trim() == "ran hub", "{out}");
        std::fs::write(dir.path().join("transfer-bad.result"), r#"{"opId":"bad","status":"failed","message":"disk full"}"#).unwrap();
        let (ok, out, err) = run("bad");
        assert!(!ok && out.is_empty() && err.contains("disk full"), "{out} / {err}");
    }

    #[test]
    fn windows_paths_as_wsl_mounts_them() {
        assert_eq!(windows_to_wsl_path(r"C:\Users\me\AppData\Local\Programs\YAAC\yaac.exe").as_deref(), Some("/mnt/c/Users/me/AppData/Local/Programs/YAAC/yaac.exe"));
        assert_eq!(windows_to_wsl_path(r"\\?\D:\YAAC\yaac.exe").as_deref(), Some("/mnt/d/YAAC/yaac.exe"));
        for bad in [r"\\server\share\yaac.exe", "/usr/bin/yaac", "yaac.exe", "C:yaac.exe"] {
            assert_eq!(windows_to_wsl_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn wsl_is_told_by_its_distro_name_or_interop() {
        let linux = cfg!(target_os = "linux");
        assert_eq!(in_wsl(Some("Ubuntu".into()), false), linux);
        assert_eq!(in_wsl(None, true), linux);
        assert!(!in_wsl(None, false));
        assert!(!in_wsl(Some("".into()), false));
    }

    #[test]
    fn op_ids_name_a_directory() {
        assert!(op_id_ok("op-1_A"));
        for bad in ["", "a/b", "..", "a b", &"x".repeat(129)] {
            assert!(!op_id_ok(bad), "{bad}");
        }
    }
}
