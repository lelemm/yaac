//! External dialogs in the view frames (P418, docs/16-external-dialogs.md): the frame a native
//! renderer (WPF, Android) builds a dialog from — the integrator's layout tree as data, the
//! source line, and the problems of the last submit — and the commands that answer it. The
//! renderers build their controls from the tree at runtime and keep the values the user typed;
//! they send those values with the button pressed, and this controller validates and answers
//! ([`yaac_core::dialogs`]). Also the inbox: its rows (Needs you) and the open inbox dialog.

use super::*;
use yaac_core::dialogs::{self, DialogDraft};
use yaac_protocol::attach::{self, INBOX_THREAD};
use yaac_protocol::{DialogAnswer, DialogRequest, RequestId, RequestState, TargetId, ThreadId};

/// The dialog part of a frame: what the renderer walks. `enabled` is false once the integrator
/// has gone or the request is not this window's to answer; Dismiss stays.
pub(super) fn frame(d: &DialogRequest, target_name: &str, draft: Option<&DialogDraft>, actionable: bool) -> Value {
    json!({
        "title": d.title,
        "source": dialogs::source_line(d, target_name),
        "gone": dialogs::gone_line(d),
        "layout": d.layout,
        "enabled": actionable && dialogs::answerable(d),
        "hasButtons": !dialogs::buttons(&d.layout).is_empty(),
        "problems": draft.map(|d| json!(d.problems)).unwrap_or_else(|| json!({})),
        "newerYaac": dialogs::NEWER_YAAC,
    })
}

/// The answer for a renderer's `dialog:<button>` (with `values`) or `dialogDismiss`; problems
/// are kept on the draft for the next frame.
pub(super) fn answer(app: &mut YaacApp, key: (TargetId, ThreadId, RequestId), d: &DialogRequest, action: &str, p: &Value) -> Result<DialogAnswer, String> {
    if action == "dialogDismiss" {
        app.ui.dialog_drafts.remove(&key);
        return Ok(dialogs::cancel());
    }
    let Some(button) = action.strip_prefix("dialog:") else { return Err("Unknown dialog action".into()) };
    let mut draft = DialogDraft::from_wire(&d.layout, &p["values"]);
    match dialogs::answer(&d.layout, &draft, button) {
        Ok(a) => {
            app.ui.dialog_drafts.remove(&key);
            Ok(a)
        }
        Err(problems) => {
            let first = problems.values().next().cloned().unwrap_or_default();
            draft.problems = problems;
            app.ui.dialog_drafts.insert(key, draft);
            Err(first)
        }
    }
}

/// The inbox rows for Needs you: one per open dialog, under its target.
pub(super) fn rows(app: &YaacApp) -> Value {
    let state = &app.client.state;
    Value::Array(
        state
            .inbox
            .needing_you()
            .into_iter()
            .map(|e| {
                let target_name = state.target_name(e.target);
                json!({
                    "target": e.target, "requestId": e.request.request_id, "title": e.dialog.title,
                    "source": e.dialog.source.name, "targetName": target_name,
                    "detail": dialogs::source_line(e.dialog, &target_name),
                    "unknown": e.request.state == RequestState::Unknown,
                    "gone": !e.dialog.channel_open,
                })
            })
            .collect(),
    )
}

/// The open inbox dialog, when one is shown (a sheet on the phone, a modal card on desktop).
pub(super) fn view(app: &YaacApp) -> Value {
    let Some((target, request_id)) = &app.ui.inbox_dialog else { return Value::Null };
    let state = &app.client.state;
    let Some(request) = state.inbox.request(target, request_id) else { return Value::Null };
    let Some(d) = dialogs::of(&request) else { return Value::Null };
    let key = (target.clone(), ThreadId::new(INBOX_THREAD), request_id.clone());
    let pending = request.state == RequestState::Pending;
    let mut v = frame(d, &state.target_name(target), app.ui.dialog_drafts.get(&key), pending);
    let (status, tone) = match &request.state {
        RequestState::Pending => (None, "warning"),
        RequestState::Submitted => (
            Some(format!("You chose: {}\nSent · waiting for the target to confirm", state.inbox.local_summary(target, request_id).unwrap_or("your answer"))),
            "muted",
        ),
        RequestState::Unknown => (
            Some("Outcome unknown\nYAAC lost contact while sending your answer. It may or may not have arrived. Your answer will not be sent again.".to_owned()),
            "warning",
        ),
        other => (Some(other.as_str().to_owned()), "muted"),
    };
    let mut actions = vec![];
    if pending {
        actions.push(json!({ "id": "dialogDismiss", "label": "Dismiss", "enabled": true, "key": "Escape" }));
    }
    if request.state == RequestState::Unknown {
        actions.push(sidebar_view::action("checkLatest", "Check latest status", true));
    }
    actions.push(sidebar_view::action("later", "Later", true));
    v["target"] = json!(target);
    v["requestId"] = json!(request_id);
    v["pending"] = json!(pending);
    v["status"] = json!(status);
    v["tone"] = json!(tone);
    v["actions"] = json!(actions);
    v["requestLocation"] = json!(pending.then(|| attach::RequestLocation::inbox(target.clone(), request_id.clone())));
    v
}

/// `inbox.open {target, requestId}`, `inbox.action {target, requestId, action, values?}`,
/// `inbox.close {}` (Later: hidden, still in Needs you).
pub(super) fn command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let target = TargetId::new(string(p, "target"));
    let request_id = RequestId::new(string(p, "requestId"));
    match name {
        "inbox.open" => {
            app.apply(vec![Action::OpenInboxDialog(target, request_id)]);
            Ok(())
        }
        "inbox.close" => {
            app.ui.inbox_dialog = None;
            Ok(())
        }
        "inbox.action" => {
            let act = string(p, "action");
            match act {
                "later" => {
                    app.ui.inbox_dialog = None;
                    return Ok(());
                }
                "checkLatest" => {
                    app.apply(vec![Action::CheckInbox(target)]);
                    return Ok(());
                }
                _ => {}
            }
            let Some(request) = app.client.state.inbox.request(&target, &request_id) else { return Ok(()) };
            let Some(d) = dialogs::of(&request).cloned() else { return Ok(()) };
            if request.state != RequestState::Pending {
                return Ok(());
            }
            let key = (target.clone(), ThreadId::new(INBOX_THREAD), request_id.clone());
            let answer = answer(app, key, &d, act, p)?;
            if answer.cancelled {
                app.ui.inbox_dialog = None;
            }
            app.apply(vec![Action::RespondInbox { target, request: request_id, answer }]);
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::attention::AttentionMap;

    fn dialog(id: &str, open: bool) -> Value {
        json!({ "requestId": id, "state": "pending", "requestKind": "dialog", "title": "Deploy",
            "source": { "channelId": "c1", "name": "deployer" }, "channelOpen": open,
            "layout": { "type": "column", "children": [
                { "type": "text", "text": "Ship the build?" },
                { "type": "select", "id": "env", "label": "Environment", "required": true,
                  "options": [{ "value": "prod", "label": "Production" }, { "value": "stage", "label": "Staging" }] },
                { "type": "textInput", "id": "token", "label": "Token", "secret": true },
                { "type": "sparkles", "id": "future" },
                { "type": "button", "id": "go", "label": "Deploy", "kind": "primary" } ] } })
    }

    fn with_inbox(app: &mut YaacApp, open: bool) -> TargetId {
        let target = app.client.state.targets[0].entry.id.clone();
        app.client.state.inbox.replace(serde_json::from_value(json!({
            "target": target, "requests": [dialog("d1", open)], "channels": [{ "channelId": "c1", "name": "deployer", "connected": open }] })).unwrap());
        target
    }

    #[test]
    fn an_inbox_dialog_is_listed_opened_validated_and_answered_from_frames() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let target = with_inbox(&mut app, true);
        let listed = rows(&app);
        assert_eq!(listed[0]["title"], "Deploy");
        assert_eq!(listed[0]["source"], "deployer");
        assert_eq!(view(&app), Value::Null, "not shown until opened");

        command(&mut app, "inbox.open", &json!({ "target": target, "requestId": "d1" })).unwrap();
        let v = view(&app);
        assert_eq!(v["title"], "Deploy");
        assert!(v["source"].as_str().unwrap().starts_with("From deployer on "));
        assert_eq!(v["layout"]["children"][1]["type"], "select", "the tree travels as data");
        assert_eq!(v["layout"]["children"][3]["type"], "sparkles", "unknown parts are kept for the renderer's muted line");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["requestLocation"]["threadId"], yaac_protocol::attach::INBOX_THREAD);

        // A required choice missing: refused here, the problem comes back in the frame.
        let err = command(&mut app, "inbox.action", &json!({ "target": target, "requestId": "d1", "action": "dialog:go", "values": { "token": "s3cret" } }));
        assert!(err.is_err());
        assert_eq!(view(&app)["problems"]["env"], "Environment is required");
        assert!(app.client.state.inbox.answers.is_empty());

        command(&mut app, "inbox.action", &json!({ "target": target, "requestId": "d1", "action": "dialog:go", "values": { "env": "prod", "token": "s3cret" } })).unwrap();
        let sent = app.client.state.inbox.answers.values().next().expect("answered").clone();
        assert!(sent.summary.starts_with("Deploy · Environment: Production"), "{}", sent.summary);
        assert!(!sent.summary.contains("s3cret"));
        assert!(app.demo.as_ref().unwrap().requests().iter().any(|(t, method, p)| t.as_ref() == Some(&target)
            && method == "inbox/respond" && p["response"]["action"] == "go" && p["opId"].is_string()));
        assert!(view(&app)["status"].as_str().unwrap().starts_with("You chose: Deploy"));
        assert!(rows(&app).as_array().unwrap().is_empty(), "a submitted dialog leaves Needs you");
    }

    #[test]
    fn a_dialog_whose_integrator_is_away_says_so_and_still_answers() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let target = with_inbox(&mut app, false);
        command(&mut app, "inbox.open", &json!({ "target": target, "requestId": "d1" })).unwrap();
        let v = view(&app);
        assert_eq!(v["gone"], "deployer is not connected · your answer will be delivered when it reconnects");
        assert_eq!(v["enabled"], true);
        command(&mut app, "inbox.action", &json!({ "target": target, "requestId": "d1", "action": "dialogDismiss" })).unwrap();
        assert_eq!(app.client.state.inbox.answers.values().next().unwrap().summary, "Dismissed");
        assert_eq!(view(&app), Value::Null, "closing it closes the card");
    }

    #[test]
    fn a_dialog_in_a_thread_is_a_dialog_row_with_its_layout() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::parser();
        app.apply(vec![Action::Open(key.clone())]);
        let transcript = app.client.state.transcripts.get(&key).unwrap();
        let mut item = transcript.items()[0].clone();
        item.body = yaac_protocol::ItemBody::Request(serde_json::from_value(dialog("d1", true)).unwrap());
        let map = AttentionMap::compute(&app.client.state);
        let mut md = super::super::markdown_view::MarkdownView::default();
        let row = super::super::transcript::render_for_tests(&app, &map, &key, transcript, &item, &mut md).unwrap();
        assert_eq!(row["kind"], "dialog");
        assert_eq!(row["title"], "Deploy");
        assert_eq!(row["dialog"]["layout"]["children"][4]["id"], "go");
        assert!(row["dialog"]["source"].as_str().unwrap().starts_with("From deployer on "));
        assert!(row["actions"].as_array().unwrap().iter().any(|a| a["id"] == "dialogDismiss"));
        assert_eq!(row["requestLocation"]["requestId"], "d1");
    }
}
