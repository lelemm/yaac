//! Channel connections and runtime-defined external dialogs (docs/16-external-dialogs.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use super::{OpId, RequestId, RequestItem, ThreadId};
use crate::macros::{open_enum, open_union};

open_enum! {
    pub enum DialogTextStyle fallback Unknown {
        Body = "body",
        Heading = "heading",
        Caption = "caption",
        Code = "code",
    }
}

open_enum! {
    pub enum DialogTextTone fallback Unknown {
        Normal = "normal",
        Muted = "muted",
        Warning = "warning",
        Error = "error",
        Success = "success",
    }
}

open_enum! {
    pub enum DialogButtonKind fallback Unknown {
        Primary = "primary",
        Danger = "danger",
        Normal = "normal",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogContainer {
    pub children: Vec<DialogNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<Number>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub children: Vec<DialogNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogText {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<DialogTextStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<DialogTextTone>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogMarkdown {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogImage {
    pub src: String,
    pub alt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<Number>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogLink {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogList {
    pub items: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Progress from 0 to 1, or indeterminate when absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Number>,
}

/// The common properties embedded in every input node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

/// Secret inputs never carry a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogTextInput {
    #[serde(flatten)]
    pub input: DialogInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogNumber {
    #[serde(flatten)]
    pub input: DialogInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Number>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogCheckbox {
    #[serde(flatten)]
    pub input: DialogInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// An integrator-supplied choice, with the wire value and the displayed label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogSelect {
    #[serde(flatten)]
    pub input: DialogInput,
    pub options: Vec<DialogOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogMultiSelect {
    #[serde(flatten)]
    pub input: DialogInput,
    pub options: Vec<DialogOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

/// A calendar date (YYYY-MM-DD).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogDate {
    #[serde(flatten)]
    pub input: DialogInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogSlider {
    #[serde(flatten)]
    pub input: DialogInput,
    pub min: Number,
    pub max: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Number>,
}

/// An action button. Submit defaults to true and validates all enabled required inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogButton {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DialogButtonKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<String>>,
}

open_union! {
    /// A runtime dialog component. Nodes carry a "type" discriminator.
    pub enum DialogNode tag "type" {
        Column(DialogContainer) = "column",
        Row(DialogContainer) = "row",
        Group(DialogGroup) = "group",
        Divider(super::Empty) = "divider",
        Text(DialogText) = "text",
        Markdown(DialogMarkdown) = "markdown",
        Image(DialogImage) = "image",
        Link(DialogLink) = "link",
        List(DialogList) = "list",
        Table(DialogTable) = "table",
        Progress(DialogProgress) = "progress",
        TextInput(DialogTextInput) = "textInput",
        Number(DialogNumber) = "number",
        Checkbox(DialogCheckbox) = "checkbox",
        Toggle(DialogCheckbox) = "toggle",
        Select(DialogSelect) = "select",
        Radio(DialogSelect) = "radio",
        MultiSelect(DialogMultiSelect) = "multiSelect",
        Date(DialogDate) = "date",
        Slider(DialogSlider) = "slider",
        Button(DialogButton) = "button",
    }
}

/// The origin stamped by yaacd from the channel connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogSource {
    pub channel_id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogRequest {
    pub title: String,
    pub layout: DialogNode,
    pub source: DialogSource,
    pub channel_open: bool,
}

/// A button id and input values, or a cancellation. Recorded answers omit secret values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default)]
    pub values: Map<String, Value>,
    #[serde(default)]
    pub cancelled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRegisterParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRegisterResult {
    pub channel_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRegisteredParams {
    pub channel_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelClosedParams {
    pub channel_id: String,
}

/// The request id is unique on the channel and is the integrator operation id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogOpenParams {
    pub request_id: RequestId,
    pub title: String,
    pub layout: DialogNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
}

/// True once the dialog is durable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogOpenResult {
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogCloseParams {
    pub request_id: RequestId,
}

open_enum! {
    pub enum DialogOutcome fallback Other {
        Answered = "answered",
        Cancelled = "cancelled",
        Unknown = "unknown",
    }
}

/// A terminal outcome sent only to the channel that opened the dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DialogResultParams {
    pub request_id: RequestId,
    pub outcome: DialogOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Map<String, Value>>,
}

/// The pending target inbox and current channel registry on this target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxListResult {
    pub requests: Vec<RequestItem>,
    pub channels: Vec<ChannelRegisteredParams>,
}

/// Upsert a pending request, or remove it from the inbox when it reaches a terminal state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxUpdatedParams {
    pub request: RequestItem,
}

/// Answer or dismiss a target-inbox request by its daemon-stamped request id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxRespondParams {
    pub op_id: OpId,
    pub request_id: RequestId,
    pub response: super::RequestResponse,
}
