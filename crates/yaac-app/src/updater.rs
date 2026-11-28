//! The in-app updater (D136, docs/install.md#updating). Windows only; a build for any other
//! system compiles it but never starts it, so it never offers an update.
//!
//! - **At start**, before the window exists: a verified pending update for a newer build is
//!   installed instead of starting ([`at_start`]); the decision table is
//!   [`yaac_core::update::decide_start`].
//! - **As the window opens and every ten minutes while it runs**, on a worker thread:
//!   one `GET` of the repository's latest GitHub release, then its `manifest.json` ([`check`]).
//!   Network failures are silent. Checks pause while downloading or waiting for restart.
//! - **On request**, on a worker thread: the installer is downloaded into the updates folder,
//!   its size and SHA-256 checked against the manifest, and only then `pending.json` written
//!   ([`download`]).
//! - **"Restart to update"**: the installer shows progress and runs with `/RELAUNCH` ([`run_installer`])
//!   and the window quits; the installer stops the window and the hub itself (`yaac quit`),
//!   starts the hub again, and then starts YAAC.
//!
//! Nothing here reaches a target: targets get the new `yaacd` from the new hub, which carries
//! it (docs/install.md#how-targets-get-the-new-daemon). No personal data is sent: the request
//! carries a generic User-Agent and nothing else.

use std::io::Read;
#[cfg(feature = "desktop")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
#[cfg(feature = "desktop")]
use yaac_core::update::Release;
use yaac_core::update::{CheckSchedule, Manifest, Offer, Pending, PendingRead, ReleaseCheck, StartDecision, Throttle, decide_start, next_check, offer};

/// Whether this build offers updates at all.
pub(crate) const ENABLED: bool = cfg!(all(windows, feature = "desktop"));

/// The latest release of the public repository.
pub(crate) const LATEST_URL: &str = "https://api.github.com/repos/lelemm/yaac/releases/latest";

/// Sent with every request: generic, nothing about the user or the machine.
const USER_AGENT: &str = "yaac-updater";

/// The verified update waiting to be installed.
const PENDING: &str = "pending.json";

/// Where the installer writes its log when the updater runs it.
const INSTALL_LOG: &str = "install.log";

/// Where to look and where to keep downloads. Production uses [`Config::github`]; tests point
/// `latest_url` at a local server.
#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub latest_url: String,
    /// The updates folder (`<data>\updates`, YAAC's own).
    pub dir: PathBuf,
    /// Refuse anything but HTTPS (off only in tests, against a local server).
    pub https_only: bool,
    /// For the release check: connecting, and waiting for each answer.
    pub timeout: Duration,
}

impl Config {
    pub(crate) fn github(dir: PathBuf) -> Self {
        Self { latest_url: LATEST_URL.to_owned(), dir, https_only: true, timeout: Duration::from_secs(10) }
    }
}

#[cfg(feature = "desktop")]
fn agent(cfg: &Config, body_timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .https_only(cfg.https_only)
        .timeout_connect(Some(cfg.timeout))
        .timeout_recv_response(Some(cfg.timeout))
        // A stalled transfer fails instead of hanging the worker for good.
        .timeout_per_read(Some(cfg.timeout.max(Duration::from_secs(30))))
        .timeout_recv_body(body_timeout)
        .build()
        .into()
}

/// Why a check found nothing. Shown only in Settings → About → Components.
#[cfg(feature = "desktop")]
fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "no answer from GitHub (timed out)".into(),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => "GitHub could not be reached (offline?)".into(),
        other => format!("the release check failed: {other}"),
    }
}

#[derive(Debug)]
struct CheckError {
    message: String,
    reset_at: Option<SystemTime>,
    retry_after: Duration,
}

impl From<String> for CheckError {
    fn from(message: String) -> Self {
        Self { message, reset_at: None, retry_after: Duration::ZERO }
    }
}

impl CheckError {
    fn rate_limit_delay(&self, checked_at: SystemTime) -> Duration {
        let until_reset = self.reset_at.and_then(|at| at.duration_since(checked_at).ok()).unwrap_or_default();
        until_reset.max(self.retry_after)
    }
}

/// The text of a small JSON answer, or why there is none.
#[cfg(feature = "desktop")]
fn get_text(agent: &ureq::Agent, url: &str, accept: &str) -> Result<String, CheckError> {
    let mut resp = agent.get(url).header("Accept", accept).call().map_err(|e| describe(&e))?;
    let status = resp.status().as_u16();
    let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok());
    let seconds = |name| header(name).and_then(|s| s.trim().parse().ok()).map(Duration::from_secs);
    let retry_after = seconds("retry-after");
    if status == 429 || (status == 403 && (header("x-ratelimit-remaining") == Some("0") || retry_after.is_some())) {
        return Err(CheckError {
            message: "GitHub's rate limit was reached".into(),
            reset_at: seconds("x-ratelimit-reset").and_then(|s| SystemTime::UNIX_EPOCH.checked_add(s)),
            retry_after: retry_after.unwrap_or_default(),
        });
    }
    if status == 404 {
        return Err("no release has been published yet".to_owned().into());
    }
    if status != 200 {
        return Err(format!("GitHub answered HTTP {status}").into());
    }
    resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| describe(&e).into())
}

/// What the check found: the latest release's manifest, and the installer's download URL
/// (the release's own asset URL when it lists one by that name, else the manifest's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub manifest: Manifest,
    pub installer_url: String,
}

/// The latest release and its manifest. Blocking: run it on a worker.
#[cfg(feature = "desktop")]
fn check(cfg: &Config) -> Result<Found, CheckError> {
    let agent = agent(cfg, Some(cfg.timeout * 2));
    let release = Release::parse(&get_text(&agent, &cfg.latest_url, "application/vnd.github+json")?)?;
    let url = release.asset_url(yaac_core::update::MANIFEST_NAME).ok_or_else(|| format!("release {} has no manifest", release.tag_name))?;
    let manifest = Manifest::parse(&get_text(&agent, url, "application/json")?)?;
    let installer = manifest.installer().ok_or_else(|| "the release has no installer".to_owned())?;
    let installer_url = release.asset_url(&installer.name).unwrap_or(&installer.url).to_owned();
    Ok(Found { manifest, installer_url })
}

#[cfg(not(feature = "desktop"))]
fn check(_: &Config) -> Result<Found, CheckError> {
    Err(NO_CLIENT.to_owned().into())
}

/// The release tag of a build the workflow tagged itself: `0.1.0+1060` → `v0.1.0-build.1060`
/// (docs/install.md#releases). `None` unless it is a release build whose version is plain
/// digits, letters, `.` and `-` (it goes into a URL).
pub(crate) fn release_tag(build: &str) -> Option<String> {
    let (version, id) = build.split_once('+')?;
    let plain = version.starts_with(|c: char| c.is_ascii_digit()) && version.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    (plain && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| format!("v{version}-build.{id}"))
}

/// The release of exactly `build`, with its manifest and installer (the phone's request,
/// [`HubInstaller`]). Blocking: run it on a worker.
#[cfg(feature = "desktop")]
fn find_build(cfg: &Config, build: &str) -> Result<Found, String> {
    let tag = release_tag(build).ok_or_else(|| format!("{build} is not a YAAC release build."))?;
    let base = cfg.latest_url.strip_suffix("/latest").ok_or("the release address is not GitHub's")?;
    let agent = agent(cfg, Some(cfg.timeout * 2));
    let release = match get_text(&agent, &format!("{base}/tags/{tag}"), "application/vnd.github+json") {
        Err(e) if e.message.contains("no release") => return Err(format!("There is no published release of YAAC {build}.")),
        other => Release::parse(&other.map_err(|e| e.message)?)?,
    };
    let url = release.asset_url(yaac_core::update::MANIFEST_NAME).ok_or_else(|| format!("release {tag} has no manifest"))?;
    let manifest = Manifest::parse(&get_text(&agent, url, "application/json").map_err(|e| e.message)?)?;
    if manifest.label() != build {
        return Err(format!("release {tag} is YAAC {}, not {build}", manifest.label()));
    }
    let installer = manifest.installer().ok_or_else(|| format!("release {tag} has no installer"))?;
    let installer_url = release.asset_url(&installer.name).unwrap_or(&installer.url).to_owned();
    Ok(Found { manifest, installer_url })
}

#[cfg(not(feature = "desktop"))]
fn find_build(_: &Config, _: &str) -> Result<Found, String> {
    Err(NO_CLIENT.to_owned())
}

// ── The hub's side: a newer phone asks for its build (docs/install.md#updating-from-the-phone) ──

/// What the hub runs for `hub/update/request` on Windows: the same release lookup, download,
/// SHA-256 check and installer as the window's updater, into the same updates folder, with the
/// installer unattended. The hub calls it on a thread of its own.
#[cfg(feature = "hub")]
pub(crate) struct HubInstaller {
    cfg: Config,
    /// The verified download, between [`yaac_hub::host_update::HostInstaller::download`] and
    /// `install`.
    pending: std::sync::Mutex<Option<Pending>>,
}

#[cfg(feature = "hub")]
impl HubInstaller {
    pub(crate) fn new(cfg: Config) -> Self {
        Self { cfg, pending: std::sync::Mutex::new(None) }
    }
}

#[cfg(feature = "hub")]
impl yaac_hub::host_update::HostInstaller for HubInstaller {
    fn find(&self, build: &str) -> Result<yaac_hub::host_update::HostRelease, String> {
        let found = find_build(&self.cfg, build)?;
        let size = found.manifest.installer().map_or(0, |i| i.size);
        Ok(yaac_hub::host_update::HostRelease { build: found.manifest.label(), size, detail: Box::new(found) })
    }

    fn download(&self, release: &yaac_hub::host_update::HostRelease, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        let found = release.detail.downcast_ref::<Found>().ok_or("not a release this updater found")?;
        let p = download(&self.cfg, found, progress)?;
        *self.pending.lock().map_err(|_| "the updater failed")? = Some(p);
        Ok(())
    }

    fn install(&self, release: &yaac_hub::host_update::HostRelease) -> Result<(), String> {
        let p = self.pending.lock().map_err(|_| "the updater failed")?.take().ok_or("nothing is downloaded")?;
        if p.version != release.build || !verify(&p) {
            discard(&self.cfg.dir);
            return Err("the downloaded installer changed on disk; it was deleted".into());
        }
        run_installer_as(&self.cfg.dir, &p, InstallerUi::Unattended)
    }
}

/// What the hub runs to move to another machine (`hub/transfer/*`, docs/15-hub-transfer.md):
/// the Linux hub of exactly this build, from the same release and manifest the updater reads,
/// and this computer's start-at-login turned off once the hub has moved.
#[cfg(feature = "hub")]
pub(crate) struct HubTransfer {
    cfg: Config,
}

#[cfg(feature = "hub")]
impl HubTransfer {
    pub(crate) fn new(cfg: Config) -> Self {
        Self { cfg }
    }
}

#[cfg(feature = "hub")]
impl yaac_hub::transfer::TransferHost for HubTransfer {
    fn hub_asset(&self, build: &str, arch: &str) -> Result<yaac_hub::transfer::HubAsset, String> {
        let found = find_build(&self.cfg, build)?;
        let name = yaac_hub::transfer::hub_asset_name(arch);
        let asset = found.manifest.asset(&name).ok_or_else(|| format!("YAAC {build} has no hub for {arch} ({name} is not in its release)."))?;
        Ok(yaac_hub::transfer::HubAsset { url: asset.url.clone(), sha256: asset.sha256.clone(), size: asset.size })
    }

    fn stop_autostart(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let entry = yaac_hub::autostart::Autostart::from_env(exe).map_err(|e| e.to_string())?;
        yaac_hub::autostart::remove_autostart(&entry).map_err(|e| e.to_string())
    }

    /// On Windows only: `yaac.exe` as this PC's WSL distros see it.
    fn interop_exe(&self) -> Option<String> {
        if !cfg!(windows) {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        yaac_hub::transfer::windows_to_wsl_path(&exe.to_string_lossy())
    }

    fn install_wsl_service(&self, service: &yaac_hub::transfer::WslService) -> Result<(), String> {
        if !cfg!(windows) {
            return Err("only YAAC for Windows sets up a hub in WSL".into());
        }
        crate::wsl_service::start_install(service)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// SHA-256 of a file, and its size.
fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex(&h.finalize()), n))
}

/// Writes `pending.json` through a temporary file, so a crash leaves the old one or none.
fn write_pending(dir: &Path, p: &Pending) -> std::io::Result<()> {
    let tmp = dir.join("pending.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(p).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, dir.join(PENDING))
}

/// What is in the updates folder.
pub(crate) fn read_pending(dir: &Path) -> PendingRead {
    let text = match std::fs::read(dir.join(PENDING)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PendingRead::Missing,
        Err(e) => return PendingRead::Invalid(e.to_string()),
    };
    match serde_json::from_slice::<Pending>(&text) {
        // Only an installer in the updates folder itself is ever run.
        Ok(p) if p.path.parent() != Some(dir) => PendingRead::Invalid(format!("{} is not in the updates folder", p.path.display())),
        Ok(p) => PendingRead::Found(p),
        Err(e) => PendingRead::Invalid(e.to_string()),
    }
}

/// Whether the installer on disk is still the one that was verified.
pub(crate) fn verify(p: &Pending) -> bool {
    hash_file(&p.path).is_ok_and(|(sha, size)| size == p.size && sha == p.sha256)
}

/// Deletes the pending update and every download in the updates folder (the install log
/// stays, for a failure to be looked into).
pub(crate) fn discard(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name != INSTALL_LOG {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A pending update already downloaded and verified for this release, if there is one.
fn already_downloaded(dir: &Path, m: &Manifest) -> Option<Pending> {
    match read_pending(dir) {
        PendingRead::Found(p) if p.build == m.build && !p.attempted && m.installer().is_some_and(|i| i.sha256 == p.sha256) && verify(&p) => Some(p),
        _ => None,
    }
}

/// Downloads the installer into the updates folder, checking it against the manifest as it
/// comes in, and writes `pending.json` once it matches. `progress(done, total)` is called for
/// every chunk. Blocking: run it on a worker.
pub(crate) fn download(cfg: &Config, found: &Found, progress: &mut dyn FnMut(u64, u64)) -> Result<Pending, String> {
    let installer = found.manifest.installer().ok_or("the release has no installer")?;
    if cfg.https_only && !found.installer_url.starts_with("https://") {
        return Err("the installer's address is not HTTPS".into());
    }
    std::fs::create_dir_all(&cfg.dir).map_err(|e| format!("cannot create {}: {e}", cfg.dir.display()))?;
    // One download at a time in the folder: anything earlier goes.
    discard(&cfg.dir);
    let path = cfg.dir.join(&installer.name);
    let part = cfg.dir.join(format!("{}.part", installer.name));
    let result = (|| {
        let sha256 = fetch(cfg, &found.installer_url, installer.size, &part, progress)?;
        if sha256 != installer.sha256 {
            return Err("the download does not match the release's SHA-256".into());
        }
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        let pending = Pending { build: found.manifest.build, version: found.manifest.label(), path: path.clone(), sha256: installer.sha256.clone(), size: installer.size, attempted: false };
        write_pending(&cfg.dir, &pending).map_err(|e| format!("cannot write {PENDING}: {e}"))?;
        Ok(pending)
    })();
    if result.is_err() {
        discard(&cfg.dir);
    }
    result
}

/// Downloads `url` into `part` (at most `size` bytes, exactly `size` expected) and returns its
/// SHA-256 in hex.
#[cfg(feature = "desktop")]
fn fetch(cfg: &Config, url: &str, size: u64, part: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    let mut resp = agent(cfg, None).get(url).call().map_err(|e| describe(&e))?;
    if resp.status().as_u16() != 200 {
        return Err(format!("the download answered HTTP {}", resp.status().as_u16()));
    }
    let mut body = resp.body_mut().with_config().limit(size.saturating_add(1)).reader();
    let mut file = std::fs::File::create(part).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut h = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    progress(0, size);
    loop {
        let k = body.read(&mut buf).map_err(|e| format!("the download stopped: {e}"))?;
        if k == 0 {
            break;
        }
        done += k as u64;
        if done > size {
            return Err("the download is larger than the release says".into());
        }
        h.update(&buf[..k]);
        file.write_all(&buf[..k]).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        progress(done, size);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if done != size {
        return Err(format!("the download ended early ({done} of {size} bytes)"));
    }
    Ok(hex(&h.finalize()))
}

/// A build without the desktop pieces has no HTTPS client: it never offers an update
/// ([`ENABLED`] is false there), so this only answers why.
#[cfg(not(feature = "desktop"))]
fn fetch(_: &Config, _: &str, _: u64, _: &Path, _: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    Err(NO_CLIENT.into())
}

#[cfg(not(feature = "desktop"))]
const NO_CLIENT: &str = "this build has no updater";

/// How the installer is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallerUi {
    /// "Restart to update" in the window: Setup shows its progress (`/SILENT`).
    Progress,
    /// The hub, asked by the phone: nobody may be at the computer (`/VERYSILENT`).
    Unattended,
}

/// The installer's arguments: silent, no reboot, and `/RELAUNCH`, which starts YAAC (the
/// window, and with it the hub) once setup has finished (installer/yaac.iss).
pub(crate) fn installer_args(ui: InstallerUi, log: &Path) -> Vec<String> {
    let quiet = match ui {
        InstallerUi::Progress => "/SILENT",
        InstallerUi::Unattended => "/VERYSILENT",
    };
    [quiet, "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/RELAUNCH"].into_iter().map(str::to_owned).chain([format!("/LOG={}", log.display())]).collect()
}

/// Starts the installer with visible progress, to start YAAC again when it is done (`/RELAUNCH`), and
/// returns at once. It is marked attempted first, so a failed install is never run again.
pub(crate) fn run_installer(dir: &Path, p: &Pending) -> Result<(), String> {
    run_installer_as(dir, p, InstallerUi::Progress)
}

fn run_installer_as(dir: &Path, p: &Pending, ui: InstallerUi) -> Result<(), String> {
    write_pending(dir, &Pending { attempted: true, ..p.clone() }).map_err(|e| format!("cannot write {PENDING}: {e}"))?;
    let mut cmd = std::process::Command::new(&p.path);
    cmd.args(installer_args(ui, &dir.join(INSTALL_LOG))).current_dir(dir);
    #[cfg(windows)]
    {
        // Out of the window's job when the job allows it: the installer outlives this window.
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        if cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(0);
    }
    cmd.spawn().map(drop).map_err(|e| format!("could not start the installer: {e}"))
}

/// What the start of the window does about updates.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AtStart {
    /// Start the window; the message (an update that did not install) is shown once.
    Continue(Option<String>),
    /// The installer was started: exit without opening the window.
    Exit,
}

/// Run before the window opens: installs a verified pending update for a newer build instead
/// of starting, and cleans up one that is spent, older, or not intact.
pub(crate) fn at_start(dir: &Path, running: &str) -> AtStart {
    match decide_start(running, read_pending(dir), verify) {
        StartDecision::Start => AtStart::Continue(None),
        StartDecision::Discard(why) => {
            eprintln!("yaac: dropping the downloaded update: {why}");
            discard(dir);
            AtStart::Continue(None)
        }
        StartDecision::Failed(msg) => {
            discard(dir);
            AtStart::Continue(Some(format!("{msg} The installer's log is {}.", dir.join(INSTALL_LOG).display())))
        }
        StartDecision::Install(p) => match run_installer(dir, &p) {
            Ok(()) => AtStart::Exit,
            Err(e) => {
                discard(dir);
                AtStart::Continue(Some(format!("The update to YAAC {} could not be installed: {e}", p.version)))
            }
        },
    }
}

// ── The window's side ───────────────────────────────────────────────────────────────────────

/// Where the update stands, for the title row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Nothing to offer.
    Idle,
    /// A newer release: the update icon.
    Available(Found),
    /// The mini progress bar.
    Downloading { found: Found, done: u64, total: u64 },
    /// Verified and waiting: "Restart to update".
    Ready(Pending),
    /// The download failed; clicking tries again.
    Failed { found: Found, message: String },
}

#[derive(Clone, Copy)]
pub(crate) enum CaptionAction {
    Download,
    Install,
}

pub(crate) struct Caption {
    pub tooltip: String,
    pub action: Option<CaptionAction>,
    pub progress: f64,
}

impl Phase {
    pub(crate) fn caption(&self) -> Option<Caption> {
        let (tooltip, action, progress) = match self {
            Self::Idle => return None,
            Self::Available(found) => (format!("YAAC {} is available · Download", found.manifest.label()), Some(CaptionAction::Download), 0.0),
            Self::Downloading { found, done, total } => {
                let progress = if *total == 0 { 0.0 } else { (*done as f64 / *total as f64).clamp(0.0, 1.0) };
                let percent = done.saturating_mul(100).checked_div(*total).unwrap_or(0).min(100);
                (format!("Downloading YAAC {} · {percent}%", found.manifest.label()), None, progress)
            }
            Self::Ready(pending) => (format!("YAAC {} is downloaded · Restart to update", pending.version), Some(CaptionAction::Install), 0.0),
            Self::Failed { found, message } => (format!("The download of YAAC {} failed: {message} · Try again", found.manifest.label()), Some(CaptionAction::Download), 0.0),
        };
        Some(Caption { tooltip, action, progress })
    }
}

enum Msg {
    Checked(Result<Found, CheckError>, Option<Pending>, SystemTime),
    Progress(u64, u64),
    Downloaded(Result<Pending, String>),
    RefreshDeferred,
}

enum Request {
    Refresh,
    Download(Found),
}

/// The window's updater: what the title row and Settings show, and the workers behind them.
pub(crate) struct Updater {
    cfg: Option<Config>,
    pub check: ReleaseCheck,
    pub last_checked: Option<SystemTime>,
    pub phase: Phase,
    /// An update that did not install, shown once in a dialog.
    pub notice: Option<String>,
    build: &'static str,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    requests: Option<mpsc::Sender<Request>>,
    pub(crate) refreshing: bool,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater").field("check", &self.check).field("phase", &self.phase).finish_non_exhaustive()
    }
}

impl Updater {
    /// An updater that never checks, and why (shown in Components).
    pub(crate) fn off(why: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { cfg: None, check: ReleaseCheck::Off(why.to_owned()), last_checked: None, phase: Phase::Idle, notice: None, build: crate::BUILD, tx, rx, requests: None, refreshing: false }
    }

    /// An updater for this build that will check `cfg` once the window is up ([`Self::start`]).
    pub(crate) fn new(cfg: Config, build: &'static str, notice: Option<String>) -> Self {
        let mut u = Self::off("");
        if yaac_core::update::build_number(build).is_none() {
            u.check = ReleaseCheck::Off("this is a development build; it is not updated from releases".into());
        } else {
            u.cfg = Some(cfg);
            u.check = ReleaseCheck::Checking;
        }
        u.build = build;
        u.notice = notice;
        u
    }

    /// Starts the updater worker. It waits until a check is due or a download is requested.
    pub(crate) fn start(&mut self, ctx: &egui::Context) {
        self.start_with_clock(ctx, Instant::now);
    }

    fn start_with_clock(&mut self, ctx: &egui::Context, now: impl FnMut() -> Instant + Send + 'static) {
        let Some(cfg) = self.cfg.clone() else { return };
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let build = self.build;
        let (send, requests) = mpsc::channel();
        match std::thread::Builder::new().name("yaac-updater".into()).spawn(move || {
            run_worker(&cfg, build, &requests, now, |msg| {
                if tx.send(msg).is_ok() {
                    ctx.request_repaint();
                }
            });
        }) {
            Ok(_) => self.requests = Some(send),
            Err(e) => self.check = ReleaseCheck::Unavailable(format!("could not start the update worker: {e}")),
        }
    }

    /// Takes the worker's results, including while the window is in the tray.
    pub(crate) fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Checked(result, ready, checked_at) => {
                    self.refreshing = false;
                    self.last_checked = Some(checked_at);
                    match result {
                        Ok(found) => {
                            self.check = ReleaseCheck::Latest { version: found.manifest.version.clone(), build: found.manifest.build };
                            // A click during the check queues the installer the user chose.
                            if !matches!(self.phase, Phase::Downloading { .. }) {
                                self.phase = match ready {
                                    Some(p) => Phase::Ready(p),
                                    None if offer(self.build, found.manifest.build) == Offer::Newer => Phase::Available(found),
                                    None => Phase::Idle,
                                };
                            }
                        }
                        Err(error) => self.check = ReleaseCheck::Unavailable(error.message),
                    }
                }
                Msg::Progress(d, t) => {
                    if let Phase::Downloading { done, total, .. } = &mut self.phase {
                        (*done, *total) = (d, t);
                    }
                }
                Msg::Downloaded(Ok(p)) => self.phase = Phase::Ready(p),
                Msg::Downloaded(Err(message)) => {
                    if let Phase::Downloading { found, .. } = &self.phase {
                        self.phase = Phase::Failed { found: found.clone(), message };
                    }
                }
                Msg::RefreshDeferred => self.refreshing = false,
            }
        }
    }

    /// Check now on the existing worker, subject to the server's retry delay.
    pub(crate) fn refresh(&mut self) {
        if self.refreshing || matches!(self.phase, Phase::Downloading { .. } | Phase::Ready(_))
            || matches!(self.check, ReleaseCheck::Checking)
        {
            return;
        }
        if let Some(send) = &self.requests {
            self.refreshing = send.send(Request::Refresh).is_ok();
        }
    }

    /// Queues the offered installer on the updater worker.
    pub(crate) fn download(&mut self) {
        let (Some(requests), Phase::Available(found) | Phase::Failed { found, .. }) = (&self.requests, &self.phase) else { return };
        let found = found.clone();
        let total = found.manifest.installer().map_or(0, |i| i.size);
        if requests.send(Request::Download(found.clone())).is_ok() {
            self.phase = Phase::Downloading { found, done: 0, total };
        } else {
            self.phase = Phase::Failed { found, message: "the update worker has stopped".into() };
        }
    }

    /// Runs the downloaded installer; the caller then quits the window.
    pub(crate) fn install(&mut self) -> Result<(), String> {
        let (Some(cfg), Phase::Ready(p)) = (&self.cfg, &self.phase) else { return Err("no update is ready".into()) };
        run_installer(&cfg.dir, p)
    }

    /// The width the title row gives the update item (0 when there is none).
    pub(crate) fn title_width(&self) -> f32 {
        if matches!(self.phase, Phase::Idle) { 0.0 } else { TITLE_W }
    }
}

/// Runs both network operations in order. While a verified installer is pending the worker
/// waits for the window to exit. A download requested during a check runs after it finishes.
fn run_worker(
    cfg: &Config,
    build: &str,
    requests: &mpsc::Receiver<Request>,
    mut now: impl FnMut() -> Instant,
    notify: impl Fn(Msg),
) {
    use mpsc::RecvTimeoutError::{Disconnected, Timeout};

    let mut last_check = None;
    let mut rate_limit = Duration::ZERO;
    let mut pending = false;
    loop {
        let time = now();
        let schedule = next_check(build, time, last_check, pending, rate_limit);
        let request = match schedule {
            CheckSchedule::Paused => requests.recv().map_err(|_| Disconnected),
            CheckSchedule::Wait(remaining) => requests.recv_timeout(remaining),
            CheckSchedule::Due => requests.recv_timeout(Duration::ZERO),
        };
        match request {
            Ok(Request::Download(found)) => {
                let started = Instant::now();
                let mut throttle = Throttle::default();
                let result = download(cfg, &found, &mut |done, total| {
                    if throttle.should_show(done, total, started.elapsed().as_secs_f64()) {
                        notify(Msg::Progress(done, total));
                    }
                });
                pending = result.is_ok();
                notify(Msg::Downloaded(result));
                continue;
            }
            Ok(Request::Refresh) if pending || (rate_limit > Duration::ZERO
                && last_check.is_some_and(|last| now().saturating_duration_since(last) < rate_limit)) => {
                notify(Msg::RefreshDeferred);
                continue;
            }
            Ok(Request::Refresh) => {}
            Err(Timeout) if schedule == CheckSchedule::Due => {}
            Err(Timeout) => continue,
            Err(Disconnected) => break,
        }
        let found = check(cfg);
        let ready = match &found {
            Ok(f) if offer(build, f.manifest.build) == Offer::Newer => already_downloaded(&cfg.dir, &f.manifest),
            _ => None,
        };
        pending = ready.is_some();
        let checked_at = SystemTime::now();
        last_check = Some(now());
        rate_limit = found.as_ref().err().map_or(Duration::ZERO, |e| e.rate_limit_delay(checked_at));
        notify(Msg::Checked(found, ready, checked_at));
    }
}

/// The update item's width in the title row.
pub(crate) const TITLE_W: f32 = 40.0;

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    type Routes = Vec<(String, u16, Vec<u8>)>;
    /// Each request's path and User-Agent.
    type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

    /// A tiny HTTP/1.1 server on 127.0.0.1. `routes` gets the server's own base URL (so the
    /// release can point at the manifest and the manifest at the installer) and maps a path to
    /// a status and a body. Every request's path and User-Agent are logged.
    struct Server {
        base: String,
        log: Log,
        routes: Arc<Mutex<Routes>>,
    }

    fn serve(routes: impl FnOnce(&str) -> Routes) -> Server {
        serve_with_headers(routes, "")
    }

    fn serve_with_headers(routes: impl FnOnce(&str) -> Routes, headers: &'static str) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = Arc::new(Mutex::new(routes(&base)));
        let active_routes = routes.clone();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let mut agent = None;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':')
                        && k.eq_ignore_ascii_case("user-agent")
                    {
                        agent = Some(v.trim().to_owned());
                    }
                }
                seen.lock().unwrap().push((path.clone(), agent));
                let (status, body) = active_routes.lock().unwrap().iter().find(|(p, _, _)| *p == path).map_or((404, b"not found".to_vec()), |(_, s, b)| (*s, b.clone()));
                let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Server { base, log, routes }
    }

    const LATEST: &str = "/repos/lelemm/yaac/releases/latest";
    const SETUP: &str = "YAAC-Setup-0.1.0-build.7.exe";

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn manifest(base: &str, build: u64, size: usize, sha256: &str) -> String {
        serde_json::json!({
            "schema": 1, "version": "0.1.0", "build": build, "tag": "v0.1.0", "releasedAt": "2026-11-21T12:00:00.000Z",
            "artifacts": [
                { "name": SETUP, "component": "installer", "version": format!("0.1.0+{build}"), "size": size, "sha256": sha256, "url": format!("{base}/from-manifest/{SETUP}") },
                { "name": "yaac.exe", "component": "app", "version": format!("0.1.0+{build}"), "size": 3, "sha256": sha(b"abc"), "url": format!("{base}/yaac.exe") }
            ]
        })
        .to_string()
    }

    fn release(base: &str) -> String {
        serde_json::json!({
            "tag_name": "v0.1.0",
            "assets": [
                { "name": "manifest.json", "browser_download_url": format!("{base}/download/manifest.json") },
                { "name": SETUP, "browser_download_url": format!("{base}/download/{SETUP}") }
            ]
        })
        .to_string()
    }

    /// A fake release host: the latest release, its manifest (build 7, saying `claimed` for
    /// the installer), and the installer's bytes.
    fn fake_host(installer: &'static [u8], claimed: Option<String>) -> Server {
        serve(move |b| {
            let sha256 = claimed.unwrap_or_else(|| sha(installer));
            vec![
                (LATEST.to_owned(), 200, release(b).into_bytes()),
                ("/download/manifest.json".to_owned(), 200, manifest(b, 7, installer.len(), &sha256).into_bytes()),
                (format!("/download/{SETUP}"), 200, installer.to_vec()),
            ]
        })
    }

    fn config(server: &Server, dir: &Path) -> Config {
        Config { latest_url: format!("{}{LATEST}", server.base), dir: dir.to_path_buf(), https_only: false, timeout: Duration::from_secs(5) }
    }

    const BYTES: &[u8] = b"pretend this is an Inno Setup installer";

    #[test]
    fn check_download_verify_and_the_pending_file_end_to_end() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        let server = fake_host(BYTES, None);
        let cfg = config(&server, &dir);
        let found = check(&cfg).unwrap();
        assert_eq!(found.manifest.build, 7);
        assert_eq!(offer("0.1.0+6", found.manifest.build), Offer::Newer);
        assert_eq!(found.installer_url, format!("{}/download/{SETUP}", server.base), "the release's own asset URL is preferred");
        let mut calls = Vec::new();
        let p = download(&cfg, &found, &mut |d, t| calls.push((d, t))).unwrap();
        assert_eq!(calls.first(), Some(&(0, BYTES.len() as u64)));
        assert_eq!(calls.last(), Some(&(BYTES.len() as u64, BYTES.len() as u64)));
        assert_eq!(p.path, dir.join(SETUP));
        assert_eq!(std::fs::read(&p.path).unwrap(), BYTES);
        assert_eq!(p.sha256, sha(BYTES));
        assert!(!p.attempted);
        assert!(verify(&p));
        // pending.json is what was returned, and no partial file is left.
        assert_eq!(read_pending(&dir), PendingRead::Found(p.clone()));
        assert!(!dir.join(format!("{SETUP}.part")).exists());
        // Found again at the next check, so it is not downloaded twice.
        assert_eq!(already_downloaded(&dir, &found.manifest), Some(p));
        // Every request said only the generic User-Agent.
        let log = server.log.lock().unwrap().clone();
        let setup = format!("/download/{SETUP}");
        assert_eq!(log.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), [LATEST, "/download/manifest.json", setup.as_str()]);
        assert!(log.iter().all(|(_, a)| a.as_deref() == Some(USER_AGENT)));
    }

    #[test]
    fn the_hub_finds_exactly_the_phones_build_and_installs_it_unattended() {
        use yaac_hub::host_update::HostInstaller as _;
        assert_eq!(release_tag("0.1.0+1060").as_deref(), Some("v0.1.0-build.1060"));
        assert_eq!(release_tag("0.1.0+dev.ab"), None);
        assert_eq!(release_tag("0.1.0/../x+7"), None);
        let args = installer_args(InstallerUi::Unattended, Path::new("u/install.log"));
        assert!(args.contains(&"/VERYSILENT".to_owned()) && args.contains(&"/RELAUNCH".to_owned()) && !args.contains(&"/SILENT".to_owned()), "{args:?}");
        assert!(installer_args(InstallerUi::Progress, Path::new("l")).contains(&"/SILENT".to_owned()));

        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        let server = serve(|b| vec![
            ("/repos/lelemm/yaac/releases/tags/v0.1.0-build.7".to_owned(), 200, release(b).into_bytes()),
            ("/download/manifest.json".to_owned(), 200, manifest(b, 7, BYTES.len(), &sha(BYTES)).into_bytes()),
            (format!("/download/{SETUP}"), 200, BYTES.to_vec()),
        ]);
        let hub = HubInstaller::new(config(&server, &dir));
        // Not published: said so, nothing downloaded.
        assert!(hub.find("0.1.0+8").unwrap_err().contains("no published release"));
        // A development build has no release.
        assert!(hub.find("0.1.0+dev").is_err());
        let release = hub.find("0.1.0+7").unwrap();
        assert_eq!((release.build.as_str(), release.size), ("0.1.0+7", BYTES.len() as u64));
        let mut last = (0, 0);
        hub.download(&release, &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(last, (BYTES.len() as u64, BYTES.len() as u64));
        let PendingRead::Found(p) = read_pending(&dir) else { panic!("verified and pending") };
        assert!(verify(&p) && !p.attempted);
        // Starting it marks it attempted first (here it is no executable, so it fails to start).
        let started = hub.install(&release);
        let PendingRead::Found(after) = read_pending(&dir) else { panic!("still there") };
        assert!(after.attempted);
        if cfg!(unix) {
            assert!(started.is_err());
        }
        // One download, one install: never again without a new download.
        assert!(hub.install(&release).unwrap_err().contains("nothing is downloaded"));
    }

    #[test]
    fn a_download_that_does_not_match_the_manifest_leaves_nothing() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        let server = fake_host(BYTES, Some(sha(b"something else")));
        let cfg = config(&server, &dir);
        let found = check(&cfg).unwrap();
        let err = download(&cfg, &found, &mut |_, _| {}).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert_eq!(read_pending(&dir), PendingRead::Missing);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the bad file is deleted");
        // A size that differs is refused too.
        let server = serve(|b| {
            vec![
                (LATEST.to_owned(), 200, release(b).into_bytes()),
                ("/download/manifest.json".to_owned(), 200, manifest(b, 7, BYTES.len() + 5, &sha(BYTES)).into_bytes()),
                (format!("/download/{SETUP}"), 200, BYTES.to_vec()),
            ]
        });
        let cfg = config(&server, &dir);
        let found = check(&cfg).unwrap();
        let err = download(&cfg, &found, &mut |_, _| {}).unwrap_err();
        assert!(err.contains("early"), "{err}");
        assert_eq!(read_pending(&dir), PendingRead::Missing);
    }

    #[test]
    fn no_release_offline_and_rate_limits_are_reasons_not_errors_to_show() {
        let t = tempfile::tempdir().unwrap();
        let none = serve(|_| Vec::new());
        assert!(check(&config(&none, t.path())).unwrap_err().message.contains("no release"));
        let limited = serve(|_| vec![(LATEST.to_owned(), 429, b"{}".to_vec())]);
        assert!(check(&config(&limited, t.path())).unwrap_err().message.contains("rate limit"));
        let limited = serve_with_headers(
            |_| vec![(LATEST.to_owned(), 403, b"{}".to_vec())],
            "x-ratelimit-remaining: 0\r\nx-ratelimit-reset: 1900\r\nretry-after: 700\r\n",
        );
        let error = check(&config(&limited, t.path())).unwrap_err();
        let checked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        assert_eq!(error.rate_limit_delay(checked_at), Duration::from_secs(900));
        assert_eq!(error.rate_limit_delay(checked_at + Duration::from_secs(300)), Duration::from_secs(700));
        // Nothing listening.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let cfg = Config { latest_url: format!("http://127.0.0.1:{port}{LATEST}"), dir: t.path().into(), https_only: false, timeout: Duration::from_secs(2) };
        assert!(check(&cfg).is_err());
        // Production refuses plain HTTP.
        let server = fake_host(BYTES, None);
        let cfg = Config { https_only: true, ..config(&server, t.path()) };
        assert!(check(&cfg).is_err());
    }

    fn pending_in(dir: &Path, build: u64, attempted: bool) -> Pending {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(SETUP);
        std::fs::write(&path, BYTES).unwrap();
        let p = Pending { build, version: format!("0.1.0+{build}"), path, sha256: sha(BYTES), size: BYTES.len() as u64, attempted };
        write_pending(dir, &p).unwrap();
        p
    }

    #[test]
    fn at_start_spent_or_broken_updates_are_cleaned_up_and_failures_said_once() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        // None: start.
        assert_eq!(at_start(&dir, "0.1.0+6"), AtStart::Continue(None));
        // The update went in (this is its build): cleaned up, start.
        pending_in(&dir, 7, true);
        assert_eq!(at_start(&dir, "0.1.0+7"), AtStart::Continue(None));
        assert_eq!(read_pending(&dir), PendingRead::Missing);
        assert!(!dir.join(SETUP).exists());
        // Unreadable: cleaned up, start.
        std::fs::write(dir.join(PENDING), "{ not json").unwrap();
        assert_eq!(at_start(&dir, "0.1.0+6"), AtStart::Continue(None));
        assert_eq!(read_pending(&dir), PendingRead::Missing);
        // Changed on disk since it was verified: never run.
        let p = pending_in(&dir, 7, false);
        std::fs::write(&p.path, b"tampered").unwrap();
        assert_eq!(at_start(&dir, "0.1.0+6"), AtStart::Continue(None));
        assert_eq!(read_pending(&dir), PendingRead::Missing);
        // Pointing outside the updates folder: refused.
        let mut p = pending_in(&dir, 7, false);
        p.path = t.path().join("elsewhere.exe");
        write_pending(&dir, &p).unwrap();
        assert!(matches!(read_pending(&dir), PendingRead::Invalid(_)));
        // The installer ran and this build is still older: start, with the message, once.
        pending_in(&dir, 7, true);
        let AtStart::Continue(Some(msg)) = at_start(&dir, "0.1.0+6") else { panic!("a message") };
        assert!(msg.contains("0.1.0+7"), "{msg}");
        assert_eq!(at_start(&dir, "0.1.0+6"), AtStart::Continue(None), "said once");
        // A development build leaves a pending update alone.
        pending_in(&dir, 7, false);
        assert_eq!(at_start(&dir, "0.1.0+dev.ab"), AtStart::Continue(None));
        assert!(matches!(read_pending(&dir), PendingRead::Found(_)));
    }

    #[test]
    fn running_the_installer_marks_the_update_attempted_first() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        let p = pending_in(&dir, 7, false);
        // Not an executable here: starting it fails, but the mark is already written, so the
        // next start does not try again.
        let err = run_installer(&dir, &p);
        let PendingRead::Found(after) = read_pending(&dir) else { panic!("still there") };
        assert!(after.attempted);
        if cfg!(unix) {
            assert!(err.is_err());
            // A newer verified update that cannot be started: start the window, say why.
            pending_in(&dir, 7, false);
            assert!(matches!(at_start(&dir, "0.1.0+6"), AtStart::Continue(Some(_))));
            assert_eq!(read_pending(&dir), PendingRead::Missing);
        }
    }

    #[test]
    fn manual_refresh_checks_without_waiting_for_the_schedule() {
        let t = tempfile::tempdir().unwrap();
        let server = fake_host(BYTES, None);
        let ctx = egui::Context::default();
        let mut u = Updater::new(config(&server, &t.path().join("updates")), "0.1.0+7", None);
        let now = Instant::now();
        u.start_with_clock(&ctx, move || now);
        let wait = |u: &mut Updater, build| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !matches!(u.check, ReleaseCheck::Latest { build: n, .. } if n == build) {
                assert!(Instant::now() < deadline, "timed out: {u:?}");
                std::thread::sleep(Duration::from_millis(10));
                u.poll();
            }
        };
        wait(&mut u, 7);
        {
            let mut routes = server.routes.lock().unwrap();
            let (_, _, body) = routes.iter_mut().find(|(path, _, _)| path == "/download/manifest.json").unwrap();
            *body = manifest(&server.base, 8, BYTES.len(), &sha(BYTES)).into_bytes();
        }
        u.refresh();
        assert!(u.refreshing);
        u.refresh();
        wait(&mut u, 8);
        assert!(!u.refreshing);
        assert!(matches!(u.phase, Phase::Available(_)));
        assert_eq!(server.log.lock().unwrap().len(), 4, "one startup check and one manual check");
    }

    #[test]
    fn the_window_picks_up_a_new_release_on_schedule_and_downloads_it() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("updates");
        let server = fake_host(BYTES, None);
        let ctx = egui::Context::default();
        let mut u = Updater::new(config(&server, &dir), "0.1.0+7", None);
        assert_eq!(u.check, ReleaseCheck::Checking);
        assert!(u.title_width() < 1.0);
        let initial_time = Instant::now();
        let (advance, advance_clock) = mpsc::channel();
        let mut reads = 0;
        u.start_with_clock(&ctx, move || {
            reads += 1;
            match reads {
                // The startup check is due immediately and completes at the initial time.
                1 | 2 => initial_time,
                // Hold the worker before it computes the next deadline until the new
                // release has been published and ten minutes have passed on the test clock.
                3 => advance_clock.recv_timeout(Duration::from_secs(10)).unwrap() - Duration::from_millis(1),
                _ => initial_time + yaac_core::update::CHECK_INTERVAL,
            }
        });
        let wait = |u: &mut Updater, done: &dyn Fn(&Updater) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done(u) {
                assert!(Instant::now() < deadline, "timed out: {u:?}");
                std::thread::sleep(Duration::from_millis(10));
                u.poll();
            }
        };
        wait(&mut u, &|u| !matches!(u.check, ReleaseCheck::Checking));
        assert_eq!(u.check, ReleaseCheck::Latest { version: "0.1.0".into(), build: 7 });
        assert_eq!(u.phase, Phase::Idle);
        let first_checked = u.last_checked.expect("startup check completion");
        {
            let mut routes = server.routes.lock().unwrap();
            let (_, _, body) = routes.iter_mut().find(|(path, _, _)| path == "/download/manifest.json").unwrap();
            *body = manifest(&server.base, 8, BYTES.len(), &sha(BYTES)).into_bytes();
        }
        advance.send(initial_time + yaac_core::update::CHECK_INTERVAL).unwrap();
        wait(&mut u, &|u| matches!(u.check, ReleaseCheck::Latest { build: 8, .. }));
        assert!(matches!(&u.phase, Phase::Available(f) if f.manifest.build == 8));
        assert!(u.title_width() > 0.0);
        assert!(u.last_checked.is_some_and(|at| at >= first_checked));
        {
            let log = server.log.lock().unwrap();
            assert_eq!(log.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), [LATEST, "/download/manifest.json", LATEST, "/download/manifest.json"]);
            assert!(log.iter().all(|(_, agent)| agent.as_deref() == Some(USER_AGENT)));
        }
        u.download();
        assert!(matches!(u.phase, Phase::Downloading { .. }));
        wait(&mut u, &|u| !matches!(u.phase, Phase::Downloading { .. }));
        let Phase::Ready(p) = &u.phase else { panic!("ready: {u:?}") };
        assert_eq!(p.build, 8);
        assert!(verify(p));
        // The next window start finds it downloaded already.
        let mut again = Updater::new(config(&server, &dir), "0.1.0+7", None);
        again.start(&ctx);
        wait(&mut again, &|u| !matches!(u.check, ReleaseCheck::Checking));
        assert!(matches!(again.phase, Phase::Ready(_)));
        // The same build, or a development build: nothing offered.
        let mut same = Updater::new(config(&server, &dir), "0.1.0+8", None);
        same.start(&ctx);
        wait(&mut same, &|u| !matches!(u.check, ReleaseCheck::Checking));
        assert_eq!(same.phase, Phase::Idle);
        let mut dev = Updater::new(config(&server, &dir), "0.1.0+dev.ab", None);
        dev.start(&ctx);
        assert!(matches!(dev.check, ReleaseCheck::Off(_)));
        assert!(dev.requests.is_none());
        assert!(dev.last_checked.is_none());
    }
}
