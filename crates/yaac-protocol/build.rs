//! Computes `schemaHash` for every frozen schema baseline `protocol/schema/v<n>.json`, as the
//! constants `SCHEMA_HASH_V<n>`, so each hash always matches the committed snapshot it is
//! derived from.

#[path = "src/codegen/sha256.rs"]
mod sha256;

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&manifest).join("../../protocol/schema");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut versions: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()))
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix('v')?.strip_suffix(".json")?.parse().ok()?;
            Some((version, entry.path()))
        })
        .collect();
    versions.sort();
    let mut code = String::new();
    for (version, path) in versions {
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        let hex = sha256::short_hex(&sha256::sha256_lf(&bytes));
        let hex = String::from_utf8_lossy(&hex);
        let _ = write!(
            code,
            "/// `schemaHash` of protocol version {version}: the first 64 bits of the SHA-256 of\n/// `protocol/schema/v{version}.json` (carriage returns ignored), as 16 hex characters.\n/// Computed by `build.rs` from the committed snapshot.\npub const SCHEMA_HASH_V{version}: &str = \"{hex}\";\n"
        );
    }
    let out = std::env::var("OUT_DIR").unwrap_or_else(|_| ".".into());
    let target = Path::new(&out).join("schema_hash.rs");
    std::fs::write(&target, code).unwrap_or_else(|err| panic!("cannot write {}: {err}", target.display()));
}
