use super::*;
use yaac_core::pr::{AgentDraft, CreatePr, Prepare};
use yaac_core::{fork, mode_confirm, mute};
use yaac_protocol::{FsEntryKind, InstanceId, ModelId};

fn form(id: &str, title: &str, body: &str, submit: &str, fields: Vec<Value>) -> Value {
    json!({ "id": id, "title": title, "body": body, "submit": submit, "fields": Value::Array(fields) })
}
fn field(id: &str, label: &str, value: &str) -> Value {
    json!({ "id": id, "label": label, "value": value, "type": "text" })
}
fn choice(id: &str, label: &str, value: &str, options: Vec<Value>) -> Value {
    json!({ "id": id, "label": label, "value": value, "type": "choice", "options": Value::Array(options) })
}
fn option(id: &str, label: &str) -> Value {
    json!({ "id": id, "label": label })
}

fn live_field(id: &str, label: &str, value: &str) -> Value {
    let mut value = field(id, label, value);
    value["live"] = json!(true);
    value
}

pub(super) fn upkeep(app: &mut YaacApp) {
    let state = &app.client.state;
    if state.mode_confirm.as_ref().is_some_and(|d| !mode_confirm::still_relevant(state, d)) {
        app.apply(vec![Action::CancelMode]);
    }
}

pub(super) fn view(app: &YaacApp) -> Value {
    let u = &app.ui;
    let state = &app.client.state;
    if let Some(notice) = &app.updater.notice {
        form("updateNotice", "The update was not installed", notice, "OK", vec![])
    } else if u.quit_confirm {
        form("quit", "Quit YAAC?", "The window closes. The hub keeps running, and so do the agents’ turns. Open YAAC again to see them.", "Quit", vec![])
    } else if let Some(d) = &state.create_pr {
        pull_request_view(app, d)
    } else if let Some((_, title)) = &u.rename {
        form(
            "rename",
            "Rename thread",
            "",
            "Rename",
            vec![field("title", "Name", title)],
        )
    } else if let Some(d) = &u.archive {
        form("archive", &d.title, d.body, "Archive", vec![])
    } else if let Some(d) = &u.delete {
        let fields = d.harness_label.as_ref().map(|label| json!({
            "id": "also", "label": format!("Also delete in {label}"), "type": "check", "value": d.also_in_harness,
        })).into_iter().collect();
        form("delete", &d.title, d.body(), "Delete", fields)
    } else if let Some(d) = &u.bulk_archive {
        let body = [Some(d.body), d.skipped.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
        form(
            "bulkArchive",
            &d.title,
            &body,
            if d.plan.eligible.is_empty() {
                ""
            } else {
                "Archive"
            },
            vec![],
        )
    } else if let Some(d) = &u.mute {
        let options = [
            ("forever", "Until I unmute it"),
            ("for", "For a duration"),
            ("until", "Until a date and time"),
        ];
        let selected = match d.choice {
            mute::MuteChoice::Forever => "forever",
            mute::MuteChoice::For => "for",
            mute::MuteChoice::Until => "until",
        };
        let mut fields = vec![choice(
            "when",
            "Mute",
            selected,
            options
                .into_iter()
                .map(|(id, label)| option(id, label))
                .collect(),
        )];
        if selected == "for" {
            fields.push(live_field("amount", "Amount", &d.amount));
            fields.push(choice(
                "unit",
                "Unit",
                d.unit.label(),
                mute::DurationUnit::ALL
                    .into_iter()
                    .map(|unit| option(unit.label(), unit.label()))
                    .collect(),
            ));
        } else if selected == "until" {
            let mut date = live_field("date", "Date", &d.date_text);
            date["type"] = json!("date");
            date["minDate"] = json!(yaac_core::time::date_in(app.now(), app.zone));
            fields.push(date);
            fields.push(live_field("time", "Local time", &d.time));
        }
        let mut form = form("mute", &d.title, "The agent keeps working. If it gets blocked it still comes to the top of its group.", "Mute", fields);
        if d.choice == mute::MuteChoice::For {
            form["hint"] = json!(d.for_preview(app.now(), app.zone));
        }
        if let mute::MuteOutcome::Unavailable(why) = d.outcome_in(app.now(), app.zone) {
            form["submitBlocked"] = json!(why);
        }
        form
    } else if let Some(d) = &u.move_dialog {
        let mut fields = vec![];
        if d.blocked.is_none() {
            fields.push(choice(
                "project",
                "Project",
                d.project.as_deref().unwrap_or_default(),
                d.projects(state)
                    .iter()
                    .map(|p| option(&p.id, &p.name))
                    .collect(),
            ));
            let trees = d.worktrees(state);
            if !trees.is_empty() {
                fields.push(choice(
                    "worktree",
                    "Worktree",
                    d.worktree.as_deref().unwrap_or_default(),
                    std::iter::once(option("", "Project folder"))
                        .chain(
                            trees
                                .iter()
                                .filter(|tree| !tree.own)
                                .map(|tree| option(&tree.root, &tree.label)),
                        )
                        .collect(),
                ));
            }
        }
        let body = [Some(d.sentence(state)), d.skipped.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
        form(
            "move",
            &d.title,
            &body,
            if d.can_move(state) { "Move" } else { "" },
            fields,
        )
    } else if let Some(d) = &u.create_project {
        project_view(app, d)
    } else if let Some(d) = &u.fork
        && let Some(v) = fork::view(state, &d.key, &d.choice)
    {
        let mut fields = vec![choice(
            "provider",
            "Continue with",
            d.choice.instance.as_str(),
            v.destinations
                .iter()
                .map(|dest| json!({ "id": dest.instance, "label": dest.name,
                    "disabled": dest.reason.is_some(), "detail": dest.reason }))
                .collect(),
        )];
        if let Some(dest) = v.destinations.iter().find(|dest| dest.instance == d.choice.instance)
            && !dest.models.is_empty() {
            fields.push(choice("model", "Model", d.choice.model.as_ref().map_or("", ModelId::as_str),
                std::iter::once(option("", "Provider default"))
                    .chain(dest.models.iter().map(|(id, name, default)| {
                        let label = if *default { format!("{name} (default)") } else { name.clone() };
                        option(id.as_str(), &label)
                    })).collect()));
        }
        if v.handoff {
            fields.push(json!({ "id": "message", "label": "First message to the new agent", "type": "multiline", "value": d.message }));
        }
        let body = [
            Some(v.at),
            v.instance_line,
            Some(v.result_text),
            v.reason,
            Some(format!("Carries · {}", v.carried)),
            v.not_carried.map(|note| format!("Leaves behind · {note}")),
            v.size,
            v.notice,
            v.unavailable.clone(),
            v.preview.or_else(|| v.preview_loading.then_some("Loading preview…".to_owned())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
        form(
            "fork",
            &format!("Fork {}", v.from),
            &body,
            if v.confirm_waits || v.unavailable.is_some() {
                ""
            } else {
                &v.confirm
            },
            fields,
        )
    } else if let Some(d) = &state.mode_confirm {
        let v = mode_confirm::view(state, d);
        let body = [vec![format!("{}{}{}", v.body.0, v.mode_name, v.body.1)], v.lines, vec![v.footer]]
            .concat()
            .join("\n");
        form("mode", &v.title, &body, &v.confirm_label, vec![])
    } else if u.ultrafast_confirm.as_ref().and_then(|draft| app.composer_speed(draft)).is_some_and(|control| control.offers_ultrafast) {
        use yaac_core::controls::{ULTRAFAST_CONFIRM_BODY, ULTRAFAST_CONFIRM_TITLE};
        form("ultrafast", ULTRAFAST_CONFIRM_TITLE, ULTRAFAST_CONFIRM_BODY, "Enable Ultrafast", vec![])
    } else if let Some(d) = &u.update_now {
        form("update", &d.title, &d.body, "Update now", vec![])
    } else {
        Value::Null
    }
}

fn project_view(app: &YaacApp, d: &crate::app::CreateProject) -> Value {
    use yaac_core::state::FsListingState;
    let state = &app.client.state;
    let mut form = form("project", "Create project",
        &format!("On {}\nUse a parent folder to include the whole project.", state.target_name(&d.target)),
        "Create project", vec![live_field("folder", "Folder", &d.folder), live_field("name", "Name", &d.name)]);
    form["instance"] = json!(format!("project:{}:{}", d.target, d.for_draft.unwrap_or_default()));
    form["fieldRevision"] = json!(d.field_revision);
    let count = crate::shell::project_thread_count(app, &d.target, d.folder.trim());
    form["hint"] = json!(format!("{} in this folder will move into the project.", yaac_core::bulk::threads(count)));
    if d.folder.trim().is_empty() {
        form["submitBlocked"] = json!("Enter the project folder.");
    } else if d.name.trim().is_empty() {
        form["submitBlocked"] = json!("Enter a project name.");
    }
    let browse = if d.browse.is_some() { "Close browser" } else { "Browse…" };
    let mut actions = vec![sidebar_view::action("browse", browse, true)];
    if let Some(root) = &d.repo_root && root != &d.folder {
        actions.push(sidebar_view::action("repoRoot", &format!("Use repository root · {root}"), true));
    }
    form["auxiliaryActions"] = json!(actions);
    if let Some(path) = &d.browse {
        let listing = match state.fs_listings.get(&(d.target.clone(), path.clone())) {
            None | Some(FsListingState::Loading) => json!({ "text": "Reading the folder…" }),
            Some(FsListingState::Failed(error)) => json!({ "text": error }),
            Some(FsListingState::Ready(list)) => {
                let mut entries = vec![sidebar_view::action("useFolder", "Use this folder", true)];
                let folder = |p: &str, label: &str| {
                    json!({ "id": "folder", "label": label, "path": p, "enabled": true })
                };
                entries.extend(list.parent.as_ref().map(|p| folder(p.as_str(), "Parent folder")));
                entries.extend(list.entries.iter().filter(|e| e.kind == FsEntryKind::Directory)
                    .map(|e| folder(e.path.as_str(), &e.name)));
                json!({ "text": list.path, "entries": entries,
                    "hint": list.truncated.then_some("Only the first entries are shown.") })
            }
        };
        form["folders"] = listing;
    }
    form
}

fn pull_request_view(app: &YaacApp, d: &CreatePr) -> Value {
    let state = &app.client.state;
    let ready = d.ready();
    let repo = ready.and_then(|r| r.access.repo.as_ref()).map(|r| r.slug.as_str());
    let branch = ready.and_then(|r| r.branch.as_deref());
    let source = match (branch, repo) {
        (Some(branch), Some(repo)) => format!("{branch} · {repo}"),
        (Some(branch), None) => branch.to_owned(),
        (None, Some(repo)) => repo.to_owned(),
        (None, None) => state.summary(&d.key).map(|s| s.title.clone()).unwrap_or_default(),
    };
    let default = ready.and_then(|r| r.default_branch.as_deref());
    let branches: Vec<_> = d.branches().iter().map(|branch| {
        if Some(branch.as_str()) == default {
            option(branch, &format!("{branch}  (default)"))
        } else {
            option(branch, branch)
        }
    }).collect();
    let writing = matches!(d.agent, Some(AgentDraft::Sending | AgentDraft::Writing(_)));
    let (agent_status, agent_failed) = match &d.agent {
        Some(AgentDraft::Failed(error)) => (error.as_str(), true),
        _ if writing => ("The agent is writing…", false),
        _ => ("", false),
    };
    let error = match &d.prepare {
        Prepare::Failed(error) => Some(error),
        _ => d.error.as_ref(),
    };
    let ask_blocked = match &d.prepare {
        _ if d.creating => Some("Creating…".into()),
        Prepare::Failed(error) => Some(error.clone()),
        _ => d.ask_blocked(state),
    };
    json!({
        "id": "pullRequest",
        "instance": format!("pullRequest:{}", d.key.address()),
        "title": "Create pull request",
        "body": source,
        "submit": if d.creating { "Creating…" } else { "Create" },
        "submitBlocked": d.create_blocked(),
        "askBlocked": ask_blocked,
        "creating": d.creating,
        "writing": writing,
        "agentStatus": agent_status,
        "agentFailed": agent_failed,
        "loading": matches!(d.prepare, Prepare::Loading),
        "error": error,
        "notice": d.push_notice(),
        "warning": d.uncommitted_warning(),
        "branches": branches,
        "base": d.base,
        "prTitle": d.title,
        "description": d.body,
        "draft": d.draft,
        "editRevision": app.ui.wpf_pr_edit,
    })
}

fn update_mute(d: &mut mute::MuteDialog, v: &Value) {
    if let Some(when) = v["when"].as_str() {
        d.choice = match when {
            "for" => mute::MuteChoice::For,
            "until" => mute::MuteChoice::Until,
            _ => mute::MuteChoice::Forever,
        };
    }
    if let Some(amount) = v["amount"].as_str() {
        d.amount = amount.to_owned();
    }
    if let Some(unit) = v["unit"].as_str().and_then(|label| {
        mute::DurationUnit::ALL.into_iter().find(|u| u.label() == label)
    }) {
        d.unit = unit;
    }
    if let Some(date) = v["date"].as_str() {
        d.date_text = date.to_owned();
        d.date_typed();
    }
    if let Some(time) = v["time"].as_str() {
        d.time = time.to_owned();
    }
}

fn update_project(d: &mut crate::app::CreateProject, v: &Value) {
    if let Some(folder) = v["folder"].as_str() {
        d.folder = folder.to_owned();
    }
    if let Some(name) = v["name"].as_str() {
        d.name = name.to_owned();
    }
}

fn project_action(app: &mut YaacApp, p: &Value) -> Option<Action> {
    use yaac_core::state::FsListingState;
    let d = app.ui.create_project.as_mut()?;
    update_project(d, &p["values"]);
    let state = &app.client.state;
    let action = string(p, "action");
    let path = match action {
        "browse" => {
            if d.browse.take().is_some() { return None; }
            let folder = d.folder.trim();
            let path = folder.trim_end_matches('/');
            if path.is_empty() && folder.starts_with('/') { "/".to_owned() } else { path.to_owned() }
        }
        "repoRoot" => {
            d.folder = d.repo_root.clone()?;
            d.browse = None;
            d.field_revision += 1;
            return None;
        }
        "folder" | "useFolder" => {
            let current = d.browse.as_ref()?;
            let Some(FsListingState::Ready(list)) = state.fs_listings.get(&(d.target.clone(), current.clone())) else { return None };
            if action == "useFolder" {
                d.folder = format!("{}/", list.path.as_str().trim_end_matches('/'));
                d.name = yaac_core::projects::suggested_name(list.path.as_str());
                d.browse = None;
                d.field_revision += 1;
                return None;
            }
            let selected = string(p, "path");
            if !list.parent.as_ref().is_some_and(|p| p.as_str() == selected)
                && !list.entries.iter().any(|e| e.kind == FsEntryKind::Directory && e.path.as_str() == selected) {
                return None;
            }
            selected.to_owned()
        }
        _ => return None,
    };
    d.browse = Some(path.clone());
    Some(Action::FsList(d.target.clone(), path))
}

pub(super) fn command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let id = string(p, "id");
    if id == "pullRequest" {
        return pull_request_command(app, name, p);
    }
    let current = view(app);
    if current["id"] != id || current["instance"].as_str().is_some_and(|instance| p["instance"] != instance) {
        return Ok(());
    }
    let v = &p["values"];
    if name == "dialog.cancel" {
        let u = &mut app.ui;
        match id {
            "rename" => u.rename = None,
            "archive" => u.archive = None,
            "delete" => u.delete = None,
            "bulkArchive" => u.bulk_archive = None,
            "mute" => u.mute = None,
            "move" => u.move_dialog = None,
            "project" => u.create_project = None,
            "fork" => u.fork = None,
            "update" => u.update_now = None,
            "mode" => app.apply(vec![Action::CancelMode]),
            "ultrafast" => u.ultrafast_confirm = None,
            "quit" => u.quit_confirm = false,
            "updateNotice" => app.updater.notice = None,
            _ => {}
        }
        return Ok(());
    }
    if name == "dialog.action" && id == "project" {
        if let Some(action) = project_action(app, p) {
            app.apply(vec![action]);
        }
        return Ok(());
    }
    if name == "dialog.change" {
        match id {
            "mute" => {
                if let Some(d) = &mut app.ui.mute {
                    update_mute(d, v);
                }
            }
            "project" => {
                if let Some(d) = &mut app.ui.create_project {
                    update_project(d, v);
                }
            }
            "move" => {
                if let Some(d) = &mut app.ui.move_dialog {
                    if let Some(project) = v["project"].as_str() {
                        d.set_project(project);
                    }
                    if let Some(worktree) = v["worktree"].as_str() {
                        d.worktree = (!worktree.is_empty()).then(|| worktree.to_owned());
                    }
                }
            }
            "fork" => {
                if let Some(d) = &mut app.ui.fork
                    && let Some(fields) = fork::view(&app.client.state, &d.key, &d.choice) {
                    if let Some(provider) = v["provider"].as_str()
                        && fields.destinations.iter().any(|dest| dest.instance.as_str() == provider && dest.reason.is_none()) {
                        let instance = InstanceId::from(provider);
                        d.choice = fork::ForkChoice { instance: instance.clone(), model: None };
                        app.client.fork_preview(&d.key, &instance);
                    } else if let Some(model) = v["model"].as_str()
                        && fields.destinations.iter().any(|dest| dest.instance == d.choice.instance
                            && dest.reason.is_none() && (model.is_empty() || dest.models.iter().any(|(id, _, _)| id.as_str() == model))) {
                        d.choice.model = (!model.is_empty()).then(|| ModelId::from(model));
                    }
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if name != "dialog.submit" {
        return Ok(());
    }
    let now = app.now();
    let action = match id {
        "rename" => {
            let value = string(v, "title").trim();
            if value.is_empty() {
                return Err("Enter a thread name.".into());
            }
            app.ui
                .rename
                .take()
                .map(|(key, _)| Action::Rename(key, value.to_owned()))
        }
        "archive" => app
            .ui
            .archive
            .as_ref()
            .map(|d| Action::Archive(d.key.clone())),
        "delete" => app.ui.delete.as_ref().map(|d| {
            Action::Delete(
                d.key.clone(),
                d.harness_label.is_some() && v["also"] == true,
            )
        }),
        "bulkArchive" => app
            .ui
            .bulk_archive
            .as_ref()
            .map(|d| Action::BulkArchive(d.plan.eligible.clone())),
        "mute" => {
            let Some(d) = &mut app.ui.mute else {
                return Ok(());
            };
            update_mute(d, v);
            match d.outcome_in(now, app.zone) {
                mute::MuteOutcome::Mute(until) if d.many.is_empty() => {
                    Some(Action::MuteUntil(d.key.clone(), until))
                }
                mute::MuteOutcome::Mute(until) => Some(Action::MuteMany(d.many.clone(), until)),
                mute::MuteOutcome::Unavailable(reason) => return Err(reason),
            }
        }
        "project" => {
            let folder = string(v, "folder").trim();
            if folder.is_empty() {
                return Err("Enter the project folder.".into());
            }
            let name = string(v, "name").trim();
            if name.is_empty() {
                return Err("Enter a project name.".into());
            }
            app.ui.create_project.take().map(|mut d| {
                d.folder = folder.to_owned();
                d.name = name.to_owned();
                Action::CreateProject(d)
            })
        }
        "move" => app
            .ui
            .move_dialog
            .as_ref()
            .filter(|d| d.can_move(&app.client.state))
            .map(|d| Action::Move(d.moves(&app.client.state), None)),
        "fork" => {
            let Some(d) = &app.ui.fork else {
                return Ok(());
            };
            let Some(view) = fork::view(&app.client.state, &d.key, &d.choice) else {
                return Ok(());
            };
            if view.confirm_waits || view.unavailable.is_some() {
                return Ok(());
            }
            let message = string(v, "message").trim();
            if view.handoff && message.is_empty() {
                return Err("Enter a first message for the new agent.".into());
            }
            Some(Action::Fork(
                d.key.clone(),
                d.choice.clone(),
                view.handoff.then(|| message.to_owned()),
            ))
        }
        "mode" => Some(Action::ConfirmMode),
        "ultrafast" => Some(Action::ConfirmUltrafast),
        "quit" => {
            app.ui.quit_confirm = false;
            app.quit_window();
            None
        }
        "updateNotice" => {
            app.updater.notice = None;
            None
        }
        "update" => app
            .ui
            .update_now
            .as_ref()
            .map(|d| Action::ConfirmUpdateDaemon(d.target.clone())),
        _ => None,
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
    Ok(())
}

fn pull_request_command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let Some(d) = app.client.state.create_pr.as_mut() else { return Ok(()) };
    if p["instance"] != format!("pullRequest:{}", d.key.address()) {
        return Ok(());
    }
    let v = &p["values"];
    if name == "dialog.change"
        && let Some(revision) = p["editRevision"].as_u64()
        && revision > app.ui.wpf_pr_edit
    {
        app.ui.wpf_pr_edit = revision;
        if !d.creating
            && !matches!(d.agent, Some(AgentDraft::Sending | AgentDraft::Writing(_)))
            && let (Some(title), Some(body)) = (v["title"].as_str(), v["body"].as_str())
        {
            d.edited |= d.title != title || d.body != body;
            d.title = title.to_owned();
            d.body = body.to_owned();
        }
    }
    if d.creating {
        return Ok(());
    }
    let action = match name {
        "dialog.cancel" => {
            app.client.state.create_pr = None;
            None
        }
        "dialog.change" => {
            let base = string(v, "base");
            (base != d.base && d.ready().is_some_and(|r| r.branches.iter().any(|b| b == base)))
                .then(|| Action::PrBase(base.to_owned()))
        }
        "dialog.askAgent" => Some(Action::AskAgentPr),
        "dialog.submit" => {
            if let Some(reason) = d.create_blocked() {
                return Err(reason);
            }
            d.draft = v["draft"] == true;
            Some(Action::CreatePr)
        }
        _ => None,
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
    fn project_browsing_only_accepts_listed_directories_and_requires_a_name() {
        use yaac_core::state::FsListingState;
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let target = threads::billing().target;
        app.ui.create_project = Some(crate::sidebar::create_project_for(&app, &target, "/project"));
        let instance = view(&app)["instance"].clone();
        let args = json!({ "id": "project", "instance": instance, "values": { "name": "" } });
        command(&mut app, "dialog.change", &args).unwrap();
        assert_eq!(view(&app)["submitBlocked"], "Enter a project name.");
        assert!(matches!(project_action(&mut app, &json!({ "action": "browse" })), Some(Action::FsList(_, path)) if path == "/project"));
        app.client.state.fs_listings.insert((target, "/project".to_owned()), FsListingState::Ready(
            serde_json::from_value(json!({
                "path": "/project", "parent": "/", "entries": [
                    { "path": "/project/src", "name": "src", "kind": "directory" },
                    { "path": "/project/readme", "name": "readme", "kind": "file" }
                ]
            })).unwrap()
        ));
        assert_eq!(view(&app)["folders"]["entries"].as_array().unwrap().len(), 3);
        assert!(project_action(&mut app, &json!({ "action": "folder", "path": "/project/readme" })).is_none());
        project_action(&mut app, &json!({ "action": "useFolder" }));
        let result = view(&app);
        assert_eq!(result["fields"][0]["value"], "/project/");
        assert_eq!(result["fields"][1]["value"], "project");
        assert_eq!(result["fieldRevision"], 1);
        assert!(result["submitBlocked"].is_null());
    }

    #[test]
    fn mute_duration_edits_keep_the_selected_choice_and_validate_before_submit() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        app.ui.mute = Some(mute::MuteDialog::new(threads::billing(), "Billing", app.now(), app.zone));
        assert!(view(&app)["hint"].is_null());
        let change = |app: &mut YaacApp, values| {
            command(app, "dialog.change", &json!({ "id": "mute", "values": values })).unwrap();
        };
        change(&mut app, json!({ "when": "for" }));
        change(&mut app, json!({ "amount": "0" }));
        assert_eq!(view(&app)["submitBlocked"], "Enter a number above zero");
        change(&mut app, json!({ "amount": "5" }));
        assert!(string(&view(&app), "hint").starts_with("until "));
        assert!(view(&app)["submitBlocked"].is_null());
        change(&mut app, json!({ "when": "until" }));
        assert_eq!(view(&app)["fields"][1]["type"], "date");
        change(&mut app, json!({ "date": "not a date" }));
        assert!(!view(&app)["submitBlocked"].is_null());
    }

    #[test]
    fn pull_request_edits_survive_preparation_and_create_cannot_be_cancelled() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let send = |app: &mut YaacApp, name, payload: &Value| {
            super::super::dispatch(app, &ctx, &mut None, name, payload).unwrap();
        };
        let key = threads::billing();
        let instance = format!("pullRequest:{}", key.address());
        let payload = json!({ "id": "pullRequest", "instance": instance });
        app.apply(vec![Action::OpenCreatePr(key)]);
        assert_eq!(view(&app)["submitBlocked"], "Reading the branch…");

        let mut change = payload.clone();
        change["editRevision"] = json!(1);
        change["values"] = json!({ "title": "My PR title", "body": "My description" });
        send(&mut app, "dialog.change", &change);
        app.pump_demo();
        let ready = view(&app);
        assert_eq!(ready["body"], "auth/billing · acme/platform");
        assert_eq!(ready["branches"][0]["label"], "main  (default)");
        assert_eq!(ready["base"], "main");
        assert_eq!(ready["prTitle"], "My PR title");
        assert_eq!(ready["description"], "My description");
        assert_eq!(ready["editRevision"], 1);
        assert!(ready["submitBlocked"].is_null());

        send(&mut app, "dialog.askAgent", &payload);
        assert_eq!(view(&app)["agentStatus"], "The agent is writing…");
        for _ in 0..200 {
            app.demo.as_ref().unwrap().tick();
            app.pump_demo();
            if app.client.poll_create_pr() {
                break;
            }
        }
        assert_eq!(view(&app)["prTitle"], "Fix the frame decoder's off-by-one on the last frame");

        send(&mut app, "dialog.submit", &payload);
        assert_eq!(view(&app)["submit"], "Creating…");
        send(&mut app, "dialog.cancel", &payload);
        send(&mut app, "dialog.askAgent", &payload);
        assert!(app.client.state.create_pr.as_ref().is_some_and(|d| d.creating && d.agent.is_none()));
        app.pump_demo();
        assert!(view(&app).is_null());
    }
}
