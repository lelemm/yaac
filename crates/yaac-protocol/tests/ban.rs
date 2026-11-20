//! The ban (docs/09-compatibility.md#tests): nothing outside the dialect module may read the
//! negotiated protocol version.
//!
//! The Rust half is enforced by privacy — `ProtocolVersion` and friends have private fields
//! and no accessors, ordering, or equality (see the `compile_fail` examples in
//! `src/dialect/mod.rs`). This test is the belt to those braces: it scans every Rust source in
//! the workspace, and the TypeScript of `yaacd` outside its protocol directory, for the names
//! that carry a version number.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use std::path::{Path, PathBuf};

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules" | ".git") {
                files(&path, ext, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}

fn violations(root: &Path, scan: &str, ext: &str, words: &[&str], allowed: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let mut paths = Vec::new();
    files(&root.join(scan), ext, &mut paths);
    for path in paths {
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        if allowed.iter().any(|a| rel.starts_with(a)) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for word in words {
                let hit = code.match_indices(word).any(|(i, _)| {
                    let before = code[..i].chars().next_back();
                    let after = code[i + word.len()..].chars().next();
                    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                    !ident(before) && !ident(after)
                });
                if hit {
                    found.push(format!("{rel}:{}: `{word}`", n + 1));
                }
            }
        }
    }
    found
}

#[test]
fn no_rust_code_outside_the_dialect_module_reads_a_protocol_version() {
    let root = yaac_protocol::codegen::repo_root();
    let found = violations(
        &root,
        "crates",
        "rs",
        &["ProtocolVersion", "PROTOCOL_VERSION", "codegen_numbers", ".agreed"],
        &[
            "crates/yaac-protocol/src/dialect/",
            // Writes the numbers into generated files; no logic depends on them.
            "crates/yaac-protocol/src/codegen/",
            "crates/yaac-protocol/src/bin/yaac-protocol-gen.rs",
            // This test names the words it looks for.
            "crates/yaac-protocol/tests/ban.rs",
        ],
    );
    assert!(found.is_empty(), "protocol version read outside the dialect module:\n{}", found.join("\n"));
}

#[test]
fn no_daemon_code_outside_its_protocol_directory_reads_a_protocol_version() {
    let root = yaac_protocol::codegen::repo_root();
    let found = violations(
        &root,
        "daemon/src",
        "ts",
        &["PROTOCOL_VERSION", "PROTOCOL_MIN", "PROTOCOL_MAX"],
        &["daemon/src/protocol/"],
    );
    assert!(found.is_empty(), "protocol version read outside daemon/src/protocol/:\n{}", found.join("\n"));
}
