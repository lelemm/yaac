use super::*;
use yaac_core::attachments::{self, ChipStatus, ChipView, Source};
use yaac_core::controls::{self, Choice, ControlRow, ControlRowView, Speed, ULTRAFAST_WARNING};
use yaac_core::header::{self, ComposerView};
use yaac_core::images::ImageRoute;
use yaac_core::mode_confirm::ModeTarget;
use yaac_core::prefs::SendKey;
use yaac_core::state::{AgentsPolicy, SendAs};
use yaac_protocol::{EffortId, InstanceId, ModeId, ModelId};

use crate::files::ThumbKey;
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
        let mut selector = choices("mode", mode, ToString::to_string);
        selector["warn"] = json!(mode.warn);
        v.push(selector);
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
        if fast.offers_ultrafast {
            let current = fast.current();
            v.push(json!({
                "id": "speed", "label": "Speed", "detail": format!("Speed · {}", current.label()),
                "enabled": true, "tone": current.id(),
                "children": fast.options().map(|speed| {
                    option(&format!("speed:{}", speed.id()), speed.label(), true, speed == current, None)
                }).collect::<Vec<_>>(),
            }));
        } else {
            v.push(option("fast", "Fast mode", true, fast.on, None));
        }
    }
    if row.fork {
        v.push(action("fork", "Fork", true));
    }
    v
}

fn thumbnail_key(draft: &DraftRef, id: u64, source: Option<&Source>) -> ThumbKey {
    match source {
        Some(Source::Path { path }) => ThumbKey::Path(path.clone()),
        _ => ThumbKey::Chip(draft.clone(), id),
    }
}

pub(super) fn upkeep(app: &mut YaacApp) {
    let Some(draft) = app.open_composer() else { return };
    if let Some(value) = app.client.draft(&draft) {
        for attachment in &value.attachments {
            app.files.request_thumb(&thumbnail_key(&draft, attachment.id, attachment.source.as_ref()));
        }
    }
}

/// Bytes of composer text the markdown styler reads; the rest stays plain. WPF needs it to know
/// which styled lines of an older text still hold for the text it is showing (P317).
const MARKDOWN_BUDGET: usize = 64 * 1024;

fn markdown_spans(text: &str) -> Vec<Value> {
    use yaac_core::markstyle::{self, Style};
    let mut offset = 0;
    markstyle::style(text, MARKDOWN_BUDGET).into_iter().filter_map(|span| {
        // WPF indexes UTF-16; the parser gives contiguous UTF-8 byte ranges.
        let style = match span.style {
            Style::Plain if span.range.end == text.len() => return None,
            Style::Plain => None,
            Style::Marker => Some("marker"),
            Style::Heading(_) => Some("heading"),
            Style::Bold => Some("bold"),
            Style::Italic => Some("italic"),
            Style::Code => Some("code"),
            // Kept apart from inline code: a line wholly inside a fence stays code while it is
            // edited, so WPF can keep it styled until this text's own spans arrive.
            Style::CodeBlock => Some("block"),
        };
        let start = offset;
        offset += text[span.range].encode_utf16().count();
        style.map(|style| json!({ "start": start, "end": offset, "style": style }))
    }).collect()
}

fn chips(app: &YaacApp, draft: &DraftRef, chips: &[ChipView]) -> Vec<Value> {
    let current = app.client.draft(draft);
    let attachments = current.map(|value| value.attachments.as_slice()).unwrap_or_default();
    let uploads = chips.iter().map(|chip| {
        let id = chip.id;
        let source = attachments.iter().find(|attachment| attachment.id == id).and_then(|attachment| attachment.source.as_ref());
        let thumbnail = app.files.wire_thumb(&thumbnail_key(draft, id, source));
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
        json!({ "id": id.to_string(), "label": chip.name, "detail": detail, "progress": progress,
            "image": chip.image || thumbnail.is_some(), "thumbnail": thumbnail, "actions": actions })
    });
    let excerpts = current.into_iter().flat_map(|value| {
        value.excerpts.iter().filter(|excerpt| value.text.contains(&excerpt.token))
    }).map(|excerpt| json!({
        "id": excerpt.token, "label": excerpt.label,
        "actions": [action(&format!("removeExcerpt:{}", excerpt.token), "Remove", true)],
    }));
    uploads.chain(excerpts).collect()
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
    if let Some(paste) = app
        .files
        .large_paste
        .as_ref()
        .filter(|p| p.draft == draft)
    {
        let size = crate::composer::human_size(paste.text.len() as u64);
        notices.push(json!({ "id": "largePaste", "text": format!("The pasted text is long ({size})."),
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
                    "reference": app.client.composer_file_reference(&draft, file.path.as_str()),
                })).collect();
                let hint = if files.truncated { "Keep typing to narrow the search." } else if entries.is_empty() { "No matching files." } else { "" };
                json!({ "query": search.params.query, "entries": entries, "hint": hint })
            }
        };
    }
    match draft.clone() {
        DraftRef::Thread(key) => {
            #[cfg(feature = "desktop")]
            if let Some(call) = crate::desktop_call::view(app, &key) {
                v["call"] = call;
            }
            // A voice call with the thread, where its harness offers one (its capability).
            // The desktop build adds its own peer's fields on top (desktop_call::view).
            #[cfg(not(feature = "desktop"))]
            if let Some(call) = crate::voice_call::view(app, &key) {
                v["call"] = call;
            }
            v["runningCommands"] = command_strip(app, &key);
            notices.extend(
                app.client.errors.iter().filter(|e| e.key.as_ref() == Some(&key)).map(|e| {
                    let id = format!("error:{}", e.id);
                    json!({ "id": id, "text": e.text, "error": true, "actions": [action(&id, "Dismiss", true)] })
                }),
            );
            if let Some(notice) = header::composer_connection_notice(state, &key.target) {
                let stopped = state.stopped_distro(&key.target).is_some();
                v["showEditor"] = json!(false);
                notices.push(json!({
                    "id": "targetConnection", "text": notice,
                    "actions": stopped.then(|| action("startWsl", "Start it", true)).into_iter().collect::<Vec<_>>()
                }));
                v["notices"] = json!(notices);
                return v;
            }
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
                    shell,
                    ..
                } => {
                    // `!` mode (P346) only for the draft itself, never a queued message's edit.
                    let shell = shell.filter(|_| !editing);
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
                            .map(|what| now_action("sendNow", what, allowed))
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
                        v["attachments"] = json!(chips(app, &draft, &chip_views));
                        // Dictation: on every composer, through any transcriber (P392).
                        if !dim {
                            v["voice"] = crate::voice::view(app, &draft);
                        }
                        v["shellCapable"] = json!(shell_capable(state, &key));
                        if let Some(mode) = &shell {
                            // `send`, `hint`, Send now and attach stay those of a message: WPF
                            // shows these instead while in the mode, and goes back to them the
                            // moment the user leaves it, before this view is redrawn (P346).
                            let mut view = shell_view(state, &key, mode);
                            view["run"] = action("send", mode.run_label, allowed && mode.ready);
                            view["hint"] = json!(dimmed_hint.as_deref().or(blocked.as_deref()).unwrap_or(&mode.hint));
                            v["shell"] = view;
                        }
                    }
                    let queued = state.transcripts.get(&key).map_or(&[][..], |t| t.queued.as_slice());
                    let queue = yaac_core::queue::strip(state, &key).into_iter().map(|q| {
                        let mut actions = vec![
                            action(&format!("queueEdit:{}", q.queue_id), "Edit", !q.busy && !q.editing),
                            action(&format!("queueCancel:{}", q.queue_id), "Cancel", !q.busy),
                        ];
                        if let Some(what) = send_now { actions.insert(0, now_action(&format!("queueNow:{}", q.queue_id), what, !q.busy)); }
                        let command = queued.iter().find(|e| e.queue_id == q.queue_id).and_then(|e| e.shell_command.as_deref());
                        let mut row = json!({ "id": q.queue_id, "label": command.unwrap_or(&q.preview), "detail": q.extra, "busy": q.busy, "actions": actions });
                        if command.is_some() {
                            row["shell"] = json!(true);
                        }
                        row
                    }).collect::<Vec<_>>();
                    v["queueLabel"] = json!("Queued messages");
                    v["queue"] = json!(queue);
                }
            }
            if let ControlRowView::Row(row) = controls::thread_row(state, &key) {
                v["controls"] = json!(controls(&row));
                if row.fast.is_some_and(|speed| speed.ultrafast_on) {
                    v["ultrafastWarning"] = json!(ULTRAFAST_WARNING);
                }
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
            if d.target.is_some() {
                options.push(action("createProject", "New project…", true));
            } else {
                let targets = state.targets.iter().map(|t| {
                    action(&format!("createProject:{}", t.entry.id), t.entry.name.as_str(), true)
                }).collect();
                options.push(menu("createProject", "New project…", !state.targets.is_empty(), targets));
            }
            v["project"] = menu("project", label, true, options);
            let worktrees = yaac_core::moving::draft_worktrees(state, d);
            if !worktrees.is_empty() {
                let selected = worktrees.iter().find(|(_, selected)| *selected);
                let label = selected.map_or("Worktree: choose".to_owned(), |(w, _)| format!("Worktree: {}", w.label));
                let options = worktrees.iter().map(|(w, selected)| {
                    option(&format!("worktree:{}", w.root), &w.label, true, *selected, Some(&w.root))
                }).collect();
                v["worktree"] = menu("worktree", &label, worktrees.len() > 1 || selected.is_none(), options);
            }
            let support = attachments::draft_image_support(state, d);
            let views = attachments::chips(&d.draft.attachments, &support, d.target.as_ref());
            v["attachments"] = json!(chips(app, &draft, &views));
            // Dictation needs no thread: any transcriber serves a draft too (P392).
            v["voice"] = crate::voice::view(app, &draft);
            v["placeholder"] = json!("Write the first message…");
            let blocked = attachments::blocked(&views);
            v["hint"] = if yaac_core::compose::is_compact_command(text) {
                json!("Start the conversation before compacting context.")
            } else if d.target.is_none() {
                json!("Choose a project to start the thread.")
            } else {
                json!(blocked)
            };
            let provider_ready = d.target.as_ref().and_then(|id| state.target(id)).is_some_and(|target| {
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
                if row.fast.is_some_and(|speed| speed.ultrafast_on) {
                    v["ultrafastWarning"] = json!(ULTRAFAST_WARNING);
                }
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
    if !v["shell"].is_null() {
        // A command is not Markdown: the `!`-mode field is plain monospace.
        v["markdown"] = json!([]);
        v["markdownBudget"] = json!(MARKDOWN_BUDGET);
    } else if let Some(text) = v["text"].as_str() {
        v["markdown"] = if text.contains("\r\n") {
            json!(markdown_spans(&text.replace("\r\n", "\n")))
        } else {
            json!(markdown_spans(text))
        };
        v["markdownBudget"] = json!(MARKDOWN_BUDGET);
    }
    v
}

/// Whether a leading `!` in this thread's composer runs a command (P344): WPF enters `!` mode
/// the moment one is typed at the start, before the controller echoes the edit.
fn shell_capable(state: &yaac_core::state::AppState, key: &ThreadKey) -> bool {
    state.transcripts.get(key).and_then(|t| t.effective.as_ref()).is_some_and(|e| {
        yaac_protocol::has_capability(&e.capabilities, &yaac_protocol::Capability::UserShellCommand)
    })
}

/// The `!`-mode composer (P346): the prompt row, and the thread's earlier commands newest
/// first for ↑. The draft text keeps its `!`; WPF shows the field without it.
fn shell_view(state: &yaac_core::state::AppState, key: &ThreadKey, mode: &header::ShellMode) -> Value {
    let mut history = yaac_core::compose::shell_history(state, key);
    history.reverse();
    json!({ "prompt": mode.prompt, "cwd": mode.cwd, "history": history })
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
    #[cfg(feature = "desktop")]
    if let Some(key) = &thread {
        use crate::desktop_call;
        match name {
            "composer.desktopCallAsk" => { desktop_call::ask(app, key); return Ok(()); }
            "composer.desktopCallConsent" => { desktop_call::consent(app, key, p["accepted"] == true); return Ok(()); }
            "composer.desktopCallMute" => { desktop_call::mute(app, key); return Ok(()); }
            _ => {}
        }
    }
    let id = string(p, "id");
    if name == "composer.action" && let Some(key) = &thread {
        if let Some(id) = id.strip_prefix("error:").and_then(|id| id.parse().ok()) {
            app.apply(vec![Action::DismissError(id, Some(key.clone()))]);
            return Ok(());
        }
        if id == "startWsl" && app.client.state.stopped_distro(&key.target).is_some() {
            app.ui.dismissed.insert(key.target.clone());
            app.apply(vec![Action::Retry(key.target.clone())]);
            return Ok(());
        }
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
    if name == "composer.action" && let Some(id) = id.strip_prefix("previewAttachment:").and_then(|id| id.parse::<u64>().ok()) {
        if let Some(attachment) = app.client.draft(&draft).and_then(|d| d.attachments.iter().find(|a| a.id == id)) {
            let source = match &attachment.source {
                Some(Source::Path { path }) => crate::images::ImageSource::Path(path.clone()),
                _ => crate::images::ImageSource::Draft(draft, id),
            };
            app.images.open(source, attachment.name.clone());
        }
        return Ok(());
    }
    // Stopping or dropping a dictation is always allowed; starting one needs the composer. Any
    // composer dictates, a new-thread draft's too (P392).
    match name {
        "composer.voiceStop" => { crate::voice::stop(app, &draft); return Ok(()); }
        "composer.voiceCancel" => { crate::voice::cancel(app, &draft); return Ok(()); }
        "composer.voiceAudio" => {
            use base64::Engine as _;
            let pcm = base64::engine::general_purpose::STANDARD.decode(string(p, "pcm")).map_err(|_| "composer.voiceAudio: pcm is not base64".to_owned())?;
            crate::voice::audio(app, &draft, &pcm);
            return Ok(());
        }
        "composer.voiceStart" if can_write => { crate::voice::start(app, &draft); return Ok(()); }
        _ => {}
    }
    if let Some(key) = &thread {
        match name {
            // A voice call (docs/plan/voice-call.md): ending is always allowed, starting needs
            // the composer; the renderer made the offer and owns the media.
            "composer.callEnd" => {
                #[cfg(feature = "desktop")]
                crate::desktop_call::end(app, key);
                #[cfg(not(feature = "desktop"))]
                crate::voice_call::end(app, key);
                return Ok(());
            }
            "composer.callStart" if can_write => {
                // A refusal is shown in the frame's `call.error`, not as a command failure.
                let offer = string(p, "sdpOffer").to_owned();
                let _ = crate::voice_call::start(app, key, offer);
                return Ok(());
            }
            _ => {}
        }
    }
    if !can_write
        && !(name == "composer.action" && (matches!(id, "takeOver" | "dismissNotice") || id.starts_with("openRequest:")))
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
                    let draft = app.client.state.drafts.entry(k.clone()).or_default();
                    draft.text = text.to_owned();
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
            "speed:standard" => Some(Action::ChooseSpeed(draft, Speed::Standard)),
            "speed:fast" => Some(Action::ChooseSpeed(draft, Speed::Fast)),
            "speed:ultrafast" => Some(Action::ChooseSpeed(draft, Speed::Ultrafast)),
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
                    && let Some(target) = app.client.draft_target(&DraftRef::New(id))
                {
                    app.create_project_for_draft(id, &target);
                }
                None
            }
            _ => {
                let Some((kind, value)) = id.split_once(':') else {
                    return Err(format!("Unavailable composer action {id}"));
                };
                match kind {
                    "createProject" => {
                        if let Some(id) = new {
                            app.create_project_for_draft(id, &yaac_protocol::TargetId::new(value));
                        }
                        None
                    }
                    "openRequest" => key(app, value).map(Action::OpenRequest),
                    "project" => new.map(|id| Action::DraftProject(id, value.to_owned())),
                    "worktree" => new.map(|id| Action::DraftWorktree(id, value.to_owned())),
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
                    "removeExcerpt" => {
                        if let Some(draft) = thread.as_ref()
                            .filter(|key| !app.client.state.queue_edits.contains_key(*key))
                            .and_then(|key| app.client.state.drafts.get_mut(key))
                        {
                            draft.remove_excerpt(value);
                        }
                        Some(Action::DraftEdited(draft))
                    }
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
    fn dismissing_a_thread_notice_works_while_its_composer_is_read_only() {
        use yaac_core::client::ClientError;
        use yaac_protocol::attach::ConnectionStatus;

        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.client.open(&key);
        app.client.pump();
        let target = app.client.state.targets.iter_mut().find(|target| target.entry.id == key.target).unwrap();
        target.entry.state.update = Some(yaac_protocol::attach::DaemonUpdate {
            from: "old".into(), to: "new".into(), phase: yaac_protocol::attach::DaemonUpdatePhase::Updating,
            forced: false, detail: None,
        });
        target.entry.state.status = ConnectionStatus::Offline;
        app.client.errors.extend([
            ClientError { id: 1, key: None, text: "Window error".into() },
            ClientError { id: 2, key: Some(key.clone()), text: "History error".into() },
        ]);
        let draft = DraftRef::Thread(key).store_key();
        let map = AttentionMap::compute(&app.client.state);
        let before = view(&app, &map, None);
        assert_eq!(before["showEditor"], false);
        assert!(before["notices"].as_array().unwrap().iter().any(|n| n["actions"][0]["id"] == "error:2"));
        for id in ["error:1", "error:2", "error:2"] {
            command(&mut app, "composer.action", &json!({ "key": draft, "id": id })).unwrap();
        }
        assert_eq!(app.client.errors.iter().map(|e| e.id).collect::<Vec<_>>(), [1]);
        super::super::dispatch(&mut app, &ctx, &mut None, "shell.dismissStatus", &json!({ "id": 1 })).unwrap();
        assert!(app.client.errors.is_empty());
    }

    #[test]
    fn composer_markdown_ranges_use_utf16_offsets() {
        assert_eq!(markdown_spans("🦀\n**já**"), vec![
            json!({ "start": 3, "end": 5, "style": "marker" }),
            json!({ "start": 5, "end": 7, "style": "bold" }),
            json!({ "start": 7, "end": 9, "style": "marker" }),
        ]);
    }

    #[test]
    fn composer_markdown_above_the_caret_never_changes_while_the_last_line_grows() {
        // The owner's draft: a pasted block closed by a fence, then their own line.
        let head = "```\n• Brotli · CVE-2026-111497. The gap was for quantum-monthly-report:5549 on TOOLS.\n\
            Debian fix 1.1.0-2+deb13u1 in a **complete** SBOM.\nDebian advisory\n```\n";
        let line = "so, the image is gone, but the gap still here, this should be resolved automatically";
        let head_end = head.encode_utf16().count() as u64;
        let above = |text: &str| -> Vec<Value> {
            markdown_spans(text).into_iter().filter(|span| span["start"].as_u64().unwrap() < head_end).collect()
        };
        let first = above(head);
        assert_eq!(first, vec![json!({ "start": 0, "end": head_end, "style": "block" })]);
        for cut in (0..=line.len()).filter(|&cut| line.is_char_boundary(cut)) {
            assert_eq!(above(&format!("{head}{}", &line[..cut])), first, "after typing {:?}", &line[..cut]);
        }
    }

    #[test]
    fn composer_markdown_keeps_fenced_code_apart_from_inline_code() {
        assert_eq!(markdown_spans("`a`\n```\nb\n```\n"), vec![
            json!({ "start": 0, "end": 1, "style": "marker" }),
            json!({ "start": 1, "end": 2, "style": "code" }),
            json!({ "start": 2, "end": 3, "style": "marker" }),
            json!({ "start": 4, "end": 14, "style": "block" }),
        ]);
    }

    #[test]
    fn ultrafast_requires_a_confirmation_and_updates_the_composer() {
        use yaac_protocol::{Capability, ModelCapability, Support};
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.client.open(&key);
        app.client.pump();
        let summary = app.client.state.summary(&key).unwrap().clone();
        let target = app.client.state.targets.iter_mut().find(|target| target.entry.id == key.target).unwrap();
        let provider = target.providers.iter_mut().find(|provider| provider.instance == summary.instance).unwrap();
        let model = provider.models.iter_mut().find(|model| Some(&model.id) == summary.model.as_ref()).unwrap();
        model.capabilities.extend([Capability::FastMode, Capability::UltrafastMode].map(|capability| ModelCapability {
            capability, support: Support::Supported,
        }));
        let draft = DraftRef::Thread(key.clone()).store_key();
        let select = |app: &mut YaacApp, id| command(app, "composer.action", &json!({ "key": draft, "id": id })).unwrap();
        select(&mut app, "speed:ultrafast");
        assert_eq!(super::super::dialogs::view(&app)["id"], "ultrafast");
        assert!(!app.client.state.thread_ultrafast(&key));
        super::super::dialogs::command(&mut app, "dialog.cancel", &json!({ "id": "ultrafast" })).unwrap();
        assert!(!app.client.state.thread_ultrafast(&key));
        select(&mut app, "speed:ultrafast");
        app.apply(vec![Action::ConfirmUltrafast]);
        let view = view(&app, &AttentionMap::compute(&app.client.state), None);
        assert_eq!(view["ultrafastWarning"], ULTRAFAST_WARNING);
        let speed = view["controls"].as_array().unwrap().iter().find(|item| item["id"] == "speed").unwrap();
        assert_eq!(speed["tone"], "ultrafast");
        select(&mut app, "speed:standard");
        assert!(!app.client.state.thread_ultrafast(&key));
    }

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

    fn composer_view(app: &YaacApp) -> Value {
        view(app, &AttentionMap::compute(&app.client.state), None)
    }

    fn voice_command(app: &mut YaacApp, key: &ThreadKey, name: &str, extra: Value) {
        let mut payload = json!({ "key": DraftRef::Thread(key.clone()).store_key() });
        if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        command(app, name, &payload).unwrap();
    }

    /// Half a second of silence at the demo's 16 kHz mono, as `composer.voiceAudio` sends it.
    fn half_second() -> Value {
        use base64::Engine as _;
        json!({ "pcm": base64::engine::general_purpose::STANDARD.encode(vec![0u8; 16_000]) })
    }

    fn voice_pass(app: &mut YaacApp) {
        settle(app);
        crate::voice::upkeep(app);
    }

    fn call_pass(app: &mut YaacApp) {
        settle(app);
        crate::voice_call::upkeep(app);
    }

    /// The `call` object as this build's controller makes it (the desktop build adds its own
    /// peer's fields on top, in `desktop_call`).
    fn call_view(app: &YaacApp, key: &ThreadKey) -> Value {
        crate::voice_call::view(app, key).unwrap_or(Value::Null)
    }

    #[test]
    fn a_call_is_offered_from_the_capability_alone() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        let call = call_view(&app, &key);
        assert_eq!(call["state"], "idle");
        assert_eq!(call["button"]["id"], "composer.callStart");
        assert_eq!(call["button"]["enabled"], true);
        assert!(call["button"]["detail"].as_str().unwrap().contains("can start work here"), "the consent note");
        #[cfg(not(feature = "desktop"))]
        assert_eq!(composer_view(&app)["call"], call);
        let effective = app.client.state.transcripts.get_mut(&key).and_then(|t| t.effective.as_mut()).unwrap();
        effective.capabilities.retain(|c| *c != yaac_protocol::Capability::VoiceCall);
        assert!(call_view(&app, &key).is_null());
        assert!(composer_view(&app)["call"].is_null());
    }

    #[test]
    fn a_call_hands_the_offer_on_shows_the_answer_and_the_words_and_hangs_up() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        let queued = app.client.state.transcripts.get(&key).map_or(0, |t| t.queued.len());
        voice_command(&mut app, &key, "composer.callStart", json!({ "sdpOffer": "v=0\r\no=offer\r\n" }));
        assert_eq!(call_view(&app, &key)["state"], "starting");
        let sent = app.demo.as_ref().unwrap().requests();
        let (_, _, params) = sent.iter().rev().find(|(_, m, _)| m == "hub/voiceCall/start").expect("start sent");
        assert_eq!(params["sdpOffer"], "v=0\r\no=offer\r\n");
        call_pass(&mut app);
        let call = call_view(&app, &key);
        assert_eq!(call["state"], "active");
        assert!(call["callId"].as_str().unwrap().starts_with("demo-call-"));
        assert_eq!(call["answer"], "v=0\r\ns=yaac-demo\r\n");
        assert_eq!(call["button"]["id"], "composer.callEnd");
        assert_eq!(call["transcript"], json!([
            { "role": "user", "text": "Can you run the tests?", "final": true },
            { "role": "assistant", "text": "Sure, starting them now.", "final": true },
        ]));
        // One call per window: another thread cannot start one now.
        if let Some(other) = app.client.state.transcripts.keys().find(|k| **k != key && crate::voice_call::offered(&app, k)).cloned() {
            assert_eq!(call_view(&app, &other)["button"]["enabled"], false);
            assert!(crate::voice_call::start(&mut app, &other, "v=0".into()).is_err());
        }
        voice_command(&mut app, &key, "composer.callEnd", json!({}));
        assert_eq!(call_view(&app, &key)["state"], "ending");
        call_pass(&mut app);
        let call = call_view(&app, &key);
        assert_eq!(call["state"], "idle");
        assert!(call["answer"].is_null(), "the answer goes with the call");
        assert_eq!(call["transcript"].as_array().unwrap().len(), 2, "what was said stays until the next call");
        // The window itself sent nothing to the conversation.
        assert_eq!(app.client.state.transcripts.get(&key).map_or(0, |t| t.queued.len()), queued);
        assert!(!app.demo.as_ref().unwrap().requests().iter().any(|(_, m, _)| m == "turn/start"));
    }

    #[test]
    fn a_refused_call_says_why_in_the_frame() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        voice_command(&mut app, &key, "composer.callStart", json!({ "sdpOffer": "" }));
        let call = call_view(&app, &key);
        assert_eq!(call["state"], "error");
        assert!(call["error"].as_str().unwrap().contains("SDP offer"));
        assert_eq!(call["button"]["id"], "composer.callStart");
    }

    #[test]
    fn a_codex_thread_dictates_through_claude_and_without_any_transcriber_says_why() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        // The demo's "Fix frame decoder" is a Codex thread; the demo's Codex cannot transcribe,
        // its Claude can (P392: dictation is not bound to the thread's harness).
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        let summary = app.client.state.summary(&key).unwrap();
        let own = app.client.state.target(&key.target).unwrap().provider(&summary.instance).unwrap();
        assert!(!own.capabilities.contains(&yaac_protocol::Capability::Transcription), "the thread's own instance does not transcribe");
        let target_name = app.client.state.target(&key.target).unwrap().entry.name.to_string();
        let voice = composer_view(&app)["voice"].clone();
        assert_eq!(voice["state"], "idle");
        assert_eq!(voice["format"], json!({ "format": "pcm16le", "sampleRate": 16_000, "channels": 1, "maxSeconds": 120 }));
        assert_eq!(voice["transcriber"], format!("Claude on {target_name}"));
        assert_eq!(voice["button"]["id"], "composer.voiceStart");
        assert_eq!(voice["button"]["enabled"], true);
        assert_eq!(voice["capture"], if crate::voice_capture::AVAILABLE { "controller" } else { "renderer" });
        // No instance anywhere transcribes: the mic stays, disabled, with the nearest reason
        // (the thread's own instance first).
        for t in &mut app.client.state.targets {
            for p in &mut t.providers {
                p.capabilities.retain(|c| *c != yaac_protocol::Capability::Transcription);
            }
        }
        let voice = composer_view(&app)["voice"].clone();
        assert_eq!(voice["button"]["enabled"], false);
        assert!(voice["format"].is_null());
        let why = voice["unavailable"].as_str().unwrap();
        assert!(why.starts_with("No transcriber available."), "{why}");
        assert!(why.contains("Codex transcribes only with an OpenAI API key sign-in."), "{why}");
        assert_eq!(voice["button"]["detail"], why);
        crate::voice::start(&mut app, &DraftRef::Thread(key.clone()));
        assert!(app.client.dictation().is_none(), "nothing starts without a transcriber");
    }

    #[test]
    fn a_new_thread_draft_has_the_mic_and_gets_the_words() {
        if crate::voice_capture::AVAILABLE {
            return; // This build records in the controller; the renderer's audio is ignored.
        }
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        settle(&mut app);
        let id = app.client.blank_thread();
        let draft = DraftRef::New(id);
        let view = composer_view(&app);
        assert_eq!(view["key"], draft.store_key());
        assert_eq!(view["voice"]["button"]["enabled"], true, "{}", view["voice"]);
        assert!(view["voice"]["transcriber"].as_str().unwrap().starts_with("Claude on "));
        let payload = |extra: Value| {
            let mut p = json!({ "key": draft.store_key() });
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            p
        };
        command(&mut app, "composer.voiceStart", &payload(json!({}))).unwrap();
        voice_pass(&mut app);
        assert_eq!(composer_view(&app)["voice"]["state"], "listening");
        assert!(composer_view(&app)["voice"]["status"].as_str().unwrap().starts_with("Dictating with Claude on "));
        command(&mut app, "composer.voiceAudio", &payload(half_second())).unwrap();
        command(&mut app, "composer.voiceAudio", &payload(half_second())).unwrap();
        command(&mut app, "composer.voiceStop", &payload(json!({}))).unwrap();
        voice_pass(&mut app);
        let mut insert = None;
        super::super::voice_insert(&mut app, &mut insert);
        let (to, value) = insert.unwrap();
        assert_eq!(to, draft.store_key());
        assert_eq!(value["text"], "Add a retry with");
        assert!(!app.demo.as_ref().unwrap().requests().iter().any(|(_, m, _)| m == "thread/start" || m == "turn/start"), "no thread was started");
    }

    #[test]
    fn dictated_words_show_live_then_land_at_the_caret_and_are_never_sent() {
        if crate::voice_capture::AVAILABLE {
            return; // This build records in the controller; the renderer's audio is ignored.
        }
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        let store = DraftRef::Thread(key.clone()).store_key();
        command(&mut app, "composer.edit", &json!({ "key": store, "text": "Please", "caret": 6 })).unwrap();
        let queued = app.client.state.transcripts.get(&key).map_or(0, |t| t.queued.len());

        voice_command(&mut app, &key, "composer.voiceStart", json!({}));
        assert_eq!(composer_view(&app)["voice"]["state"], "starting");
        voice_pass(&mut app);
        assert_eq!(composer_view(&app)["voice"]["state"], "listening");
        voice_command(&mut app, &key, "composer.voiceAudio", half_second());
        voice_pass(&mut app);
        // The interim words, shown while listening; the draft is untouched.
        let voice = composer_view(&app)["voice"].clone();
        assert_eq!(voice["partial"], "Add a");
        assert_eq!(voice["status"], "Add a");
        assert_eq!(voice["button"]["id"], "composer.voiceStop");
        assert_eq!(app.client.state.drafts[&key].text, "Please");

        voice_command(&mut app, &key, "composer.voiceAudio", half_second());
        voice_command(&mut app, &key, "composer.voiceStop", json!({}));
        assert_eq!(composer_view(&app)["voice"]["state"], "stopping");
        voice_pass(&mut app);
        assert_eq!(composer_view(&app)["voice"]["state"], "idle");
        // The final words go to the renderer once, at its caret, after a space.
        let mut insert = None;
        super::super::voice_insert(&mut app, &mut insert);
        let (to, value) = insert.clone().unwrap();
        assert_eq!(to, store);
        assert_eq!(value["text"], " Add a retry with");
        super::super::voice_insert(&mut app, &mut insert);
        assert_eq!(insert.unwrap().1["id"], value["id"], "handed out once");
        // Nothing was sent: no message queued or started.
        assert_eq!(app.client.state.transcripts.get(&key).map_or(0, |t| t.queued.len()), queued);
        assert!(!app.demo.as_ref().unwrap().requests().iter().any(|(_, m, _)| m == "turn/start"));
    }

    #[test]
    fn one_dictation_per_window_and_cancel_drops_the_words() {
        if crate::voice_capture::AVAILABLE {
            return;
        }
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        voice_command(&mut app, &key, "composer.voiceStart", json!({}));
        voice_pass(&mut app);
        // Another thread offers the mic, but not while this one listens.
        let other = app.client.state.transcripts.keys().find(|k| **k != key).cloned().map(DraftRef::Thread);
        if let Some(other) = &other {
            let v = crate::voice::view(&app, other);
            assert_eq!((v["state"].as_str(), v["button"]["enabled"].as_bool()), (Some("idle"), Some(false)));
            crate::voice::start(&mut app, other);
            assert_eq!(app.client.dictation().map(|d| d.key.clone()), Some(DraftRef::Thread(key.clone())));
        }
        voice_command(&mut app, &key, "composer.voiceAudio", half_second());
        voice_pass(&mut app);
        voice_command(&mut app, &key, "composer.voiceCancel", json!({}));
        voice_pass(&mut app);
        assert_eq!(composer_view(&app)["voice"]["state"], "idle");
        let mut insert = None;
        super::super::voice_insert(&mut app, &mut insert);
        assert!(insert.is_none(), "a cancelled dictation leaves nothing");
    }

    #[test]
    fn leaving_the_thread_while_listening_drops_the_dictation() {
        if crate::voice_capture::AVAILABLE {
            return;
        }
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        voice_command(&mut app, &key, "composer.voiceStart", json!({}));
        voice_pass(&mut app);
        voice_command(&mut app, &key, "composer.voiceAudio", half_second());
        voice_pass(&mut app);
        assert!(app.client.dictation().is_some());
        app.ui.settings = true;
        voice_pass(&mut app);
        assert!(app.client.dictation().is_none());
        app.ui.settings = false;
        voice_pass(&mut app);
        let mut insert = None;
        super::super::voice_insert(&mut app, &mut insert);
        assert!(insert.is_none());
        assert!(app.client.state.drafts.get(&key).is_none_or(|d| d.text.is_empty()));
    }

    #[test]
    fn words_for_a_composer_that_is_not_open_go_to_the_end_of_its_draft() {
        if crate::voice_capture::AVAILABLE {
            return;
        }
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let key = threads::fix();
        app.apply(vec![Action::Open(key.clone())]);
        settle(&mut app);
        app.client.state.drafts.entry(key.clone()).or_default().text = "Note:".into();
        voice_command(&mut app, &key, "composer.voiceStart", json!({}));
        voice_pass(&mut app);
        voice_command(&mut app, &key, "composer.voiceAudio", half_second());
        voice_command(&mut app, &key, "composer.voiceStop", json!({}));
        // The user went to Settings before the words came back.
        app.ui.settings = true;
        voice_pass(&mut app);
        let mut insert = None;
        super::super::voice_insert(&mut app, &mut insert);
        assert!(insert.is_none());
        assert_eq!(app.client.state.drafts[&key].text, "Note: Add a");
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

/// The Send now button: always labelled "Send now"; its tooltip says whether it steers the
/// running turn or interrupts it.
fn now_action(id: &str, what: &str, enabled: bool) -> Value {
    let mut v = action(id, yaac_core::queue::SEND_NOW, enabled);
    v["detail"] = json!(what);
    v
}
