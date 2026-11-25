use super::*;
use crate::commander::{self, CommanderUi, Row};
use yaac_core::commands::{self, Avail};

pub(super) fn command_view(app: &YaacApp) -> Value {
    let Some(ui) = &app.ui.commander else {
        return Value::Null;
    };
    let rows = commander::rows(app, ui).into_iter().enumerate().map(|(index, row)| match row {
        Row::Cmd(row) => {
            let detail = match row.avail { Avail::Disabled(ref why) => Some(why), _ => None };
            json!({
                "id": commands::def_of(row.cmd).id, "label": row.name, "detail": detail,
                "enabled": row.avail.enabled(), "checked": row.checked,
                "heading": row.heading, "key": app.ui.bindings.label(row.cmd),
            })
        }
        Row::Step(row) => json!({
            "id": index.to_string(), "label": row.label, "detail": row.reason.as_ref().or(row.detail.as_ref()),
            "enabled": row.reason.is_none(), "checked": row.current,
        }),
    }).collect::<Vec<_>>();
    json!({
        "rows": rows, "query": ui.query, "step": ui.step.map(|(cmd, _)| commands::def_of(cmd).id),
    })
}

pub(super) fn command(
    app: &mut YaacApp,
    ctx: &egui::Context,
    cmd: &str,
    payload: &Value,
) -> Result<(), String> {
    match cmd {
        "commander.open" => {
            if app.ui.commander.is_none() {
                let card = focused_card(app, payload);
                app.ui.commander = Some(CommanderUi::at(None, card));
            }
        }
        "commander.close" => app.ui.commander = None,
        "commander.back" => app.ui.commander = app.ui.commander.take().map(CommanderUi::back),
        "commander.query" => {
            if let Some(ui) = &mut app.ui.commander {
                ui.query = string(payload, "text").to_owned();
            }
        }
        "commander.pick" => {
            let Some(ui) = app.ui.commander.take() else {
                return Ok(());
            };
            let id = string(payload, "id");
            let mut actions = Vec::new();
            let rows = commander::rows(app, &ui);
            let found = if let Some((cmd, _)) = ui.step {
                if let Ok(index) = id.parse::<usize>()
                    && let Some(Row::Step(row)) = rows.into_iter().nth(index)
                    && row.reason.is_none()
                {
                    if string(payload, "query") != ui.query { app.ui.commander = Some(ui); return Ok(()); }
                    commander::run_step(app, cmd, row.value, &mut actions);
                    true
                } else { false }
            } else if let Some(Row::Cmd(row)) = rows.into_iter().find(|row| matches!(row, Row::Cmd(row) if commands::def_of(row.cmd).id == id && row.avail.enabled())) {
                let card = ui.card(app).cloned();
                commander::run(app, ctx, row.cmd, card, true, &mut actions);
                commands::note_recent(&mut app.ui.recent, id);
                true
            } else { false };
            if found {
                app.apply(actions);
            } else {
                app.ui.commander = Some(ui);
            }
        }
        _ => return Err(format!("Unknown Commander action {cmd}")),
    }
    Ok(())
}

fn focused_card(app: &YaacApp, payload: &Value) -> Option<(ThreadKey, ItemId)> {
    let thread = selected_thread(app)?;
    let card = payload["card"].as_str()?;
    if app.ui.settings || thread.address().as_str() != payload["threadId"].as_str()? {
        return None;
    }
    Some((thread.clone(), selected_item(app, card)?))
}

pub(super) fn places(app: &YaacApp, forward: bool) -> Vec<Value> {
    app.ui
        .history
        .menu(forward, &|p| app.place_exists(p))
        .into_iter()
        .map(|(steps, entry)| {
            let (label, detail) = crate::nav::label(app, entry);
            json!({ "steps": steps, "label": label, "detail": detail })
        })
        .collect()
}

pub(super) fn shortcuts(app: &YaacApp) -> Vec<Value> {
    use yaac_core::keys::{ChordKey, MouseButton};
    commands::COMMANDS.iter().filter(|d| d.scope == commands::Scope::Global
        || matches!(d.cmd, commands::Cmd::NextTerminal | commands::Cmd::PrevTerminal)).flat_map(|d| {
        app.ui.bindings.chords(d.cmd).into_iter().map(move |chord| {
            let in_text = chord.works_in_text();
            let key = match chord.key {
                ChordKey::Key(key) => key,
                ChordKey::Mouse(MouseButton::Back) => "MouseBack".to_owned(),
                ChordKey::Mouse(MouseButton::Forward) => "MouseForward".to_owned(),
            };
            let repeat = matches!(d.cmd, commands::Cmd::NextThread | commands::Cmd::PrevThread | commands::Cmd::ZoomIn | commands::Cmd::ZoomOut);
            json!({ "id": d.id, "key": key, "ctrl": chord.ctrl, "shift": chord.shift, "alt": chord.alt, "inText": in_text, "inTerminal": app.ui.bindings.works_in_terminal(d.cmd), "repeat": repeat })
        })
    }).collect()
}

pub(super) fn shortcut(app: &mut YaacApp, ctx: &egui::Context, payload: &Value) {
    let Some(def) = commands::by_id(string(payload, "id")) else {
        return;
    };
    if payload["terminalFocus"] == true && !app.ui.bindings.works_in_terminal(def.cmd) { return; }
    if def.scope != commands::Scope::Global
        && !(payload["terminalFocus"] == true
            && matches!(def.cmd, commands::Cmd::NextTerminal | commands::Cmd::PrevTerminal)) {
        return;
    }
    if app.ui.commander.is_some() {
        if def.cmd == commands::Cmd::Commander {
            app.ui.commander = None;
        }
        return;
    }
    let item = focused_card(app, payload).map(|(_, item)| item);
    if commander::with_ctx(app, item.as_ref(), |c| commands::availability(def.cmd, c)).enabled() {
        let mut actions = Vec::new();
        commander::run(
            app,
            ctx,
            def.cmd,
            item,
            payload["inText"] == true,
            &mut actions,
        );
        app.apply(actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::demo::threads;
    use yaac_protocol::{ItemBody, RequestState};

    #[test]
    fn commander_keeps_the_focused_approval_scoped_to_its_thread() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let fix = threads::fix();
        let index = threads::index();
        let item_id = ItemId::from("f5");
        app.apply(vec![Action::Open(fix.clone()), Action::Open(index.clone())]);
        let transcript = app.client.state.transcripts.get_mut(&index).unwrap();
        let mut items = transcript.items().to_vec();
        let other = items.iter_mut().find(|item| item.id.as_str() == "i3").unwrap();
        other.id = item_id.clone();
        transcript.replace_all(items, 99);
        app.apply(vec![Action::Open(fix.clone())]);

        let enabled = |app: &YaacApp, id: &str| {
            command_view(app)["rows"].as_array().unwrap().iter()
                .any(|row| row["id"] == id && row["enabled"] == true)
        };
        for payload in [json!({}), json!({ "card": item_id, "threadId": index.address() })] {
            command(&mut app, &ctx, "commander.open", &payload).unwrap();
            assert!(!enabled(&app, "allow-once"));
            assert!(!enabled(&app, "deny"));
            command(&mut app, &ctx, "commander.close", &Value::Null).unwrap();
        }

        let focused = json!({ "card": item_id, "threadId": fix.address() });
        command(&mut app, &ctx, "commander.open", &focused).unwrap();
        assert!(enabled(&app, "allow-once"));
        assert!(enabled(&app, "allow-session"));
        command(&mut app, &ctx, "commander.pick", &json!({ "id": "theme" })).unwrap();
        command(&mut app, &ctx, "commander.back", &Value::Null).unwrap();
        assert!(enabled(&app, "deny"));

        let deny = json!({ "id": "deny" });
        app.client.state.selection = Selection::Thread(index.clone());
        assert!(!enabled(&app, "deny"));
        command(&mut app, &ctx, "commander.pick", &deny).unwrap();
        app.client.state.selection = Selection::Thread(fix.clone());
        command(&mut app, &ctx, "commander.pick", &deny).unwrap();
        for (key, state) in [(fix, RequestState::Submitted), (index, RequestState::Pending)] {
            let item = app.client.state.transcripts.get(&key).unwrap().item(&item_id).unwrap();
            assert!(matches!(&item.body, ItemBody::Request(request) if request.state == state));
        }
    }
}
