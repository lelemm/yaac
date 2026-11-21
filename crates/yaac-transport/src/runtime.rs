//! The private runtime (D17, D25; docs/02-architecture.md#how-the-runtime-gets-there): which
//! pinned Node.js build a remote target gets, and the host's own copy of it.
//!
//! A target with no runtime is sent `GET runtime` first — it downloads the archive itself and
//! verifies it against the checksum pinned here, so the bytes never cross the link. When it
//! cannot (no `curl` or `wget`, no internet, a mismatch), the host fetches the same archive
//! once with the system `curl` into its cache, verifies the same checksum, and sends it with
//! `PUT runtime` over the one stream. The checksums are compiled in, so neither path can be
//! made to run anything else.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};

/// The pinned Node.js version (the one the daemon is tested on).
pub const PINNED_NODE: &str = "22.22.3";

/// One downloadable runtime archive for one platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeArtifact {
    /// The install folder on the target, `~/.yaac/runtime/<name>/` (`node-22.22.3`).
    pub name: String,
    /// `uname -s` as stage 0 reports it (`Linux`, `Darwin`).
    pub os: String,
    /// `uname -m` values it serves (`x86_64`, `amd64`).
    pub arch: Vec<String>,
    /// `glibc` or `musl` (ignored for macOS).
    pub libc: String,
    /// Where it is published: a `.tar.gz` with one top folder holding `bin/node`.
    pub url: String,
    /// Lower-case hex SHA-256 of the archive.
    pub sha256: String,
}

impl RuntimeArtifact {
    /// The archive's file name (the last part of its URL).
    pub fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or("runtime.tar.gz")
    }
}

/// Where a host gets runtimes from, and how it caches them.
#[derive(Clone, Debug)]
pub struct RuntimeSource {
    /// The archives on offer, one per platform.
    pub artifacts: Vec<RuntimeArtifact>,
    /// The host's cache folder for archives it fetched itself. `None`: the host never fetches
    /// (only the target may download).
    pub cache_dir: Option<PathBuf>,
    /// The system `curl` the host fetches with.
    pub curl: OsString,
    /// Whether the target is asked to download the archive itself first.
    pub target_download: bool,
}

/// One pinned Node.js archive (official builds; musl ones from unofficial-builds.nodejs.org,
/// the Node.js project's own musl builds).
fn node(os: &str, arch: &[&str], libc: &str, base: &str, file: &str, sha256: &str) -> RuntimeArtifact {
    RuntimeArtifact {
        name: format!("node-{PINNED_NODE}"),
        os: os.to_owned(),
        arch: arch.iter().map(|a| (*a).to_owned()).collect(),
        libc: libc.to_owned(),
        url: format!("{base}/v{PINNED_NODE}/node-v{PINNED_NODE}-{file}.tar.gz"),
        sha256: sha256.to_owned(),
    }
}

impl RuntimeSource {
    /// The pinned Node.js builds for every POSIX platform YAAC supports as a target,
    /// downloaded from nodejs.org. Checksums from the release's `SHASUMS256.txt`.
    pub fn pinned(cache_dir: Option<PathBuf>) -> Self {
        const DIST: &str = "https://nodejs.org/dist";
        const MUSL: &str = "https://unofficial-builds.nodejs.org/download/release";
        let artifacts = vec![
            node("Linux", &["x86_64", "amd64"], "glibc", DIST, "linux-x64", "c7a10d6816da8eaaa7534dd73c71c6e2b2c391dbbf845e364902d156615dd1b8"),
            node("Linux", &["aarch64", "arm64"], "glibc", DIST, "linux-arm64", "cc8bc82b2dd0b595c3b95a4c3c9c8c350907cff011afbdee3d1379e812e1e3e3"),
            node("Linux", &["armv7l"], "glibc", DIST, "linux-armv7l", "18fbe1bfdd42045af15f02256ae3e59490d1b9ff93dfc27a223acc2c92740250"),
            node("Linux", &["x86_64", "amd64"], "musl", MUSL, "linux-x64-musl", "d227692583d6ae17c44028fd8f17cb1b3672167a6baa035818240cbb9db6ea4a"),
            node("Linux", &["aarch64", "arm64"], "musl", MUSL, "linux-arm64-musl", "09c87acbafae65e18fcc0eb188376c1254cab74dc769118f4b60479c5fa326cf"),
            node("Darwin", &["x86_64"], "", DIST, "darwin-x64", "45830ba752fa0d892c6dcd640946669801293cac820a33591ded40ac075198ec"),
            node("Darwin", &["arm64"], "", DIST, "darwin-arm64", "0da7ff74ef8611328c8212f17943368713a2ad953fb7d89a8c8a0eae87c23207"),
        ];
        Self { artifacts, cache_dir, curl: OsString::from(if cfg!(windows) { "curl.exe" } else { "curl" }), target_download: true }
    }

    /// The archive for a target, from its stage 0 report.
    pub fn pick(&self, os: &str, arch: &str, libc: &str) -> Option<&RuntimeArtifact> {
        self.artifacts
            .iter()
            .find(|a| a.os == os && a.arch.iter().any(|x| x == arch) && (a.os == "Darwin" || a.libc == libc))
    }

    /// The runtime folder name stage 0 should look for (`YAAC_RUNTIME_VERSION`), when every
    /// archive installs under the same name.
    pub fn name(&self) -> Option<&str> {
        let first = self.artifacts.first()?.name.as_str();
        self.artifacts.iter().all(|a| a.name == first).then_some(first)
    }
}

/// Lower-case hex SHA-256 of a file, read in chunks.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The host's verified copy of `artifact`: from the cache when it is there and its checksum
/// matches, otherwise downloaded with the system `curl` and verified first. Never returns a
/// file whose checksum differs from the pinned one.
pub async fn host_copy(source: &RuntimeSource, artifact: &RuntimeArtifact) -> Result<PathBuf, String> {
    let dir = source.cache_dir.as_ref().ok_or("this host does not fetch runtimes")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file = dir.join(format!("{}-{}", &artifact.sha256[..16.min(artifact.sha256.len())], artifact.file_name()));
    let want = artifact.sha256.clone();
    if file.is_file() {
        let path = file.clone();
        let got = tokio::task::spawn_blocking(move || sha256_file(&path)).await.map_err(|e| e.to_string())?;
        if got.as_deref().ok() == Some(want.as_str()) {
            return Ok(file);
        }
        let _ = std::fs::remove_file(&file);
    }
    let part = file.with_extension("part");
    let _ = std::fs::remove_file(&part);
    let mut curl = tokio::process::Command::new(&source.curl);
    curl.args(["-fsSL", "--retry", "2", "-o"]).arg(&part).arg(&artifact.url);
    curl.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    curl.creation_flags(crate::CREATE_NO_WINDOW);
    let out = curl.output().await.map_err(|e| format!("cannot run {}: {e}", source.curl.to_string_lossy()))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the host could not download {}: {}", artifact.url, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let path = part.clone();
    let got = tokio::task::spawn_blocking(move || sha256_file(&path)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    if got != want {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the runtime downloaded from {} does not match its pinned checksum", artifact.url));
    }
    std::fs::rename(&part, &file).map_err(|e| format!("cannot store {}: {e}", file.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_table_covers_the_posix_targets() {
        let s = RuntimeSource::pinned(None);
        assert_eq!(s.name(), Some(format!("node-{PINNED_NODE}").as_str()));
        let x = s.pick("Linux", "x86_64", "glibc").unwrap();
        assert!(x.url.ends_with("node-v22.22.3-linux-x64.tar.gz"), "{}", x.url);
        assert_eq!(x.file_name(), "node-v22.22.3-linux-x64.tar.gz");
        assert!(s.pick("Linux", "aarch64", "musl").unwrap().url.contains("unofficial-builds"));
        assert!(s.pick("Darwin", "arm64", "glibc").is_some());
        assert!(s.pick("Linux", "riscv64", "glibc").is_none());
        for a in &s.artifacts {
            assert_eq!(a.sha256.len(), 64);
            assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
    }
}
