//! Schema snapshot, schema hash, and TypeScript generation.
//!
//! * [`current_schema`] builds the JSON Schema of the protocol from the Rust types: every
//!   definition reachable from the method catalogue, plus the catalogue itself
//!   (`x-yaac-methods`, `x-yaac-notifications`).
//! * `protocol/schema/v<n>.json` is the **frozen baseline** of version *n*: written once, when
//!   the version is introduced (`yaac-protocol-gen --freeze`), never edited afterwards.
//!   [`SCHEMA_HASH_V1`] is computed by `build.rs` from that file. Later additive changes
//!   do not touch it; [`compat::check`] proves they are additive.
//! * The TypeScript bindings in `daemon/src/protocol/generated/` always follow the current
//!   types. [`generated_files`] produces them; a test fails when the committed copies are
//!   stale.
//!
//! Regenerate everything with `cargo run -p yaac-protocol --bin yaac-protocol-gen`.

pub mod compat;
mod sha256;
mod ts;

use std::path::{Path, PathBuf};

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::frame;
use crate::methods::{NOTIFICATIONS, REQUESTS};
use crate::rpc::{ErrorCode, ErrorObject, RpcId};
use crate::schedule::{self, Class};

include!(concat!(env!("OUT_DIR"), "/schema_hash.rs"));

/// The schema hash of arbitrary snapshot bytes, the same way [`SCHEMA_HASH_V1`] is computed.
pub fn schema_hash(snapshot: &[u8]) -> String {
    String::from_utf8_lossy(&sha256::short_hex(&sha256::sha256_lf(snapshot))).into_owned()
}

/// Paths relative to the repository root.
pub mod paths {
    /// The frozen baseline of protocol version `n`.
    pub fn schema_snapshot(version: u16) -> String {
        format!("protocol/schema/v{version}.json")
    }
    /// Generated TypeScript directory.
    pub const TS_DIR: &str = "daemon/src/protocol/generated";
}

/// The repository root, from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// This build's protocol version number, for generated constants. Read here only to write
/// it into generated files; no logic depends on it.
pub(crate) fn version_numbers() -> (u16, u16, u16) {
    crate::dialect::codegen_numbers()
}

fn class_name(class: Class) -> &'static str {
    match class {
        Class::Control => "control",
        Class::Bulk => "bulk",
    }
}

/// Builds the JSON Schema of the protocol as it is now.
pub fn current_schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let mut methods = Vec::new();
    for info in REQUESTS {
        let params = (info.params_schema)(&mut generator);
        let result = (info.result_schema)(&mut generator);
        methods.push(json!({
            "name": info.name,
            "doc": info.doc,
            "direction": info.direction.as_str(),
            "frozen": info.frozen,
            "stateChanging": info.state_changing,
            "requestClass": class_name(info.request_class),
            "responseClass": class_name(info.response_class),
            "params": params,
            "result": result,
        }));
    }
    let mut notifications = Vec::new();
    for info in NOTIFICATIONS {
        let params = (info.params_schema)(&mut generator);
        notifications.push(json!({
            "name": info.name,
            "doc": info.doc,
            "direction": info.direction.as_str(),
            "class": class_name(info.class),
            "params": params,
        }));
    }
    // Envelope types are not reachable from params and results; add them explicitly.
    let _ = generator.subschema_for::<RpcId>();
    let _ = generator.subschema_for::<ErrorObject>();
    let _ = generator.subschema_for::<ErrorCode>();
    // Error data types, carried in `ErrorObject.data`.
    let _ = generator.subschema_for::<crate::types::StoreChangeData>();
    let mut defs: Map<String, Value> = generator.take_definitions(true).into_iter().collect();
    for (name, schema) in crate::rpc::envelope_schemas() {
        defs.insert(name.to_owned(), schema);
    }
    let (_, _, version) = version_numbers();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": format!("YAAC protocol v{version}"),
        "description": "The YAAC wire protocol (host ⇄ yaacd). Generated from the Rust types in crates/yaac-protocol; never edit by hand. Members marked x-yaac-unknown stand for values a newer peer may send: they must be preserved, never rejected.",
        "x-yaac-protocol-version": version,
        "x-yaac-frame": {
            "lengthPrefix": "u32 little-endian; counts the kind byte and the payload",
            "maxFrameLen": frame::MAX_FRAME_LEN,
            "kinds": { "message": frame::KIND_MESSAGE, "data": frame::KIND_DATA },
            "dataChunkMin": schedule::CHUNK_MIN,
            "dataChunkMax": schedule::CHUNK_MAX,
        },
        "x-yaac-methods": methods,
        "x-yaac-notifications": notifications,
        "$defs": defs,
    })
}

/// Serialises a schema the way snapshots are written: pretty, sorted keys, trailing newline.
pub fn render_schema(schema: &Value) -> String {
    let mut text = serde_json::to_string_pretty(schema).unwrap_or_default();
    text.push('\n');
    text
}

/// A file to generate, relative to the repository root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the repository root.
    pub path: String,
    /// Full contents.
    pub contents: String,
}

/// Every generated TypeScript file, from the current schema. `schema_hash` is the hash of the
/// baseline of the current version (what this build sends as `schemaHash`).
pub fn generated_files(schema_hash: &str) -> Vec<GeneratedFile> {
    let schema = current_schema();
    ts::generate(&schema, schema_hash)
        .into_iter()
        .map(|(name, contents)| GeneratedFile { path: format!("{}/{name}", paths::TS_DIR), contents })
        .collect()
}

/// Generated files whose committed copy differs (or is missing), with the expected contents.
pub fn stale_files(root: &Path, files: &[GeneratedFile]) -> Vec<String> {
    files
        .iter()
        .filter(|file| std::fs::read_to_string(root.join(&file.path)).ok().as_deref() != Some(file.contents.as_str()))
        .map(|file| file.path.clone())
        .collect()
}

/// This build's `PROTOCOL_VERSION`, as a number, for naming the snapshot file.
pub fn current_version() -> u16 {
    version_numbers().2
}
