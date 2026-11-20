//! Approvals, questions, and forms: things a harness asks the user
//! (docs/03-protocol.md#requests-approvals-and-questions).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ClientId, OpId, QuestionId, RequestId, TargetPath, ThreadId, Timestamp};
use crate::macros::{open_enum, open_union};

open_enum! {
    /// The state of a request item. Always exactly one.
    pub enum RequestState fallback Other {
        /// Waiting for an answer.
        Pending = "pending",
        /// This UI answered; the harness has not confirmed yet.
        Submitted = "submitted",
        /// Answered; `resolution` records the answer and who gave it.
        Resolved = "resolved",
        /// No longer answerable (turn ended, interrupted, process stopped). Not an error.
        Expired = "expired",
        /// `yaacd` cannot tell whether the answer reached the harness. Never re-sent.
        Unknown = "unknown",
    }
}

open_enum! {
    /// What an approval is for.
    pub enum ApprovalKind fallback Unknown {
        /// Run a command.
        Command = "command",
        /// Change files.
        FileChange = "fileChange",
        /// Grant a permission.
        Permission = "permission",
        /// Anything else; `title` and `details` describe it.
        Other = "other",
    }
}

open_enum! {
    /// The nature of a decision the harness offers. The UI renders the offered decisions; it
    /// never assumes a fixed Allow/Deny pair.
    pub enum DecisionKind fallback Unknown {
        /// Allow this once.
        Allow = "allow",
        /// Allow for the rest of the session.
        AllowSession = "allowSession",
        /// Allow and remember.
        AllowAlways = "allowAlways",
        /// Deny.
        Deny = "deny",
        /// Deny and stop the turn.
        Abort = "abort",
    }
}

open_enum! {
    /// How a file is changed.
    pub enum FileChangeKind fallback Unknown {
        /// Created.
        Add = "add",
        /// Modified.
        Modify = "modify",
        /// Deleted.
        Delete = "delete",
        /// Moved.
        Rename = "rename",
    }
}

/// One decision the harness offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecision {
    /// Sent back in the answer.
    pub id: String,
    /// Label to show.
    pub label: String,
    /// Its nature, for styling.
    pub kind: DecisionKind,
    /// Longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the user may attach a reason ("deny with a reason").
    #[serde(default)]
    pub accepts_reason: bool,
}

/// One file in a file-change approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// The file.
    pub path: TargetPath,
    /// How it changes.
    pub change: FileChangeKind,
    /// New path, for a rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<TargetPath>,
    /// Unified diff, possibly truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// "May I run this / change these / have this permission?"
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    /// What it is for.
    pub approval_kind: ApprovalKind,
    /// One-line description.
    pub title: String,
    /// The command, for a command approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The directory it would run in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<TargetPath>,
    /// The file changes, for a file-change approval.
    #[serde(default)]
    pub changes: Vec<FileChange>,
    /// The scope of the grant, for a permission approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The harness's stated reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The decisions the harness actually offers.
    pub decisions: Vec<ApprovalDecision>,
    /// Anything else the harness sent, for a generic rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// One option of a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    /// Label; also what is sent back.
    pub label: String,
    /// Longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A preview to show (markdown), when the harness offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// One normalised question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    /// Key in the answer.
    pub id: QuestionId,
    /// Short header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// The question.
    pub text: String,
    /// Options; empty for a free-text question.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// May choose several options.
    #[serde(default)]
    pub multi_select: bool,
    /// May type a free-form answer instead.
    #[serde(default)]
    pub allow_other: bool,
    /// The answer must be masked and never cached on the host.
    #[serde(default)]
    pub secret: bool,
}

/// "Which of these approaches do you want?"
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRequest {
    /// The questions, in order.
    pub questions: Vec<Question>,
}

/// A schema-driven form (MCP elicitation and the like). Until the UI renders forms it shows
/// `message` and says the form must be answered in the harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormRequest {
    /// The text to show.
    pub message: String,
    /// Who asks (e.g. an MCP server name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// JSON Schema of the requested fields, as the harness sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

open_union! {
    /// What a request asks for.
    pub enum RequestBody tag "requestKind" {
        /// An approval.
        Approval(ApprovalRequest) = "approval",
        /// Structured questions.
        Question(QuestionRequest) = "question",
        /// A schema-driven form.
        Form(FormRequest) = "form",
    }
}

/// The answer to one question: chosen labels and/or free text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswer {
    /// Chosen option labels.
    #[serde(default)]
    pub selected: Vec<String>,
    /// Free-form answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    /// Set by `yaacd` when it reports the answer to a `secret` question: the content is
    /// withheld, so it can never reach the host cache.
    #[serde(default)]
    pub redacted: bool,
}

/// Answer to an approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalAnswer {
    /// The chosen `ApprovalDecision.id`.
    pub decision: String,
    /// A reason, when the decision accepts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Answers to a question request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswers {
    /// Question id → answer.
    pub answers: BTreeMap<QuestionId, QuestionAnswer>,
}

/// Answer to a form.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormAnswer {
    /// Field values, shaped by the form's schema.
    pub values: serde_json::Map<String, Value>,
}

open_union! {
    /// An answer to a request. Its `requestKind` matches the request's.
    pub enum RequestResponse tag "requestKind" {
        /// Answers an approval.
        Approval(ApprovalAnswer) = "approval",
        /// Answers questions.
        Question(QuestionAnswers) = "question",
        /// Answers a form.
        Form(FormAnswer) = "form",
    }
}

open_enum! {
    /// Who answered a request.
    pub enum AnsweredBy fallback Unknown {
        /// A YAAC client; `clientId` says which (compare with your own to show "you").
        Client = "client",
        /// Somewhere YAAC cannot see — for example the harness's own terminal.
        Elsewhere = "elsewhere",
    }
}

/// How a request was resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestResolution {
    /// Who answered.
    pub by: AnsweredBy,
    /// The answering client, when `by` is `client`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<ClientId>,
    /// The answering client's host name, for "answered on …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_host: Option<String>,
    /// The answer, when known. Secret answers are redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RequestResponse>,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

/// `request/respond`: answer a pending request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestRespondParams {
    /// Operation id.
    pub op_id: OpId,
    /// The thread.
    pub thread_id: ThreadId,
    /// The request.
    pub request_id: RequestId,
    /// The answer.
    pub response: RequestResponse,
}
