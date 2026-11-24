use super::*;
use yaac_core::attention::Mark;
use yaac_core::bulk::{self, BulkKind, Click};
use yaac_core::moving::{self, Drop};
use yaac_core::sidebar::{Order, RowRight, Section, SectionKind};
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
    crate::sidebar::window_sections(app, map, app.now())
}

fn search_enabled(app: &YaacApp) -> bool {
    !yaac_core::first_run::is_first_run(&app.client.state)
}

fn hub_find(app: &YaacApp) -> Option<&yaac_core::state::FindState> {
    app.client.state.find.as_ref().filter(|f| {
        search_enabled(app) && app.uses_hub_search() && f.query == app.ui.search.trim()
    })
}

pub(super) fn upkeep_search(app: &mut YaacApp) {
    if search_enabled(app) && app.uses_hub_search() {
        app.client.find(&app.ui.search);
        if app.client.state.find.is_some() {
            app.ui.list_sel.clear();
        }
    } else {
        app.client.state.find = None;
    }
}

fn search_rows(app: &YaacApp, map: &AttentionMap, find: &yaac_core::state::FindState) -> Vec<Value> {
    let state = &app.client.state;
    find.results.iter().filter_map(|key| {
        let summary = state.summary(key)?;
        let mark = yaac_core::attention::display(state, map, key).mark;
        Some(json!({
            "id": key.address(), "title": summary.title, "right": state.target_name(&key.target),
            "selected": state.selected_thread() == Some(key), "status": mark.word(), "tone": tone(mark),
        }))
    }).collect()
}

#[cfg(test)]
#[test]
fn wpf_archive_confirmation_sends_every_selected_thread() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let keys = [yaac_core::demo::threads::users(), yaac_core::demo::threads::billing()];
    for key in &keys {
        app.client.state.threads.get_mut(key).unwrap().summary.spawned_by = None;
    }
    let ids = keys.iter().map(ThreadKey::address).collect::<Vec<_>>();
    command(&mut app, &ctx, "sidebar.action", &json!({ "action": "bulk.archive", "ids": ids })).unwrap();
    assert_eq!(app.ui.bulk_archive.as_ref().unwrap().title, "Archive 2 threads?");
    assert!(app.ui.archive.is_none());
    super::dialogs::command(&mut app, "dialog.submit", &json!({ "id": "bulkArchive" })).unwrap();
    let requests: std::collections::BTreeSet<_> = app.demo.as_ref().unwrap().requests().into_iter()
        .filter(|(_, method, _)| method == yaac_protocol::methods::names::THREAD_ARCHIVE)
        .map(|(target, _, params)| (target.unwrap(), params["threadId"].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(requests, keys.into_iter().map(|key| (key.target, key.thread.to_string())).collect());
}

#[cfg(test)]
#[test]
fn wpf_collapsed_groups_keep_header_actions_and_start_drafts_in_their_checkout() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let key = yaac_core::demo::threads::fix();
    let project = app.client.state.project_of(&key).unwrap().id.clone();
    let repo = "/home/dev/yaac/.git";
    let main = "/home/dev/yaac";
    let linked = "/home/dev/yaac-wt/protocol";
    let p = app.client.state.projects.iter_mut().find(|p| p.id == project).unwrap();
    p.repo = Some(yaac_core::projects::RepoLocation {
        repo: repo.into(), relative: "src".into(), root: main.into(), checkout: None,
    });
    p.folder = format!("{main}/src");
    app.client.state.threads.get_mut(&key).unwrap().summary.repo = Some(yaac_protocol::RepoLocation {
        repo: repo.into(), relative: "src".into(), root: linked.into(), worktree_name: Some("protocol".into()),
    });
    app.ui.grouping.worktree = true;
    let group = sidebar::section_of(&app.client.state, app.ui.grouping, &key).unwrap();
    command(&mut app, &ctx, "sidebar.action", &json!({ "action": "section.toggle", "sectionId": group })).unwrap();
    let snapshot = view(&app, &AttentionMap::compute(&app.client.state), 0);
    let section = snapshot["sections"].as_array().unwrap().iter().find(|s| s["id"] == group).unwrap();
    assert_eq!(section["collapsed"], true);
    assert_eq!(section["header"]["toggle"]["id"], "section.toggle");
    assert!(section["header"]["actions"].as_array().unwrap().iter().any(|action| action["id"] == "section.archiveAll"));
    assert!(section["header"]["actions"].as_array().unwrap().iter().any(|action| action["id"] == "section.newThread"));
    app.open(&yaac_core::demo::threads::users(), None);
    command(&mut app, &ctx, "sidebar.action", &json!({ "action": "section.newThread", "sectionId": group })).unwrap();
    let Selection::NewThread(id) = app.client.state.selection else { panic!("expected a new draft") };
    let draft_key = DraftRef::New(id).store_key();
    let draft = &app.client.state.new_threads[0];
    assert_eq!(draft.target.as_ref(), Some(&key.target));
    assert_eq!(draft.project.as_deref(), Some(project.as_str()));
    assert_eq!(draft.cwd.as_ref().unwrap().as_str(), format!("{linked}/src"));
    let composer = super::composer::view(&app, &AttentionMap::compute(&app.client.state), None);
    assert_eq!(composer["worktree"]["label"], "Worktree: protocol");
    for root in [main, linked] {
        super::composer::command(&mut app, "composer.action", &json!({ "key": draft_key, "id": format!("worktree:{root}") })).unwrap();
        assert_eq!(app.client.state.new_threads[0].cwd.as_ref().unwrap().as_str(), format!("{root}/src"));
    }
    app.client.state.new_threads[0].draft.text = "Start from the linked checkout".to_owned();
    super::composer::command(&mut app, "composer.send", &json!({ "key": draft_key })).unwrap();
    let demo = app.demo.as_ref().unwrap();
    let (target, _, params) = demo.requests().into_iter().find(|(_, method, _)| method == "thread/start").unwrap();
    assert_eq!(target.as_ref(), Some(&key.target));
    assert_eq!(params["cwd"], format!("{linked}/src"));
    assert!(params.get("project").is_none());

    let title = app.client.state.summary(&key).unwrap().title.clone();
    command(&mut app, &ctx, "sidebar.search", &json!({ "text": title })).unwrap();
    upkeep_search(&mut app);
    let snapshot = view(&app, &AttentionMap::compute(&app.client.state), 0);
    assert_eq!(snapshot["search"], title);
    assert_eq!(snapshot["searchResults"], false);
    let rail = snapshot["railSections"].as_array().unwrap().iter().find(|s| s["id"] == group).unwrap();
    assert_eq!(rail["collapsed"], true);
    let sections = sections(&app, &AttentionMap::compute(&app.client.state));
    let section = sections.iter().find(|s| s.id == group).unwrap();
    assert!(section.visible_rows().iter().any(|row| row.key == key));
    assert!(app.ui.prefs().collapsed_groups.contains(&group));
    command(&mut app, &ctx, "sidebar.railToggle", &json!({ "sectionId": group })).unwrap();
    assert!(!app.ui.prefs().collapsed_groups.contains(&group));
    assert_eq!(app.ui.search, title);
    app.open(&key, None);
    super::dispatch(&mut app, &ctx, &mut None, "sidebar.new", &Value::Null).unwrap();
    let Selection::NewThread(blank) = app.client.state.selection else { panic!("expected a blank draft") };
    let draft = app.client.state.new_threads.iter().find(|d| d.id == blank).unwrap();
    assert_eq!((&draft.target, &draft.project, &draft.cwd), (&None, &None, &None));
    let composer = super::composer::view(&app, &AttentionMap::compute(&app.client.state), None);
    assert_eq!(composer["project"]["label"], "Choose a project");
    assert!(composer["worktree"].is_null());
    let draft_key = DraftRef::New(blank).store_key();
    super::composer::command(&mut app, "composer.action", &json!({ "key": draft_key, "id": format!("project:{project}") })).unwrap();
    let composer = super::composer::view(&app, &AttentionMap::compute(&app.client.state), None);
    assert_eq!(composer["worktree"]["children"].as_array().unwrap().len(), 2);
}

#[cfg(test)]
#[test]
fn wpf_first_run_disables_sidebar_search() {
    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    app.client.state.threads.clear();
    app.client.state.projects.clear();
    let snapshot = view(&app, &AttentionMap::compute(&app.client.state), 0);
    assert_eq!(snapshot["searchEnabled"], false);
    command(&mut app, &ctx, "sidebar.search", &json!({ "text": "thread" })).unwrap();
    assert!(app.ui.search.is_empty());
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
    let find = hub_find(app);
    let sections = sections(app, map);
    let order: Vec<_> = sections
        .iter()
        .flat_map(|s| s.visible_rows().iter().map(|r| r.key.clone()))
        .collect();
    let picked = app.ui.list_sel.in_order(&order);
    let group = |s: &Section, rail: bool| {
        let header = s.header.as_ref().map(|h| {
            let toggle = (rail || crate::sidebar::can_toggle_section(app, s)).then(|| {
                let label = format!("{} {}", if s.collapsed { "Expand" } else { "Collapse" }, h.left);
                json!({ "id": "section.toggle", "label": label, "detail": label, "enabled": true })
            });
            let target = h.machine.as_ref().and_then(|name| state.targets.iter().find(|t| t.entry.name.as_str() == name)).map(|t| &t.entry.id);
            let retry = h.retry.then(|| target.map(|t| action(&format!("target.retry:{t}"), "Retry", true))).flatten();
            let mut actions = Vec::new();
            if crate::sidebar::new_thread_in_section(app, s).is_some() {
                actions.push(action("section.newThread", "New Thread", true));
            }
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
            let provider = state.provider_of(&r.key);
            let logo = if line2.as_ref().is_some_and(|line| line["logo"].is_string()) {
                None
            } else {
                provider.and_then(|p| crate::logos::for_kind(p.kind.as_str()))
            };
            let color = logo.and_then(|(_, color)| color)
                .map(|c| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()));
            let badge = pull_requests::badge(state, &r.key);
            json!({
                "id": r.key.address(), "copyId": state.summary(&r.key).and_then(|s| s.native_id.as_ref()),
                "title": r.title, "right": right, "status": display.mark.word(),
                "tone": tone(display.mark), "detail": display.attention.map(|a| a.word()),
                "selected": r.selected, "unseen": display.unseen, "readOnly": display.read_only,
                "muted": r.muted, "fast": display.fast, "ultrafast": display.ultrafast, "draggable": moving::can_drag(grouping) && moving::blocked(state, &r.key).is_none(),
                "actions": menu(app, &r.key), "line2": line2, "badge": badge,
                "providerName": provider.map(|p| &p.display_name),
                "logo": logo.map(|(logo, _)| crate::logos::source(logo).0), "logoColor": color,
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
    };
    let groups: Vec<_> = sections.iter().map(|s| group(s, false)).collect();
    let rail_groups = (!app.ui.search.trim().is_empty()).then(|| {
        crate::sidebar::sections_for_query(app, map, now, "").iter()
            .map(|s| group(s, true)).collect::<Vec<_>>()
    });
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
        "search": app.ui.search, "searchEnabled": search_enabled(app), "sections": groups, "selectionReset": selection_epoch.to_string(),
        "railSections": rail_groups,
        "searchResults": find.is_some(), "searchRows": find.map(|f| search_rows(app, map, f)),
        "searchStatus": find.map(|f| {
            if f.loading && f.results.is_empty() { "Searching…".to_owned() } else { format!("{} found", f.total) }
        }),
        "searchError": find.and_then(|f| f.error.as_deref()),
        "searchEmpty": find.is_some_and(|f| f.results.is_empty() && !f.loading && f.error.is_none()),
        "searchMore": find.filter(|f| f.cursor.is_some() && !f.loading).map(|_| action("search.more", "More results", true)),
        "newThread": action("new", "New", !state.targets.is_empty()),
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
        .filter(|section| !section.collapsed)
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
            if search_enabled(app) {
                app.ui.search = string(p, "text").to_owned();
            }
            return Ok(());
        }
        "sidebar.searchMore" => {
            if hub_find(app).is_some_and(|f| !f.loading && f.query == string(p, "query").trim()) {
                app.client.find_more();
            }
            return Ok(());
        }
        "sidebar.railToggle" => {
            let map = AttentionMap::compute(&app.client.state);
            if let Some(section) = crate::sidebar::sections_for_query(app, &map, app.now(), "").into_iter()
                .find(|s| s.id == string(p, "sectionId"))
            {
                crate::sidebar::toggle_section(app, &section);
            }
            return Ok(());
        }
        "sidebar.selection" => {
            app.ui.list_sel.clear();
            if hub_find(app).is_some() {
                return Ok(());
            }
            let rows = selection(app, p);
            for k in &rows {
                app.ui.list_sel.click(&rows, k, Click::Toggle, None);
            }
            return Ok(());
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
        | "section.newThread"
        | "section.createProject"
        | "section.dismissAll"
        | "section.archiveAll"
        | "section.muteAll" => section_action(app, id, p),
        _ => {
            let Some(&(op, _)) = MENU_ACTIONS.iter().find(|(_, menu_id)| *menu_id == id) else {
                return Err(format!("Unavailable preview menu action {id}"));
            };
            let key = if matches!(name, "agents.action" | "sidebar.railAction") {
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
        "section.newThread" => return crate::sidebar::new_thread_in_section(app, &section),
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
        "section.toggle" if crate::sidebar::can_toggle_section(app, &section) => {
            crate::sidebar::toggle_section(app, &section);
        }
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
