//! JSON-RPC 2.0 *shapes* (docs/03-protocol.md#messages). Part of the frozen core.
//!
//! Two deliberate differences from JSON-RPC 2.0: the `"jsonrpc": "2.0"` member is omitted (and
//! ignored if present), and an error's `code` is a stable **string**.
//!
//! Canonical encoding, so the frozen-core byte fixture is reproducible on both sides: compact
//! JSON; the envelope members in a fixed order (`id`, `method`, `params` / `id`, `result` /
//! `id`, `error`; no `id` on a notification); and every object *inside* them with its keys
//! sorted by code point. (This relies on `serde_json` without its `preserve_order` feature;
//! the frozen-core test fails if anything in the workspace turns it on.)

use std::fmt;

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::ser::SerializeMap as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::macros::open_enum;

/// A request id. JSON-RPC allows a number or a string; YAAC sends numbers. Each side has its
/// own id space for the requests it originates.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RpcId {
    /// A numeric id.
    Number(i64),
    /// A string id.
    String(String),
}

impl fmt::Display for RpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpcId::Number(n) => n.fmt(f),
            RpcId::String(s) => f.write_str(s),
        }
    }
}

open_enum! {
    /// A stable error code for the UI to branch on. Open: codes this build does not know are
    /// kept, and the `message` is always shown as-is.
    pub enum ErrorCode fallback Unknown {
        /// The two sides have no protocol version in common. Frozen core.
        ProtocolUnsupported = "protocol_unsupported",
        /// The instance cannot be reached.
        ProviderUnavailable = "provider_unavailable",
        /// A `providerInstance/update` would point the instance at another store and lacks
        /// `confirmStoreChange` (P300). Nothing was changed; `data` is a `StoreChangeData`.
        StoreChange = "store_change",
        /// The harness is not signed in.
        NeedsAuth = "needs_auth",
        /// The instance or model cannot do what was asked.
        CapabilityMissing = "capability_missing",
        /// The thread does not exist on this target.
        ThreadNotFound = "thread_not_found",
        /// The thread is being driven elsewhere; this client may only watch.
        ThreadInUse = "thread_in_use",
        /// A turn is already running.
        TurnInProgress = "turn_in_progress",
        /// The turn id given is not the running one.
        TurnMismatch = "turn_mismatch",
        /// The operation may or may not have taken effect. Never retried automatically.
        OutcomeUnknown = "outcome_unknown",
        /// The uploaded bytes do not match the declared hash or size.
        UploadMismatch = "upload_mismatch",
        /// The upload id is not known on this target.
        UploadNotFound = "upload_not_found",
        /// The provider instance does not exist.
        InstanceNotFound = "instance_not_found",
        /// The model is not in the instance's live model list.
        ModelUnavailable = "model_unavailable",
        /// The request item does not exist.
        RequestNotFound = "request_not_found",
        /// The request is no longer pending (resolved elsewhere, expired, unknown).
        RequestNotPending = "request_not_pending",
        /// A size, count, or mesh limit was hit; the message says which.
        LimitExceeded = "limit_exceeded",
        /// The mesh policy, or the target's remote switch, refused it.
        PolicyDenied = "policy_denied",
        /// The named target does not exist.
        TargetUnknown = "target_unknown",
        /// The named target is not connected.
        TargetOffline = "target_offline",
        /// A method other than the frozen daemon-management calls arrived before `initialize`
        /// succeeded.
        NotInitialized = "not_initialized",
        /// The message was not valid JSON.
        ParseError = "parse_error",
        /// The message was JSON but not a valid request.
        InvalidRequest = "invalid_request",
        /// The method is not known.
        MethodNotFound = "method_not_found",
        /// The params did not decode.
        InvalidParams = "invalid_params",
        /// Something went wrong on the other side.
        InternalError = "internal_error",
    }
}

/// The error object: `code`, `message`, `data`. Frozen core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorObject {
    /// Stable code to branch on.
    pub code: ErrorCode,
    /// Human-readable; shown as-is.
    pub message: String,
    /// Structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    /// An error with no data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// Adds structured data.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for ErrorObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ErrorObject {}

/// One JSON-RPC message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// `{ "id", "method", "params" }`.
    Request {
        /// Id, unique among the sender's outstanding requests.
        id: RpcId,
        /// Method name.
        method: String,
        /// Params object (`{}` when there are none).
        params: Value,
    },
    /// `{ "id", "result" }`.
    Response {
        /// The request's id.
        id: RpcId,
        /// The result.
        result: Value,
    },
    /// `{ "id", "error" }`. `id` is `null` when the request's id could not be read.
    Error {
        /// The request's id.
        id: Option<RpcId>,
        /// The error.
        error: ErrorObject,
    },
    /// `{ "method", "params" }` — no id, no response.
    Notification {
        /// Method name.
        method: String,
        /// Params object.
        params: Value,
    },
}

/// Why a message payload could not be decoded.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// Not valid JSON.
    #[error("message is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Valid JSON, but not one of the four shapes.
    #[error("message is not a JSON-RPC request, response, error, or notification: {0}")]
    Shape(&'static str),
}

impl MessageError {
    /// The error code to answer with.
    pub fn code(&self) -> ErrorCode {
        match self {
            MessageError::Json(_) => ErrorCode::ParseError,
            MessageError::Shape(_) => ErrorCode::InvalidRequest,
        }
    }
}

impl Message {
    /// A request.
    pub fn request(id: RpcId, method: impl Into<String>, params: Value) -> Self {
        Message::Request { id, method: method.into(), params }
    }

    /// A notification.
    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Message::Notification { method: method.into(), params }
    }

    /// The method, for requests and notifications.
    pub fn method(&self) -> Option<&str> {
        match self {
            Message::Request { method, .. } | Message::Notification { method, .. } => Some(method),
            Message::Response { .. } | Message::Error { .. } => None,
        }
    }

    /// Encodes as compact JSON (the payload of a `0x01` frame).
    pub fn to_json(&self) -> Vec<u8> {
        // Serialising a `Message` cannot fail: every part is already a JSON value.
        serde_json::to_vec(self).unwrap_or_else(|_| unreachable_json())
    }

    /// Decodes a `0x01` frame payload.
    pub fn from_json(payload: &[u8]) -> Result<Self, MessageError> {
        let value: Value = serde_json::from_slice(payload)?;
        Self::from_value(value)
    }

    /// Decodes from a parsed JSON value.
    pub fn from_value(value: Value) -> Result<Self, MessageError> {
        let Value::Object(mut map) = value else {
            return Err(MessageError::Shape("not a JSON object"));
        };
        let id = match map.remove("id") {
            None | Some(Value::Null) => None,
            Some(raw) => Some(
                serde_json::from_value::<RpcId>(raw).map_err(|_| MessageError::Shape("id is not a number or string"))?,
            ),
        };
        if let Some(method) = map.remove("method") {
            let Value::String(method) = method else {
                return Err(MessageError::Shape("method is not a string"));
            };
            let params = match map.remove("params") {
                None | Some(Value::Null) => Value::Object(Map::new()),
                Some(params) => params,
            };
            return Ok(match id {
                Some(id) => Message::Request { id, method, params },
                None => Message::Notification { method, params },
            });
        }
        if let Some(error) = map.remove("error") {
            let error = serde_json::from_value::<ErrorObject>(error)?;
            return Ok(Message::Error { id, error });
        }
        if let Some(result) = map.remove("result") {
            let id = id.ok_or(MessageError::Shape("response without id"))?;
            return Ok(Message::Response { id, result });
        }
        Err(MessageError::Shape("no method, result, or error"))
    }
}

#[cold]
fn unreachable_json() -> Vec<u8> {
    b"{}".to_vec()
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        match self {
            Message::Request { id, method, params } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("method", method)?;
                map.serialize_entry("params", params)?;
            }
            Message::Response { id, result } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("result", result)?;
            }
            Message::Error { id, error } => {
                map.serialize_entry("id", id)?;
                // Through `Value`, so its keys are sorted like every other payload object.
                let error = serde_json::to_value(error).map_err(serde::ser::Error::custom)?;
                map.serialize_entry("error", &error)?;
            }
            Message::Notification { method, params } => {
                map.serialize_entry("method", method)?;
                map.serialize_entry("params", params)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Message::from_value(value).map_err(D::Error::custom)
    }
}

/// JSON Schema pieces for the envelope, used by the schema snapshot.
pub(crate) fn envelope_schemas() -> Vec<(&'static str, Value)> {
    use serde_json::json;
    vec![
        (
            "RpcRequest",
            json!({
                "description": "A request. `id` is unique among the sender's outstanding requests.",
                "type": "object",
                "properties": {
                    "id": { "$ref": "#/$defs/RpcId" },
                    "method": { "type": "string" },
                    "params": true
                },
                "required": ["id", "method", "params"]
            }),
        ),
        (
            "RpcResponse",
            json!({
                "description": "A successful response.",
                "type": "object",
                "properties": { "id": { "$ref": "#/$defs/RpcId" }, "result": true },
                "required": ["id", "result"]
            }),
        ),
        (
            "RpcError",
            json!({
                "description": "An error response. `id` is null when the request id could not be read.",
                "type": "object",
                "properties": {
                    "id": { "anyOf": [{ "$ref": "#/$defs/RpcId" }, { "type": "null" }] },
                    "error": { "$ref": "#/$defs/ErrorObject" }
                },
                "required": ["id", "error"]
            }),
        ),
        (
            "RpcNotification",
            json!({
                "description": "A notification: no id, no response.",
                "type": "object",
                "properties": { "method": { "type": "string" }, "params": true },
                "required": ["method", "params"]
            }),
        ),
        (
            "RpcMessage",
            json!({
                "description": "Any message. No `jsonrpc` member is sent; it is ignored if present.",
                "anyOf": [
                    { "$ref": "#/$defs/RpcRequest" },
                    { "$ref": "#/$defs/RpcResponse" },
                    { "$ref": "#/$defs/RpcError" },
                    { "$ref": "#/$defs/RpcNotification" }
                ]
            }),
        ),
    ]
}
