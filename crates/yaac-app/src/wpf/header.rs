use super::*;
use yaac_core::header::{self, Banner, LinkLine, LinkPart, Tone};

fn link(part: &LinkPart) -> Value {
    json!({ "label": part.title, "id": part.key.as_ref().map(ThreadKey::address) })
}

fn label(text: &str) -> Value {
    json!({ "label": text })
}

pub(super) fn fields(app: &YaacApp, map: &AttentionMap) -> Value {
    let state = &app.client.state;
    let Some(key) = selected_thread(app) else {
        let (title, location) = if let Selection::NewThread(id) = state.selection {
            let draft = state.new_threads.iter().find(|d| d.id == id);
            let location = draft.and_then(|d| {
                let target = d.target.as_ref()?;
                let project = state
                    .projects
                    .iter()
                    .find(|p| Some(&p.id) == d.project.as_ref());
                let name = project.map_or("No project yet", |p| p.name.as_str());
                Some(format!("{} · {name}", state.target_name(target)))
            });
            ("New thread", location)
        } else {
            ("", None)
        };
        return json!({ "title": title, "location": location, "links": [], "banners": [] });
    };
    let Some(h) = header::header(state, map, key) else {
        return json!({});
    };
    let (tag, tone) = h.tag.as_ref().map_or(("", "idle"), |(tag, tone)| {
        let tone = match tone {
            Tone::NeedsYou => "needsYou",
            Tone::Running => "running",
            Tone::Failed => "failed",
            Tone::Neutral => "idle",
        };
        (tag.as_str(), tone)
    });
    let links = match h.links {
        LinkLine::None => vec![],
        LinkLine::SpawnedBy { parent, target } => vec![
            label("Spawned by"),
            link(&parent),
            label(&format!("on {target}")),
        ],
        LinkLine::Ancestry(parts) => parts
            .iter()
            .enumerate()
            .flat_map(|(i, part)| {
                (i > 0)
                    .then(|| label("›"))
                    .into_iter()
                    .chain(std::iter::once(link(part)))
            })
            .collect(),
        LinkLine::ForkedFrom { source, detail } => {
            vec![label("Forked from"), link(&source), label(&detail)]
        }
    };
    let banners = h.banners.iter().map(|banner| match banner {
        Banner::InUseElsewhere { text, take_over } => {
            let fork = json!({ "id": "fork", "label": "Fork to continue here", "enabled": true, "primary": take_over.is_none() });
            let mut actions = vec![fork];
            if let Some(t) = take_over {
                actions.push(json!({ "id": "takeOver", "label": if t.busy { "Taking over…" } else { "Take over" },
                    "enabled": t.enabled, "primary": true, "detail": t.tooltip, "busy": t.busy }));
            }
            let detail = take_over.as_ref().and_then(|t| {
                t.failure.as_ref().map(|why| format!("Not taken over: {why}"))
                    .or_else(|| (!t.enabled && !t.busy).then(|| t.tooltip.clone()))
            });
            json!({ "label": text, "detail": detail, "failed": take_over.as_ref().is_some_and(|t| t.failure.is_some()), "actions": actions })
        }
        Banner::Subagent { text, reason, parent } => {
            let actions = parent.as_ref().and_then(|p| p.key.as_ref()).map(|k| {
                sidebar_view::action(&format!("open:{}", k.address()), "Open parent", true)
            }).into_iter().collect::<Vec<_>>();
            json!({ "label": text, "detail": reason, "actions": actions })
        }
        Banner::OlderDaemon { text, update_now } => {
            let actions = update_now.as_ref().map(|_| sidebar_view::action("update", "Update now", true)).into_iter().collect::<Vec<_>>();
            json!({ "label": text, "actions": actions })
        }
        Banner::StoreAway { text } => json!({ "label": text, "actions": [] }),
    }).collect::<Vec<_>>();
    json!({
        "title": h.title, "location": h.breadcrumb.join(" · "), "hasThread": true, "threadId": key.address(),
        "showEditor": !h.display.read_only, "readOnly": h.display.read_only, "mark": sidebar_view::tone(h.display.mark),
        "fast": h.display.fast, "ultrafast": h.display.ultrafast, "tag": tag, "tagTone": tone, "links": links, "banners": banners, "budget": h.budget.is_some(),
    })
}

pub(super) fn command(app: &mut YaacApp, p: &Value) {
    let Some(thread) = selected_thread(app)
        .cloned()
        .filter(|k| k.address().as_str() == string(p, "threadId"))
    else {
        return;
    };
    let state = &app.client.state;
    let Some(h) = header::header(state, &AttentionMap::compute(state), &thread) else {
        return;
    };
    let action = match string(p, "id") {
        "continue" => h.budget.map(Action::ContinueBudget),
        "takeOver" => yaac_core::thread_menu::take_over(state, &thread)
            .filter(|t| t.enabled)
            .map(|_| Action::TakeOver(thread)),
        "fork" => Some(Action::OpenFork(thread)),
        "update" => h.banners.into_iter().find_map(|banner| match banner {
            Banner::OlderDaemon { update_now, .. } => update_now.map(Action::UpdateDaemon),
            _ => None,
        }),
        id if id.starts_with("open:") => key(app, &id["open:".len()..]).map(Action::Open),
        _ => None,
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
}
