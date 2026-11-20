//! Input parts of a user message (docs/03-protocol.md#input-parts, docs/06-attachments.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::UploadId;
use crate::macros::open_union;

/// Plain text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    /// The text.
    pub text: String,
}

/// An upload given to the model as real image input. Only valid when the thread's effective
/// `imageInput` is not `unsupported`; `yaacd` rejects it with `capability_missing` otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImagePart {
    /// The upload, on this target.
    pub upload_id: UploadId,
}

/// An upload referenced by its path on the target; the agent reads it with its own tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    /// The upload, on this target.
    pub upload_id: UploadId,
}

open_union! {
    /// One part of a user message.
    pub enum InputPart tag "type" {
        /// Text.
        Text(TextPart) = "text",
        /// An image the model sees.
        Image(ImagePart) = "image",
        /// A file referenced by path.
        File(FilePart) = "file",
    }
}

impl InputPart {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        InputPart::Text(TextPart { text: text.into() })
    }
}
