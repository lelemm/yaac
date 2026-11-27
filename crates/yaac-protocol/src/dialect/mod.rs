//! Protocol versions, negotiation, and dialects (docs/09-compatibility.md).
//!
//! **This module is the only place in the program where a protocol version number can be
//! read.** [`ProtocolVersion`], [`ProtocolRange`], [`AgreedProtocol`], and [`Negotiated`] keep
//! their numbers in private fields with no accessor, no ordering, and no equality, so code
//! outside this module cannot name, compare, or branch on a version. It asks the selected
//! [`Dialect`] to do a step instead, or checks a capability.
//!
//! A dialect is a **table of functions**, one entry per overridable step. The current
//! protocol is [`CURRENT`], complete, in [`current`]. An older version would be a `const`
//! built with struct-update syntax from the next-newer one — `Dialect { step: vN::step,
//! ..NEWER }` — overriding only what differed. With protocol version 1 there is exactly one
//! dialect. The procedure for an incompatible change is in this directory's `README.md`.
//!
//! ## The version cannot be read from outside
//!
//! Each of the following fails to compile anywhere outside this module. (They are
//! `compile_fail` doctests, so `cargo test` proves it.)
//!
//! The version's number is a private field:
//! ```compile_fail
//! let n: u16 = yaac_protocol::dialect::PROTOCOL_VERSION.0;
//! ```
//! Versions have no equality, so they cannot be compared to branch on:
//! ```compile_fail
//! use yaac_protocol::dialect::PROTOCOL_VERSION;
//! let newer = PROTOCOL_VERSION == PROTOCOL_VERSION;
//! ```
//! …and no ordering:
//! ```compile_fail
//! use yaac_protocol::dialect::PROTOCOL_VERSION;
//! let newer = PROTOCOL_VERSION > PROTOCOL_VERSION;
//! ```
//! The negotiated version inside [`Negotiated`] is private, and has no accessor:
//! ```compile_fail
//! fn agreed(n: &yaac_protocol::dialect::Negotiated) { let _ = n.agreed; }
//! ```
//! ```compile_fail
//! fn agreed(n: &yaac_protocol::dialect::Negotiated) { let _ = n.agreed(); }
//! ```
//! A range's bounds are private:
//! ```compile_fail
//! let min = yaac_protocol::dialect::SUPPORTED.min;
//! ```
//! A dialect's version is private:
//! ```compile_fail
//! let v = yaac_protocol::dialect::CURRENT.version;
//! ```
//! An older dialect can only be built inside this module (its fields are private):
//! ```compile_fail
//! use yaac_protocol::dialect::{CURRENT, Dialect};
//! const V0: Dialect = Dialect { ..CURRENT };
//! ```
//! What the rest of the program *can* do — negotiate, select, and ask questions that do not
//! reveal a number — compiles:
//! ```
//! use yaac_protocol::dialect::{self, SUPPORTED};
//! let negotiated = dialect::negotiate(SUPPORTED, SUPPORTED).unwrap();
//! let dialect = dialect::select(&negotiated).expect("a dialect for the agreed version");
//! assert!(!negotiated.peer_is_older());
//! let _hash: &str = dialect.schema_hash();
//! ```

mod current;

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::flows::{OpenThread, StartThread, Started};
use crate::rpc::{ErrorCode, ErrorObject};
use crate::types::{
    Capability, Item, MeshQueryParams, MeshReceiptParams, MeshRelayParams, MeshUsageParams, ProjectListResult, PageCursor, ProviderUpdatedParams, StreamCreditParams,
    ThreadEventParams, ThreadReadResult, ThreadRemovedParams, ThreadSubscribeResult, ThreadSummary, SyncToken,
};

// ── Versions ──────────────────────────────────────────────────────────────────────────────

/// A protocol version. Opaque outside this module: it cannot be read, compared, or ordered.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolVersion(u16);

impl fmt::Debug for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// The protocol version this build speaks natively.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// How many protocol versions older than its own each side supports.
const WINDOW: u16 = 3;

/// The versions this build speaks: its own and up to [`WINDOW`] older ones (never below 1).
pub const SUPPORTED: ProtocolRange = ProtocolRange {
    min: ProtocolVersion(if PROTOCOL_VERSION.0 > WINDOW { PROTOCOL_VERSION.0 - WINDOW } else { 1 }),
    max: PROTOCOL_VERSION,
};

/// A range of protocol versions, `{ "min": …, "max": … }` on the wire. Frozen core.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ProtocolRange {
    min: ProtocolVersion,
    max: ProtocolVersion,
}

/// The daemon's range and the version it agreed, `{ "min", "max", "agreed" }`. Frozen core.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AgreedProtocol {
    min: ProtocolVersion,
    max: ProtocolVersion,
    agreed: ProtocolVersion,
}

impl AgreedProtocol {
    /// The range part.
    pub fn range(&self) -> ProtocolRange {
        ProtocolRange { min: self.min, max: self.max }
    }
}

impl JsonSchema for ProtocolVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ProtocolVersion".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A protocol version number. Read only by the dialect layer.",
            "type": "integer", "format": "uint16", "minimum": 1, "maximum": 65535
        })
    }
}

impl JsonSchema for ProtocolRange {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ProtocolRange".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let version = generator.subschema_for::<ProtocolVersion>();
        schemars::json_schema!({
            "description": "The protocol versions one side speaks. Frozen core.",
            "type": "object",
            "properties": { "min": &version, "max": &version },
            "required": ["min", "max"]
        })
    }
}

impl JsonSchema for AgreedProtocol {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AgreedProtocol".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let version = generator.subschema_for::<ProtocolVersion>();
        schemars::json_schema!({
            "description": "The daemon's range and the version agreed: the lower of the two maxima, if it is not below either minimum. Frozen core.",
            "type": "object",
            "properties": { "min": &version, "max": &version, "agreed": &version },
            "required": ["min", "max", "agreed"]
        })
    }
}

// ── Negotiation ───────────────────────────────────────────────────────────────────────────

/// Which side of a failed negotiation is too old.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OlderSide {
    /// This side (it must be updated).
    Local,
    /// The peer.
    Remote,
}

/// The outcome of a successful negotiation. Holds the agreed version privately; the only
/// thing the rest of the program can do with it is [`select`] a dialect and ask yes/no
/// questions that do not reveal the number.
#[derive(Clone, Copy, Debug)]
pub struct Negotiated {
    agreed: ProtocolVersion,
    local: ProtocolRange,
    remote: ProtocolRange,
}

impl Negotiated {
    /// Whether the peer is older than this build — i.e. this side is adapting. The UI uses it
    /// for "This target is running an older YAAC daemon" and to offer **Update now**.
    pub fn peer_is_older(&self) -> bool {
        self.remote.max.0 < self.local.max.0
    }

    /// Whether this build is older than the peer (the peer adapts to us).
    pub fn local_is_older(&self) -> bool {
        self.local.max.0 < self.remote.max.0
    }

    /// The `protocol` member of an `initialize` result answering this negotiation, for a
    /// daemon-role implementation (test doubles, the attach server's own handshake).
    pub fn answer(&self) -> AgreedProtocol {
        AgreedProtocol { min: self.local.min, max: self.local.max, agreed: self.agreed }
    }
}

/// No common version.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("{}", self.message())]
pub struct NegotiationError {
    local: ProtocolRange,
    remote: ProtocolRange,
}

impl NegotiationError {
    /// Which side is too old.
    pub fn older_side(&self) -> OlderSide {
        if self.local.max.0 < self.remote.min.0 { OlderSide::Local } else { OlderSide::Remote }
    }

    /// A human-readable message naming both ranges and which side to update.
    pub fn message(&self) -> String {
        let (l, r) = (self.local, self.remote);
        let which = match self.older_side() {
            OlderSide::Local => "this side is too old and must be updated",
            OlderSide::Remote => "the other side is too old and must be updated",
        };
        format!(
            "no common protocol version: this side speaks {}–{}, the other side speaks {}–{}; {which}",
            l.min.0, l.max.0, r.min.0, r.max.0
        )
    }

    /// The `protocol_unsupported` error object to answer `initialize` with, from the daemon
    /// role (`local` is the daemon): the message names both ranges and which side to update,
    /// and `data` carries the ranges and `tooOld` (`"client"` or `"daemon"`).
    pub fn to_error_object(&self) -> ErrorObject {
        let (daemon, client) = (self.local, self.remote);
        let (too_old, advice) = match self.older_side() {
            OlderSide::Local => ("daemon", "the daemon on this target is too old for this YAAC; update it"),
            OlderSide::Remote => ("client", "this YAAC is too old for the daemon on this target; update YAAC"),
        };
        let message = format!(
            "no common protocol version: the daemon speaks {}–{}, the client speaks {}–{}; {advice}",
            daemon.min.0, daemon.max.0, client.min.0, client.max.0
        );
        ErrorObject::new(ErrorCode::ProtocolUnsupported, message).with_data(json!({
            "daemon": { "min": daemon.min.0, "max": daemon.max.0 },
            "client": { "min": client.min.0, "max": client.max.0 },
            "tooOld": too_old,
        }))
    }
}

/// Agrees on a version: the lower of the two maxima, provided it is not below either minimum.
pub fn negotiate(local: ProtocolRange, remote: ProtocolRange) -> Result<Negotiated, NegotiationError> {
    let agreed = local.max.0.min(remote.max.0);
    if agreed < local.min.0 || agreed < remote.min.0 || local.min.0 > local.max.0 || remote.min.0 > remote.max.0 {
        return Err(NegotiationError { local, remote });
    }
    Ok(Negotiated { agreed: ProtocolVersion(agreed), local, remote })
}

/// The client role: checks the daemon's `initialize` answer. The daemon's `agreed` must be
/// exactly what negotiation over both ranges yields; anything else is treated as no common
/// version, because the two sides would disagree about the wire.
pub fn confirm(local: ProtocolRange, answer: &AgreedProtocol) -> Result<Negotiated, NegotiationError> {
    let negotiated = negotiate(local, answer.range())?;
    if negotiated.agreed.0 != answer.agreed.0 {
        return Err(NegotiationError { local, remote: answer.range() });
    }
    Ok(negotiated)
}

/// What a `protocol_unsupported` error received from the daemon says, without exposing the
/// version numbers: which side must be updated. `None` if it is not such an error.
pub fn describe_unsupported(error: &ErrorObject) -> Option<OlderSide> {
    if error.code != ErrorCode::ProtocolUnsupported {
        return None;
    }
    match error.data.as_ref()?.get("tooOld")?.as_str()? {
        "client" => Some(OlderSide::Local),
        "daemon" => Some(OlderSide::Remote),
        _ => None,
    }
}

// ── Dialects ──────────────────────────────────────────────────────────────────────────────

/// A dialect step failed: the wire value did not have the shape this version uses.
#[derive(Debug, thiserror::Error)]
pub enum DialectError {
    /// The value did not decode.
    #[error("{context}: {source}")]
    Decode {
        /// What was being decoded.
        context: &'static str,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// The value had the wrong shape.
    #[error("{0}")]
    Shape(&'static str),
}

impl DialectError {
    pub(crate) fn decode(context: &'static str) -> impl FnOnce(serde_json::Error) -> DialectError {
        move |source| DialectError::Decode { context, source }
    }
}

/// Wire-level features a version has. A version that lacks one **withholds** the matching
/// capabilities, which are then removed from every capability set the UI sees, so a control
/// is greyed out for the usual reason — a capability is absent.
#[derive(Clone, Copy, Debug, Default)]
pub struct FeatureSet {
    withheld: &'static [Capability],
    missing_port_forward: bool,
    missing_file_view: bool,
    missing_terminal: bool,
    missing_budget_time_limit: bool,
    missing_voice: bool,
}

impl FeatureSet {
    /// Every feature present.
    pub const FULL: FeatureSet = FeatureSet { withheld: &[], missing_port_forward: false, missing_file_view: false, missing_terminal: false, missing_budget_time_limit: false, missing_voice: false };

    /// A version that lacks the wire support for `withheld`.
    pub const fn withholding(withheld: &'static [Capability]) -> Self {
        Self { withheld, ..Self::FULL }
    }

    pub(crate) fn advertised(mut self, features: &[Capability]) -> Self {
        self.missing_port_forward = !features.contains(&Capability::PortForward);
        self.missing_terminal = !features.contains(&Capability::Terminal);
        self.missing_file_view = !features.contains(&Capability::FileView);
        self.missing_budget_time_limit = !features.contains(&Capability::BudgetTimeLimit);
        self.missing_voice = !features.contains(&Capability::VoiceInput);
        self
    }

    /// Refine additive features from the methods actually implemented by this peer.
    pub(crate) fn observe_missing(&mut self, method: &str) {
        if matches!(method, crate::methods::names::TUNNEL_OPEN | crate::methods::names::PREVIEW_PORTS) {
            self.missing_port_forward = true;
        }
        if method.starts_with("terminal/") { self.missing_terminal = true; }
        if method.starts_with("voice/") { self.missing_voice = true; }
        if matches!(method, crate::methods::names::FS_TREE | crate::methods::names::FS_READ | crate::methods::names::GIT_FILE_DIFF) {
            self.missing_file_view = true;
        }
    }

    /// Removes the capabilities this version cannot carry.
    pub fn fold(&self, capabilities: &mut Vec<Capability>) {
        capabilities.retain(|c| self.allows(c));
    }

    /// Whether this version can carry `capability`.
    pub fn allows(&self, capability: &Capability) -> bool {
        !(self.withheld.contains(capability)
            || self.missing_port_forward && *capability == Capability::PortForward
            || self.missing_terminal && *capability == Capability::Terminal
            || self.missing_file_view && *capability == Capability::FileView
            || self.missing_budget_time_limit && *capability == Capability::BudgetTimeLimit
            || self.missing_voice && *capability == Capability::VoiceInput)
    }
}

/// A typed notification from the daemon, decoded by the dialect. Moved once from the session
/// to its caller; boxing the summary would only make every match clumsier.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbound {
    /// `thread/event`.
    ThreadEvent(ThreadEventParams),
    /// `thread/updated`.
    ThreadUpdated {
        /// The new summary.
        thread: ThreadSummary,
        /// The index token including the change.
        token: SyncToken,
    },
    /// `thread/removed`.
    ThreadRemoved(ThreadRemovedParams),
    /// `provider/updated`.
    ProviderUpdated(ProviderUpdatedParams),
    /// A tunnel socket closed.
    TunnelClose(crate::TunnelCloseParams),
    /// `stream/credit`.
    StreamCredit(StreamCreditParams),
    /// `project/updated`: every project on the target.
    ProjectUpdated(ProjectListResult),
    /// A notification this build does not know. Ignore it (forward compatibility).
    Other {
        /// Method.
        method: String,
        /// Params as received.
        params: Value,
    },
}

/// A typed request originated by the daemon (the mesh exception), decoded by the dialect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerRequest {
    /// Inspect a thread’s browser.
    BrowserCall(crate::BrowserCallParams),
    /// Read the hub's skill catalog.
    CatalogSkills(crate::CatalogSkillsParams),
    /// Window appearance operation.
    ThemeCall(crate::ThemeCallParams),
    /// `mesh/relay`.
    MeshRelay(MeshRelayParams),
    /// `mesh/deliveryReceipt`.
    MeshDeliveryReceipt(MeshReceiptParams),
    /// `mesh/query`.
    MeshQuery(MeshQueryParams),
    /// `mesh/usage`.
    MeshUsage(MeshUsageParams),
    /// A request this build does not know: answer `method_not_found`.
    Other {
        /// Method.
        method: String,
        /// Params as received.
        params: Value,
    },
}

/// One protocol version's translation between the wire and the version-free model.
///
/// Every field is a step: domain values in, raw JSON out (or the reverse). Steps that call
/// other steps receive the selected `&Dialect` and call through it, so an older dialect's
/// overrides are honoured all the way down. Fields are private to this module; an older
/// dialect lives in a child module and is built with `Dialect { …, ..NEWER }`.
pub struct Dialect {
    /// The version this dialect speaks. Read only by [`select`] and the tests below.
    version: ProtocolVersion,
    /// Hash of the frozen baseline schema of this version (`protocol/schema/v<n>.json`).
    schema_hash: &'static str,
    features: fn() -> FeatureSet,

    // Generic message paths.
    encode_request: fn(&Dialect, &str, Value) -> Result<Value, DialectError>,
    decode_result: fn(&Dialect, &str, Value) -> Result<Value, DialectError>,
    encode_notification: fn(&Dialect, &str, Value) -> Result<Value, DialectError>,
    decode_notification: fn(&Dialect, &str, Value) -> Result<Inbound, DialectError>,
    decode_peer_request: fn(&Dialect, &str, Value) -> Result<PeerRequest, DialectError>,
    encode_peer_result: fn(&Dialect, &str, Value) -> Result<Value, DialectError>,

    // Shared decoders.
    decode_summary: fn(&Dialect, Value) -> Result<ThreadSummary, DialectError>,
    decode_item: fn(&Dialect, Value) -> Result<Item, DialectError>,
    decode_event: fn(&Dialect, Value) -> Result<ThreadEventParams, DialectError>,

    // Flow: start a thread.
    build_start: fn(&Dialect, &StartThread) -> Result<Value, DialectError>,
    decode_started: fn(&Dialect, Value) -> Result<Started, DialectError>,

    // Flow: open a thread.
    build_subscribe: fn(&Dialect, &OpenThread) -> Result<Value, DialectError>,
    decode_subscribed: fn(&Dialect, Value) -> Result<ThreadSubscribeResult, DialectError>,
    read_first_page: fn(&Dialect, &OpenThread, &ThreadSubscribeResult) -> Result<Option<Value>, DialectError>,
    read_next_page: fn(&Dialect, &OpenThread, &PageCursor) -> Result<Value, DialectError>,
    decode_page: fn(&Dialect, Value) -> Result<ThreadReadResult, DialectError>,
}

impl fmt::Debug for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately does not print the version.
        f.debug_struct("Dialect").field("schema_hash", &self.schema_hash).finish_non_exhaustive()
    }
}

/// The current protocol, complete. The whole of it lives in [`current`].
pub const CURRENT: Dialect = Dialect {
    version: PROTOCOL_VERSION,
    schema_hash: crate::codegen::SCHEMA_HASH_V1,
    features: current::features,
    encode_request: current::encode_request,
    decode_result: current::decode_result,
    encode_notification: current::encode_notification,
    decode_notification: current::decode_notification,
    decode_peer_request: current::decode_peer_request,
    encode_peer_result: current::encode_peer_result,
    decode_summary: current::decode_summary,
    decode_item: current::decode_item,
    decode_event: current::decode_event,
    build_start: current::build_start,
    decode_started: current::decode_started,
    build_subscribe: current::build_subscribe,
    decode_subscribed: current::decode_subscribed,
    read_first_page: current::read_first_page,
    read_next_page: current::read_next_page,
    decode_page: current::decode_page,
};

/// Every dialect, newest first. The compatibility window allows at most four: the current
/// one plus three older ones.
const ALL: &[&Dialect] = &[&CURRENT];

/// Selects the dialect for a negotiated version. **The only function in the program that
/// looks at a protocol version number.**
pub fn select(negotiated: &Negotiated) -> Option<&'static Dialect> {
    ALL.iter().copied().find(|dialect| dialect.version.0 == negotiated.agreed.0)
}

impl Dialect {
    /// Hash of this version's frozen baseline schema, to compare with the peer's `schemaHash`.
    pub fn schema_hash(&self) -> &'static str {
        self.schema_hash
    }

    /// The wire-level features of this version.
    pub fn features(&self) -> FeatureSet {
        (self.features)()
    }

    /// Current-shape request params → wire params for `method`.
    pub fn encode_request(&self, method: &str, params: Value) -> Result<Value, DialectError> {
        (self.encode_request)(self, method, params)
    }

    /// Wire result of `method` → current-shape result.
    pub fn decode_result(&self, method: &str, result: Value) -> Result<Value, DialectError> {
        (self.decode_result)(self, method, result)
    }

    /// Current-shape notification params → wire params (for notifications this side sends).
    pub fn encode_notification(&self, method: &str, params: Value) -> Result<Value, DialectError> {
        (self.encode_notification)(self, method, params)
    }

    /// Wire notification → typed domain notification.
    pub fn decode_notification(&self, method: &str, params: Value) -> Result<Inbound, DialectError> {
        (self.decode_notification)(self, method, params)
    }

    /// Wire request from the daemon → typed domain request.
    pub fn decode_peer_request(&self, method: &str, params: Value) -> Result<PeerRequest, DialectError> {
        (self.decode_peer_request)(self, method, params)
    }

    /// Current-shape result for a daemon-originated request → wire result.
    pub fn encode_peer_result(&self, method: &str, result: Value) -> Result<Value, DialectError> {
        (self.encode_peer_result)(self, method, result)
    }

    /// Wire thread summary → domain summary.
    pub fn decode_summary(&self, wire: Value) -> Result<ThreadSummary, DialectError> {
        (self.decode_summary)(self, wire)
    }

    /// Wire item → domain item.
    pub fn decode_item(&self, wire: Value) -> Result<Item, DialectError> {
        (self.decode_item)(self, wire)
    }

    /// Wire `thread/event` params → domain event.
    pub fn decode_event(&self, wire: Value) -> Result<ThreadEventParams, DialectError> {
        (self.decode_event)(self, wire)
    }

    pub(crate) fn build_start(&self, command: &StartThread) -> Result<Value, DialectError> {
        (self.build_start)(self, command)
    }

    pub(crate) fn decode_started(&self, wire: Value) -> Result<Started, DialectError> {
        (self.decode_started)(self, wire)
    }

    pub(crate) fn build_subscribe(&self, command: &OpenThread) -> Result<Value, DialectError> {
        (self.build_subscribe)(self, command)
    }

    pub(crate) fn decode_subscribed(&self, wire: Value) -> Result<ThreadSubscribeResult, DialectError> {
        (self.decode_subscribed)(self, wire)
    }

    pub(crate) fn read_first_page(
        &self,
        command: &OpenThread,
        subscribed: &ThreadSubscribeResult,
    ) -> Result<Option<Value>, DialectError> {
        (self.read_first_page)(self, command, subscribed)
    }

    pub(crate) fn read_next_page(&self, command: &OpenThread, cursor: &PageCursor) -> Result<Value, DialectError> {
        (self.read_next_page)(self, command, cursor)
    }

    pub(crate) fn decode_page(&self, wire: Value) -> Result<ThreadReadResult, DialectError> {
        (self.decode_page)(self, wire)
    }
}

/// Helpers for tests elsewhere in the crate that need ranges. They build values without
/// exposing the numbers they contain.
#[doc(hidden)]
pub mod testing {
    use super::{ProtocolRange, ProtocolVersion};

    /// A range from raw numbers, for negotiation tests and fixtures only.
    pub fn range(min: u16, max: u16) -> ProtocolRange {
        ProtocolRange { min: ProtocolVersion(min), max: ProtocolVersion(max) }
    }
}

/// `(supported min, supported max, PROTOCOL_VERSION)`, for writing generated constants (the
/// TypeScript side has its own dialect module, which reads them). Crate-private.
pub(crate) fn codegen_numbers() -> (u16, u16, u16) {
    (SUPPORTED.min.0, SUPPORTED.max.0, PROTOCOL_VERSION.0)
}

impl Negotiated {
    /// A label for diagnostics ("protocol v1; peer speaks v1–v1"). Display only: diagnostics
    /// list the protocol version, build, and state format separately.
    pub fn diagnostic_label(&self) -> String {
        format!("protocol v{}; peer speaks v{}–v{}", self.agreed.0, self.remote.min.0, self.remote.max.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_dialect_per_supported_version_and_at_most_four() {
        assert!(ALL.len() <= 1 + WINDOW as usize, "retire the oldest dialect (step 7)");
        let span = SUPPORTED.max.0 - SUPPORTED.min.0 + 1;
        assert_eq!(ALL.len(), span as usize, "one dialect per supported version");
        for v in SUPPORTED.min.0..=SUPPORTED.max.0 {
            let n = negotiate(testing::range(v, v), SUPPORTED).expect("in range");
            assert!(select(&n).is_some(), "no dialect for v{v}");
        }
        assert_eq!(ALL[0].version.0, PROTOCOL_VERSION.0, "newest first; CURRENT is the base");
    }
}
