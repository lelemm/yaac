//! Generated files are up to date, the frozen baseline is respected, and the schema hash is
//! derived from the committed baseline.

// Test code: small index casts and by-value helpers keep the tests readable.
#![allow(clippy::cast_possible_truncation, clippy::needless_pass_by_value, clippy::type_complexity)]

use serde_json::{Value, json};
use yaac_protocol::codegen::{self, compat};

const REGENERATE: &str = "cargo run -p yaac-protocol --bin yaac-protocol-gen";

#[test]
fn committed_typescript_is_up_to_date() {
    let root = codegen::repo_root();
    let files = codegen::generated_files(yaac_protocol::dialect::CURRENT.schema_hash());
    assert!(files.iter().any(|f| f.path.ends_with("types.ts")));
    let stale = codegen::stale_files(&root, &files);
    assert!(stale.is_empty(), "stale generated files {stale:?}; run `{REGENERATE}`");
}

#[test]
fn the_frozen_baseline_exists_and_the_current_schema_is_an_additive_change_of_it() {
    let root = codegen::repo_root();
    let path = root.join(codegen::paths::schema_snapshot(codegen::current_version()));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("missing frozen baseline {}: {e}", path.display()));
    let baseline: Value = serde_json::from_slice(&bytes).unwrap();
    let problems = compat::check(&baseline, &codegen::current_schema());
    assert!(
        problems.is_empty(),
        "the protocol changed incompatibly without a version bump (docs/09-compatibility.md):\n{}",
        problems.join("\n")
    );
}

#[test]
fn schema_hash_is_derived_from_the_committed_baseline() {
    let bytes = std::fs::read(codegen::repo_root().join("protocol/schema/v1.json")).unwrap();
    assert_eq!(codegen::schema_hash(&bytes), codegen::SCHEMA_HASH_V1);
    assert_eq!(yaac_protocol::dialect::CURRENT.schema_hash(), codegen::SCHEMA_HASH_V1);
    assert_eq!(codegen::SCHEMA_HASH_V1.len(), 16);
    assert!(codegen::SCHEMA_HASH_V1.chars().all(|c| c.is_ascii_hexdigit()));
    let constants = std::fs::read_to_string(codegen::repo_root().join("daemon/src/protocol/generated/constants.ts")).unwrap();
    assert!(
        constants.contains(&format!("SCHEMA_HASH = \"{}\"", yaac_protocol::dialect::CURRENT.schema_hash())),
        "TypeScript SCHEMA_HASH differs; run `{REGENERATE}`"
    );
}

#[test]
fn the_schema_is_deterministic() {
    assert_eq!(codegen::render_schema(&codegen::current_schema()), codegen::render_schema(&codegen::current_schema()));
}

#[test]
fn the_compatibility_check_catches_incompatible_changes() {
    let baseline = json!({
        "$defs": {
            "A": { "type": "object", "properties": { "x": { "type": "string" }, "y": { "type": "integer" } }, "required": ["x"] },
            "E": { "anyOf": [{ "type": "string", "enum": ["a", "b"] }, { "type": "string", "x-yaac-unknown": true }] },
            "Gone": { "type": "string" }
        },
        "x-yaac-methods": [{ "name": "m", "params": { "$ref": "#/$defs/A" }, "result": { "$ref": "#/$defs/A" }, "direction": "hostToDaemon" }]
    });
    // Additive: a new optional field, a new enum value, a new definition, a new method.
    let mut additive = baseline.clone();
    additive["$defs"]["A"]["properties"]["z"] = json!({ "type": "boolean" });
    additive["$defs"]["E"]["anyOf"][0]["enum"] = json!(["a", "b", "c"]);
    additive["$defs"]["New"] = json!({ "type": "string" });
    additive["x-yaac-methods"].as_array_mut().unwrap().push(json!({ "name": "n" }));
    assert_eq!(compat::check(&baseline, &additive), Vec::<String>::new());

    let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        ("definition removed", Box::new(|v| { v["$defs"].as_object_mut().unwrap().remove("Gone"); })),
        ("property removed", Box::new(|v| { v["$defs"]["A"]["properties"].as_object_mut().unwrap().remove("y"); })),
        ("became required", Box::new(|v| v["$defs"]["A"]["required"] = json!(["x", "y"]))),
        ("no longer required", Box::new(|v| v["$defs"]["A"]["required"] = json!([]))),
        ("type changed", Box::new(|v| v["$defs"]["A"]["properties"]["x"]["type"] = json!("integer"))),
        ("enum value", Box::new(|v| v["$defs"]["E"]["anyOf"][0]["enum"] = json!(["a"]))),
        ("removed", Box::new(|v| v["x-yaac-methods"] = json!([]))),
        ("params changed", Box::new(|v| v["x-yaac-methods"][0]["params"] = json!({ "$ref": "#/$defs/Gone" }))),
    ];
    for (expected, change) in cases {
        let mut current = baseline.clone();
        change(&mut current);
        let problems = compat::check(&baseline, &current);
        assert!(problems.iter().any(|p| p.contains(expected)), "{expected}: {problems:?}");
    }
}

#[test]
fn every_definition_is_documented() {
    let schema = codegen::current_schema();
    let undocumented: Vec<&String> = schema["$defs"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, def)| def.get("description").is_none())
        .map(|(name, _)| name)
        .collect();
    assert!(undocumented.is_empty(), "add doc comments to {undocumented:?}");
}

/// Type-checks the committed TypeScript under `strict`. Needs Node.js and network access the
/// first time (`npx` fetches TypeScript 5), so it only runs when `YAAC_TSC=1`.
#[test]
fn generated_typescript_type_checks_under_strict() {
    if std::env::var("YAAC_TSC").as_deref() != Ok("1") {
        eprintln!("skipped: set YAAC_TSC=1 to run tsc --strict over the generated TypeScript");
        return;
    }
    let dir = codegen::repo_root().join(codegen::paths::TS_DIR);
    let status = std::process::Command::new("npx")
        .args(["-y", "-p", "typescript@5", "tsc", "--noEmit", "--strict", "--target", "es2022", "--module", "nodenext"])
        .args(["--moduleResolution", "nodenext", "types.ts", "constants.ts", "index.ts"])
        .current_dir(&dir)
        .status()
        .expect("run npx");
    assert!(status.success(), "tsc --strict failed on {}", dir.display());
}
