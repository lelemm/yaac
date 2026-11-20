//! Small constructors for demo items. Demo only.

use serde_json::json;
use yaac_protocol::{
    Address, AgentMessageItem, ApprovalDecision, ApprovalKind, ApprovalRequest, DecisionKind, FilePart, ImagePart,
    InputPart, Item, ItemBody, ItemId, ItemStatus, MeshMessage, MeshMessageKind, MeshPeer, MeshReceivedItem,
    MeshSentItem, MeshSpawnItem, MeshWaitItem, MessageId, NoticeItem, NoticeKind, Question, QuestionId,
    QuestionOption, QuestionRequest, ReasoningItem, Receipt, ReceiptReason, ReceiptStatus, RequestBody, RequestItem,
    RequestId, RequestResolution, RequestState, SpawnedThread, TargetPath, ThreadState, Timestamp, ToolCallItem,
    ToolKind, TurnId, UploadId, UserMessageItem, WaitTarget,
};

pub(crate) fn item(id: &str, turn: &str, status: ItemStatus, body: ItemBody) -> Item {
    Item { id: ItemId::new(id), turn_id: TurnId::new(turn), status, body }
}

pub(crate) fn done(id: &str, turn: &str, body: ItemBody) -> Item {
    item(id, turn, ItemStatus::Completed, body)
}

pub(crate) fn user(text: &str, files: &[&str], images: &[&str]) -> ItemBody {
    let mut parts = vec![InputPart::text(text)];
    parts.extend(files.iter().map(|f| InputPart::File(FilePart { upload_id: UploadId::new(*f) })));
    parts.extend(images.iter().map(|f| InputPart::Image(ImagePart { upload_id: UploadId::new(*f) })));
    ItemBody::UserMessage(UserMessageItem { parts })
}

pub(crate) fn agent(text: &str) -> ItemBody {
    ItemBody::AgentMessage(AgentMessageItem { text: text.to_owned() })
}

pub(crate) fn reasoning(text: &str) -> ItemBody {
    ItemBody::Reasoning(ReasoningItem { reasoning: text.to_owned(), summary: true })
}

pub(crate) fn tool(kind: ToolKind, title: &str, exit: Option<i32>, output: &str) -> ItemBody {
    ItemBody::ToolCall(ToolCallItem {
        tool: kind,
        title: title.to_owned(),
        input: None,
        output: output.to_owned(),
        output_truncated: false,
        output_bytes: None,
        exit_code: exit,
        cwd: None,
    })
}

pub(crate) fn message(id: &str, from: &Address, to: &Address, kind: MeshMessageKind, text: &str, at: Timestamp) -> MeshMessage {
    MeshMessage {
        id: MessageId::new(id),
        from: from.clone(),
        to: to.clone(),
        kind,
        text: text.to_owned(),
        in_reply_to: None,
        sent_at: at,
        seq: 1,
        root: None,
    }
}

pub(crate) fn received(msg: MeshMessage, from: MeshPeer) -> ItemBody {
    ItemBody::MeshReceived(MeshReceivedItem { message: msg, from })
}

pub(crate) fn sent(msg: MeshMessage, to: MeshPeer, receipt: Receipt) -> ItemBody {
    ItemBody::MeshSent(MeshSentItem { message: msg, to, receipt })
}

pub(crate) fn delivered(detail: Option<&str>) -> Receipt {
    Receipt { status: ReceiptStatus::Delivered, reason: None, detail: detail.map(str::to_owned) }
}

pub(crate) fn queued(reason: ReceiptReason) -> Receipt {
    Receipt { status: ReceiptStatus::Queued, reason: Some(reason), detail: None }
}

pub(crate) fn failed(reason: ReceiptReason) -> Receipt {
    Receipt { status: ReceiptStatus::Failed, reason: Some(reason), detail: None }
}

pub(crate) fn spawn(threads: Vec<(MeshPeer, ThreadState)>) -> ItemBody {
    ItemBody::MeshSpawn(MeshSpawnItem {
        threads: threads.into_iter().map(|(peer, state)| SpawnedThread { peer, state, error: None }).collect(),
    })
}

pub(crate) fn wait(targets: Vec<(MeshPeer, ThreadState)>, for_messages: bool) -> ItemBody {
    ItemBody::MeshWait(MeshWaitItem {
        targets: targets.into_iter().map(|(peer, state)| WaitTarget { peer, state, finished: false }).collect(),
        for_messages,
    })
}

pub(crate) fn notice(kind: NoticeKind, text: &str, details: Option<serde_json::Value>) -> ItemBody {
    ItemBody::Notice(NoticeItem { notice_kind: kind, text: text.to_owned(), about: None, details })
}

pub(crate) fn decision(id: &str, label: &str, kind: DecisionKind) -> ApprovalDecision {
    ApprovalDecision { id: id.to_owned(), label: label.to_owned(), kind, description: None, accepts_reason: false }
}

pub(crate) fn approval(request: &str, command: &str, cwd: &str, decisions: Vec<ApprovalDecision>) -> ItemBody {
    ItemBody::Request(RequestItem {
        request_id: RequestId::new(request),
        blocking: true,
        state: RequestState::Pending,
        resolution: None,
        body: RequestBody::Approval(ApprovalRequest {
            approval_kind: ApprovalKind::Command,
            title: "Run command".to_owned(),
            command: Some(command.to_owned()),
            cwd: Some(TargetPath::new(cwd)),
            changes: vec![],
            scope: None,
            reason: None,
            decisions,
            details: None,
        }),
    })
}

pub(crate) fn with_state(mut body: ItemBody, state: RequestState, resolution: Option<RequestResolution>) -> ItemBody {
    if let ItemBody::Request(r) = &mut body {
        r.state = state;
        r.resolution = resolution;
    }
    body
}

pub(crate) fn question(request: &str, questions: Vec<Question>) -> ItemBody {
    ItemBody::Request(RequestItem {
        request_id: RequestId::new(request),
        blocking: true,
        state: RequestState::Pending,
        resolution: None,
        body: RequestBody::Question(QuestionRequest { questions }),
    })
}

pub(crate) fn q(id: &str, header: Option<&str>, text: &str, options: &[&str], multi: bool, other: bool, secret: bool) -> Question {
    Question {
        id: QuestionId::new(id),
        header: header.map(str::to_owned),
        text: text.to_owned(),
        options: options
            .iter()
            .map(|o| QuestionOption { label: (*o).to_owned(), description: None, preview: None })
            .collect(),
        multi_select: multi,
        allow_other: other,
        secret,
    }
}

pub(crate) fn budget_details(waiting: u32) -> serde_json::Value {
    json!({ "waitingMessages": waiting })
}
