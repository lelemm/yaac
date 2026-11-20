//! Embeds the `yaacd` bundle the hub installs on remote targets (docs/02-architecture.md#bootstrap):
//! `daemon/dist/yaacd.mjs`, built by `npm run build` in `daemon/`. When it has not been built,
//! the hub is built without one (remote targets then report that clearly) rather than failing,
//! so the Rust crates build and test without Node.js.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root = manifest.join("..").join("..");
    let bundle = root.join("daemon").join("dist").join("yaacd.mjs");
    let package = root.join("daemon").join("package.json");
    println!("cargo:rerun-if-changed={}", bundle.display());
    println!("cargo:rerun-if-changed={}", package.display());
    println!("cargo:rustc-check-cfg=cfg(yaac_daemon_bundle)");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let target = out.join("yaacd.mjs");
    match std::fs::read(&bundle) {
        Ok(bytes) => {
            std::fs::write(&target, bytes).expect("write the embedded bundle");
            println!("cargo:rustc-cfg=yaac_daemon_bundle");
        }
        Err(_) => {
            std::fs::write(&target, b"").expect("write the empty bundle");
            println!("cargo:warning=daemon/dist/yaacd.mjs not found: this hub cannot install yaacd on WSL targets (run `npm run build` in daemon/)");
        }
    }
    // The daemon's version, for the install folder name (`<version>-<sha256 prefix>`).
    let version = std::fs::read_to_string(&package)
        .ok()
        .and_then(|text| {
            let at = text.find("\"version\"")?;
            let rest = &text[at + "\"version\"".len()..];
            let start = rest.find('"')? + 1;
            let end = start + rest[start..].find('"')?;
            Some(rest[start..end].to_owned())
        })
        .unwrap_or_else(|| "0.0.0".to_owned());
    println!("cargo:rustc-env=YAAC_DAEMON_VERSION={version}");
}
