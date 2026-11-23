use super::*;
use std::collections::BTreeMap;
use yaac_core::activity::{self, Block, Expansion};
use yaac_core::agentcall::{self, Echoes};
use yaac_core::attention::AttentionMap;
use yaac_core::permission::{self, BudgetFields, PermissionDecision};
use yaac_core::requests::{self, CardState};
use yaac_core::sending::{self, SendState};
use yaac_core::transcript::Transcript;
use yaac_protocol::{
    AgentCallKind, AgentCallOutcome, ApprovalAnswer, InputPart, Item, ItemBody, ItemStatus, MeshPeer, QuestionAnswer,
    QuestionAnswers, RequestBody, RequestItem, RequestResponse, RequestState, TextRole,
};

fn peer(peer: &MeshPeer) -> &str {
    peer.title.as_deref().unwrap_or(peer.address.as_str())
}

fn peer_action(app: &YaacApp, peer_data: &MeshPeer) -> Value {
    let peer_key = key(app, peer_data.address.as_str());
    let mut action = sidebar_view::action(
        &format!("openPeer:{}", peer_data.address),
        peer(peer_data),
        peer_key.is_some(),
    );
    action["machine"] = json!(peer_data.target_name);
    action["logo"] = json!(peer_key.as_ref().and_then(|key| app.client.state.provider_of(key))
        .and_then(|p| crate::logos::for_kind(p.kind.as_str())).map(|(logo, _)| crate::logos::source(logo).0));
    action
}

fn card(app: &YaacApp, key: &ThreadKey, transcript: &Transcript, request: &RequestItem) -> Value {
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
        CardState::Submitted { summary } => (format!("Submitted · {summary}"), "muted"),
        CardState::Confirmed { summary, when } => (
            when.map_or_else(|| summary.clone(), |w| format!("{summary} · {w}")),
            "success",
        ),
        CardState::ResolvedElsewhere { text } => (text, "muted"),
        CardState::Expired => ("Expired".to_owned(), "muted"),
        CardState::OutcomeUnknown => ("Outcome unknown".to_owned(), "warning"),
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
                [a.reason.as_deref(), a.scope.as_deref()]
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
                    ("threads", "Threads spawned", prompt.budget.threads),
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
                prompt.actions.join("\n"),
            )
        }
        RequestBody::Form(f) => (
            "form",
            f.source.clone().unwrap_or_else(|| "Form".to_owned()),
            f.message.clone(),
        ),
        _ => ("request", "Agent request".into(), String::new()),
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

fn render(app: &YaacApp, map: &AttentionMap, key: &ThreadKey, transcript: &Transcript, item: &Item, markdown: &mut super::markdown_view::MarkdownView) -> Option<Value> {
    let mut row = match &item.body {
        ItemBody::UserMessage(message) => {
            let mut parts = vec![];
            let mut attachments = vec![];
            for part in &message.parts {
                match part {
                    InputPart::Text(t) => parts.push(json!({
                        "kind": match t.role { Some(TextRole::Pasted) => "pasted", Some(TextRole::Command) => "command",
                            Some(TextRole::Context) => "detail", _ => "markdown" },
                        "text": t.text, "label": t.label,
                    })),
                    InputPart::Image(image) => attachments.push(json!({ "label": image.name.as_deref().unwrap_or(image.upload_id.as_str()),
                        "image": true, "missing": transcript.missing_uploads.contains(&image.upload_id) })),
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
                        vec![sidebar_view::action("dismissSend", "Dismiss", true)],
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
        ItemBody::Reasoning(r) => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item)?;
            json!({ "kind": "activity", "mark": "•", "lead": line.text, "detail": r.reasoning.trim(),
                "disclosureName": "thought", "copyText": line.text })
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
            let parts: Vec<_> = [("Input", detail.raw_input), ("Output", detail.raw_output)]
                .into_iter().filter(|(_, text)| !text.is_empty())
                .map(|(label, text)| json!({ "kind": "code", "label": label, "text": text, "expanded": false }))
                .collect();
            json!({ "kind": "call", "mark": mark, "lead": line.lead, "after": line.after,
                "preview": line.preview, "tail": tail, "pills": pills, "tone": if failed { "error" } else { "muted" },
                "heading": detail.heading, "message": detail.text, "result": detail.result, "receipt": detail.status,
                "disclosureLabel": "Raw call",
                "parts": parts, "copyText": line.plain(),
            })
        }
        ItemBody::ToolCall(t) => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item).expect("tool activity");
            let files = yaac_core::activity::file_stats(item);
            let changes = files.as_ref().map(|files| {
                let mut totals = yaac_core::diff::TurnTotals::default();
                for file in files {
                    totals.add(file);
                }
                let (added, removed) = yaac_core::diff::counts(totals.added, totals.removed);
                (totals.text(), added, removed)
            });
            let mut row = json!({
                "kind": "tool", "mark": if line.failed { "×" } else { "•" },
                "lead": changes.as_ref().map_or(line.text.as_str(), |(title, _, _)| title.as_str()),
                "tone": if line.failed { "error" } else { "muted" },
                "parts": [{ "kind": if files.is_some() { "diff" } else { "code" },
                    "text": t.output, "label": if t.output_truncated { "Truncated output" } else { "" }, "expanded": true }],
                "receipt": t.cwd, "copyText": line.text,
            });
            if let Some((_, added, removed)) = changes {
                row["added"] = json!(added);
                row["removed"] = json!(removed);
            }
            row
        }
        ItemBody::Plan(p) => {
            let line = yaac_core::activity::line_of(&app.client.state, map, key, 0, item)?;
            let detail = p.explanation.iter().map(String::as_str).map(str::to_owned)
                .chain(p.steps.iter().map(|s| format!("{} {}", if s.state == yaac_protocol::PlanStepState::Completed { "✓" } else { "○" }, s.text)))
                .collect::<Vec<_>>().join("\n");
            json!({ "kind": "activity", "mark": "•", "lead": line.text, "detail": detail,
                "disclosureName": "plan", "copyText": line.text })
        }
        ItemBody::Request(r) => card(app, key, transcript, r),
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
        ItemBody::Notice(n) => json!({ "kind": "notice", "title": n.text, "detail": n.body }),
        ItemBody::Handoff(h) => {
            json!({ "kind": "handoff", "title": "Forked history", "detail": h.transcript, "disclosureLabel": "Carried transcript" })
        }
        ItemBody::MeshSent(m) => {
            json!({ "kind": "call", "mark": "↗", "heading": "To", "message": m.message.text,
                "preview": yaac_core::message::one_line(&m.message.text), "pills": [peer_action(app, &m.to)],
                "tail": yaac_core::header::receipt_text(&m.receipt.status, m.receipt.reason.as_ref(), m.receipt.detail.as_deref(), m.to.target_name.as_deref().unwrap_or("the target")) })
        }
        ItemBody::MeshReceived(m) => {
            json!({ "kind": "call", "mark": "↙", "heading": "From", "message": m.message.text,
                "preview": yaac_core::message::one_line(&m.message.text), "pills": [peer_action(app, &m.from)] })
        }
        ItemBody::MeshSpawn(m) => json!({ "kind": "call", "mark": "⑂", "lead": "Started", "heading": "Workers",
            "pills": m.threads.iter().map(|t| peer_action(app, &t.peer)).collect::<Vec<_>>() }),
        ItemBody::MeshWait(m) => json!({ "kind": "call", "mark": "⌛", "lead": "Waiting for", "heading": "Agents",
            "pills": m.targets.iter().map(|t| peer_action(app, &t.peer)).collect::<Vec<_>>() }),
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
        "agent" => &["text"],
        "handoff" => &["detail"],
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

pub(super) fn view(app: &YaacApp, map: &AttentionMap, markdown: &mut super::markdown_view::MarkdownView) -> Value {
    let selected = selected_thread(app);
    markdown.select(selected);
    let Some(key) = selected else {
        return json!({ "emptyText": "", "rows": [] });
    };
    let state = &app.client.state;
    let transcript = state.transcripts.get(key);
    let focus = app.ui.focus_card.as_ref().or(app.ui.scroll_to.as_ref());
    let echoes = transcript.map(|t| Echoes::of(t.items()));
    let mut rows = Vec::new();
    let mut hidden_anchor = None;
    if let Some(t) = transcript {
        let groups: BTreeMap<_, _> = activity::blocks(state, map, key, &Expansion::default())
            .into_iter()
            .filter_map(|block| match block {
                Block::Folded { first, text, failed, last_index, .. } => {
                    Some((t.index_of(&first)?, (first, text, failed, last_index)))
                }
                _ => None,
            })
            .collect();
        let focus_index = focus.and_then(|id| t.index_of(id));
        let anchor_index = app.ui.restore.as_ref().and_then(|anchor| t.index_of(&anchor.item));
        let mut skip_through = None;
        for (index, item) in t.items().iter().enumerate() {
            if skip_through.is_some_and(|end| index <= end) {
                continue;
            }
            if let Some((first, text, failed, last)) = groups.get(&index) {
                let id = format!("~run:{first}");
                let contains = |i: usize| (index..=*last).contains(&i);
                let open = app.ui.expansion.runs.contains(&(key.clone(), first.clone()))
                    || focus_index.is_some_and(contains);
                if !open {
                    skip_through = Some(*last);
                    if anchor_index.is_some_and(contains) {
                        hidden_anchor = Some(json!({ "id": id, "offset": 0 }));
                    }
                }
                rows.push(json!({ "id": id, "actionId": first, "kind": "activity",
                    "mark": if *failed { "×" } else { "›" }, "tone": if *failed { "error" } else { "muted" },
                    "lead": text, "disclosureName": "command group", "groupOpen": open,
                    "groupToggle": sidebar_view::action("toggleRun", "Toggle command group", true) }));
                if !open {
                    continue;
                }
            }
            if (focus == Some(&item.id) || !echoes.as_ref().is_some_and(|e| e.hides(item)))
                && let Some(row) = render(app, map, key, t, item, markdown)
            {
                rows.push(row);
            }
        }
    }
    let anchor = hidden_anchor.or_else(|| app.ui.restore.as_ref().and_then(|anchor| {
        transcript?
            .item(&anchor.item)
            .filter(|item| !echoes.as_ref().is_some_and(|e| e.hides(item)))
            .map(|item| json!({ "id": row_id(item), "offset": anchor.offset }))
    }));
    let focus = focus.and_then(|id| transcript?.item(id)).map(row_id);
    json!({ "threadId": key.address(), "rows": rows, "loading": transcript.is_some_and(|t| !t.is_loaded()),
        "emptyText": "No messages yet", "focusId": focus, "focusToken": focus, "anchor": anchor })
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
                threads: budget_value(p, "threads", fallback.threads),
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
    if name == "transcript.anchor" {
        let offset = p["offset"]
            .as_f64()
            .unwrap_or_default()
            .clamp(0.0, 100_000.0);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "bounded DIP offset is stored at UI precision"
        )]
        let id = string(p, "id");
        let id = id.strip_prefix("~run:").unwrap_or(id);
        let anchor = selected_item(app, id).map(|item| yaac_core::history::Anchor {
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
    let action = match act {
        "toggleRun" => {
            app.ui.expansion.toggle_run(&k, &item);
            return Ok(());
        }
        "retrySend" => Action::RetrySend(k, item),
        "editSend" => Action::EditSend(k, item),
        "dismissSend" => Action::DismissSend(k, item),
        "checkLatest" => Action::CheckLatest(k),
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
            let Some(peer) = key(app, &id["openPeer:".len()..]) else {
                return Ok(());
            };
            Action::Open(peer)
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
    fn changed_conversations_send_a_full_transcript() {
        let old = json!({ "threadId": "local/a", "rows": [{ "id": "item", "text": "first" }] });
        let mut new =
            json!({ "threadId": "local/b", "rows": [{ "id": "item", "text": "second" }] });
        diff(&old, &mut new);
        assert!(new.get("rowChanges").is_none());
        assert_eq!(new["rows"][0]["text"], "second");
    }
}
