use super::*;
use yaac_core::attention::Mark;
use yaac_core::bulk::{self, BulkKind, Click};
use yaac_core::moving::{self, Drop};
use yaac_core::sidebar::{Open, Order, RowRight, Section, SectionKind};
use yaac_core::thread_menu::{self, MenuAction};
use yaac_protocol::TargetId;

const MENU_ACTIONS: &[(MenuAction, &str)] = &[
    (MenuAction::CopyId, "copyId"),
    (MenuAction::Rename, "rename"),
    (MenuAction::Fork, "fork"),
    (MenuAction::TakeOver, "takeOver"),
    (MenuAction::Move, "move"),
    (MenuAction::Pin, "pin"),
    (MenuAction::Unpin, "unpin"),
    (MenuAction::Mute, "mute"),
    (MenuAction::Unmute, "unmute"),
    (MenuAction::ChangeMute, "changeMute"),
    (MenuAction::Dismiss, "dismiss"),
    (MenuAction::ShowAgain, "showAgain"),
    (MenuAction::Archive, "archive"),
    (MenuAction::Unarchive, "unarchive"),
    (MenuAction::Delete, "delete"),
    (MenuAction::CreatePr, "createPr"),
];

pub(super) fn tone(mark: Mark) -> &'static str {
    match mark {
        Mark::NeedsYou => "needsYou",
        Mark::Idle => "idle",
        _ => mark.word(),
    }
}

pub(super) fn action(id: &str, label: &str, enabled: bool) -> Value {
    json!({ "id": id, "label": label, "enabled": enabled })
}

fn check(id: &str, label: &str, checked: bool) -> Value {
    json!({ "id": id, "label": label, "enabled": true, "checked": checked })
}

pub(super) fn menu(app: &YaacApp, key: &ThreadKey) -> Vec<Value> {
    thread_menu::thread_menu(&app.client.state, key, app.now()).iter().map(|entry| {
        let Some(item) = entry.item() else { return action("", "", false) };
        let id = MENU_ACTIONS.iter().find(|(a, _)| *a == item.action).map_or("", |(_, id)| *id);
        json!({ "id": id, "label": item.label, "enabled": item.enabled, "detail": item.reason, "danger": item.danger })
    }).collect()
}

fn sections(app: &YaacApp, map: &AttentionMap) -> Vec<Section> {
    sidebar::sections(
        &app.client.state,
        map,
        app.ui.grouping,
        app.ui.ordering,
        &app.ui.search,
        Open {
            muted: app.ui.muted_open,
            archived: app.ui.archived_open,
        },
        app.now(),
        app.zone,
    )
}

fn usage(app: &YaacApp, now: i64) -> Vec<Value> {
    use yaac_core::limits::{self, Segment};
    let state = &app.client.state;
    let names = |target: &TargetId| state.target(target).map_or_else(|| target.to_string(), |r| r.entry.name.to_string());
    let panel = limits::panel(&state.rate_limits.accounts, &names, now, app.zone);
    panel.groups.into_iter().map(|group| {
        let logo = crate::logos::for_kind(group.kind.as_str());
        let hex = |c: egui::Color32| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
        let bars: Vec<_> = group.bars.iter().map(|bar| json!({
            "label": bar.label, "text": bar.text, "near": bar.near,
            "segments": bar.segments.map(|segment| match segment {
                Segment::Full => 1.0,
                Segment::Partial => 0.45,
                Segment::Empty => 0.0,
            }),
        })).collect();
        let colour = crate::limits_ui::colour(group.kind.as_str(), &app.p);
        json!({
            "label": group.kind, "tooltip": group.tooltip, "stale": group.stale,
            "tightest": group.tightest, "bars": bars, "color": hex(colour),
            "logo": logo.map(|(logo, _)| crate::logos::source(logo).0),
            "logoColor": logo.and_then(|(_, colour)| colour).map(hex),
        })
    }).collect()
}

pub(super) fn view(app: &YaacApp, map: &AttentionMap, selection_epoch: u64) -> Value {
    let grouping = app.ui.grouping;
    let ordering = app.ui.ordering;
    let state = &app.client.state;
    let now = app.now();
    let sections = sections(app, map);
    let order: Vec<_> = sections
        .iter()
        .flat_map(|s| s.rows.iter().map(|r| r.key.clone()))
        .collect();
    let picked = app.ui.list_sel.in_order(&order);
    let groups: Vec<_> = sections.iter().map(|s| {
        let header = s.header.as_ref().map(|h| {
            let toggle = matches!(s.kind, SectionKind::Muted | SectionKind::Archived)
                .then(|| action("section.toggle", &h.left, true));
            let target = h.machine.as_ref().and_then(|name| state.targets.iter().find(|t| t.entry.name.as_str() == name)).map(|t| &t.entry.id);
            let retry = h.retry.then(|| target.map(|t| action(&format!("target.retry:{t}"), "Retry", true))).flatten();
            let mut actions = Vec::new();
            if h.temporary {
                actions.push(action("section.createProject", "Create project…", h.folder.is_some()));
                actions.push(action("section.dismissAll", "Dismiss all", !s.rows.is_empty()));
                if !s.rows.is_empty() {
                    actions.push(action("", "", false));
                }
            }
            if matches!(s.kind, SectionKind::Group | SectionKind::Pinned) && !s.rows.is_empty() {
                actions.push(action("section.archiveAll", &format!("Archive all ({})…", s.rows.len()), true));
                actions.push(action("section.muteAll", "Mute all…", true));
            }
            json!({
                "label": h.left, "machine": h.machine, "connection": h.connection.as_ref().map(|(_, s)| s),
                "tone": h.connection.as_ref().map(|(mark, _)| tone(*mark)), "temporary": h.temporary,
                "olderDaemon": h.older_daemon, "toggle": toggle, "primary": retry, "actions": actions,
            })
        });
        let rows: Vec<_> = s.rows.iter().map(|r| {
            let right = match &r.right {
                RowRight::Age(s) | RowRight::Until(s) => s.clone(),
                RowRight::Machine { name, muted } => if *muted { format!("{name} · muted") } else { name.clone() },
            };
            let line2 = r.line2.as_ref().map(|l| {
                let logo = l.kind.as_deref().and_then(crate::logos::for_kind);
                let color = logo.and_then(|(_, color)| color)
                    .map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()));
                json!({
                    "provider": l.kind, "logo": logo.map(|(logo, _)| crate::logos::source(logo).0), "logoColor": color,
                    "branch": l.branch, "added": l.added, "modified": l.modified,
                    "deleted": l.deleted, "ahead": l.ahead, "behind": l.behind, "workers": l.workers,
                })
            });
            let display = r.display;
            let badge = yaac_core::pr::badge_of(state, &r.key).and_then(|b| {
                (b.primary.is_some() || b.can_create).then(|| {
                    let label = if b.primary.is_some() { b.number_text() } else { "Create PR".to_owned() };
                    json!({ "id": "pullRequest", "label": label, "detail": b.tooltip(), "enabled": b.enabled })
                })
            });
            json!({
                "id": r.key.address(), "copyId": state.summary(&r.key).and_then(|s| s.native_id.as_ref()),
                "title": r.title, "right": right, "status": display.mark.word(),
                "tone": tone(display.mark), "detail": display.attention.map(|a| a.word()),
                "selected": r.selected, "unseen": display.unseen, "readOnly": display.read_only,
                "muted": r.muted, "fast": display.fast, "ultrafast": display.ultrafast, "draggable": moving::can_drag(grouping) && moving::blocked(state, &r.key).is_none(),
                "actions": menu(app, &r.key), "line2": line2, "badge": badge,
            })
        }).collect();
        let drop = if moving::can_drag(grouping) && s.kind == SectionKind::Group {
            let result = moving::drop_on(state, grouping, s, &picked);
            let allowed = picked.is_empty() || matches!(result, Drop::Move { .. });
            let detail = match result { Drop::Refused(reason) => Some(reason), _ => None };
            Some(json!({ "action": action("section.drop", "Move here", allowed), "detail": detail }))
        } else {
            None
        };
        json!({ "id": s.id, "header": header, "rows": rows, "collapsed": s.collapsed, "drop": drop })
    }).collect();
    let footer = sidebar::hub_footer(state);
    let down = if footer.down > 0 {
        format!("   ↓ {}", footer.down)
    } else {
        String::new()
    };
    let footer_label = if footer.reachable {
        format!("⌁   ↑ {}{down}", footer.up)
    } else {
        "Hub offline".to_owned()
    };
    let bulk = [
        ("bulk.archive", "Archive…", BulkKind::Archive),
        ("bulk.mute", "Mute…", BulkKind::Mute),
        ("bulk.unmute", "Unmute", BulkKind::Unmute),
    ]
    .into_iter()
    .map(|(id, label, kind)| {
        let enabled =
            picked.is_empty() || !bulk::plan(state, &picked, kind, now).eligible.is_empty();
        action(id, label, enabled)
    })
    .collect::<Vec<_>>();
    json!({
        "search": app.ui.search, "searchEnabled": true, "sections": groups, "selectionReset": selection_epoch.to_string(),
        "newThread": action("new", "New", yaac_core::first_run::preferred_start_target(state).is_some()),
        "grouping": [
            check("group.machine", "Group by machine", grouping.machine),
            check("group.project", "Group by project", grouping.project),
            check("group.worktree", "Group by worktree", grouping.worktree),
            check("group.dismissed", "Show dismissed", grouping.show_dismissed),
        ],
        "ordering": [
            check("order.last", Order::LastMessage.label(), ordering.order == Order::LastMessage),
            check("order.az", Order::AToZ.label(), ordering.order == Order::AToZ),
            check("order.za", Order::ZToA.label(), ordering.order == Order::ZToA),
            check("order.unseen", "Unseen first", ordering.unseen_first),
            check("order.needs", "Needs you first", ordering.needs_first),
            check("order.running", "Show only running", ordering.running_only),
        ],
        "bulkActions": bulk, "usage": usage(app, now),
        "footer": { "label": footer_label, "detail": format!("{} connected · {} offline", footer.up, footer.down) },
        "notice": app.client.move_notice.as_ref().map(|n| json!({
            "label": n.text, "actions": [action("move.undo", "Undo", true), action("move.dismiss", "Dismiss", true)],
        })),
    })
}

fn selection(app: &YaacApp, p: &Value) -> Vec<ThreadKey> {
    let state = &app.client.state;
    let visible = sections(app, &AttentionMap::compute(state));
    let ids: std::collections::BTreeSet<_> = p["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    visible
        .into_iter()
        .flat_map(|section| section.rows)
        .filter_map(|row| ids.contains(row.key.address().as_str()).then_some(row.key))
        .collect()
}

pub(super) fn command(
    app: &mut YaacApp,
    ctx: &egui::Context,
    name: &str,
    p: &Value,
) -> Result<(), String> {
    match name {
        "sidebar.search" => {
            app.ui.search = string(p, "text").to_owned();
            return Ok(());
        }
        "sidebar.selection" => {
            let rows = selection(app, p);
            app.ui.list_sel.clear();
            for k in &rows {
                app.ui.list_sel.click(&rows, k, Click::Toggle, None);
            }
            return Ok(());
        }
        "dialog.cancel" | "dialog.submit" | "dialog.change" => {
            return super::dialogs::command(app, name, p);
        }
        _ => {}
    }
    let id = string(p, "action");
    let grouping = &mut app.ui.grouping;
    let ordering = &mut app.ui.ordering;
    let action = match id {
        "group.machine" => {
            grouping.machine = !grouping.machine;
            None
        }
        "group.project" => {
            grouping.project = !grouping.project;
            None
        }
        "group.worktree" => {
            grouping.worktree = !grouping.worktree;
            None
        }
        "group.dismissed" => {
            grouping.show_dismissed = !grouping.show_dismissed;
            None
        }
        "order.last" => {
            ordering.order = Order::LastMessage;
            None
        }
        "order.az" => {
            ordering.order = Order::AToZ;
            None
        }
        "order.za" => {
            ordering.order = Order::ZToA;
            None
        }
        "order.unseen" => {
            ordering.unseen_first = !ordering.unseen_first;
            None
        }
        "order.needs" => {
            ordering.needs_first = !ordering.needs_first;
            None
        }
        "order.running" => {
            ordering.running_only = !ordering.running_only;
            None
        }
        "bulk.archive" => Some(Action::OpenBulkArchive(selection(app, p))),
        "bulk.mute" => Some(Action::OpenBulkMute(selection(app, p))),
        "bulk.unmute" => Some(Action::UnmuteMany(selection(app, p))),
        "move.undo" => Some(Action::UndoMove),
        "move.dismiss" => Some(Action::DismissMoveNotice),
        s if s.starts_with("target.retry:") => {
            Some(Action::Retry(TargetId::from(&s["target.retry:".len()..])))
        }
        "section.toggle"
        | "section.drop"
        | "section.createProject"
        | "section.dismissAll"
        | "section.archiveAll"
        | "section.muteAll" => section_action(app, id, p),
        "pullRequest" => {
            let chosen = if name == "agents.action" {
                key(app, string(p, "id"))
            } else {
                selection(app, p).into_iter().next()
            };
            if let Some(k) = chosen
                && let Some(badge) = yaac_core::pr::badge_of(&app.client.state, &k)
            {
                match yaac_core::pr::click(&badge) {
                    yaac_core::pr::BadgeClick::Open(url) => Some(Action::OpenUrl(url)),
                    yaac_core::pr::BadgeClick::Create => Some(Action::OpenCreatePr(k)),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => {
            let Some(&(op, _)) = MENU_ACTIONS.iter().find(|(_, menu_id)| *menu_id == id) else {
                return Err(format!("Unavailable preview menu action {id}"));
            };
            let key = if name == "agents.action" {
                key(app, string(p, "id"))
            } else {
                selection(app, p).into_iter().next()
            };
            if let Some(k) = key
                && thread_menu::thread_menu(&app.client.state, &k, app.now())
                    .iter()
                    .filter_map(|entry| entry.item())
                    .any(|item| item.action == op && item.enabled)
            {
                let mut actions = Vec::new();
                crate::sidebar::run_menu_action(app, ctx, &k, op, &mut actions);
                app.apply(actions);
            }
            None
        }
    };
    if let Some(action) = action {
        app.apply(vec![action]);
    }
    Ok(())
}

fn section_action(app: &mut YaacApp, id: &str, p: &Value) -> Option<Action> {
    let state = &app.client.state;
    let section = sections(app, &AttentionMap::compute(state))
        .into_iter()
        .find(|s| s.id == string(p, "sectionId"))?;
    match id {
        "section.createProject" => {
            let (target, folder) = section.header?.folder?;
            app.ui.create_project = Some(crate::sidebar::create_project_for(app, &target, &folder));
        }
        "section.dismissAll" if section.header.as_ref().is_some_and(|h| h.temporary) => {
            app.apply(
                section
                    .rows
                    .into_iter()
                    .map(|r| Action::Dismiss(r.key, true))
                    .collect(),
            );
        }
        "section.archiveAll" | "section.muteAll"
            if matches!(section.kind, SectionKind::Group | SectionKind::Pinned)
                && !section.rows.is_empty() =>
        {
            let keys = section.rows.into_iter().map(|r| r.key).collect();
            return Some(if id == "section.archiveAll" {
                Action::OpenBulkArchive(keys)
            } else {
                Action::OpenBulkMute(keys)
            });
        }
        "section.toggle" => match section.kind {
            SectionKind::Muted => app.ui.muted_open = !app.ui.muted_open,
            SectionKind::Archived => app.ui.archived_open = !app.ui.archived_open,
            _ => {}
        },
        "section.drop" => {
            let rows = selection(app, p);
            if let Drop::Move {
                keys,
                to,
                label,
                confirm,
            } = moving::drop_on(state, app.ui.grouping, &section, &rows)
            {
                if confirm {
                    return Some(Action::OpenMove(keys, Some(to)));
                }
                let notice = moving::notice(state, &keys, &label);
                let moves = keys.into_iter().map(|k| (k, to.clone())).collect();
                return Some(Action::Move(moves, Some(notice)));
            }
        }
        _ => {}
    }
    None
}
