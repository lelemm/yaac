//! Uploads (docs/03-protocol.md#uploads, docs/06-attachments.md). Bytes travel in `0x02` data
//! frames under credit; credit is granted with `stream/credit`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Sha256Hex, StreamId, UploadId};

/// `upload/begin`: declare a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadBeginParams {
    /// Original file name; `yaacd` sanitises it.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Media type, when known (sniffed from content, not the extension).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Full SHA-256 of the content.
    pub sha256: Sha256Hex,
}

/// `upload/begin` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadBeginResult {
    /// The upload.
    pub upload_id: UploadId,
    /// The content is already stored: send no bytes and no `upload/end`.
    #[serde(default)]
    pub exists: bool,
    /// The data stream to send the bytes on, when `exists` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamId>,
    /// Initial credit in bytes on that stream. More arrives in `stream/credit`.
    #[serde(default)]
    pub credit: u64,
}

/// `upload/end`: all bytes sent; verify.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadEndParams {
    /// The upload.
    pub upload_id: UploadId,
}

/// `upload/end` result (or the error `upload_mismatch`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadEndResult {
    /// The upload, now usable in input parts on this target.
    pub upload_id: UploadId,
    /// Bytes stored.
    pub size: u64,
}

/// `upload/cancel`: abort; the daemon removes the partial file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadCancelParams {
    /// The upload.
    pub upload_id: UploadId,
}

/// `upload/read`: a bounded read of a stored attachment (thumbnails on another host).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadReadParams {
    /// The upload.
    pub upload_id: UploadId,
    /// Byte offset.
    #[serde(default)]
    pub offset: u64,
    /// Upper bound on bytes returned. The daemon may return fewer.
    pub max_bytes: u64,
}

/// `upload/read` result. Sent in the bulk class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadReadResult {
    /// Whether the file is still on the target. When false, the other fields are empty.
    pub present: bool,
    /// The bytes, standard base64 with padding.
    #[serde(default)]
    pub data_base64: String,
    /// Total size of the stored file.
    #[serde(default)]
    pub size: u64,
    /// Media type, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Whether bytes remain after this read.
    #[serde(default)]
    pub more: bool,
}

/// `stream/credit` notification: the receiver of a data stream grants the sender more bytes.
/// Either side may send it, for a stream it receives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamCreditParams {
    /// The stream.
    pub stream: StreamId,
    /// Additional bytes the sender may send.
    pub bytes: u64,
}
