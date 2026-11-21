//! The in-app updater's decisions (D136, docs/install.md#updating): what a release's manifest
//! says, whether it is newer than this build, what to do with a downloaded update at start,
//! and how often a download's progress is shown. Pure: the window does the network and file
//! work (`yaac-app`'s `updater`), on worker threads.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The manifest format this build reads. A manifest of another schema is ignored (no update
/// is offered), so a format change must bump it only when old readers cannot use the new one.
pub const MANIFEST_SCHEMA: u32 = 1;

/// The manifest's asset name in every release.
pub const MANIFEST_NAME: &str = "manifest.json";

/// A release's `manifest.json`, written by `scripts/make-manifest.mjs`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    /// The app's version, without the build id (`0.1.0`).
    pub version: String,
    /// The build id every artefact of the release carries (D135).
    pub build: u64,
    /// The release's git tag.
    pub tag: String,
    /// When the release was made (ISO 8601, UTC).
    pub released_at: String,
    pub artifacts: Vec<Artifact>,
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Its asset name in the release.
    pub name: String,
    /// `installer`, `app`, `daemon` or `daemon-mcp`.
    pub component: String,
    /// Its own version with the build id: `<version>+<build>`.
    pub version: String,
    pub size: u64,
    /// SHA-256 of its bytes, lower-case hex.
    pub sha256: String,
    pub url: String,
}

/// The component name of the installer in a manifest.
pub const INSTALLER: &str = "installer";

impl Manifest {
    /// Parses and checks a manifest: the schema this build reads, a build number, an
    /// installer, and well-formed sizes and hashes.
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Self = serde_json::from_str(text).map_err(|e| format!("not a release manifest: {e}"))?;
        if m.schema != MANIFEST_SCHEMA {
            return Err(format!("manifest schema {} is not one this build reads ({MANIFEST_SCHEMA})", m.schema));
        }
        if m.build == 0 {
            return Err("the manifest has no build number".into());
        }
        for a in &m.artifacts {
            if !is_sha256(&a.sha256) {
                return Err(format!("{}: not a SHA-256", a.name));
            }
            if a.size == 0 {
                return Err(format!("{}: empty", a.name));
            }
            if a.name.is_empty() || a.name.contains(['/', '\\']) || a.name.starts_with('.') {
                return Err(format!("{:?}: not a file name", a.name));
            }
        }
        if m.installer().is_none() {
            return Err("the release has no installer".into());
        }
        Ok(m)
    }

    /// The installer artefact.
    pub fn installer(&self) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.component == INSTALLER)
    }

    /// `<version>+<build>`, as builds are shown everywhere.
    pub fn label(&self) -> String {
        format!("{}+{}", self.version, self.build)
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The part of GitHub's `GET /repos/{owner}/{repo}/releases/latest` answer the updater reads.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// One asset of a GitHub release.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

impl Release {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("not a GitHub release: {e}"))
    }

    /// The download URL of the asset called `name`.
    pub fn asset_url(&self, name: &str) -> Option<&str> {
        self.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.as_str())
    }
}

/// A build's number: the `<id>` of `<version>+<id>` when it is a number. `None` for a
/// development build (`+dev…`), which has no order and never updates itself.
pub fn build_number(build: &str) -> Option<u64> {
    build.split_once('+').and_then(|(_, id)| id.trim().parse().ok())
}

/// The latest release against the running build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// The release is newer: offer it.
    Newer,
    /// This is the release's build.
    Same,
    /// This build is newer than the latest release.
    Ahead,
    /// A development build: never updated from releases.
    Dev,
}

/// Compares the running build (`<version>+<id>`) with a release's build number. Build ids
/// are shared by every artefact of a build and only grow (release ids come from CI), so the
/// number alone orders them.
pub fn offer(running: &str, release_build: u64) -> Offer {
    match build_number(running) {
        None => Offer::Dev,
        Some(n) if release_build > n => Offer::Newer,
        Some(n) if release_build == n => Offer::Same,
        Some(_) => Offer::Ahead,
    }
}

/// A downloaded and verified update, `pending.json` in the updates folder. Written only after
/// the installer's size and SHA-256 matched the manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub build: u64,
    /// `<version>+<build>`.
    pub version: String,
    /// The installer.
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    /// The installer was started for it. Set before starting it: if YAAC then starts and is
    /// still older, the installer failed, and it is not run again (no loop).
    #[serde(default)]
    pub attempted: bool,
}

/// What was found in the updates folder at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingRead {
    Missing,
    /// There, but unreadable.
    Invalid(String),
    Found(Pending),
}

/// What YAAC does with a pending update as the window starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartDecision {
    /// Start as usual.
    Start,
    /// Delete the pending update (and its installer), then start as usual.
    Discard(String),
    /// Run this installer (marked attempted first) and exit without starting.
    Install(Pending),
    /// The installer ran and this build is still the older one: delete the pending update,
    /// start, and show this once.
    Failed(String),
}

/// The decision table for a pending update at start. `verified` checks the installer on disk
/// against the pending file's size and SHA-256; it is called only for an update that would be
/// installed.
pub fn decide_start(running: &str, pending: PendingRead, verified: impl FnOnce(&Pending) -> bool) -> StartDecision {
    let p = match pending {
        PendingRead::Missing => return StartDecision::Start,
        PendingRead::Invalid(why) => return StartDecision::Discard(why),
        PendingRead::Found(p) => p,
    };
    match offer(running, p.build) {
        // A development build never installs a release; the update is left for a release build.
        Offer::Dev => StartDecision::Start,
        Offer::Same | Offer::Ahead => StartDecision::Discard(format!("{} is installed already", p.version)),
        Offer::Newer if p.attempted => StartDecision::Failed(format!("The update to YAAC {} could not be installed; YAAC {running} started instead.", p.version)),
        Offer::Newer if !verified(&p) => StartDecision::Discard(format!("the downloaded installer for {} does not match its hash", p.version)),
        Offer::Newer => StartDecision::Install(p),
    }
}

/// How often a download's progress is drawn: when the bar moves by a visible step, at most
/// every [`Throttle::MIN_INTERVAL`] seconds, and always at the end.
#[derive(Clone, Debug, Default)]
pub struct Throttle {
    shown_at: Option<f64>,
    shown_step: Option<u32>,
    finished: bool,
}

impl Throttle {
    /// Steps across the whole bar (it is a few dozen pixels wide).
    pub const STEPS: u32 = 50;
    /// The shortest time between two repaints for progress, in seconds.
    pub const MIN_INTERVAL: f64 = 0.1;

    /// Whether `done` of `total` bytes at `now` (seconds) is worth a repaint.
    pub fn should_show(&mut self, done: u64, total: u64, now: f64) -> bool {
        if self.finished {
            return false;
        }
        if total > 0 && done >= total {
            self.finished = true;
            self.shown_at = Some(now);
            return true;
        }
        let step = done.saturating_mul(u64::from(Self::STEPS)).checked_div(total).map_or(0, |s| u32::try_from(s).unwrap_or(Self::STEPS));
        let due = self.shown_at.is_none_or(|t| now - t >= Self::MIN_INTERVAL);
        if self.shown_step != Some(step) && due {
            self.shown_at = Some(now);
            self.shown_step = Some(step);
            return true;
        }
        false
    }
}

/// What the window knows about the latest release, for Settings → Advanced → Components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseCheck {
    /// Not checked, and why (not Windows, a development build, …).
    Off(String),
    Checking,
    /// The check did not get an answer (offline, rate limited, …).
    Unavailable(String),
    /// The latest release.
    Latest { version: String, build: u64 },
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const SAMPLE: &str = include_str!("../../../scripts/testdata/manifest.sample.json");

    #[test]
    fn the_sample_manifest_from_make_manifest_parses() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.build, 1005);
        assert_eq!(m.label(), "0.1.0+1005");
        let i = m.installer().unwrap();
        assert_eq!(i.name, "YAAC-Setup-0.1.0-build.1005.exe");
        assert_eq!(i.version, "0.1.0+1005");
        assert_eq!(m.artifacts.iter().map(|a| a.component.as_str()).collect::<Vec<_>>(), ["installer", "app", "daemon", "daemon-mcp"]);
    }

    #[test]
    fn bad_manifests_are_refused() {
        let edit = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
            f(&mut v);
            Manifest::parse(&v.to_string())
        };
        assert!(edit(&|_| {}).is_ok());
        assert!(edit(&|v| v["schema"] = 2.into()).unwrap_err().contains("schema"));
        assert!(edit(&|v| v["build"] = 0.into()).is_err());
        assert!(edit(&|v| v["artifacts"][0]["sha256"] = "abc".into()).unwrap_err().contains("SHA-256"));
        assert!(edit(&|v| v["artifacts"][0]["name"] = "../evil.exe".into()).is_err());
        assert!(edit(&|v| v["artifacts"][0]["component"] = "app".into()).unwrap_err().contains("installer"));
        assert!(Manifest::parse("<html>").is_err());
        // Fields added later are ignored.
        assert!(edit(&|v| v["notes"] = "new".into()).is_ok());
    }

    #[test]
    fn a_github_release_gives_the_manifest_url() {
        let r = Release::parse(r#"{"tag_name":"v0.1.0","name":"x","assets":[{"name":"manifest.json","browser_download_url":"https://github.com/o/r/releases/download/v0.1.0/manifest.json","size":9}]}"#).unwrap();
        assert_eq!(r.tag_name, "v0.1.0");
        assert_eq!(r.asset_url(MANIFEST_NAME), Some("https://github.com/o/r/releases/download/v0.1.0/manifest.json"));
        assert_eq!(r.asset_url("other"), None);
        assert!(Release::parse(r#"{"message":"Not Found"}"#).is_err());
    }

    #[test]
    fn builds_compare_by_number_and_development_builds_never_update() {
        assert_eq!(build_number("0.1.0+1005"), Some(1005));
        assert_eq!(build_number("0.1.0+dev.ab12"), None);
        assert_eq!(build_number("0.1.0"), None);
        assert_eq!(offer("0.1.0+1004", 1005), Offer::Newer);
        assert_eq!(offer("0.1.0+1005", 1005), Offer::Same);
        assert_eq!(offer("0.2.0+1006", 1005), Offer::Ahead);
        assert_eq!(offer("0.1.0+dev.ab12", 1005), Offer::Dev);
        assert_eq!(offer("0.1.0+dev", 1), Offer::Dev);
    }

    fn pending(build: u64, attempted: bool) -> Pending {
        Pending { build, version: format!("0.1.0+{build}"), path: "/u/YAAC-Setup.exe".into(), sha256: "0".repeat(64), size: 10, attempted }
    }

    #[test]
    fn the_pending_update_decision_table() {
        use StartDecision as D;
        let ok = |_: &Pending| true;
        // Nothing there.
        assert_eq!(decide_start("0.1.0+10", PendingRead::Missing, ok), D::Start);
        // Unreadable: deleted.
        assert!(matches!(decide_start("0.1.0+10", PendingRead::Invalid("bad json".into()), ok), D::Discard(_)));
        // A verified newer update: installed.
        assert_eq!(decide_start("0.1.0+10", PendingRead::Found(pending(11, false)), ok), D::Install(pending(11, false)));
        // The file on disk no longer matches: deleted, never run.
        assert!(matches!(decide_start("0.1.0+10", PendingRead::Found(pending(11, false)), |_| false), D::Discard(_)));
        // Older than, or the same as, the running build (the update went in): deleted.
        assert!(matches!(decide_start("0.1.0+11", PendingRead::Found(pending(11, true)), ok), D::Discard(_)));
        assert!(matches!(decide_start("0.1.0+12", PendingRead::Found(pending(11, false)), ok), D::Discard(_)));
        // The installer already ran and this is still the older build: start, say so once.
        let D::Failed(msg) = decide_start("0.1.0+10", PendingRead::Found(pending(11, true)), ok) else { panic!("failed") };
        assert!(msg.contains("0.1.0+11") && msg.contains("0.1.0+10"), "{msg}");
        // A development build leaves it alone.
        assert_eq!(decide_start("0.1.0+dev.ab", PendingRead::Found(pending(11, false)), ok), D::Start);
    }

    #[test]
    fn verification_runs_only_for_an_update_that_would_be_installed() {
        let calls = Cell::new(0);
        let count = |_: &Pending| {
            calls.set(calls.get() + 1);
            true
        };
        let _ = decide_start("0.1.0+12", PendingRead::Found(pending(11, false)), count);
        let _ = decide_start("0.1.0+10", PendingRead::Found(pending(11, true)), count);
        assert_eq!(calls.get(), 0);
        let _ = decide_start("0.1.0+10", PendingRead::Found(pending(11, false)), count);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn progress_is_shown_by_visible_steps_at_most_ten_times_a_second() {
        let mut t = Throttle::default();
        let total = 1000;
        assert!(t.should_show(0, total, 0.0), "the first report");
        assert!(!t.should_show(5, total, 0.5), "less than a step");
        assert!(t.should_show(25, total, 0.5), "one step");
        assert!(!t.should_show(100, total, 0.55), "too soon after the last");
        assert!(t.should_show(100, total, 0.7));
        assert!(t.should_show(total, total, 0.71), "the end is always shown");
        assert!(!t.should_show(total, total, 2.0), "once");
        // Many small chunks, fast: no more repaints than steps, and none closer than the interval.
        let mut t = Throttle::default();
        let shown = (0..=10_000u64).filter(|i| t.should_show(i * 100, 1_000_000, *i as f64 * 0.0001)).count();
        assert!(shown <= 11, "{shown} repaints for 1 s of chunks");
        let mut t = Throttle::default();
        let shown = (0..=10_000u64).filter(|i| t.should_show(i * 100, 1_000_000, *i as f64 * 0.01)).count();
        assert!(shown <= Throttle::STEPS as usize + 2, "{shown} repaints for 100 s of chunks");
    }
}
