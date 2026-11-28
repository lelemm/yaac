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
    /// The hub moves into this PC's WSL (docs/15-hub-transfer.md#into-this-pcs-wsl-p416): starts,
    /// detached, the one-shot `yaac wsl-hub install` ([`WslService`]). It waits for this hub to
    /// exit (so no connection to the distro is left, invariant 1) and for `yaacd` there to place
    /// the files, then installs and starts the hub's system service through `wsl.exe -u root`.
    fn install_wsl_service(&self, service: &WslService) -> Result<(), String> {
        let _ = service;
        Err("This build of YAAC cannot run a hub in WSL.".into())
    }
}

/// A hub moving into one of this PC's WSL distros (P416). There it runs as the systemd system
/// service [`SERVICE_UNIT`], which starts it whenever the distro boots. WSL starts a distro on
/// demand (the PC's window attaching through `wsl.exe` does) and stops it when no Windows
/// process uses it; a systemd service does not keep it running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslService {
    /// The distro, as the WSL target names it.
    pub distro: String,
    /// The hub profile's data directory in the distro (`bin/yaac`, `transfer-<op>.result`).
    pub data_dir: String,
    /// The unit file's text ([`system_unit`]).
    pub unit: String,
    /// The transfer whose files the install waits for.
    pub op: OpId,
}

/// `yaac wsl-hub` (installs or removes the hub's system service in a WSL distro).
pub const WSL_HUB_COMMAND: &str = "wsl-hub";

/// The hub's system unit in a WSL distro.
pub const SERVICE_UNIT: &str = "yaac-hub.service";

/// Where the unit goes in the distro.
pub const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

/// The environment keys the hub's service may set (what `yaacd` reports in
/// `HubHostWslService::environment`).
const SERVICE_ENV: &[&str] = &["HOME", "YAAC_CONFIG_DIR", "YAAC_DATA_DIR", "YAAC_RUNTIME_DIR", "YAAC_HOME", "PATH"];

/// A value quoted for a unit directive, including systemd's `%` specifiers (the same rules as
/// `yaacd`'s `systemd --user` unit).
fn unit_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '%' => out.push_str("%%"),
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The hub's system unit for a WSL distro, from what `yaacd` there reports: it runs
/// `<data>/bin/yaac hub` as `yaacd`'s own user with `yaacd`'s profile directories, starts at
/// every boot of the distro (`multi-user.target`), and is restarted only after a crash (a hub
/// that exits cleanly, or finds it has moved, stays stopped). An error is a sentence for the
/// user.
pub fn system_unit(service: &yaac_protocol::HubHostWslService) -> Result<String, String> {
    let user_ok = !service.user.is_empty()
        && service.user.len() <= 64
        && !service.user.starts_with('-')
        && service.user.chars().all(|c| c.is_ascii_alphanumeric() || "._-$".contains(c));
    if !user_ok {
        return Err(format!("The WSL user name {:?} cannot run a system service.", service.user));
    }
    let data = &service.data_dir;
    if !data.starts_with('/') || data.chars().any(char::is_control) {
        return Err(format!("The hub's data directory there ({data:?}) is not an absolute path."));
    }
    let uid = service.uid;
    let mut out = vec![
        "[Unit]".to_owned(),
        "Description=YAAC hub".into(),
        // The hub's socket is under the user's runtime directory, as in a WSL session.
        format!("Wants=user-runtime-dir@{uid}.service"),
        format!("After=user-runtime-dir@{uid}.service network-online.target"),
        "[Service]".into(),
        "Type=exec".into(),
        format!("User={}", service.user),
    ];
    for entry in &service.environment {
        let Some((key, value)) = entry.split_once('=') else { continue };
        if SERVICE_ENV.contains(&key) && !value.is_empty() {
            out.push(format!("Environment={}", unit_value(entry)));
        }
    }
    out.extend([
        format!("ExecStart=:{} hub", unit_value(&format!("{}/bin/yaac", data.trim_end_matches('/')))),
        "Restart=on-failure".into(),
        "RestartSec=5".into(),
        "[Install]".into(),
        "WantedBy=multi-user.target".into(),
        String::new(),
    ]);
    Ok(out.join("\n"))
}

/// The root script that installs the hub's system service: `$1` the unit text, `$2` the hub's
/// data directory, `$3` the transfer, `$4` the unit directory ([`SYSTEM_UNIT_DIR`]). It waits
/// (at most [`PLACED_WAIT_SECS`]) for `yaacd` to have placed the moved files
/// (`<data>/transfer-<op>.result` saying `placed`), writes the unit, and enables and starts it.
pub fn install_script() -> String {
    format!(
        r#"d="$2"; u="$4/{SERVICE_UNIT}"; r="$d/transfer-$3.result"; i=0
while [ ! -f "$r" ]; do
  i=$((i+1)); if [ "$i" -gt {PLACED_WAIT_SECS} ]; then echo "yaac wsl-hub: the moved hub's files were never placed ($r)" >&2; exit 1; fi
  sleep 1
done
if ! grep -Eq '"status"[[:space:]]*:[[:space:]]*"placed"' "$r"; then echo "yaac wsl-hub: the moved hub's files were not placed:" >&2; cat "$r" >&2; exit 1; fi
if [ ! -x "$d/bin/yaac" ]; then echo "yaac wsl-hub: no hub at $d/bin/yaac" >&2; exit 1; fi
umask 022
printf '%s' "$1" > "$u.tmp" && mv -f "$u.tmp" "$u" || exit 1
systemctl daemon-reload || exit 1
systemctl enable --now {SERVICE_UNIT}"#
    )
}

/// The root script that removes the hub's system service (**Start the hub here again**): `$1`
/// the unit directory. Stopping it is SIGTERM, on which the hub stops as on `hub/shutdown`.
/// It fails if the hub's service still runs afterwards.
pub fn remove_script() -> String {
    format!(
        r#"u="$1/{SERVICE_UNIT}"
if [ -e "$u" ]; then
  systemctl disable --now {SERVICE_UNIT} || exit 1
  rm -f "$u"
  systemctl daemon-reload || exit 1
fi
if systemctl is-active --quiet {SERVICE_UNIT}; then echo "yaac wsl-hub: {SERVICE_UNIT} still runs" >&2; exit 1; fi
exit 0"#
    )
}

/// What `yaac wsl-hub install` runs after `wsl.exe`: `-d <distro> -u root --exec sh -c
/// <install_script> sh <unit> <data> <op> /etc/systemd/system`. WSL lets the Windows user be
/// root in their own distros without a password or elevation.
pub fn wsl_service_install_argv(service: &WslService) -> Vec<String> {
    let mut out: Vec<String> = ["-d", &service.distro, "-u", "root", "--exec", "sh", "-c"].map(str::to_owned).into();
    out.extend([install_script(), "sh".into(), service.unit.clone(), service.data_dir.clone(), service.op.as_str().to_owned(), SYSTEM_UNIT_DIR.into()]);
    out
}

/// What **Start the hub here again** runs after `wsl.exe` to remove the hub's service from
/// `distro` (as root, [`remove_script`]).
pub fn wsl_service_remove_argv(distro: &str) -> Vec<String> {
    let mut out: Vec<String> = ["-d", distro, "-u", "root", "--exec", "sh", "-c"].map(str::to_owned).into();
    out.extend([remove_script(), "sh".into(), SYSTEM_UNIT_DIR.into()]);
    out
}

/// `yaac wsl-hub …`, read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WslHubCommand {
    /// `install --distro <d> --data-dir <p> --unit-file <f> --after-transfer <op>`: right after
    /// a move into WSL. `unit_file` is on this PC (the unit text, written by the old hub).
    Install { distro: String, data_dir: String, unit_file: PathBuf, op: OpId },
    /// `remove --distro <d>`.
    Remove { distro: String },
    /// `--distro <d> …`: the login entry of the keeper that ran the hub in WSL before P416.
    /// It does nothing now.
    Legacy,
}

/// The arguments of `yaac wsl-hub install` (after the executable).
pub fn wsl_install_args(service: &WslService, unit_file: &Path) -> Vec<String> {
    vec![
        WSL_HUB_COMMAND.to_owned(),
        "install".into(),
        "--distro".into(),
        service.distro.clone(),
        "--data-dir".into(),
        service.data_dir.clone(),
        "--unit-file".into(),
        unit_file.to_string_lossy().into_owned(),
        "--after-transfer".into(),
        service.op.as_str().to_owned(),
    ]
}

/// Reads `yaac wsl-hub` arguments back (after `wsl-hub`). An error is the usage line.
pub fn parse_wsl_hub_args(args: &[String]) -> Result<WslHubCommand, String> {
    const USAGE: &str = "usage: yaac wsl-hub install --distro <name> --data-dir <path> --unit-file <path> --after-transfer <opId>\n       yaac wsl-hub remove --distro <name>";
    let (verb, rest) = match args.split_first() {
        Some((v, rest)) if v == "install" || v == "remove" => (v.as_str(), rest),
        Some((v, _)) if v == "--distro" => return Ok(WslHubCommand::Legacy),
        _ => return Err(USAGE.into()),
    };
    let (mut distro, mut data_dir, mut unit_file, mut op) = (None, None, None, None);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let value = it.next().filter(|v| !v.is_empty()).ok_or(USAGE)?;
        match flag.as_str() {
            "--distro" => distro = Some(value.clone()),
            "--data-dir" => data_dir = Some(value.clone()),
            "--unit-file" => unit_file = Some(PathBuf::from(value)),
            "--after-transfer" if op_id_ok(value) => op = Some(OpId::new(value.clone())),
            _ => return Err(USAGE.into()),
        }
    }
    match (verb, distro, data_dir, unit_file, op) {
        ("install", Some(distro), Some(data_dir), Some(unit_file), Some(op)) => Ok(WslHubCommand::Install { distro, data_dir, unit_file, op }),
        ("remove", Some(distro), None, None, None) => Ok(WslHubCommand::Remove { distro }),
        _ => Err(USAGE.into()),
    }
}

/// The hub's data directory on a Linux machine (a WSL distro, an SSH server), as `yaacd` there
/// finds it (`YAAC_DATA_DIR`, else `$XDG_DATA_HOME/yaac`, else `~/.local/share/yaac`), in `sh`.
const DISTRO_DATA_DIR: &str = r#"d="${YAAC_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yaac}""#;

/// The `sh` script that runs the moved hub's `yaac hub-bridge` on its machine: the PC's window
/// reaches a hub that moved into WSL or onto an SSH server through it, without Tailscale.
pub fn bridge_script() -> String {
    format!(r#"{DISTRO_DATA_DIR}; exec "$d/bin/yaac" {}"#, crate::interop::HUB_BRIDGE_COMMAND)
}

/// The `sh` script the PC's window runs in a WSL distro to reach the hub there (P416). Starting
/// `wsl.exe` boots the distro when it is stopped, and systemd starts the hub's service with it;
/// `yaac hub-bridge` waits for the hub to listen. A service that is enabled but not running is
/// started only if this user may do so without a password (`--no-ask-password`). When the
/// bridge fails, the service's state is said on standard error (standard output is the attach
/// stream).
pub fn wsl_bridge_script() -> String {
    format!(
        r#"{DISTRO_DATA_DIR}
if systemctl is-enabled --quiet {SERVICE_UNIT} 2>/dev/null && ! systemctl is-active --quiet {SERVICE_UNIT} 2>/dev/null; then
  systemctl start --no-block --no-ask-password {SERVICE_UNIT} >/dev/null 2>&1
fi
"$d/bin/yaac" {bridge} && exit 0
s=$(systemctl is-active {SERVICE_UNIT} 2>/dev/null)
echo "yaac: the hub's service {SERVICE_UNIT} is ${{s:-not installed}} in this distro (systemctl status {SERVICE_UNIT})" >&2
exit 1"#,
        bridge = crate::interop::HUB_BRIDGE_COMMAND
    )
}

/// What the PC's window runs after `wsl.exe` to reach a hub in that distro: `-d <distro> [-u
/// <user>] --exec sh -c <wsl_bridge_script>`.
pub fn wsl_bridge_argv(distro: &str, user: Option<&str>) -> Vec<String> {
    let mut out = vec!["-d".to_owned(), distro.to_owned()];
    if let Some(user) = user {
        out.extend(["-u".into(), user.to_owned()]);
    }
    out.extend(["--exec".into(), "sh".into(), "-c".into(), wsl_bridge_script()]);
    out
}

/// The remote command (one string, for the server's shell) that runs [`bridge_script`] over
/// `ssh`.
pub fn ssh_bridge_remote() -> String {
    format!("exec sh -c '{}'", bridge_script().replace('\'', r"'\''"))
}

/// How long the service install waits for `yaacd` to place the moved hub's files, in seconds.
pub const PLACED_WAIT_SECS: u32 = 120;

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
        assert_eq!(wsl_bridge_argv("Ubuntu", None).last().unwrap(), &wsl_bridge_script());
        // Over ssh the remote shell gets one string; the script survives its quoting.
        let remote = ssh_bridge_remote();
        let out = std::process::Command::new("sh").arg("-c").arg(remote.replace(r#"exec "$d/bin/yaac" hub-bridge"#, r#"echo "$d""#)).env("YAAC_DATA_DIR", "/x y").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/x y");
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", remote.strip_prefix("exec sh -c ").unwrap())).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), script, "single-quoted whole");
    }

    fn service_probe() -> yaac_protocol::HubHostWslService {
        yaac_protocol::HubHostWslService {
            systemd: true,
            user: "me".into(),
            uid: 1000,
            data_dir: "/home/me/.local/share/yaac".into(),
            environment: vec![
                "HOME=/home/me".into(),
                "YAAC_RUNTIME_DIR=/run/user/1000/yaac".into(),
                "PATH=/usr/bin:/mnt/c/Program Files/x%y".into(),
                "LD_PRELOAD=/evil.so".into(),
            ],
        }
    }

    #[test]
    fn the_hubs_system_unit_runs_it_as_the_user_at_every_boot() {
        let unit = system_unit(&service_probe()).unwrap();
        let lines: Vec<&str> = unit.lines().collect();
        for want in [
            "User=me",
            "Wants=user-runtime-dir@1000.service",
            r#"Environment="YAAC_RUNTIME_DIR=/run/user/1000/yaac""#,
            r#"Environment="PATH=/usr/bin:/mnt/c/Program Files/x%%y""#,
            r#"ExecStart=:"/home/me/.local/share/yaac/bin/yaac" hub"#,
            "Restart=on-failure",
            "WantedBy=multi-user.target",
        ] {
            assert!(lines.contains(&want), "{want} in\n{unit}");
        }
        assert!(!unit.contains("LD_PRELOAD"), "only the profile's own variables");
        assert!(unit.ends_with('\n'));
        assert_eq!(unit_value("a\"b\\c\nd"), r#""a\"b\\c\x0ad""#);
        for bad in ["", "root\nExecStartPre=/x", "-x", "a b"] {
            let mut p = service_probe();
            p.user = bad.into();
            assert!(system_unit(&p).is_err(), "{bad:?}");
        }
        let mut p = service_probe();
        p.data_dir = "relative".into();
        assert!(system_unit(&p).is_err());
    }

    #[test]
    fn the_service_is_installed_and_removed_as_root_through_wsl() {
        let service = WslService { distro: "Ubuntu".into(), data_dir: "/home/me/.local/share/yaac".into(), unit: "[Unit]\n".into(), op: OpId::new("op-1") };
        let argv = wsl_service_install_argv(&service);
        assert_eq!(&argv[..7], ["-d", "Ubuntu", "-u", "root", "--exec", "sh", "-c"]);
        assert_eq!(&argv[8..], ["sh", "[Unit]\n", "/home/me/.local/share/yaac", "op-1", SYSTEM_UNIT_DIR]);
        let remove = wsl_service_remove_argv("Ubuntu");
        assert_eq!(&remove[..7], ["-d", "Ubuntu", "-u", "root", "--exec", "sh", "-c"]);
        assert_eq!(&remove[8..], ["sh", SYSTEM_UNIT_DIR]);

        let args = wsl_install_args(&service, Path::new("/tmp/u.service"));
        assert_eq!(args[0], WSL_HUB_COMMAND);
        assert_eq!(
            parse_wsl_hub_args(&args[1..]).unwrap(),
            WslHubCommand::Install { distro: "Ubuntu".into(), data_dir: service.data_dir, unit_file: "/tmp/u.service".into(), op: OpId::new("op-1") }
        );
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(parse_wsl_hub_args(&s(&["remove", "--distro", "Ubuntu"])).unwrap(), WslHubCommand::Remove { distro: "Ubuntu".into() });
        // The keeper's login entry from before P416 does nothing now.
        assert_eq!(parse_wsl_hub_args(&s(&["--distro", "Ubuntu", "--user", "dev"])).unwrap(), WslHubCommand::Legacy);
        for bad in [&[][..], &["install", "--distro", "U"][..], &["remove"][..], &["remove", "--distro", "U", "--after-transfer", "x"][..], &["install", "--distro", "U", "--data-dir", "/d", "--unit-file", "/f", "--after-transfer", "../x"][..]] {
            assert!(parse_wsl_hub_args(&s(bad)).is_err(), "{bad:?}");
        }
    }

    /// Runs a root script with a fake `systemctl` on `PATH` (never the real one).
    #[cfg(unix)]
    fn run_script(script: &str, args: &[&str], bin: &Path) -> (bool, String) {
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let out = std::process::Command::new("sh").arg("-c").arg(script).arg("sh").args(args).env("PATH", path).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    #[cfg(unix)]
    fn fake_systemctl(bin: &Path, log: &Path, active: bool) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(bin).unwrap();
        let fake = bin.join("systemctl");
        let code = if active { 0 } else { 3 };
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1\" in is-active) exit {code};; esac\nexit 0\n", log.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_install_script_waits_for_placed_then_enables_the_service() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let (data, units, bin, log) = (dir.path().join("data"), dir.path().join("units"), dir.path().join("bin"), dir.path().join("log"));
        std::fs::create_dir_all(data.join("bin")).unwrap();
        std::fs::create_dir_all(&units).unwrap();
        fake_systemctl(&bin, &log, false);
        let yaac = data.join("bin/yaac");
        std::fs::write(&yaac, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&yaac, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (d, u) = (data.to_str().unwrap(), units.to_str().unwrap());

        std::fs::write(data.join("transfer-bad.result"), r#"{"opId":"bad","status":"failed","message":"disk full"}"#).unwrap();
        let (ok, err) = run_script(&install_script(), &["[Unit]\n", d, "bad", u], &bin);
        assert!(!ok && err.contains("disk full"), "{err}");
        assert!(!units.join(SERVICE_UNIT).exists() && !log.exists(), "nothing installed");

        std::fs::write(data.join("transfer-ok.result"), r#"{ "opId": "ok", "status": "placed" }"#).unwrap();
        let (ok, err) = run_script(&install_script(), &["[Unit]\nUser=me\n", d, "ok", u], &bin);
        assert!(ok, "{err}");
        assert_eq!(std::fs::read_to_string(units.join(SERVICE_UNIT)).unwrap(), "[Unit]\nUser=me\n");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("daemon-reload\nenable --now {SERVICE_UNIT}\n"));

        // Remove: disabled and stopped, the unit gone; nothing to do the second time.
        std::fs::remove_file(&log).unwrap();
        let (ok, err) = run_script(&remove_script(), &[u], &bin);
        assert!(ok, "{err}");
        assert!(!units.join(SERVICE_UNIT).exists());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("disable --now {SERVICE_UNIT}\ndaemon-reload\nis-active --quiet {SERVICE_UNIT}\n"));
        let (ok, _) = run_script(&remove_script(), &[u], &bin);
        assert!(ok);
        // A hub service that still runs afterwards is a failure.
        fake_systemctl(&bin, &log, true);
        let (ok, err) = run_script(&remove_script(), &[u], &bin);
        assert!(!ok && err.contains("still runs"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn the_wsl_bridge_says_how_the_service_is_when_the_hub_does_not_answer() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let (data, bin, log) = (dir.path().join("data"), dir.path().join("bin"), dir.path().join("log"));
        std::fs::create_dir_all(data.join("bin")).unwrap();
        fake_systemctl(&bin, &log, false);
        let yaac = data.join("bin/yaac");
        std::fs::write(&yaac, "#!/bin/sh\necho 'yaac hub-bridge: no hub' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&yaac, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let out = std::process::Command::new("sh").arg("-c").arg(wsl_bridge_script()).env("PATH", path).env("YAAC_DATA_DIR", &data).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && err.contains(&format!("{SERVICE_UNIT} is")), "{err}");
        assert!(out.stdout.is_empty(), "standard output is the attach stream");
        // Enabled (the fake says so) and not active: a start is tried, without asking.
        assert!(std::fs::read_to_string(&log).unwrap().contains(&format!("start --no-block --no-ask-password {SERVICE_UNIT}")));
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
