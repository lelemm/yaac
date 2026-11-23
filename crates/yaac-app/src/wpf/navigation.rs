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
        "commander.open" => app.ui.commander = Some(CommanderUi::default()),
        "commander.close" => app.ui.commander = None,
        "commander.back" => app.ui.commander = Some(CommanderUi::default()),
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
                commander::run(app, ctx, row.cmd, ui.card.clone(), true, &mut actions);
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
    commands::COMMANDS.iter().filter(|d| d.scope == commands::Scope::Global).flat_map(|d| {
        app.ui.bindings.chords(d.cmd).into_iter().map(move |chord| {
            let in_text = chord.works_in_text();
            let key = match chord.key {
                ChordKey::Key(key) => key,
                ChordKey::Mouse(MouseButton::Back) => "MouseBack".to_owned(),
                ChordKey::Mouse(MouseButton::Forward) => "MouseForward".to_owned(),
            };
            let repeat = matches!(d.cmd, commands::Cmd::NextThread | commands::Cmd::PrevThread | commands::Cmd::ZoomIn | commands::Cmd::ZoomOut);
            json!({ "id": d.id, "key": key, "ctrl": chord.ctrl, "shift": chord.shift, "alt": chord.alt, "inText": in_text, "repeat": repeat })
        })
    }).collect()
}

pub(super) fn shortcut(app: &mut YaacApp, ctx: &egui::Context, payload: &Value) {
    let Some(def) = commands::by_id(string(payload, "id")) else {
        return;
    };
    if def.scope != commands::Scope::Global {
        return;
    }
    if app.ui.commander.is_some() {
        if def.cmd == commands::Cmd::Commander {
            app.ui.commander = None;
        }
        return;
    }
    let item = selected_item(app, string(payload, "card"));
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
