//! The current protocol (version 1), complete and readable top to bottom.
//!
//! Every function here is one step of [`super::Dialect`]. Each takes domain values and
//! produces raw wire JSON, or the reverse; none mentions a wire *struct* in its signature, so
//! an older dialect can override a step to produce a shape the current types no longer have.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{Dialect, DialectError, FeatureSet, Inbound, PeerRequest};
use crate::flows::{OpenThread, StartThread, Started};
use crate::methods::names;
use crate::types::{
    Item, PageCursor, SubscribeMode, ThreadEventParams, ThreadReadParams, ThreadReadResult, ThreadStartParams,
    ThreadStartResult, ThreadSubscribeParams, ThreadSubscribeResult, ThreadSummary,
};

fn decode<T: DeserializeOwned>(context: &'static str, wire: Value) -> Result<T, DialectError> {
    serde_json::from_value(wire).map_err(DialectError::decode(context))
}

fn encode<T: serde::Serialize>(context: &'static str, value: &T) -> Result<Value, DialectError> {
    serde_json::to_value(value).map_err(DialectError::decode(context))
}

/// Version 1 has every wire feature.
pub(super) fn features() -> FeatureSet {
    FeatureSet::FULL
}

/// Current shapes are the wire shapes.
pub(super) fn encode_request(_: &Dialect, _method: &str, params: Value) -> Result<Value, DialectError> {
    Ok(params)
}

/// Current shapes are the wire shapes.
pub(super) fn decode_result(_: &Dialect, _method: &str, result: Value) -> Result<Value, DialectError> {
    Ok(result)
}

/// Current shapes are the wire shapes.
pub(super) fn encode_notification(_: &Dialect, _method: &str, params: Value) -> Result<Value, DialectError> {
    Ok(params)
}

/// Decodes the notifications the host understands; anything else is passed through as
/// [`Inbound::Other`] and ignored by callers.
pub(super) fn decode_notification(d: &Dialect, method: &str, params: Value) -> Result<Inbound, DialectError> {
    Ok(match method {
        names::THREAD_EVENT => Inbound::ThreadEvent(d.decode_event(params)?),
        names::THREAD_UPDATED => {
            let Value::Object(mut map) = params else {
                return Err(DialectError::Shape("thread/updated params must be an object"));
            };
            let thread = map.remove("thread").ok_or(DialectError::Shape("thread/updated without thread"))?;
            let thread = d.decode_summary(thread)?;
            let rest: ThreadUpdatedTokenOnly = decode("thread/updated", Value::Object(map))?;
            Inbound::ThreadUpdated { thread, token: rest.token }
        }
        names::THREAD_REMOVED => Inbound::ThreadRemoved(decode("thread/removed", params)?),
        names::PROVIDER_UPDATED => Inbound::ProviderUpdated(decode("provider/updated", params)?),
        names::TUNNEL_CLOSE => Inbound::TunnelClose(decode("tunnel/close", params)?),
        names::STREAM_CREDIT => Inbound::StreamCredit(decode("stream/credit", params)?),
        names::PROJECT_UPDATED => Inbound::ProjectUpdated(decode("project/updated", params)?),
        other => Inbound::Other { method: other.to_owned(), params },
    })
}

/// The part of `thread/updated` other than the summary (`ThreadUpdatedParams` without
/// `thread`, which goes through the summary step).
#[derive(serde::Deserialize)]
struct ThreadUpdatedTokenOnly {
    token: crate::types::SyncToken,
}

/// Decodes the requests a daemon may originate (the mesh exception).
pub(super) fn decode_peer_request(_: &Dialect, method: &str, params: Value) -> Result<PeerRequest, DialectError> {
    Ok(match method {
        names::MESH_RELAY => PeerRequest::MeshRelay(decode("mesh/relay", params)?),
        names::MESH_DELIVERY_RECEIPT => PeerRequest::MeshDeliveryReceipt(decode("mesh/deliveryReceipt", params)?),
        names::BROWSER_CALL => PeerRequest::BrowserCall(decode("browser/call", params)?),
        names::MESH_QUERY => PeerRequest::MeshQuery(decode("mesh/query", params)?),
        names::MESH_USAGE => PeerRequest::MeshUsage(decode("mesh/usage", params)?),
        other => PeerRequest::Other { method: other.to_owned(), params },
    })
}

/// Current shapes are the wire shapes.
pub(super) fn encode_peer_result(_: &Dialect, _method: &str, result: Value) -> Result<Value, DialectError> {
    Ok(result)
}

pub(super) fn decode_summary(_: &Dialect, wire: Value) -> Result<ThreadSummary, DialectError> {
    decode("thread summary", wire)
}

pub(super) fn decode_item(_: &Dialect, wire: Value) -> Result<Item, DialectError> {
    decode("item", wire)
}

pub(super) fn decode_event(_: &Dialect, wire: Value) -> Result<ThreadEventParams, DialectError> {
    decode("thread/event", wire)
}

/// `thread/start` is one combined call: create the thread and send its first message.
pub(super) fn build_start(_: &Dialect, command: &StartThread) -> Result<Value, DialectError> {
    let params = ThreadStartParams {
        op_id: command.op_id.clone(),
        instance: command.instance.clone(),
        cwd: command.cwd.clone(),
        project: command.project.clone(),
        agents: command.agents.clone(),
        model: command.model.clone(),
        effort: command.effort.clone(),
        mode: command.mode.clone(),
        fast: command.fast,
        ultrafast: command.ultrafast,
        input: command.input.clone(),
    };
    encode("thread/start", &params)
}

pub(super) fn decode_started(_: &Dialect, wire: Value) -> Result<Started, DialectError> {
    let result: ThreadStartResult = decode("thread/start result", wire)?;
    Ok(Started { thread_id: result.thread_id, turn_id: result.turn_id })
}

pub(super) fn build_subscribe(_: &Dialect, command: &OpenThread) -> Result<Value, DialectError> {
    let params = ThreadSubscribeParams {
        thread_id: command.thread_id.clone(),
        epoch: command.after.map(|p| p.epoch),
        after_seq: command.after.map(|p| p.seq),
    };
    encode("thread/subscribe", &params)
}

pub(super) fn decode_subscribed(_: &Dialect, wire: Value) -> Result<ThreadSubscribeResult, DialectError> {
    decode("thread/subscribe result", wire)
}

/// After a replay nothing needs reading. After a reset — or a mode this build does not know,
/// which is handled as a reset — the first page of a fresh snapshot is read.
pub(super) fn read_first_page(
    _: &Dialect,
    command: &OpenThread,
    subscribed: &ThreadSubscribeResult,
) -> Result<Option<Value>, DialectError> {
    if matches!(subscribed.mode, SubscribeMode::Replay(_)) {
        return Ok(None);
    }
    let params = ThreadReadParams { thread_id: command.thread_id.clone(), cursor: None, max_bytes: command.page_bytes, max_items: None, around: None, whole_turns: None };
    encode("thread/read", &params).map(Some)
}

pub(super) fn read_next_page(_: &Dialect, command: &OpenThread, cursor: &PageCursor) -> Result<Value, DialectError> {
    let params = ThreadReadParams {
        thread_id: command.thread_id.clone(),
        cursor: Some(cursor.clone()),
        max_bytes: command.page_bytes,
        max_items: None,
        around: None,
        whole_turns: None,
    };
    encode("thread/read", &params)
}

/// Decodes a page; each item goes through the dialect's item step.
pub(super) fn decode_page(d: &Dialect, wire: Value) -> Result<ThreadReadResult, DialectError> {
    let Value::Object(mut map) = wire else {
        return Err(DialectError::Shape("thread/read result must be an object"));
    };
    let items = match map.remove("items") {
        Some(Value::Array(items)) => items.into_iter().map(|item| d.decode_item(item)).collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(DialectError::Shape("thread/read items must be an array")),
        None => Vec::new(),
    };
    map.insert("items".into(), Value::Array(Vec::new()));
    let mut page: ThreadReadResult = decode("thread/read result", Value::Object(map))?;
    page.items = items;
    Ok(page)
}
