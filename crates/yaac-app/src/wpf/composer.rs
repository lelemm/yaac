use super::*;
use yaac_core::attachments::{self, ChipStatus, ChipView};
use yaac_core::controls::{self, Choice, ControlRow, ControlRowView};
use yaac_core::header::{self, ComposerView};
use yaac_core::images::ImageRoute;
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::prefs::SendKey;
use yaac_core::state::{AgentsPolicy, SendAs};
use yaac_protocol::{EffortId, InstanceId, ModeId, ModelId};

use sidebar_view::action;

fn option(id: &str, label: &str, enabled: bool, selected: bool, detail: Option<&str>) -> Value {
    json!({ "id": id, "label": label, "enabled": enabled, "checked": selected, "detail": detail })
}

fn menu(id: &str, label: &str, enabled: bool, children: Vec<Value>) -> Value {
    json!({ "id": id, "label": label, "enabled": enabled, "children": Value::Array(children) })
}

fn choices<T: PartialEq>(id: &str, value: &Choice<T>, to_id: impl Fn(&T) -> String) -> Value {
    menu(
        id,
        &value.label,
        !value.options.is_empty(),
        value
            .options
            .iter()
            .map(|(v, label, default)| {
                let selected = value.current.as_ref() == Some(v);
                option(
                    &format!("{id}:{}", to_id(v)),
                    label,
                    true,
                    selected,
                    default.then_some("Default"),
                )
            })
            .collect(),
    )
}

fn controls(row: &ControlRow) -> Vec<Value> {
    let mut v = Vec::new();
    if let Some(model) = &row.model {
        let groups = model
            .groups
            .iter()
            .map(|group| {
                let models = group
                    .models
                    .iter()
                    .map(|m| {
                        option(
                            &format!("model:{}", json!([group.instance, m.id])),
                            &m.name,
                            m.enabled,
                            m.selected,
                            m.effort_range.as_deref(),
                        )
                    })
                    .collect();
                let mut action = menu(group.instance.as_str(), &group.name, group.enabled, models);
                action["detail"] = json!(group.reason);
                action
            })
            .collect();
        v.push(menu(
            "model",
            &format!("{} · {}", model.instance_label, model.model_label),
            model.changeable,
            groups,
        ));
    }
    if let Some(effort) = &row.effort {
        v.push(choices("effort", effort, ToString::to_string));
    }
    if let Some(mode) = &row.mode {
        v.push(choices("mode", mode, ToString::to_string));
    }
    if let Some(agents) = &row.agents {
        v.push(choices("agents", agents, |policy| {
            match policy {
                AgentsPolicy::Ask => "ask",
                AgentsPolicy::Allow => "allow",
            }
            .to_owned()
        }));
    }
    if let Some(fast) = row.fast {
        v.push(option("fast", "Fast mode", true, fast.on, None));
    }
    if row.fork {
        v.push(action("fork", "Fork", true));
    }
    v
}

fn chips(chips: &[ChipView]) -> Vec<Value> {
    chips.iter().map(|chip| {
        let id = chip.id;
        let mut actions = vec![action(&format!("remove:{id}"), "Remove", true)];
        let (status, progress) = match &chip.status {
            ChipStatus::Storing => ("Preparing…".to_owned(), None),
            ChipStatus::Uploading { fraction, phase } => (format!("{phase:?}"), *fraction),
            ChipStatus::Ready => (String::new(), None),
            ChipStatus::Failed { message, retry } => {
                if *retry { actions.push(action(&format!("retry:{id}"), "Retry", true)); }
                (message.clone(), None)
            }
        };
        let route = match chip.route {
            ImageRoute::Ask => {
                actions.push(action(&format!("asImage:{id}"), "Send as image", true));
                actions.push(action(&format!("asFile:{id}"), "Send as file", true));
                Some("Choose how to send this image")
            }
            ImageRoute::AsFileCannotSee => Some("This model cannot see images. Sent as a file."),
            _ => None,
        };
        let detail = [(!status.is_empty()).then_some(status.as_str()), route].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        json!({ "id": id.to_string(), "label": chip.name, "detail": detail, "progress": progress, "image": chip.image, "actions": actions })
    }).collect()
}

pub(super) fn view(app: &YaacApp, map: &AttentionMap, insert: Option<&Value>) -> Value {
    let Some(draft) = app
        .open_composer()
        .or_else(|| selected_thread(app).cloned().map(DraftRef::Thread))
    else {
        return json!({ "showEditor": false });
    };
    let state = &app.client.state;
    let draft_key = draft.store_key();
    let text = app.client.draft(&draft).map_or("", |d| d.text.as_str());
    let mut notices = vec![];
    if let Some(a) = state.arrivals.last() {
        let title = state.summary(&a.key).map_or("Thread", |s| s.title.as_str());
        notices.push(
            json!({ "id": "arrival", "text": format!("{title} needs {}", a.kind.word()),
            "actions": [action(&format!("openRequest:{}", a.key.address()), "Open", true)] }),
        );
    }
    if let Some(why) = &app.files.notice {
        notices.push(json!({ "id": "clipboard", "text": why, "actions": [action("dismissNotice", "Dismiss", true)] }));
    }
    if app
        .files
        .large_paste
        .as_ref()
        .is_some_and(|p| p.draft == draft)
    {
        notices.push(json!({ "id": "largePaste", "text": "Attach this large paste as a file?",
            "actions": [action("largeFile", "Attach as file", true), action("largeText", "Paste as text", true), action("largeCancel", "Cancel", true)] }));
    }
    let mut v = json!({
        "key": draft_key, "text": text, "focus": app.ui.focus_composer,
        "sendKey": if app.ui.send_key == SendKey::CtrlEnter { "ctrlEnter" } else { "enter" },
        "attach": action("attach", "Attach files…", !app.files.picking),
        "drop": action("attachPaths", "Attach files", true),
        "paste": action("paste", "Paste", true),
        "insert": insert,
        "fileScope": app.client.composer_cwd(&draft).map(|scope| json!(scope).to_string()),
    });
    if let Some(search) = app.client.composer_files(&draft) {
        v["filePicker"] = match &search.result {
            None => json!({ "query": search.params.query, "hint": "Finding files…" }),
            Some(Err(error)) => json!({ "query": search.params.query, "hint": error }),
            Some(Ok(files)) => {
                let entries: Vec<_> = files.entries.iter().map(|file| json!({
                    "name": file.name, "path": file.path,
                    "reference": yaac_core::compose::file_reference(file.path.as_str()),
                })).collect();
                let hint = if files.truncated { "Keep typing to narrow the search." } else if entries.is_empty() { "No matching files." } else { "" };
                json!({ "query": search.params.query, "entries": entries, "hint": hint })
            }
        };
    }
    match draft.clone() {
        DraftRef::Thread(key) => {
            let compacting = app.client.compacting(&key);
            if app.client.offers_compact(&key) {
                let enabled = !compacting && state.may_act(&key) && !yaac_core::queue::running(state, &key)
                    && !yaac_core::queue::awaiting_user(state, &key)
                    && state.summary(&key).is_some_and(|s| s.hold.is_none());
                v["compact"] = action("compact", "/compact", enabled);
            }
            if compacting {
                notices.push(json!({ "id": "compacting", "text": "Compacting context…" }));
            }
            notices.extend(
                app.client
                    .errors
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.key.as_ref() == Some(&key))
                    .map(|(index, e)| json!({ "id": format!("error:{index}"), "text": e.text })),
            );
            v["runningCommands"] = command_strip(app, &key);
            match header::composer(state, map, &key) {
                ComposerView::Absent { .. } => {
                    v["showEditor"] = json!(false);
                }
                ComposerView::Shown {
                    placeholder,
                    dimmed_hint,
                    stop,
                    chips: chip_views,
                    blocked,
                    running,
                    send_now,
                    editing,
                    ..
                } => {
                    let running = running || compacting;
                    let send_now = send_now.filter(|_| !compacting);
                    let dim = dimmed_hint.is_some() && !running;
                    let edit = state.queue_edits.get(&key);
                    let has = edit.map_or(!text.trim().is_empty() || !chip_views.is_empty(), |e| {
                        e.input().is_some()
                    });
                    let label = if editing {
                        "Save"
                    } else if running {
                        "Queue"
                    } else {
                        "Send"
                    };
                    let ready = !dim && (editing || blocked.is_none());
                    let allowed = has && ready;
                    v["submitReady"] = json!(ready);
                    v["submitWithoutText"] =
                        json!(edit.map_or(!chip_views.is_empty(), |e| !e.keep.is_empty()));
                    v["editable"] = json!(!dim);
                    v["placeholder"] = json!(if editing {
                        "The queued message…"
                    } else {
                        &placeholder
                    });
                    v["hint"] = json!(if editing {
                        Some("Editing a queued message")
                    } else {
                        dimmed_hint.as_deref().or(blocked.as_deref())
                    });
                    v["send"] = action("send", label, allowed);
                    v["sendNow"] = json!(
                        send_now
                            .filter(|_| !editing)
                            .map(|label| action("sendNow", label, allowed))
                    );
                    v["stop"] = json!((stop && !editing).then(|| action("stop", "Stop", true)));
                    if let Some(e) = edit {
                        v["text"] = json!(e.text);
                        v["textKey"] = json!(e.queue_id);
                        v["cancelEdit"] = action("cancelEdit", "Cancel edit", true);
                        v["attach"] = Value::Null;
                        v["drop"] = Value::Null;
                        v["paste"] = Value::Null;
                    } else {
                        v["attachments"] = json!(chips(&chip_views));
                    }
                    let queue = yaac_core::queue::strip(state, &key).into_iter().map(|q| {
                        let mut actions = vec![
                            action(&format!("queueEdit:{}", q.queue_id), "Edit", !q.busy && !q.editing),
                            action(&format!("queueCancel:{}", q.queue_id), "Cancel", !q.busy),
                        ];
                        if let Some(label) = send_now { actions.insert(0, action(&format!("queueNow:{}", q.queue_id), label, !q.busy)); }
                        json!({ "id": q.queue_id, "label": q.preview, "detail": q.extra, "busy": q.busy, "actions": actions })
                    }).collect::<Vec<_>>();
                    v["queueLabel"] = json!("Queued messages");
                    v["queue"] = json!(queue);
                }
            }
            if let ControlRowView::Row(row) = controls::thread_row(state, &key) {
                v["controls"] = json!(controls(&row));
                v["contextPercent"] = json!(row.context_percent);
                v["note"] = json!(row.note);
            }
        }
        DraftRef::New(id) => {
            let Some(d) = state.new_threads.iter().find(|d| d.id == id) else {
                return Value::Null;
            };
            let project = state
                .projects
                .iter()
                .find(|p| Some(&p.id) == d.project.as_ref());
            let label = project.map_or("Choose a project", |p| p.name.as_str());
            let mut options = yaac_core::first_run::project_picker(state, d.project.as_deref())
                .into_iter()
                .map(|machine| {
                    let children = machine
                        .projects
                        .iter()
                        .map(|p| {
                            option(
                                &format!("project:{}", p.id),
                                &p.name,
                                true,
                                p.selected,
                                Some(&p.folder),
                            )
                        })
                        .collect();
                    menu(machine.target.as_str(), &machine.name, true, children)
                })
                .collect::<Vec<_>>();
            options.push(action("createProject", "New project…", true));
            v["project"] = menu("project", label, true, options);
            let support = attachments::draft_image_support(state, d);
            let views = attachments::chips(&d.draft.attachments, &support, Some(&d.target));
            v["attachments"] = json!(chips(&views));
            v["placeholder"] = json!("Write the first message…");
            let blocked = attachments::blocked(&views);
            v["hint"] = if yaac_core::compose::is_compact_command(text) {
                json!("Start the conversation before compacting context.")
            } else {
                json!(blocked)
            };
            let provider_ready = state.target(&d.target).is_some_and(|target| {
                target.providers.iter().any(|p| {
                    p.status == yaac_protocol::ProviderStatus::Ready
                        && d.instance.as_ref().is_none_or(|id| &p.instance == id)
                })
            });
            let ready = d.cwd.is_some() && provider_ready && blocked.is_none();
            v["submitReady"] = json!(ready);
            v["submitWithoutText"] = json!(!views.is_empty());
            v["send"] = action(
                "send",
                "Send",
                ready && (!text.trim().is_empty() || !views.is_empty()),
            );
            if let Some(row) = controls::draft_row_of(state, id) {
                v["controls"] = json!(controls(&row));
                v["note"] = json!(row.note);
            }
        }
    }
    v["notices"] = json!(notices);
    // The skill picker (`/` and `$`), unless a queued message is being edited.
    if v["textKey"].is_null() {
        let comp = match &draft {
            DraftRef::Thread(k) => yaac_core::skills::for_thread(state, k),
            DraftRef::New(id) => state.new_threads.iter().find(|d| d.id == *id).and_then(|d| yaac_core::skills::for_new_thread(state, d)),
        };
        if let Some(c) = comp {
            v["skills"] = app.ui.wpf_skills.view(&draft_key, text, &c, state);
        }
    }
    v
}

fn command_strip(app: &YaacApp, key: &ThreadKey) -> Value {
    let rows = yaac_core::running_commands::rows(&app.client.state, key);
    let log = app.client.command_log.as_ref().filter(|log| &log.view == key);
    if rows.is_empty() && log.is_none() {
        return Value::Null;
    }
    let expanded = !app.ui.commands_collapsed.contains(key);
    let arrow = if expanded { "▾" } else { "▸" };
    let title = format!("{arrow} Running commands · {}", rows.len());
    let mut out = json!({ "title": title, "expanded": expanded });
    if !expanded {
        return out;
    }
    out["rows"] = json!(rows.iter().map(|r| {
        let id = &r.command.id;
        let mut actions = vec![action(&format!("commandLogs:{id}"), "Logs", true)];
        if r.can_stop {
            let waiting = app.client.stopping_command(&r.key());
            actions.push(action(&format!("commandStop:{id}"), if waiting { "Stopping…" } else { "Stop" }, !waiting));
        }
        json!({ "id": id, "label": r.command.command, "detail": r.detail(app.now()), "actions": actions })
    }).collect::<Vec<_>>());
    if let Some(log) = log {
        let listed = rows.iter().any(|r| r.key() == log.command);
        out["log"] = json!({
            "title": log.title.lines().next(),
            "status": log.status(listed),
            "output": log.result.as_ref().and_then(|r| r.output.as_ref()),
            "close": action(&format!("commandLogs:{}", log.command.id), "Close logs", true),
        });
    }
    out
}

/// The skills context and text of the open composer, for the picker's commands.
fn skills_of(app: &YaacApp, draft: &DraftRef) -> Option<(yaac_core::skills::Composer, String)> {
    let state = &app.client.state;
    match draft {
        DraftRef::Thread(k) => {
            if state.queue_edits.contains_key(k) {
                return None;
            }
            Some((yaac_core::skills::for_thread(state, k)?, state.drafts.get(k).map_or_else(String::new, |d| d.text.clone())))
        }
        DraftRef::New(id) => {
            let d = state.new_threads.iter().find(|d| d.id == *id)?;
            Some((yaac_core::skills::for_new_thread(state, d)?, d.draft.text.clone()))
        }
    }
}

/// The caret moved (`composer.caret`) or the text changed (`composer.edit`, which carries the
/// caret): the picker follows, and asks for the list when it needs it.
fn follow_caret(app: &mut YaacApp, draft: &DraftRef, p: &Value) {
    let Some(caret) = p["caret"].as_u64() else { return };
    let key = draft.store_key();
    app.ui.wpf_skills.caret(&key, usize::try_from(caret).unwrap_or(usize::MAX));
    if let Some((comp, text)) = skills_of(app, draft)
        && let Some(scope) = app.ui.wpf_skills.settle(&key, &text, &comp, &app.client.state)
    {
        app.client.skills_list(&scope, false);
    }
}

pub(super) fn command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let Some(draft) = app
        .open_composer()
        .or_else(|| selected_thread(app).cloned().map(DraftRef::Thread))
    else {
        return Ok(());
    };
    if string(p, "key") != draft.store_key() {
        return Ok(());
    }
    let thread = match &draft {
        DraftRef::Thread(k) => Some(k.clone()),
        _ => None,
    };
    let new = match &draft {
        DraftRef::New(id) => Some(*id),
        _ => None,
    };
    let can_write = thread.as_ref().is_none_or(|k| app.client.state.may_act(k));
    let id = string(p, "id");
    if name == "composer.action" {
        if let Some(key) = &thread {
            let command = if id == "toggleCommands" {
                Some(Action::ToggleCommands(key.clone()))
            } else if let Some(id) = id.strip_prefix("commandLogs:") {
                Some(Action::CommandLogs(key.clone(), id.to_owned()))
            } else {
                id.strip_prefix("commandStop:").map(|id| Action::CommandStop(key.clone(), id.to_owned()))
            };
            if let Some(command) = command {
                app.apply(vec![command]);
                return Ok(());
            }
        }
    }
    if !can_write
        && !(name == "composer.action" && (id == "takeOver" || id.starts_with("openRequest:")))
    {
        return Ok(());
    }
    let action = match name {
        "composer.fileQuery" => {
            app.client.search_composer_files(&draft, string(p, "query"));
            None
        }
        "composer.edit" => {
            let text = string(p, "text");
            match &draft {
                DraftRef::Thread(k) => {
                    if let Some(edit) = app.client.state.queue_edits.get_mut(k) {
                        edit.text = text.to_owned();
                        return Ok(());
                    }
                    app.client.state.drafts.entry(k.clone()).or_default().text = text.to_owned();
                }
                DraftRef::New(id) => {
                    if let Some(d) = app
                        .client
                        .state
                        .new_threads
                        .iter_mut()
                        .find(|d| d.id == *id)
                    {
                        d.draft.text = text.to_owned();
                    }
                }
            }
            follow_caret(app, &draft, p);
            Some(Action::DraftEdited(draft))
        }
        "composer.caret" => {
            follow_caret(app, &draft, p);
            None
        }
        "composer.skillDismiss" => {
            if let Some((comp, text)) = skills_of(app, &draft) {
                app.ui.wpf_skills.dismiss(&draft.store_key(), &text, &comp);
            }
            None
        }
        "composer.skillRetry" => {
            if let Some((comp, _)) = skills_of(app, &draft) {
                app.client.skills_list(&comp.scope, true);
            }
            None
        }
        "composer.send" => thread
            .map(Action::Send)
            .or_else(|| new.map(Action::StartDraft)),
        "composer.sendNow" => thread
            .map(Action::SendNow)
            .or_else(|| new.map(Action::StartDraft)),
        "composer.stop" => thread.map(Action::Stop),
        "composer.action" => match id {
            "takeOver" => thread.map(Action::TakeOver),
            "compact" => thread.map(Action::Compact),
            "attach" => Some(Action::AttachPick(draft)),
            "attachPaths" => {
                let paths = p["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                Some(Action::AttachPaths(draft, paths))
            }
            "paste" => {
                use chrono::{Datelike as _, Timelike as _};
                let t = chrono::Local::now();
                app.files.paste(
                    Some(draft),
                    (t.year(), t.month(), t.day(), t.hour(), t.minute()),
                );
                None
            }
            "largeFile" | "largeText" | "largeCancel" => {
                if app
                    .files
                    .large_paste
                    .as_ref()
                    .is_some_and(|p| p.draft == draft)
                {
                    if id == "largeCancel" {
                        app.files.large_paste = None;
                        None
                    } else {
                        Some(Action::LargePaste(id == "largeFile"))
                    }
                } else {
                    None
                }
            }
            "dismissNotice" => {
                app.files.notice = None;
                None
            }
            "cancelEdit" => thread.map(Action::QueueEditEnd),
            "fork" => thread.map(Action::OpenFork),
            "fast" => match (thread, new) {
                (Some(key), _) => {
                    let on = matches!(controls::thread_row(&app.client.state, &key), ControlRowView::Row(r) if r.fast.is_some_and(|f| f.on));
                    Some(Action::SetFast(key, !on))
                }
                (_, Some(id)) => {
                    let on = controls::draft_row_of(&app.client.state, id)
                        .and_then(|r| r.fast)
                        .is_some_and(|f| f.on);
                    Some(Action::DraftFast(id, !on))
                }
                _ => None,
            },
            "createProject" => {
                if let Some(id) = new
                    && let Some(d) = app.client.state.new_threads.iter().find(|d| d.id == id)
                {
                    let folder = d.cwd.as_ref().map_or("~", |p| p.as_str());
                    let mut cp = crate::sidebar::create_project_for(app, &d.target, folder);
                    cp.for_draft = Some(id);
                    app.ui.create_project = Some(cp);
                }
                None
            }
            _ => {
                let Some((kind, value)) = id.split_once(':') else {
                    return Err(format!("Unavailable composer action {id}"));
                };
                match kind {
                    "openRequest" => key(app, value).map(Action::OpenRequest),
                    "project" => new.map(|id| Action::DraftProject(id, value.to_owned())),
                    "effort" => thread
                        .map(|k| Action::SetEffort(k, EffortId::new(value)))
                        .or_else(|| new.map(|id| Action::DraftEffort(id, EffortId::new(value)))),
                    "model" => {
                        let Ok((instance, model)) =
                            serde_json::from_str::<(InstanceId, ModelId)>(value)
                        else {
                            return Ok(());
                        };
                        if let Some(k) = thread {
                            Some(Action::SetModel(k, model))
                        } else {
                            new.map(|id| Action::DraftModel(id, instance, model))
                        }
                    }
                    "mode" => thread
                        .map(ModeTarget::Thread)
                        .or_else(|| new.map(ModeTarget::Draft))
                        .map(|target| Action::ChooseMode(target, ModeId::new(value))),
                    "agents" => thread.map(|k| {
                        Action::SetAgents(
                            k,
                            if value == "allow" {
                                AgentsPolicy::Allow
                            } else {
                                AgentsPolicy::Ask
                            },
                        )
                    }),
                    "queueEdit" => thread.map(|k| Action::QueueEdit(k, value.to_owned())),
                    "queueCancel" => thread.map(|k| Action::QueueCancel(k, value.to_owned())),
                    "queueNow" => thread.map(|k| Action::QueuePromote(k, value.to_owned())),
                    "remove" => value
                        .parse()
                        .ok()
                        .map(|id| Action::RemoveAttachment(draft, id)),
                    "retry" => value
                        .parse()
                        .ok()
                        .map(|id| Action::RetryAttachment(draft, id)),
                    "asImage" => value
                        .parse()
                        .ok()
                        .map(|id| Action::SendAs(draft, id, SendAs::Image)),
                    "asFile" => value
                        .parse()
                        .ok()
                        .map(|id| Action::SendAs(draft, id, SendAs::File)),
                    _ => return Err(format!("Unavailable composer action {id}")),
                }
            }
        },
        _ => return Err(format!("Unknown composer command {name}")),
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::demo::threads;

    #[test]
    fn exposes_file_choices_and_manual_compaction_to_wpf() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = yaac_core::demo::threads::users();
        app.client.open(&key);
        app.client.pump();
        let draft_key = DraftRef::Thread(key.clone()).store_key();
        let payload = json!({ "key": draft_key, "query": "notes" });
        command(&mut app, "composer.fileQuery", &payload).unwrap();
        app.client.pump();
        let model = view(&app, &AttentionMap::compute(&app.client.state), None);
        let cwd = app.client.state.summary(&key).unwrap().cwd.as_str();
        assert_eq!(model["filePicker"]["query"], "notes");
        assert_eq!(model["filePicker"]["entries"][0]["reference"], format!("`{cwd}/docs/design notes.md`"));
        assert_eq!(model["compact"]["id"], "compact");

        command(&mut app, "composer.action", &json!({ "key": draft_key, "id": "compact" })).unwrap();
        assert!(app.client.compacting(&key));
        let model = view(&app, &AttentionMap::compute(&app.client.state), None);
        assert!(model["notices"].as_array().unwrap().iter().any(|notice| notice["text"] == "Compacting context…"));
    }


    fn settle(app: &mut YaacApp) {
        for _ in 0..50 {
            let ticked = app.demo.as_ref().is_some_and(|h| h.tick());
            if !app.pump_demo() && !ticked {
                break;
            }
        }
    }

    fn edit(app: &mut YaacApp, name: &str, text: &str, caret: usize) {
        let key = DraftRef::Thread(threads::fix()).store_key();
        command(app, name, &json!({ "key": key, "text": text, "caret": caret })).unwrap();
    }

    fn skills_view(app: &YaacApp) -> Value {
        let map = AttentionMap::compute(&app.client.state);
        view(app, &map, None)["skills"].clone()
    }

    #[test]
    fn a_leading_slash_lists_skills_a_pick_is_its_insertion_and_a_path_lists_nothing() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        app.apply(vec![Action::Open(threads::fix())]);
        // Nothing before the caret is known.
        assert!(skills_view(&app).is_null());
        edit(&mut app, "composer.edit", "/re", 3);
        // The list is asked for, and meanwhile the picker says it is loading.
        assert_eq!(skills_view(&app)["status"], "loading");
        settle(&mut app);
        let v = skills_view(&app);
        assert_eq!(v["status"], "rows");
        assert_eq!((v["start"].as_u64(), v["end"].as_u64()), (Some(0), Some(3)));
        let labels: Vec<&str> = v["rows"].as_array().unwrap().iter().map(|r| r["label"].as_str().unwrap()).collect();
        // Names that start with the query first, then names that contain it, then descriptions.
        assert_eq!(&labels[..2], ["$review", "$release-notes"], "the demo's Codex names skills inline");
        // What a pick puts in place of `[start, end)`.
        assert_eq!(v["rows"][0]["insert"], "$review ");

        // Esc closes it for this trigger; typing on keeps it closed; clearing the trigger ends that.
        let key = DraftRef::Thread(threads::fix()).store_key();
        command(&mut app, "composer.skillDismiss", &json!({ "key": key })).unwrap();
        assert!(skills_view(&app).is_null());
        edit(&mut app, "composer.edit", "/rev", 4);
        assert!(skills_view(&app).is_null());
        edit(&mut app, "composer.edit", "hello", 5);
        edit(&mut app, "composer.edit", "/re", 3);
        assert_eq!(skills_view(&app)["status"], "rows");

        // A path, and money, list nothing: the text is the user's.
        edit(&mut app, "composer.edit", "/home/me/x", 10);
        assert!(skills_view(&app).is_null());
        edit(&mut app, "composer.edit", "it costs $5", 11);
        assert!(skills_view(&app).is_null());
        // The caret is counted in UTF-16 units: a name after an astral character.
        edit(&mut app, "composer.edit", "/ré", 3);
        assert_eq!(skills_view(&app)["end"], 3);
    }
}
