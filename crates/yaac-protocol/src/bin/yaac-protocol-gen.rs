//! Regenerates the protocol's generated files.
//!
//! ```text
//! cargo run -p yaac-protocol --bin yaac-protocol-gen            # TypeScript bindings
//! cargo run -p yaac-protocol --bin yaac-protocol-gen -- --check # fail if anything is stale
//! cargo run -p yaac-protocol --bin yaac-protocol-gen -- --freeze [--force]
//! ```
//!
//! `--freeze` writes the frozen baseline `protocol/schema/v<PROTOCOL_VERSION>.json` (step 1 of
//! the procedure in crates/yaac-protocol/src/dialect/README.md). It refuses to overwrite an
//! existing baseline unless `--force` is given, which is only legitimate before the version
//! has been released. Because `schemaHash` is computed at compile time from the baseline, run
//! the generator once more after freezing so the TypeScript constant picks up the new hash.

use std::process::ExitCode;

use yaac_protocol::codegen::{self, compat};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let root = codegen::repo_root();
    let version = codegen::current_version();
    let baseline_path = root.join(codegen::paths::schema_snapshot(version));

    if has("--freeze") {
        if baseline_path.exists() && !has("--force") {
            eprintln!("{} exists; a frozen baseline is never rewritten (use --force only before release)", baseline_path.display());
            return ExitCode::FAILURE;
        }
        let text = codegen::render_schema(&codegen::current_schema());
        if let Err(err) = std::fs::write(&baseline_path, text) {
            eprintln!("cannot write {}: {err}", baseline_path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("froze {} (hash {}); run the generator again to refresh the TypeScript hash", baseline_path.display(), codegen::schema_hash(&std::fs::read(&baseline_path).unwrap_or_default()));
        return ExitCode::SUCCESS;
    }

    // The hash this build will send: computed at compile time from the committed baseline.
    let files = codegen::generated_files(yaac_protocol::dialect::CURRENT.schema_hash());
    if has("--check") {
        let mut failed = false;
        let stale = codegen::stale_files(&root, &files);
        for path in &stale {
            eprintln!("stale: {path}");
            failed = true;
        }
        match std::fs::read(&baseline_path) {
            Ok(bytes) => {
                let baseline: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                for problem in compat::check(&baseline, &codegen::current_schema()) {
                    eprintln!("incompatible with the frozen baseline: {problem}");
                    failed = true;
                }
            }
            Err(err) => {
                eprintln!("missing baseline {}: {err}", baseline_path.display());
                failed = true;
            }
        }
        return if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }

    let dir = root.join(codegen::paths::TS_DIR);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {err}", dir.display());
        return ExitCode::FAILURE;
    }
    for file in &files {
        let path = root.join(&file.path);
        if let Err(err) = std::fs::write(&path, &file.contents) {
            eprintln!("cannot write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("wrote {}", file.path);
    }
    ExitCode::SUCCESS
}
