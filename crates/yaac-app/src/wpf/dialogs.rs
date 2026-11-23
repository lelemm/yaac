use super::*;
use yaac_core::{fork, mode_confirm, mute};
use yaac_protocol::{InstanceId, ModelId};

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

pub(super) fn view(app: &YaacApp) -> Value {
    let u = &app.ui;
    let state = &app.client.state;
    if let Some((_, title)) = &u.rename {
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
            fields.push(field("amount", "Amount", &d.amount));
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
            fields.push(field("date", "Date", &d.date_text));
            fields.push(field("time", "Local time", &d.time));
        }
        form("mute", &d.title, "", "Mute", fields)
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
        form(
            "project",
            "Create project",
            &format!("On {}", state.target_name(&d.target)),
            "Create",
            vec![
                field("folder", "Folder", &d.folder),
                field("name", "Name (optional)", &d.name),
            ],
        )
    } else if let Some(d) = &u.fork
        && let Some(v) = fork::view(state, &d.key, &d.choice)
    {
        let mut fields = vec![choice(
            "destination",
            "Provider and model",
            &fork_id(&d.choice.instance, d.choice.model.as_ref()),
            v.destinations
                .iter()
                .filter(|dest| dest.reason.is_none())
                .flat_map(|dest| {
                    std::iter::once(option(&fork_id(&dest.instance, None), &dest.name)).chain(
                        dest.models.iter().map(|(id, name, _)| {
                            option(
                                &fork_id(&dest.instance, Some(id)),
                                &format!("{} · {name}", dest.name),
                            )
                        }),
                    )
                })
                .collect(),
        )];
        if v.handoff {
            fields.push(json!({ "id": "message", "label": "First message to the new agent", "type": "multiline", "value": d.message }));
        }
        let body = [
            Some(v.result_text),
            v.notice,
            v.unavailable.clone(),
            v.preview,
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
        let body = [vec![v.body.0, v.body.1], v.lines, vec![v.footer]]
            .concat()
            .join("\n");
        form("mode", &v.title, &body, &v.confirm_label, vec![])
    } else if let Some(d) = &u.update_now {
        form("update", &d.title, &d.body, "Update now", vec![])
    } else if let Some(d) = &state.create_pr {
        let fields = vec![
            choice(
                "base",
                "Base branch",
                &d.base,
                d.branches().iter().map(|b| option(b, b)).collect(),
            ),
            field("title", "Title", &d.title),
            json!({ "id": "body", "label": "Description", "type": "multiline", "value": d.body }),
            json!({ "id": "draft", "label": "Draft pull request", "type": "check", "value": d.draft }),
        ];
        let body = [d.error.clone(), d.push_notice(), d.uncommitted_warning()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n");
        form(
            "pullRequest",
            "Create pull request",
            &body,
            if d.ready().is_some() && !d.creating {
                "Create"
            } else {
                ""
            },
            fields,
        )
    } else {
        Value::Null
    }
}

fn fork_id(instance: &InstanceId, model: Option<&ModelId>) -> String {
    json!([instance, model]).to_string()
}

pub(super) fn command(app: &mut YaacApp, name: &str, p: &Value) -> Result<(), String> {
    let id = string(p, "id");
    if view(app)["id"] != id {
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
            "pullRequest" => app.client.state.create_pr = None,
            _ => {}
        }
        return Ok(());
    }
    if name == "dialog.change" {
        match id {
            "mute" => {
                if let Some(d) = &mut app.ui.mute {
                    d.choice = match string(v, "when") {
                        "for" => mute::MuteChoice::For,
                        "until" => mute::MuteChoice::Until,
                        _ => mute::MuteChoice::Forever,
                    };
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
                    && let Ok((instance, model)) = serde_json::from_str::<(
                        InstanceId,
                        Option<ModelId>,
                    )>(string(v, "destination"))
                {
                    d.choice = fork::ForkChoice {
                        instance: instance.clone(),
                        model,
                    };
                    app.client.fork_preview(&d.key, &instance);
                }
            }
            "pullRequest" => {
                if let Some(base) = v["base"].as_str() {
                    app.apply(vec![Action::PrBase(base.to_owned())]);
                }
            }
            _ => {}
        }
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
            if let Some(amount) = v["amount"].as_str() {
                d.amount = amount.to_owned();
            }
            if let Some(unit) = v["unit"].as_str().and_then(|label| {
                mute::DurationUnit::ALL
                    .into_iter()
                    .find(|u| u.label() == label)
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
            app.ui.create_project.take().map(|mut d| {
                d.folder = folder.to_owned();
                d.name = string(v, "name").trim().to_owned();
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
        "update" => app
            .ui
            .update_now
            .as_ref()
            .map(|d| Action::ConfirmUpdateDaemon(d.target.clone())),
        "pullRequest" => {
            if let Some(d) = &mut app.client.state.create_pr {
                d.title = string(v, "title").to_owned();
                d.body = string(v, "body").to_owned();
                d.draft = v["draft"] == true;
                d.edited = true;
                if let Some(reason) = d.create_blocked() {
                    return Err(reason);
                }
            }
            Some(Action::CreatePr)
        }
        _ => None,
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
    Ok(())
}
