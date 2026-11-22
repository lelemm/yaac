//! Input parts of a user message (docs/03-protocol.md#input-parts, docs/06-attachments.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::UploadId;
use crate::macros::{open_enum, open_union};

open_enum! {
    /// What a text part of a message is, when it is not text the user typed. Set by `yaacd`
    /// when it splits a stored or streamed message along the harness's own markup
    /// (docs/03-protocol.md#input-parts); a client never sends it.
    pub enum TextRole fallback Unknown {
        /// Text the user pasted, which the harness kept apart from what they typed.
        Pasted = "pasted",
        /// A command the user ran: a slash command ("/name args") or a shell command ("!cmd").
        Command = "command",
        /// Context the harness added for the model (a system reminder, environment details,
        /// hook output, an IDE selection, YAAC's own handoff note). Not the user's text: shown
        /// only on request.
        Context = "context",
    }
}

/// Plain text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    /// The text, with the harness's markup taken off.
    pub text: String,
    /// What the text is. Absent: text the user typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<TextRole>,
    /// A short name for what it is ("System reminder", "IDE selection"), for `context`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// An upload given to the model as real image input. Only valid when the thread's effective
/// `imageInput` is not `unsupported`; `yaacd` rejects it with `capability_missing` otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImagePart {
    /// The upload, on this target.
    pub upload_id: UploadId,
    /// The file's name, as uploaded (sanitised). Filled in by `yaacd` when it journals the
    /// message; a client need not send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type `yaacd` recorded for the upload. Filled in like `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// An upload referenced by its path on the target; the agent reads it with its own tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    /// The upload, on this target.
    pub upload_id: UploadId,
    /// The file's name, as uploaded (sanitised). Filled in by `yaacd` when it journals the
    /// message; a client need not send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type `yaacd` recorded for the upload. Filled in like `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
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
        InputPart::Text(TextPart { text: text.into(), role: None, label: None })
    }

    /// A text part with a role.
    pub fn text_as(text: impl Into<String>, role: TextRole, label: Option<&str>) -> Self {
        InputPart::Text(TextPart { text: text.into(), role: Some(role), label: label.map(str::to_owned) })
    }
}
