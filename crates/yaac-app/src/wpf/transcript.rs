use super::*;
use std::collections::BTreeMap;
use yaac_core::activity::{self, ActivityLine, Block, Expansion};
use yaac_core::diff::{self, FileDiff, LineKind};
use yaac_core::agentcall;
use yaac_core::attention::AttentionMap;
use yaac_core::permission::{self, BudgetFields, PermissionDecision};
use yaac_core::requests::{self, CardState};
use yaac_core::sending::{self, SendState};
use yaac_core::transcript::Transcript;
use yaac_core::turns;
use yaac_protocol::{
    AgentCallKind, AgentCallOutcome, ApprovalAnswer, InputPart, Item, ItemBody, ItemStatus, MeshPeer, QuestionAnswer,
    QuestionAnswers, RequestBody, RequestItem, RequestResponse, RequestState, TextRole, ToolKind, TurnId,
};

fn peer(peer: &MeshPeer) -> &str {
    peer.title.as_deref().unwrap_or(peer.address.as_str())
}

fn peer_action(app: &YaacApp, peer_data: &MeshPeer, gone: bool) -> Value {
    let peer_key = key(app, peer_data.address.as_str());
    let mut action = sidebar_view::action(
        &format!("openPeer:{}", peer_data.address),
        peer(peer_data),
        peer_key.is_some() && !gone,
    );
    action["machine"] = json!(peer_data.target_name);
    action["logo"] = json!(peer_key.as_ref().and_then(|key| app.client.state.provider_of(key))
        .and_then(|p| crate::logos::for_kind(p.kind.as_str())).map(|(logo, _)| crate::logos::source(logo).0));
    action
}

fn budget_card(app: &YaacApp, key: &ThreadKey, text: &str) -> Value {
    let state = &app.client.state;
    let exhausted = yaac_core::agents::root_of(state, key)
        .is_some_and(|root| state.budgets.get(&root).is_some_and(|b| b.exhausted));
    let (title, body) = text.split_once(':').unwrap_or((text, ""));
    let actions = if exhausted {
        vec![
            json!({ "id": "continueBudget", "label": "Continue (+100 turns)", "primary": true, "enabled": true }),
            sidebar_view::action("reviewWorkers", "Review workers", true),
        ]
    } else {
        vec![]
    };
    json!({ "kind": "budget", "title": title, "text": body.trim(),
        "status": (!exhausted).then_some("Continued · the orchestration has a new allowance."),
        "tone": if exhausted { "warning" } else { "muted" }, "actions": actions })
}

fn card(app: &YaacApp, key: &ThreadKey, transcript: &Transcript, item: &Item, request: &RequestItem) -> Value {
    let state = &app.client.state;
    let local = transcript.local_answers.get(&request.request_id);
    let status = requests::card_state(
        request,
        state.may_answer(key),
        state.client_id.as_ref(),
        local.map(|a| a.summary.as_str()),
        &state.target_name(&key.target),
        app.now(),
    );
    let pending = matches!(status, CardState::Pending { actionable: true });
    let (label, tone) = match status {
        CardState::Pending { actionable: true } => ("Waiting for you".to_owned(), "warning"),
        CardState::Pending { actionable: false } => (
            "Waiting for an answer in the other program".to_owned(),
            "muted",
        ),
        CardState::Submitted { summary } => (format!("Submitted\nYou chose: {summary}\nSent · waiting for the agent to confirm"), "muted"),
        CardState::Confirmed { summary, when } => (
            when.map_or_else(|| format!("Confirmed\nAnswered by you · {summary}"), |w| format!("Confirmed\nAnswered by you · {summary} · {w}")),
            "success",
        ),
        CardState::ResolvedElsewhere { text } => (text, "muted"),
        CardState::Expired => ("Expired\nNo longer needed · the turn ended before this was answered.".to_owned(), "muted"),
        CardState::OutcomeUnknown => ("Outcome unknown\nYAAC lost contact while sending your answer. It may or may not have arrived. Your answer will not be sent again.".to_owned(), "warning"),
        CardState::Other(s) => (s, "muted"),
    };
    let mut actions = vec![];
    let mut parts = vec![];
    let mut questions = vec![];
    let mut fields = vec![];
    let (kind, title, text) = match &request.body {
        RequestBody::Approval(a) => {
            if let Some(cmd) = &a.command {
                parts.push(json!({ "kind": "command", "text": cmd, "label": a.cwd }));
            }
            for f in &a.changes {
                parts.push(json!({ "kind": "diff", "text": f.diff.as_deref().unwrap_or(f.path.as_str()), "label": f.path }));
            }
            if pending {
                let keys = requests::decision_keys(&a.decisions);
                for (d, key) in a.decisions.iter().zip(keys) {
                    let key = key.map(|k| if k == "Esc" { "Escape" } else { k });
                    actions.push(json!({
                        "id": format!("approval:{}", d.id), "label": d.label, "enabled": true, "detail": d.description,
                        "primary": key == Some("Enter"), "key": key,
                    }));
                }
            }
            (
                "approval",
                a.title.clone(),
                [a.reason.as_deref(), a.scope.as_deref(),
                    (pending && requests::started_by_agent(transcript, &item.turn_id)).then_some("Started by an agent, not by you")]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        RequestBody::Question(q) => {
            if pending {
                questions = q.questions.iter().map(|q| json!({
                    "id": q.id, "header": q.header, "text": q.text, "multiSelect": q.multi_select,
                    "allowOther": q.allow_other || q.options.is_empty(), "secret": q.secret,
                    "options": q.options.iter().map(|o| json!({ "id": o.label, "label": o.label, "enabled": true,
                        "detail": o.description.as_ref().or(o.preview.as_ref()) })).collect::<Vec<_>>(),
                })).collect();
                actions.push(json!({ "id": "answerQuestions", "label": "Submit answer", "enabled": true, "primary": true }));
            }
            let text = if pending {
                String::new()
            } else {
                q.questions
                    .iter()
                    .map(|q| q.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ("question", "Question".to_owned(), text)
        }
        RequestBody::MeshPermission(_) => {
            let prompt = permission::parse(&request.body).expect("mesh permission");
            if pending {
                fields = [
                    ("turns", "Agent-started turns", prompt.budget.agent_turns),
                    ("hours", "Hours without the user", prompt.budget.hours),
                ].into_iter().map(|(id, label, value)| json!({ "id": id, "label": label, "value": value, "min": 0, "max": u32::MAX })).collect();
                actions = vec![
                    json!({ "id": "permission:allow", "label": "Allow", "enabled": true, "primary": true }),
                    sidebar_view::action("permission:always", "Always allow", true),
                    json!({ "id": "permission:deny", "label": "Deny", "enabled": true, "key": "Escape" }),
                ];
            }
            (
                "permission",
                "This agent wants to work with other agents".to_owned(),
                if pending {
                    format!("It will:\n{}\n\nWhen a budget runs out, work pauses and asks you. Nothing is cancelled.", prompt.actions.join("\n"))
                } else {
                    prompt.actions.join("\n")
                },
            )
        }
        RequestBody::Form(f) => (
            "form",
            f.source.clone().unwrap_or_else(|| "Form".to_owned()),
            if pending { format!("{}\nThis form must be answered in the harness itself.", f.message) } else { f.message.clone() },
        ),
        _ => ("request", "Agent request".into(), if pending { "This request must be answered in the harness itself.".into() } else { String::new() }),
    };
    if request.state == RequestState::Unknown {
        actions.push(sidebar_view::action(
            "checkLatest",
            "Check latest status",
            true,
        ));
    }
    json!({ "kind": kind, "title": title, "text": text, "tone": tone, "status": label,
        "actions": actions, "parts": parts, "questions": questions, "fields": fields })
}

fn render(app: &YaacApp, map: &AttentionMap, key: &ThreadKey, transcript: &Transcript, item: &Item, markdown: &mut super::markdown_view::MarkdownView, task: bool) -> Option<Value> {
    let mut row = match &item.body {
        ItemBody::UserMessage(message) => {
            let mut parts = vec![];
            let mut attachments = vec![];
            for (index, part) in message.parts.iter().enumerate() {
                match part {
                    InputPart::Text(t) => parts.push(json!({
                        "kind": match t.role { Some(TextRole::Pasted) => "pasted", Some(TextRole::Command) => "command",
                            Some(TextRole::Context) => "detail", _ => "markdown" },
                        "text": t.text, "label": t.label,
                    })),
                    InputPart::Image(image) => attachments.push(json!({ "label": image.name.as_deref().unwrap_or(image.upload_id.as_str()),
                        "image": true, "missing": transcript.missing_uploads.contains(&image.upload_id),
                        "action": (!transcript.missing_uploads.contains(&image.upload_id)).then(|| sidebar_view::action(
                            &format!("openAttachment:{index}"), "Open image", true)) })),
                    InputPart::File(file) => attachments.push(json!({ "label": file.name.as_deref().unwrap_or(file.upload_id.as_str()),
                        "missing": transcript.missing_uploads.contains(&file.upload_id) })),
                    _ => {}
                }
            }
            let mut row = json!({ "kind": "user", "title": "You", "parts": parts, "attachments": attachments });
            if let Some(p) = transcript.pending_of(&item.id) {
                let (status, actions) = match &p.state {
                    SendState::Sending => ("Sending…", vec![]),
                    SendState::Sent => ("", vec![]),
                    SendState::Failed(reason) => (
                        reason.as_str(),
                        vec![
                            sidebar_view::action("editSend", "Edit", true),
                            sidebar_view::action("retrySend", "Retry", true),
                        ],
                    ),
                    SendState::Unknown => (
                        "Send outcome unknown",
                        vec![
                            sidebar_view::action("editSend", "Edit", true),
                            sidebar_view::action("dismissSend", "Dismiss", true),
                        ],
                    ),
                };
                row["status"] = json!(status);
                row["actions"] = json!(actions);
            }
            row
        }
        ItemBody::AgentMessage(a) => {
            json!({ "kind": "agent", "text": a.text, "streaming": item.status == ItemStatus::InProgress })
        }
        ItemBody::Reasoning(_) => {
            let line = activity::line_of(&app.client.state, map, key, 0, item)?;
            let thought = activity::thought_text(item);
            json!({ "kind": "activity", "mark": "•", "lead": line.text, "message": thought,
                "defaultExpanded": thought.is_some(), "disclosureName": "thought", "copyText": line.text })
        }
        ItemBody::ToolCall(t) if t.agent_call.is_some() => {
            let call = t.agent_call.as_ref().expect("agent call");
            let line = agentcall::line(&app.client.state, map, key, Some(transcript), item, call);
            let detail = agentcall::detail(Some(transcript), t, call);
            let failed = item.status == ItemStatus::Failed || call.outcome == Some(AgentCallOutcome::Failed);
            let mark = if failed { "×" } else { match (&call.kind, &call.outcome) {
                (AgentCallKind::Message, Some(AgentCallOutcome::Resumed)) | (AgentCallKind::Resume, _) => "↻",
                (AgentCallKind::Message, _) => "↗",
                (AgentCallKind::Spawn, _) => "⑂",
                (AgentCallKind::Wait, Some(AgentCallOutcome::Returned)) if item.status != ItemStatus::InProgress => "↙",
                (AgentCallKind::Wait, _) => "⌛",
                _ => "•",
            }};
            let pills: Vec<_> = line.pills.iter().enumerate().map(|(index, pill)| {
                let mut action = sidebar_view::action(&format!("openCall:{index}"), &pill.label, pill.key.is_some());
                action["machine"] = json!(pill.machine);
                action["logo"] = json!(pill.kind.as_deref().and_then(crate::logos::for_kind).map(|(logo, _)| crate::logos::source(logo).0));
                action
            }).collect();
            let tail = line.meta.iter().chain(line.status.iter()).cloned().collect::<Vec<_>>().join(" · ");
            let mut parts: Vec<_> = [("Input", detail.raw_input), ("Output", detail.raw_output)]
                .into_iter().filter(|(_, text)| !text.is_empty())
                .map(|(label, text)| json!({ "kind": "code", "label": label, "text": text, "expanded": false }))
                .collect();
            tool_images(app, key, item, t, &mut parts);
            json!({ "kind": "call", "mark": mark, "lead": line.lead, "after": line.after,
                "preview": line.preview, "tail": tail, "pills": pills, "tone": if failed { "error" } else { "muted" },
                "heading": detail.heading, "message": detail.text, "result": detail.result, "receipt": detail.status,
                "disclosureLabel": "Raw call",
                "parts": parts, "copyText": line.plain(),
            })
        }
        ItemBody::ToolCall(t) => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item).expect("tool activity");
            let mut parts = Vec::new();
            if t.tool == ToolKind::Command {
                let command = t.input.as_ref()
                    .and_then(|input| input.get("command"))
                    .and_then(Value::as_str)
                    .filter(|command| !command.is_empty())
                    .unwrap_or(&t.title);
                if !command.is_empty() {
                    parts.push(json!({ "kind": "command", "text": command }));
                }
            }
            if !t.output.is_empty() {
                parts.push(json!({ "kind": "code", "text": t.output }));
            }
            tool_output_details(t, &mut parts);
            tool_images(app, key, item, t, &mut parts);
            json!({
                "kind": "tool", "mark": if line.failed { "×" } else { "•" },
                "lead": yaac_core::message::one_line(&line.text),
                "tone": if line.failed { "error" } else { "muted" },
                "parts": parts,
                "receipt": t.cwd, "copyText": line.text,
            })
        }
        ItemBody::Plan(p) => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item)?;
            let detail = p.explanation.iter().map(String::as_str).map(str::to_owned)
                .chain(p.steps.iter().map(|s| format!("{} {}", if s.state == yaac_protocol::PlanStepState::Completed { "✓" } else { "○" }, s.text)))
                .collect::<Vec<_>>().join("\n");
            json!({ "kind": "activity", "mark": "•", "lead": line.text, "detail": detail,
                "disclosureName": "plan", "copyText": line.text })
        }
        ItemBody::Request(r) => card(app, key, transcript, item, r),
        ItemBody::Notice(n) if n.notice_kind != yaac_protocol::NoticeKind::BudgetExhausted => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item)?;
            let mark = match line.mark {
                yaac_core::activity::LineMark::Interrupted => "▪",
                _ if line.failed => "×",
                _ => "ⓘ",
            };
            json!({ "kind": "activity", "mark": mark, "lead": line.text, "detail": n.body,
                "tone": if line.failed { "error" } else { "muted" },
                "disclosureName": "notice", "copyText": line.text })
        }
        ItemBody::Notice(n) => budget_card(app, key, &n.text),
        ItemBody::Handoff(h) => {
            let source = ThreadKey::new(key.target.clone(), h.forked_from.clone());
            let title = app.client.state.summary(&source).map_or(h.forked_from.as_str(), |s| s.title.as_str());
            json!({ "kind": "handoff", "text": h.transcript, "divider": format!("Inherited from {title} · read-only above this line") })
        }
        ItemBody::MeshSent(m) => {
            use yaac_protocol::{ReceiptReason, ReceiptStatus};
            let gone = m.receipt.status == ReceiptStatus::Failed
                && matches!(m.receipt.reason, Some(ReceiptReason::ThreadDeleted | ReceiptReason::UnknownAddress));
            let mut receipt = yaac_core::header::receipt_text(&m.receipt.status, m.receipt.reason.as_ref(), m.receipt.detail.as_deref(), m.to.target_name.as_deref().unwrap_or("the target"));
            if m.receipt.status == ReceiptStatus::Delivered && m.receipt.detail.is_none()
                && let Some(ago) = yaac_core::time::ago(app.now(), &m.message.sent_at) {
                receipt.push_str(&format!(" · {ago}"));
            }
            json!({ "kind": "call", "mark": "↗", "heading": "To", "message": m.message.text,
                "preview": yaac_core::message::one_line(&m.message.text), "pills": [peer_action(app, &m.to, gone)],
                "tail": receipt, "receipt": receipt, "tone": if m.receipt.status == ReceiptStatus::Failed { "error" } else { "muted" } })
        }
        ItemBody::MeshReceived(m) => {
            if task {
                json!({ "kind": "task", "title": "From", "text": m.message.text, "taskPeer": peer_action(app, &m.from, false) })
            } else {
                json!({ "kind": "call", "mark": "↙", "heading": "From", "message": m.message.text,
                    "receipt": yaac_core::time::ago(app.now(), &m.message.sent_at).map(|ago| format!("Sent {ago}")),
                    "preview": yaac_core::message::one_line(&m.message.text), "pills": [peer_action(app, &m.from, false)] })
            }
        }
        ItemBody::MeshSpawn(m) => json!({ "kind": "call", "mark": "⑂", "lead": "Started", "heading": "Workers",
            "pills": m.threads.iter().map(|t| {
                let mut pill = peer_action(app, &t.peer, false);
                let agent = app.client.state.key_of(&t.peer.address).and_then(|k| yaac_core::agents::row_for(&app.client.state, map, &k, 0));
                pill["detail"] = json!([t.peer.provider.as_deref(), t.peer.model.as_ref().map(|m| m.as_str()),
                    agent.as_ref().map(|r| r.activity.as_str()), t.error.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "));
                pill
            }).collect::<Vec<_>>() }),
        ItemBody::MeshWait(m) => json!({ "kind": "call", "mark": "⌛", "lead": "Waiting for",
            "heading": if m.for_messages { "Ends on any message, or when these finish" } else { "Waiting for" },
            "pills": m.targets.iter().map(|t| peer_action(app, &t.peer, false)).collect::<Vec<_>>() }),
        _ => json!({ "kind": "activity", "mark": "•", "lead": item.kind() }),
    };
    let complete = item.status != ItemStatus::InProgress;
    if let Some(parts) = row["parts"].as_array_mut() {
        for (index, part) in parts.iter_mut().enumerate() {
            let text = string(part, "text");
            let kind = string(part, "kind");
            let fenced = text.trim_start().starts_with("```")
                || text.trim_start().starts_with("~~~");
            if kind == "markdown" || kind == "pasted" && fenced {
                part["blocks"] = markdown.blocks(&item.id, &format!("part{index}"), text, kind == "markdown", complete);
            }
        }
    }
    let fields: &[&str] = match string(&row, "kind") {
        "agent" | "task" | "handoff" => &["text"],
        "activity" => &["message"],
        "call" => &["message", "result"],
        _ => &[],
    };
    for &field in fields {
        if let Some(text) = row[field].as_str() {
            row[format!("{field}Blocks")] = markdown.blocks(&item.id, field, text, false, complete);
        }
    }
    row["id"] = json!(row_id(item));
    row["actionId"] = json!(item.id);
    if matches!(string(&row, "kind"), "user" | "task" | "agent") {
        row["stamp"] = json!(crate::messages::item_time(item).map(|at| yaac_core::time::message_time(app.now(), at, app.zone)));
    }
    Some(row)
}

fn row_id(item: &Item) -> ItemId {
    if let ItemBody::UserMessage(message) = &item.body
        && let Some(op) = &message.op_id
    {
        return sending::echo_id(op);
    }
    item.id.clone()
}

fn tool_output_details(tool: &yaac_protocol::ToolCallItem, parts: &mut Vec<Value>) {
    if tool.output_truncated {
        let text = match tool.output_bytes {
            Some(bytes) => format!("Output is longer ({bytes} bytes). The rest is on the target."),
            None => "Output is longer. The rest is on the target.".to_owned(),
        };
        parts.push(json!({ "kind": "text", "text": text, "tone": "muted" }));
    }
    if let Some(exit) = tool.exit_code {
        let (text, tone) = if exit == 0 { ("Success".to_owned(), "success") }
            else { (format!("Failed (exit {exit})"), "error") };
        parts.push(json!({ "kind": "text", "text": text, "tone": tone }));
    }
}

fn tool_images(app: &YaacApp, key: &ThreadKey, item: &Item, tool: &yaac_protocol::ToolCallItem, parts: &mut Vec<Value>) {
    if tool.images.is_empty() {
        return;
    }
    let address = (key.clone(), item.id.clone());
    let expanded = app.ui.expansion.lines.contains(&address) || app.client.state.focus_item.as_ref() == Some(&address);
    for (index, image) in tool.images.iter().enumerate() {
        let source = crate::images::ImageSource::Target(key.target.clone(), image.upload_id.clone());
        parts.push(json!({ "kind": "image", "id": format!("image:{index}"), "label": image.name,
            "preview": expanded.then(|| app.images.wire_preview(&source)).flatten(),
            "action": sidebar_view::action(&format!("openImage:{index}"), &image.name, true),
            "request": sidebar_view::action(&format!("previewImage:{index}"), &image.name, true) }));
    }
}

fn render_file(app: &YaacApp, key: &ThreadKey, item: &Item, line: &ActivityLine, id: &str,
    markdown: &mut super::markdown_view::MarkdownView) -> Value {
    let part = line.diff.expect("file activity");
    let (added, removed) = diff::counts(part.added, part.removed);
    let mut row = json!({
        "id": id, "actionId": item.id, "kind": "tool", "mark": "•", "lead": line.text,
        "added": added, "removed": removed, "activityOpen": line.expanded, "copyText": line.text,
        "activityToggle": sidebar_view::action(&format!("toggleFile:{}", part.file), "Toggle file diff", true),
        "disclosureName": "file diff",
    });
    if line.expanded && let ItemBody::ToolCall(tool) = &item.body {
        let files = markdown.diffs.get(&item.id, &tool.output);
        if let Some(file) = files.get(part.file) {
            let rest_open = app.ui.expansion.rest.contains(&(key.clone(), item.id.clone(), part.file));
            let (shown, hidden) = diff::fold(file, rest_open);
            let (text, prefix) = diff_excerpt(file, shown);
            let mut parts = vec![json!({ "kind": "diff", "text": text, "blocks": {
                "copyText": file.text, "prefixWidth": prefix,
            } })];
            tool_output_details(tool, &mut parts);
            row["parts"] = json!(parts);
            if hidden > 0 {
                let label = format!("Show the rest ({hidden} more lines)");
                row["restAction"] = sidebar_view::action(&format!("showDiffRest:{}", part.file), &label, true);
            }
        }
    }
    row
}

fn diff_excerpt(file: &FileDiff, shown: usize) -> (String, usize) {
    if file.hunks.is_empty() {
        return ("Renamed without changes.".to_owned(), 0);
    }
    let max_no = file.hunks.iter().flat_map(|h| &h.lines).filter_map(|l| l.old.max(l.new)).max().unwrap_or(1);
    let digits = max_no.to_string().len().max(2);
    let prefix = 2 * (digits + 2);
    let blank = " ".repeat(prefix);
    let mut lines = Vec::new();
    for hunk in &file.hunks[..shown] {
        lines.push(format!("{blank}{}", hunk.header));
        for line in &hunk.lines {
            let old = line.old.map(|n| n.to_string()).unwrap_or_default();
            let new = line.new.map(|n| n.to_string()).unwrap_or_default();
            let sign = match line.kind {
                LineKind::Added => "+",
                LineKind::Removed => "-",
                LineKind::Context => " ",
                LineKind::NoNewline => "",
            };
            lines.push(format!("{old:>digits$}  {new:>digits$}  {sign}{}", line.text));
        }
    }
    (lines.join("\n"), prefix)
}

fn run_row(first: &ItemId, text: &str, failed: bool, open: bool) -> Value {
    json!({ "id": format!("~run:{first}"), "actionId": first, "kind": "activity",
        "mark": if failed { "×" } else { "›" }, "tone": if failed { "error" } else { "muted" },
        "lead": text, "disclosureName": "command group", "groupOpen": open, "copyText": text,
        "groupToggle": sidebar_view::action("toggleRun", "Toggle command group", true) })
}

struct Projection<'a> {
    app: &'a YaacApp,
    map: &'a AttentionMap,
    key: &'a ThreadKey,
    transcript: &'a Transcript,
    markdown: &'a mut super::markdown_view::MarkdownView,
    rows: Vec<Value>,
    hidden_runs: Vec<(std::ops::RangeInclusive<usize>, String)>,
}

impl Projection<'_> {
    fn push(&mut self, mut row: Value, turn: &TurnId) {
        row["turnId"] = json!(turn);
        self.rows.push(row);
    }

    fn render(&mut self, index: usize, line: Option<&activity::ActivityLine>, run: Option<&str>) {
        let item = &self.transcript.items()[index];
        let mut row = match line.and_then(|line| line.diff) {
            Some(diff) => {
                let id = if diff.file == 0 { item.id.to_string() } else { format!("~file:{}:{}", diff.file, item.id) };
                render_file(self.app, self.key, item, line.expect("file line"), &id, self.markdown)
            }
            None => {
                let Some(mut row) = render(self.app, self.map, self.key, self.transcript, item, self.markdown, line.is_none()) else {
                    return;
                };
                if let Some(line) = line
                    && (row["parts"].as_array().is_some_and(|parts| !parts.is_empty())
                        || row["pills"].as_array().is_some_and(|pills| !pills.is_empty())
                        || ["detail", "message", "result", "receipt"].iter()
                            .any(|field| row[*field].as_str().is_some_and(|text| !text.is_empty())))
                {
                    row["activityOpen"] = json!(line.expanded);
                    row["activityToggle"] = sidebar_view::action("toggleActivity", "Toggle details", true);
                }
                row
            }
        };
        if line.is_some_and(|line| line.highlighted) {
            row["highlighted"] = json!(true);
        }
        if let Some(id) = run {
            row["foldRun"] = json!(id);
        }
        self.push(row, &item.turn_id);
    }

    fn block(&mut self, block: Block, open_runs: &mut BTreeMap<ItemId, (String, bool, usize)>) {
        let items = self.transcript.items();
        match block {
            Block::User { index } | Block::Task { index } | Block::Prose { index }
            | Block::Card { index } | Block::Handoff { index } => self.render(index, None, None),
            Block::Run { lines } => {
                let mut open_run: Option<(String, usize)> = None;
                for line in lines {
                    let item = &items[line.index];
                    if let Some((text, failed, last)) = open_runs.remove(&line.id) {
                        let row = run_row(&line.id, &text, failed, true);
                        open_run = Some((string(&row, "id").to_owned(), last));
                        self.push(row, &item.turn_id);
                    }
                    let run = open_run.as_ref().filter(|(_, last)| line.index <= *last).map(|(id, _)| id.as_str());
                    self.render(line.index, Some(&line), run);
                }
            }
            Block::Folded { first, text, failed, last_index, .. } => {
                if let Some(index) = self.transcript.index_of(&first) {
                    let row = run_row(&first, &text, failed, false);
                    self.hidden_runs.push((index..=last_index, string(&row, "id").to_owned()));
                    self.push(row, &items[index].turn_id);
                }
            }
            Block::Worked { turn, text, failed, error, expanded } => {
                if let Some(first) = items.iter().find(|item| item.turn_id == turn) {
                    self.push(json!({ "id": format!("~worked:{turn}"), "actionId": first.id, "kind": "worked",
                        "mark": "›", "tone": if failed { "error" } else { "muted" }, "lead": text,
                        "groupError": error, "groupOpen": expanded, "disclosureName": "turn work",
                        "groupToggle": sidebar_view::action("toggleWorked", "Toggle turn work", true) }), &turn);
                }
            }
            Block::TurnChanges { turn, totals, expanded, items } => {
                let (added, removed) = yaac_core::diff::counts(totals.added, totals.removed);
                let text = totals.text();
                self.push(json!({ "id": format!("~changes:{turn}"), "actionId": items.first(), "kind": "changes",
                    "mark": "›", "lead": text, "added": added, "removed": removed, "copyText": text,
                    "tail": if expanded { "hide all" } else { "show all" },
                    "groupOpen": expanded, "disclosureName": "turn file changes",
                    "groupToggle": sidebar_view::action("toggleTurnChanges", "Toggle turn file changes", true) }), &turn);
            }
        }
    }

    fn anchor(&self, anchor: &yaac_core::history::Anchor) -> Option<Value> {
        let id = anchor.item.as_str();
        let visible = |id| self.rows.iter().any(|row| string(row, "id") == id);
        if visible(id) {
            return Some(json!({ "id": id, "offset": anchor.offset }));
        }
        let items = self.transcript.items();
        let index = match id.split_once(':') {
            Some(("~worked", turn)) => items.iter().position(|item| item.turn_id.as_str() == turn),
            Some(("~changes", turn)) => items.iter().rposition(|item| item.turn_id.as_str() == turn),
            Some(("~run", first)) => self.transcript.index_of(&ItemId::from(first)),
            Some(("~file", file)) => file.split_once(':').and_then(|(_, item)| self.transcript.index_of(&ItemId::from(item))),
            _ => self.transcript.index_of(&anchor.item),
        }?;
        let item = &items[index];
        let id = row_id(item);
        if visible(id.as_str()) {
            return Some(json!({ "id": id, "offset": anchor.offset }));
        }
        if let Some((_, id)) = self.hidden_runs.iter().find(|(range, _)| range.contains(&index)) {
            return Some(json!({ "id": id, "offset": 0 }));
        }
        let worked = format!("~worked:{}", item.turn_id);
        visible(worked.as_str()).then(|| json!({ "id": worked, "offset": 0 }))
    }
}

fn work_focus<'a>(app: &'a YaacApp, key: &ThreadKey) -> Option<&'a ItemId> {
    app.client.state.focus_item.as_ref().filter(|(thread, _)| thread == key).map(|(_, id)| id)
        .or(app.ui.focus_card.as_ref()).or(app.ui.scroll_to.as_ref())
}

pub(super) fn view(app: &YaacApp, map: &AttentionMap, markdown: &mut super::markdown_view::MarkdownView) -> Value {
    let selected = selected_thread(app);
    markdown.select(selected);
    let Some(key) = selected else {
        let state = &app.client.state;
        let empty = if matches!(state.selection, Selection::None)
            && !yaac_core::first_run::is_first_run(state)
            && !matches!(state.hub.outcome, yaac_core::state::HelloOutcome::WindowOutdated { .. }) {
            crate::shell::empty_workspace(app)
        } else {
            String::new()
        };
        return json!({ "emptyText": empty, "rows": [] });
    };
    let state = &app.client.state;
    let transcript = state.transcripts.get(key);
    let focus = app.ui.focus_card.as_ref().or(app.ui.scroll_to.as_ref());
    let (rows, anchor) = if let Some(t) = transcript {
        let expansion = &app.ui.expansion;
        let blocks = activity::blocks(state, map, key, expansion);
        let blocks = turns::collapse(t, key, &blocks, &app.ui.worked, expansion, work_focus(app, key));
        // Core emits the constituent lines when a run is open. WPF also retains the
        // disclosure for explicitly opened groups, so the user can close one in place.
        let mut open_runs = BTreeMap::new();
        if expansion.runs.iter().any(|(thread, _)| thread == key) {
            for block in activity::blocks(state, map, key, &Expansion::default()) {
                if let Block::Folded { first, text, failed, last_index, .. } = block
                    && expansion.runs.contains(&(key.clone(), first.clone()))
                {
                    open_runs.insert(first, (text, failed, last_index));
                }
            }
        }
        let mut projection = Projection {
            app, map, key, transcript: t, markdown, rows: Vec::new(), hidden_runs: Vec::new(),
        };
        for block in blocks {
            projection.block(block, &mut open_runs);
        }
        let anchor = app.ui.restore.as_ref().and_then(|anchor| projection.anchor(anchor));
        (projection.rows, anchor)
    } else {
        (Vec::new(), None)
    };
    let focus = focus.and_then(|id| transcript?.item(id)).map(row_id);
    let anchor_token = app.ui.restore.as_ref().map(|a| format!("{}:{}", a.offset, a.item));
    let page = transcript.map(|t| json!({
        "older": t.older_cursor, "newer": t.newer_cursor,
        "loading": t.loading.is_some() || t.loading_older || t.loading_newer,
    }));
    let now = app.now();
    let prompts = transcript.and_then(|t| t.prompts.as_ref()).map(|prompts| prompts.iter().map(|p| {
        let stamp = p.at.as_ref().and_then(yaac_core::time::secs).map(|at| yaac_core::time::message_time(now, at, app.zone));
        json!({ "id": p.item_id, "title": p.title, "stamp": stamp })
    }).collect::<Vec<_>>());
    json!({ "threadId": key.address(), "rows": rows, "loading": transcript.is_some_and(|t| !t.is_loaded()),
        "page": page, "prompts": prompts, "historyAway": transcript.is_some_and(|t| t.history_away),
        "emptyText": "No messages yet", "focusId": focus, "focusToken": focus,
        "anchor": anchor, "anchorToken": anchor_token })
}

fn budget_value(p: &Value, id: &str, fallback: u32) -> u32 {
    p["fields"][id]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(fallback)
}

fn answer(r: &RequestItem, id: &str, p: &Value) -> Option<RequestResponse> {
    match &r.body {
        RequestBody::Approval(a) => {
            let decision = id.strip_prefix("approval:")?;
            a.decisions.iter().any(|d| d.id == decision).then(|| {
                RequestResponse::Approval(ApprovalAnswer {
                    decision: decision.to_owned(),
                    reason: None,
                })
            })
        }
        RequestBody::Question(q) if id == "answerQuestions" => {
            let mut answers = BTreeMap::new();
            for question in &q.questions {
                let entry = p["answers"].get(question.id.as_str())?;
                let selected: Vec<_> = entry["selected"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|v| question.options.iter().any(|o| o.label == *v))
                    .map(str::to_owned)
                    .collect();
                let other = entry["other"]
                    .as_str()
                    .filter(|t| {
                        !t.trim().is_empty()
                            && (question.allow_other || question.options.is_empty())
                    })
                    .map(str::to_owned);
                if (!question.multi_select && selected.len() > 1)
                    || (selected.is_empty() && other.is_none())
                {
                    return None;
                }
                answers.insert(
                    question.id.clone(),
                    QuestionAnswer {
                        selected,
                        other,
                        redacted: false,
                    },
                );
            }
            Some(RequestResponse::Question(QuestionAnswers { answers }))
        }
        RequestBody::MeshPermission(_) => {
            let decision = match id {
                "permission:allow" => PermissionDecision::Allow,
                "permission:always" => PermissionDecision::AlwaysAllow,
                "permission:deny" => PermissionDecision::Deny,
                _ => return None,
            };
            let fallback = permission::parse(&r.body)?.budget;
            let budget = BudgetFields {
                agent_turns: budget_value(p, "turns", fallback.agent_turns),
                hours: budget_value(p, "hours", fallback.hours),
            };
            Some(permission::response(decision, budget))
        }
        _ => None,
    }
}

pub(super) fn command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let Some(k) = selected_thread(app).cloned() else {
        return Ok(());
    };
    if let Some(thread) = p["threadId"].as_str()
        && k.address().as_str() != thread
    {
        return Ok(());
    }
    if name == "transcript.page" {
        match string(p, "id") {
            "older" => app.apply(vec![Action::OlderHistory(k)]),
            "newer" => app.apply(vec![Action::NewerHistory(k)]),
            "latest" => app.apply(vec![Action::LatestHistory(k)]),
            _ => {}
        }
        return Ok(());
    }
    if name == "transcript.prompt" {
        let id = ItemId::new(string(p, "id"));
        if app.client.state.transcripts.get(&k).and_then(|t| t.prompts.as_ref())
            .is_some_and(|prompts| prompts.iter().any(|p| p.item_id == id))
        {
            app.ui.focus_card = None;
            app.ui.scroll_to = Some(id.clone());
            app.apply(vec![Action::FocusHistory(k, id)]);
        }
        return Ok(());
    }
    if name == "transcript.anchor" {
        use yaac_core::history::Place;
        if !matches!(app.ui.history.current().map(|entry| &entry.place), Some(Place::Thread(thread)) if thread == &k) {
            return Ok(());
        }
        let offset = p["offset"]
            .as_f64()
            .unwrap_or_default()
            .clamp(0.0, 100_000.0);
        let id = string(p, "id");
        let item_id = p["itemId"].as_str().unwrap_or_else(|| id.strip_prefix("~run:").unwrap_or(id));
        let anchor_item = selected_item(app, item_id).and_then(|item| {
            let source = app.client.state.transcripts.get(&k)?.item(&item)?;
            let summary = id == format!("~worked:{}", source.turn_id)
                || id == format!("~changes:{}", source.turn_id)
                || id.strip_prefix("~run:") == Some(source.id.as_str())
                || id.strip_prefix("~file:").and_then(|part| part.split_once(':')).is_some_and(|(index, item)| {
                    item == source.id.as_str() && index.parse::<usize>().is_ok()
                });
            Some(if summary { ItemId::from(id) } else { item })
        });
        #[allow(
            clippy::cast_possible_truncation,
            reason = "bounded DIP offset is stored at UI precision"
        )]
        let anchor = anchor_item.map(|item| yaac_core::history::Anchor {
            item,
            offset: offset as f32,
        });
        app.ui.history.set_anchor(anchor);
        return Ok(());
    }
    if name != "transcript.action" {
        return Ok(());
    }
    let Some(item) = selected_item(app, string(p, "id")) else {
        return Ok(());
    };
    let act = string(p, "action");
    if let Some((verb, index)) = act.split_once(':')
        && matches!(verb, "openImage" | "previewImage" | "openAttachment")
        && let Ok(index) = index.parse::<usize>()
        && let Some(entry) = app.client.state.transcripts.get(&k).and_then(|t| t.item(&item))
    {
        let image = match &entry.body {
            ItemBody::ToolCall(tool) if verb != "openAttachment" => tool.images.get(index)
                .map(|image| (image.upload_id.clone(), image.name.clone())),
            ItemBody::UserMessage(message) if verb == "openAttachment" => match message.parts.get(index) {
                Some(InputPart::Image(image)) => Some((image.upload_id.clone(), image.name.clone().unwrap_or_else(|| "Image attachment".to_owned()))),
                _ => None,
            },
            _ => None,
        };
        if let Some((id, label)) = image {
            let source = crate::images::ImageSource::Target(k.target.clone(), id);
            if verb == "previewImage" {
                app.images.preview(source);
            } else {
                app.images.open(source, label);
            }
        }
        return Ok(());
    }

    let action = match act {
        "toggleRun" => {
            app.ui.expansion.toggle_run(&k, &item);
            return Ok(());
        }
        "toggleActivity" => {
            let reasoning = app.client.state.transcripts.get(&k).and_then(|t| t.item(&item))
                .is_some_and(|item| matches!(item.body, ItemBody::Reasoning(_)));
            if reasoning {
                app.ui.expansion.toggle_thought(&k, &item);
            } else {
                app.ui.expansion.toggle_line(&k, &item);
            }
            return Ok(());
        }
        "toggleWorked" | "toggleTurnChanges" => {
            let Some(t) = app.client.state.transcripts.get(&k) else { return Ok(()) };
            let Some(turn) = t.item(&item).map(|item| &item.turn_id) else { return Ok(()) };
            let files_open = app.ui.expansion.turns.contains(&(k.clone(), turn.clone()));
            let worked_open = if act == "toggleWorked" {
                let state = &app.client.state;
                let blocks = activity::blocks(state, &AttentionMap::compute(state), &k, &app.ui.expansion);
                let Some(expanded) = turns::collapse(t, &k, &blocks, &app.ui.worked, &app.ui.expansion, work_focus(app, &k))
                    .into_iter().find_map(|block| match block {
                        Block::Worked { turn: id, expanded, .. } if &id == turn => Some(expanded),
                        _ => None,
                    })
                else { return Ok(()) };
                expanded
            } else {
                false
            };
            if act == "toggleTurnChanges" || (worked_open && files_open) {
                let items: Vec<_> = t.items().iter().filter(|item| &item.turn_id == turn)
                    .map(|item| item.id.clone()).collect();
                app.ui.expansion.toggle_turn(&k, turn, &items);
            }
            if act == "toggleWorked" {
                if app.ui.worked.is_open(&k, turn) == worked_open {
                    app.ui.worked.toggle(&k, turn);
                }
                if worked_open {
                    let in_turn = |id: &ItemId| t.item(id).is_some_and(|item| &item.turn_id == turn);
                    let scrolled = app.ui.scroll_to.as_ref().is_some_and(in_turn);
                    let focused = app.client.state.focus_item.as_ref().is_some_and(|(thread, item)| thread == &k && in_turn(item));
                    if scrolled {
                        app.ui.scroll_to = None;
                    }
                    if focused {
                        app.client.state.focus_item = None;
                    }
                }
            }
            return Ok(());
        }
        id if id.starts_with("toggleFile:") || id.starts_with("showDiffRest:") => {
            let (verb, index) = id.split_once(':').expect("file action");
            let Some(file) = index.parse::<usize>().ok().filter(|&file| {
                app.client.state.transcripts.get(&k).and_then(|t| t.item(&item))
                    .and_then(activity::file_stats).is_some_and(|files| file < files.len())
            }) else { return Ok(()); };
            if verb == "toggleFile" {
                app.ui.expansion.toggle_file(&k, &item, file);
            } else {
                app.ui.expansion.open_rest(&k, &item, file);
            }
            return Ok(());
        }
        "retrySend" => Action::RetrySend(k, item),
        "editSend" => Action::EditSend(k, item),
        "dismissSend" => Action::DismissSend(k, item),
        "checkLatest" => Action::CheckLatest(k),
        "continueBudget" | "reviewWorkers" => {
            let state = &app.client.state;
            let notice = state.transcripts.get(&k).and_then(|t| t.item(&item))
                .is_some_and(|i| matches!(&i.body, ItemBody::Notice(n) if n.notice_kind == yaac_protocol::NoticeKind::BudgetExhausted));
            let Some(root) = yaac_core::agents::root_of(state, &k)
                .filter(|root| notice && state.budgets.get(root).is_some_and(|budget| budget.exhausted)) else { return Ok(()) };
            if act == "continueBudget" { Action::ContinueBudget(root) } else { Action::Panel(PanelInput::Dock) }
        }
        id if id.starts_with("openCall:") => {
            let state = &app.client.state;
            let Some((transcript, item, index)) = (|| {
                let transcript = state.transcripts.get(&k)?;
                Some((transcript, transcript.item(&item)?, id["openCall:".len()..].parse::<usize>().ok()?))
            })() else { return Ok(()); };
            let Some((_, call)) = agentcall::of(item) else { return Ok(()); };
            let line = agentcall::line(state, &AttentionMap::compute(state), &k, Some(transcript), item, call);
            let Some(pill) = line.pills.into_iter().nth(index) else { return Ok(()); };
            let Some(peer) = pill.key else { return Ok(()); };
            Action::OpenAt(peer, pill.at)
        }
        id if id.starts_with("openPeer:") => {
            let state = &app.client.state;
            let Some(peer) = key(app, &id["openPeer:".len()..]) else { return Ok(()) };
            let address = peer.address();
            let Some(source) = state.transcripts.get(&k).and_then(|t| t.item(&item)) else { return Ok(()) };
            let message = match &source.body {
                ItemBody::MeshReceived(m) if m.from.address == address => Some(&m.message.id),
                ItemBody::MeshSent(m) if m.to.address == address => {
                    if m.receipt.status == yaac_protocol::ReceiptStatus::Failed
                        && matches!(m.receipt.reason, Some(yaac_protocol::ReceiptReason::ThreadDeleted | yaac_protocol::ReceiptReason::UnknownAddress)) {
                        return Ok(());
                    }
                    Some(&m.message.id)
                }
                ItemBody::MeshSpawn(m) if m.threads.iter().any(|t| t.peer.address == address) => None,
                ItemBody::MeshWait(m) if m.targets.iter().any(|t| t.peer.address == address) => None,
                _ => return Ok(()),
            };
            let reciprocal = message.and_then(|id| {
                state.transcripts.get(&peer)?.items().iter().find_map(|i| match &i.body {
                    ItemBody::MeshReceived(m) if &m.message.id == id => Some(i.id.clone()),
                    ItemBody::MeshSent(m) if &m.message.id == id => Some(i.id.clone()),
                    _ => None,
                })
            });
            Action::OpenAt(peer, reciprocal)
        }
        id => {
            let state = &app.client.state;
            if !state.may_answer(&k) {
                return Ok(());
            }
            let Some(ItemBody::Request(r)) = state
                .transcripts
                .get(&k)
                .and_then(|t| t.item(&item))
                .map(|i| &i.body)
            else {
                return Ok(());
            };
            if r.state != RequestState::Pending {
                return Ok(());
            }
            let Some(response) = answer(r, id, p) else {
                return Err("Complete the request before submitting it.".into());
            };
            let summary = requests::answer_summary(&r.body, &response);
            Action::Respond {
                key: k,
                request: r.request_id.clone(),
                response,
                summary,
            }
        }
    };
    app.apply(vec![action]);
    Ok(())
}

/// Only changed rows cross the pipe during a stream. A thread switch always supplies all
/// rows, so WPF can discard its projection of the previous conversation.
pub(super) fn diff(old: &Value, new: &mut Value) {
    if old["threadId"] != new["threadId"] {
        return;
    }
    if old["prompts"] == new["prompts"] {
        new.as_object_mut().expect("transcript view").remove("prompts");
    }
    let (Some(older), Some(rows)) = (old["rows"].as_array(), new["rows"].as_array()) else {
        return;
    };
    let before: std::collections::HashMap<_, _> =
        older.iter().map(|row| (string(row, "id"), row)).collect();
    let changed: Vec<_> = rows
        .iter()
        .filter(|row| before.get(string(row, "id")) != Some(row))
        .collect();
    if !rows.is_empty() && changed.len() >= rows.len() {
        return;
    }
    let old_ids = older.iter().map(|row| string(row, "id"));
    let ids = rows.iter().map(|row| string(row, "id"));
    let order = (!ids.clone().eq(old_ids)).then(|| ids.collect::<Vec<_>>());
    let patch = json!({ "rows": changed, "order": order });
    new.as_object_mut().expect("transcript view").remove("rows");
    new["rowChanges"] = patch;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_item_patches_preserve_row_identity_and_order() {
        let old = json!({ "threadId": "local/a", "rows": [
            { "id": "user", "text": "hello" }, { "id": "agent", "text": "hel" },
        ] });
        let mut new = json!({ "threadId": "local/a", "rows": [
            { "id": "user", "text": "hello" }, { "id": "agent", "text": "hello" },
        ] });
        diff(&old, &mut new);
        assert_eq!(
            new["rowChanges"],
            json!({
                "rows": [{ "id": "agent", "text": "hello" }], "order": null,
            })
        );
        assert!(new.get("rows").is_none());
    }

    #[test]
    fn command_header_is_one_line_and_details_keep_the_full_command() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::parser();
        app.apply(vec![Action::Open(key.clone())]);
        let transcript = app.client.state.transcripts.get(&key).unwrap();
        let mut item = transcript.items().iter()
            .find(|item| matches!(&item.body, ItemBody::ToolCall(tool) if tool.tool == ToolKind::Command))
            .unwrap().clone();
        let command = "cat <<'EOF'\r\n\tHello\nEOF";
        let ItemBody::ToolCall(tool) = &mut item.body else { unreachable!() };
        tool.title = command.to_owned();
        tool.input = None;
        tool.cwd = None;
        tool.output.clear();
        tool.output_truncated = false;
        tool.exit_code = None;
        item.status = ItemStatus::Completed;

        let map = AttentionMap::compute(&app.client.state);
        let mut markdown = super::super::markdown_view::MarkdownView::default();
        let row = render(&app, &map, &key, transcript, &item, &mut markdown, false).unwrap();
        assert_eq!(row["lead"], "cat <<'EOF' Hello EOF");
        assert_eq!(row["parts"], json!([{ "kind": "command", "text": command }]));

        let ItemBody::ToolCall(tool) = &mut item.body else { unreachable!() };
        tool.title = "Read skill instructions".to_owned();
        tool.input = Some(json!({ "command": command }));
        tool.output = "Hello\n".to_owned();
        tool.output_truncated = true;
        tool.output_bytes = Some(4096);
        tool.exit_code = Some(0);
        let row = render(&app, &map, &key, transcript, &item, &mut markdown, false).unwrap();
        assert_eq!(row["lead"], "Read skill instructions");
        assert_eq!(row["parts"][0]["text"], command);
        assert_eq!(row["parts"][1]["text"], "Hello\n");
        assert_eq!(row["parts"][2]["text"], "Output is longer (4096 bytes). The rest is on the target.");
        assert_eq!(row["parts"][3], json!({ "kind": "text", "text": "Success", "tone": "success" }));

        let ItemBody::ToolCall(tool) = &mut item.body else { unreachable!() };
        tool.exit_code = Some(2);
        tool.images.push(yaac_protocol::ToolImage { upload_id: yaac_protocol::UploadId::new("u_tool_image"), name: "Result image".into() });
        item.status = ItemStatus::Failed;
        let row = render(&app, &map, &key, transcript, &item, &mut markdown, false).unwrap();
        assert_eq!(row["parts"][3], json!({ "kind": "text", "text": "Failed (exit 2)", "tone": "error" }));
        assert_eq!(row["parts"][4]["kind"], "image");
        assert_eq!(row["parts"][4]["action"]["id"], "openImage:0");
        assert_eq!(row["parts"][4]["request"]["id"], "previewImage:0");
    }

    #[test]
    fn file_diff_excerpt_reveals_only_the_requested_files_remaining_hunks() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::parser();
        app.apply(vec![Action::Open(key.clone())]);
        let transcript = app.client.state.transcripts.get_mut(&key).unwrap();
        let mut item = transcript.items().iter()
            .find(|item| matches!(&item.body, ItemBody::ToolCall(tool) if tool.tool == ToolKind::Command))
            .unwrap().clone();
        let first = format!("--- a/first\n+++ b/first\n@@ -1,40 +1,40 @@\n{}@@ -50 +50 @@\n-before\n+after\n", " context\n".repeat(40));
        let second = "--- a/second\n+++ b/second\n@@ -1 +1 @@\n-second before\n+second after\n";
        let ItemBody::ToolCall(tool) = &mut item.body else { unreachable!() };
        tool.tool = ToolKind::FileChange;
        tool.output = format!("{first}{second}");
        tool.exit_code = None;
        item.status = ItemStatus::Completed;
        transcript.upsert(item.clone(), 99);
        transcript.active_turn = Some(item.turn_id.clone());

        let map = AttentionMap::compute(&app.client.state);
        let mut markdown = super::super::markdown_view::MarkdownView::default();
        let action = |action_id| json!({ "id": item.id, "action": action_id });
        command(&mut app, "transcript.action", &action("toggleFile:0")).unwrap();
        let mut files = |app: &YaacApp| {
            let mut projection = view(app, &map, &mut markdown);
            projection["rows"].as_array_mut().unwrap().drain(..)
                .filter(|row| row["actionId"] == item.id.as_str() && row["kind"] == "tool")
                .collect::<Vec<_>>()
        };
        let initial = files(&app);
        assert_eq!(initial.len(), 2);
        let row = &initial[0];
        assert_eq!(row["lead"], "Edited first");
        assert_eq!(row["added"], "+1");
        assert_eq!(row["removed"], "−1");
        assert_eq!(row["activityOpen"], true);
        assert_eq!(initial[1]["lead"], "Edited second");
        assert_ne!(row["id"], initial[1]["id"]);
        assert_eq!(row["restAction"]["label"], "Show the rest (3 more lines)");
        assert_eq!(row["parts"][0]["blocks"]["copyText"], first.strip_suffix('\n').unwrap());
        assert!(!string(&row["parts"][0], "text").contains("+after"));
        assert_eq!(initial[1]["activityOpen"], false);

        command(&mut app, "transcript.action", &action("showDiffRest:0")).unwrap();
        let revealed = files(&app);
        let row = &revealed[0];
        assert!(row.get("restAction").is_none());
        assert!(string(&row["parts"][0], "text").contains("+after"));
        assert_eq!(revealed[1]["activityOpen"], false);

        let second_action = string(&revealed[1]["activityToggle"], "id");
        command(&mut app, "transcript.action", &action(second_action)).unwrap();
        let both = files(&app);
        assert_eq!(both[0]["activityOpen"], true);
        assert_eq!(both[1]["activityOpen"], true);
        assert!(string(&both[1]["parts"][0], "text").contains("+second after"));
    }

    #[test]
    fn failed_command_stays_visible_when_wpf_groups_are_collapsed() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::parser();
        app.apply(vec![Action::Open(key.clone())]);
        let t = app.client.state.transcripts.get_mut(&key).unwrap();
        let mut items = t.items().to_vec();
        let command_index = items.iter().position(|item| matches!(&item.body,
            ItemBody::ToolCall(tool) if tool.tool == yaac_protocol::ToolKind::Command)).unwrap();
        let commands: Vec<_> = (0..17).map(|n| {
            let mut item = items[command_index].clone();
            item.id = format!("command-{n}").into();
            if n == 8 {
                item.status = ItemStatus::Failed;
            }
            item
        }).collect();
        items.splice(command_index..command_index, commands);
        t.replace_all(items, 99);
        t.active_turn = Some(TurnId::from("p-t1"));

        let mut markdown = super::super::markdown_view::MarkdownView::default();
        let map = AttentionMap::compute(&app.client.state);
        let initial = view(&app, &map, &mut markdown);
        let rows = initial["rows"].as_array().unwrap();
        let error = rows.iter().position(|row| row["id"] == "command-8").unwrap();
        assert_eq!(rows[error]["tone"], "error");
        assert_eq!(rows[error - 1]["actionId"], "command-0");
        assert_eq!(rows[error + 1]["actionId"], "command-9");
        assert_eq!(rows[error - 1]["groupOpen"], false);
        assert_eq!(rows[error + 1]["groupOpen"], false);

        command(&mut app, "transcript.action", &json!({ "id": "command-0", "action": "toggleRun" })).unwrap();
        let open = view(&app, &map, &mut markdown);
        let rows = open["rows"].as_array().unwrap();
        assert!(rows.iter().any(|row| row["id"] == "command-7"));
        assert!(rows.iter().any(|row| row["id"] == "command-8"));
        assert!(!rows.iter().any(|row| row["id"] == "command-10"));
    }

    #[test]
    fn finished_turn_keeps_errors_and_file_summary_and_restores_hidden_anchor() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::parser();
        app.apply(vec![Action::Open(key.clone())]);
        let t = app.client.state.transcripts.get_mut(&key).unwrap();
        let mut items = t.items().to_vec();
        let file_change = items.iter_mut().find(|item| item.id.as_str() == "parser-3").unwrap();
        let ItemBody::ToolCall(tool) = &mut file_change.body else { unreachable!() };
        tool.tool = ToolKind::FileChange;
        tool.exit_code = None;
        tool.output = ["first", "second"].map(|path| {
            format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-before\n+after\n")
        }).concat();
        items.iter_mut().find(|item| item.id.as_str() == "parser-4").unwrap().status = ItemStatus::Failed;
        t.replace_all(items, 99);

        let mut markdown = super::super::markdown_view::MarkdownView::default();
        let map = AttentionMap::compute(&app.client.state);
        let act = |id, action| json!({ "threadId": key.address(), "id": id, "action": action });
        let read = |app: &YaacApp, markdown: &mut super::super::markdown_view::MarkdownView| view(app, &map, markdown);
        app.ui.restore = Some(yaac_core::history::Anchor { item: ItemId::from("parser-6"), offset: 12.0 });
        let initial = read(&app, &mut markdown);
        let rows = initial["rows"].as_array().unwrap();
        let ids: Vec<_> = rows.iter().map(|row| string(row, "id")).collect();
        assert_eq!(ids, ["parser-1", "~worked:p-t1", "parser-4", "parser-12", "~changes:p-t1"]);
        assert!(string(&rows[1], "lead").starts_with("Worked for "));
        assert_eq!(rows[2]["tone"], "error");
        assert_eq!(rows[4]["lead"], "Changed 2 files");
        assert_eq!(rows[4]["tail"], "show all");
        assert_eq!(initial["anchor"], json!({ "id": "~worked:p-t1", "offset": 0 }));

        command(&mut app, "transcript.action", &act("parser-3", "toggleTurnChanges")).unwrap();
        let opened = read(&app, &mut markdown);
        let rows = opened["rows"].as_array().unwrap();
        assert_eq!(rows[1]["groupOpen"], true);
        for id in ["parser-3", "~file:1:parser-3"] {
            assert!(rows.iter().any(|row| row["id"] == id && row["activityOpen"] == true));
        }
        assert_eq!(rows.last().unwrap()["tail"], "hide all");
        command(&mut app, "transcript.action", &act("parser-3", "toggleTurnChanges")).unwrap();
        assert_eq!(read(&app, &mut markdown)["rows"], initial["rows"]);

        app.apply(vec![Action::OpenAt(key.clone(), Some(ItemId::from("parser-6")))]);
        let focused = read(&app, &mut markdown);
        assert_eq!(focused["focusId"], "parser-6");
        assert!(focused["rows"].as_array().unwrap().iter().any(|row| row["id"] == "parser-6"));
        command(&mut app, "transcript.action", &act("parser-1", "toggleWorked")).unwrap();
        assert_eq!(read(&app, &mut markdown)["rows"], initial["rows"]);
        command(&mut app, "transcript.action", &act("parser-1", "toggleWorked")).unwrap();
        assert_eq!(read(&app, &mut markdown)["rows"][1]["groupOpen"], true);

        let t = app.client.state.transcripts.get_mut(&key).unwrap();
        t.turns.entry(TurnId::from("p-t1")).or_default().ended = Some(turns::TurnEnd {
            status: yaac_protocol::TurnStatus::Failed,
            at: None,
            error: Some("Turn failed".to_owned()),
        });
        command(&mut app, "transcript.action", &act("parser-1", "toggleWorked")).unwrap();
        let failed = read(&app, &mut markdown);
        assert_eq!(failed["rows"][1]["groupOpen"], false);
        assert_eq!(failed["rows"][1]["tone"], "error");
        assert_eq!(failed["rows"][1]["groupError"], "Turn failed");
    }

    #[test]
    fn changed_conversations_send_a_full_transcript() {
        let old = json!({ "threadId": "local/a", "rows": [{ "id": "item", "text": "first" }] });
        let mut new =
            json!({ "threadId": "local/b", "rows": [{ "id": "item", "text": "second" }] });
        diff(&old, &mut new);
        assert!(new.get("rowChanges").is_none());
        assert_eq!(new["rows"][0]["text"], "second");
    }
}
