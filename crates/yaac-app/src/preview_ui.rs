//! egui presentation of the controller's currently visible tab.
use crate::{
    app::YaacApp,
    file_pane::TabKind,
    icons,
    preview_controller::Browsers,
    theme::{self, Palette, size},
    widgets,
};
use eframe::egui;
use yaac_core::{ThreadKey, attention::AttentionMap};
use yaac_protocol::GitFileStatus;

#[derive(Default)]
pub(crate) struct Renderer {
    #[cfg(feature = "preview")]
    pages: std::collections::HashMap<u64, yaac_preview::Preview>,
    #[cfg(feature = "preview")]
    favicons: std::collections::HashMap<u64, egui::TextureHandle>,
    pub(crate) focus_address: Option<u64>,
    pub(crate) page_focus: Option<egui::Id>,
    pub(crate) file_focus: Option<egui::Id>,
    edits: std::collections::HashMap<u64, String>,
    pub(crate) external_address: String,
    /// Demo screenshots: a drawn stand-in for pages the build cannot render.
    pub(crate) demo_page: bool,
}

impl Renderer {
    pub(crate) fn sync(&mut self, browsers: &Browsers, visible: Option<&ThreadKey>) {
        let keep = |id: &u64| {
            browsers
                .areas
                .values()
                .any(|a| a.tabs.iter().any(|t| t.id == *id))
        };
        self.edits.retain(|id, _| keep(id));
        #[cfg(feature = "preview")]
        {
            self.favicons.retain(|id, _| keep(id));
            // The full-screen Chat tab hides the pane's tab.
            let active = visible
                .and_then(|k| browsers.areas.get(k))
                .filter(|a| !(a.full_screen && a.chat))
                .map(|a| a.active);
            self.pages.retain(|id, p| {
                if !keep(id) {
                    return false;
                }
                // A tab an agent is driving stays awake so its page renders for
                // screenshots and input, without being shown (docs/11-preview.md#agents).
                let agent = browsers
                    .areas
                    .values()
                    .flat_map(|a| a.tabs.iter())
                    .any(|t| t.id == *id && t.agent.is_some());
                if Some(*id) != active {
                    p.set_hidden(!agent);
                }
                true
            });
        }
        #[cfg(not(feature = "preview"))]
        let _ = visible;
    }
}

#[cfg(feature = "preview")]
impl Renderer {
    /// Start ready tabs that are not drawn (an agent's tab, or a tab of another thread)
    /// hidden, and keep the state of pages that are not drawn current in the controller.
    pub(crate) fn pump(&mut self, browsers: &mut Browsers, visible: Option<&ThreadKey>, ctx: &egui::Context) {
        let active = visible
            .and_then(|k| browsers.areas.get(k))
            .filter(|a| !(a.full_screen && a.chat))
            .map(|a| a.active);
        let tabs: Vec<(ThreadKey, u64, bool)> = browsers
            .areas
            .iter()
            .flat_map(|(k, a)| a.tabs.iter().map(move |t| (k.clone(), t.id, t.agent.is_some())))
            .collect();
        for (key, id, agent) in tabs {
            if !self.pages.contains_key(&id)
                && let Some(handle) = browsers.open_ready(&key, id, || {})
            {
                let url = browsers.tab(&key, id).map(|t| t.url.clone()).unwrap_or_default();
                let mut page = yaac_preview::Preview::from_tab(handle, &url, ctx);
                if Some(id) != active {
                    page.set_hidden(!agent);
                }
                self.pages.insert(id, page);
            }
            if Some(id) == active {
                continue;
            }
            if let (Some(p), Some(tab)) = (self.pages.get_mut(&id), browsers.tab_mut(&key, id)) {
                p.pump(ctx);
                tab.url.clone_from(&p.state.url);
                tab.title.clone_from(&p.state.title);
                tab.loading = p.state.loading;
                tab.back = p.state.can_go_back;
                tab.forward = p.state.can_go_forward;
                if let Some(e) = &p.state.error {
                    tab.error = Some(e.clone());
                }
            }
        }
    }
}

#[cfg(feature = "preview")]
impl crate::preview_agent::Driver for Renderer {
    fn engine(&mut self, id: u64) -> Option<&mut dyn crate::preview_agent::Engine> {
        self.pages.get_mut(&id).map(|p| &mut p.tab as &mut dyn crate::preview_agent::Engine)
    }

    fn take_replies(&mut self) -> Vec<(u64, serde_json::Value)> {
        self.pages
            .iter_mut()
            .flat_map(|(id, p)| p.responses.drain(..).filter(|v| v.get("id").is_some()).map(|v| (*id, v)))
            .collect()
    }
}

/// The tab strip's height (docs/12-files.md#tab-strip).
pub(crate) const STRIP_H: f32 = 36.;
/// A toolbar's height under the strip.
pub(crate) const BAR_H: f32 = 36.;
const TAB_H: f32 = 32.;
const TAB_MIN: f32 = 110.;
const TAB_MAX: f32 = 200.;
/// The chat column's minimum width beside the pane (P314), and the pane's.
pub(crate) const CHAT_MIN: f32 = 360.;
const PANE_MIN: f32 = 280.;

/// Draw the side pane and return the chat rectangle: the left of the split, the area under
/// the strip when the full-screen **Chat** tab is shown, or None when another tab fills it.
pub(crate) fn layout(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey) -> Option<egui::Rect> {
    let full = ui.available_rect_before_wrap();
    let Some(area) = app.browsers.areas.get_mut(key) else {
        return Some(full);
    };
    let full_screen = area.full_screen;
    let (pane, chat) = if full_screen {
        (full, None)
    } else {
        // The chat column never goes below its minimum width (P314); the pane gets the rest.
        let x = (full.left() + full.width() * area.ratio)
            .min(full.right() - PANE_MIN.min(full.width() - CHAT_MIN))
            .max(full.left() + CHAT_MIN.min(full.width()));
        let divider = egui::Rect::from_center_size(
            egui::pos2(x, full.center().y),
            egui::vec2(6., full.height()),
        );
        let drag = ui
            .interact(
                divider,
                ui.scope_id().with(("browser-divider", key)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if drag.dragged()
            && let Some(pos) = drag.interact_pointer_pos()
        {
            area.ratio = ((pos.x - full.left()) / full.width().max(1.)).clamp(0.15, 0.85);
        }
        ui.painter()
            .vline(x, full.y_range(), egui::Stroke::new(1., app.p.border));
        let mut page = full;
        page.min.x = divider.right().min(full.right());
        let mut chat = full;
        chat.max.x = divider.left().max(full.left());
        (page, Some(chat))
    };
    let strip = egui::Rect::from_min_size(pane.min, egui::vec2(pane.width(), STRIP_H));
    ui.scope_builder(
        egui::UiBuilder::new().id_salt(("preview", key)).max_rect(strip),
        |ui| {
            ui.set_clip_rect(strip.expand2(egui::vec2(0., 1.)));
            strip_ui(ui, app, map, key, strip);
        },
    );
    let Some(area) = app.browsers.areas.get(key) else {
        // The last tab was closed: the chat takes the whole width again.
        ui.ctx().request_repaint();
        return Some(full);
    };
    if area.full_screen != full_screen {
        ui.ctx().request_repaint();
    }
    let below = egui::Rect::from_min_max(egui::pos2(pane.left(), strip.bottom()), pane.max);
    if area.full_screen && area.chat {
        return Some(below);
    }
    ui.scope_builder(
        egui::UiBuilder::new().id_salt(("preview-content", key)).max_rect(below),
        |ui| {
            ui.set_clip_rect(below);
            ui.painter().rect_filled(below, 0., app.p.bg);
            draw(ui, app, key);
        },
    );
    chat
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Entry {
    Chat,
    Tab(u64),
}

enum Icon {
    Harness(Option<String>),
    #[cfg(feature = "preview")]
    Favicon(egui::TextureId),
    Globe,
    Spinner,
    Folder,
    File,
    Diff(String, egui::Color32),
    /// A terminal: `>_`.
    Prompt,
}

pub(crate) struct TabInfo {
    entry: Entry,
    title: String,
    hover: String,
    icon: Icon,
    agent: Option<String>,
    /// New output while not shown (terminals).
    dot: bool,
    /// The bell rang: the tab flashes.
    flash: bool,
}

/// The git mark of a worktree path from the loaded tree, if its folder is loaded.
fn git_status(tree: &crate::file_pane::FileTree, path: &str) -> Option<GitFileStatus> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    tree.folders
        .get(dir)?
        .entries
        .iter()
        .find(|e| e.name == name)?
        .git
        .clone()
}

pub(crate) fn git_color(p: &Palette, g: &GitFileStatus) -> egui::Color32 {
    match g {
        GitFileStatus::Added | GitFileStatus::Untracked => p.git_added,
        GitFileStatus::Deleted | GitFileStatus::Conflict => p.git_deleted,
        _ => p.git_modified,
    }
}

fn tab_info(app: &YaacApp, area: &crate::preview_controller::BrowserArea, tab: &crate::preview_controller::BrowserTab) -> TabInfo {
    if let TabKind::Terminal(sid) = tab.kind {
        return terminal_info(app, Entry::Tab(tab.id), sid);
    }
    let title = if !tab.title.is_empty() {
        tab.title.clone()
    } else if !tab.url.is_empty() {
        tab.url.clone()
    } else {
        "New tab".to_owned()
    };
    let hover = tab
        .kind
        .path()
        .map(str::to_owned)
        .or_else(|| (!tab.url.is_empty()).then(|| format!("{title}\n{}", tab.url)))
        .unwrap_or_else(|| title.clone());
    let icon = match &tab.kind {
        _ if tab.loading => Icon::Spinner,
        TabKind::Browser => {
            #[cfg(feature = "preview")]
            if let Some(tex) = app.preview_ui.favicons.get(&tab.id) {
                return TabInfo { entry: Entry::Tab(tab.id), title, hover, icon: Icon::Favicon(tex.id()), agent: tab.agent.clone(), dot: false, flash: false };
            }
            Icon::Globe
        }
        TabKind::Files => Icon::Folder,
        TabKind::File(_) => Icon::File,
        TabKind::Diff(path) => {
            let g = git_status(&area.tree, path).unwrap_or(GitFileStatus::Modified);
            Icon::Diff(g.as_str().to_owned(), git_color(&app.p, &g))
        }
        TabKind::Terminal(_) => Icon::Prompt,
    };
    let _ = app;
    TabInfo { entry: Entry::Tab(tab.id), title, hover, icon, agent: tab.agent.clone(), dot: false, flash: false }
}

/// A terminal session's tab: the program's title or the shell's name; the folder on hover.
pub(crate) fn terminal_info(app: &YaacApp, entry: Entry, sid: u64) -> TabInfo {
    let now = std::time::Instant::now();
    let (title, hover, dot, flash) = match app.browsers.terms.get(sid) {
        Some(s) => {
            let title = s.title();
            let state = match &s.state {
                crate::terminal::SessionState::Exited(code) => format!("\nExited with code {code}"),
                crate::terminal::SessionState::Failed(e) => format!("\n{e}"),
                crate::terminal::SessionState::Reconnecting => "\nReconnecting…".to_owned(),
                _ => String::new(),
            };
            (title.clone(), format!("{title}\n{} in {}{state}", s.shell_name(), s.cwd), s.unseen, s.bell(now))
        }
        None => ("Terminal".to_owned(), "Terminal".to_owned(), false, false),
    };
    TabInfo { entry, title, hover, icon: Icon::Prompt, agent: None, dot, flash }
}

/// Points of a rectangle's left, top and right edges with rounded top corners, open at the
/// bottom (the active tab joins the page under it).
fn tab_outline(r: egui::Rect, radius: f32) -> Vec<egui::Pos2> {
    let mut pts = vec![r.left_bottom()];
    let arc = |c: egui::Pos2, from: f32, pts: &mut Vec<egui::Pos2>| {
        for i in 0..=6 {
            let a = from + i as f32 / 6. * std::f32::consts::FRAC_PI_2;
            pts.push(c + radius * egui::vec2(a.cos(), a.sin()));
        }
    };
    arc(egui::pos2(r.left() + radius, r.top() + radius), std::f32::consts::PI, &mut pts);
    arc(egui::pos2(r.right() - radius, r.top() + radius), -std::f32::consts::FRAC_PI_2, &mut pts);
    pts.push(r.right_bottom());
    pts
}

fn draw_icon(painter: &egui::Painter, c: egui::Pos2, icon: &Icon, ink: egui::Color32, p: &Palette) {
    match icon {
        Icon::Harness(kind) => crate::agents_ui::harness_mark(painter, c, 16., kind.as_deref(), None, false, p),
        #[cfg(feature = "preview")]
        Icon::Favicon(tex) => {
            painter.image(
                *tex,
                egui::Rect::from_center_size(c, egui::Vec2::splat(16.)),
                egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                egui::Color32::WHITE,
            );
        }
        Icon::Globe => icons::globe(painter, c, 16., ink),
        Icon::Spinner => icons::spinner(painter, c, 16., ink),
        Icon::Folder => icons::folder(painter, c, 17., ink),
        Icon::File => icons::file(painter, c, 17., ink, false),
        Icon::Diff(letter, col) => icons::git_letter(painter, c, 16., *col, letter),
        Icon::Prompt => icons::prompt(painter, c, 16., ink),
    }
}

/// The Chat tab's corner mark: the sidebar's attention marks (needs you, running, new).
fn chat_mark(painter: &egui::Painter, c: egui::Pos2, app: &YaacApp, map: &AttentionMap, key: &ThreadKey, under: egui::Color32) {
    let p = &app.p;
    let d = yaac_core::attention::display(&app.client.state, map, key);
    painter.circle_filled(c, 5.5, under);
    match d.mark {
        yaac_core::attention::Mark::NeedsYou | yaac_core::attention::Mark::Paused => icons::mark(painter, c, 10., d.mark, p),
        yaac_core::attention::Mark::Running => icons::thread_mark(painter, c, 10., &d, p),
        _ => {
            painter.circle_filled(c, 3.5, p.unseen);
        }
    }
}

/// The pane's + menu rows (here and in the title bar): Browser and Files with their icons,
/// unavailable ones disabled with the reason as hover text. Returns Some(true) for Files.
/// What the pane's **+** menu chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneChoice {
    Browser,
    Files,
    /// A terminal with this shell (`None`: the default).
    Terminal(Option<yaac_protocol::TerminalShell>),
}

/// One row of the + menu: icon, label, hover reason when unavailable.
fn pane_row(ui: &mut egui::Ui, p: &Palette, label: &str, reason: Option<&str>, chevron: bool, icon: impl FnOnce(&egui::Painter, egui::Pos2, egui::Color32)) -> egui::Response {
    let enabled = reason.is_none();
    let (r, rect) = widgets::menu_row(ui, p, false, enabled);
    let ink = if enabled { p.text } else { p.unavailable };
    icon(ui.painter(), egui::pos2(rect.left() + 18., rect.center().y), ink);
    let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), theme::ui(size::UI), ink));
    ui.painter().galley(egui::pos2(rect.left() + 34., rect.center().y - g.size().y / 2.), g, ink);
    if chevron {
        icons::chevron(ui.painter(), egui::pos2(rect.right() - 14., rect.center().y), 14., false, p.text2);
    }
    match reason {
        Some(reason) => r.on_hover_text(reason),
        None => r,
    }
}

/// The side pane's **+** menu: Browser, Files, Terminal (the default shell; the other shells in
/// its submenu).
pub(crate) fn pane_menu(ui: &mut egui::Ui, app: &mut YaacApp, key: Option<&ThreadKey>) -> Option<PaneChoice> {
    let p = app.p;
    let browser = app.browsers.reason(&app.client, key);
    let files = match key {
        Some(k) if crate::file_pane::supported(&app.client, k) => None,
        Some(_) => Some("The target is offline or does not offer its files".to_owned()),
        None => Some("Open a thread first".to_owned()),
    };
    let terminal = match key {
        Some(k) => crate::terminal::unavailable(&app.client, k),
        None => Some("Open a thread first".to_owned()),
    };
    // As wide as its rows need, not as wide as the space it opens in.
    ui.set_min_width(200.);
    ui.set_max_width(220.);
    let mut pick = None;
    if pane_row(ui, &p, "Browser", browser.as_deref(), false, |pa, c, ink| icons::globe(pa, c, 16., ink)).clicked() {
        pick = Some(PaneChoice::Browser);
    }
    if pane_row(ui, &p, "Files", files.as_deref(), false, |pa, c, ink| icons::folder(pa, c, 17., ink)).clicked() {
        pick = Some(PaneChoice::Files);
    }
    let shells = match (key, &terminal) {
        (Some(k), None) => match app.browsers.terms.shells(&mut app.client, &k.target) {
            crate::terminal::Shells::Ready(list) => list.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let r = pane_row(ui, &p, "Terminal", terminal.as_deref(), shells.len() > 1, |pa, c, ink| icons::prompt(pa, c, 16., ink));
    if r.clicked() {
        pick = Some(PaneChoice::Terminal(None));
    }
    if terminal.is_none() && shells.len() > 1 {
        egui::containers::menu::SubMenu::new().show(ui, &r, |ui| {
            ui.set_min_width(220.);
            ui.set_max_width(260.);
            for (i, shell) in shells.iter().enumerate() {
                let label = if i == 0 { format!("{} (default)", shell.name) } else { shell.name.clone() };
                if pane_row(ui, &p, &label, None, false, |pa, c, ink| icons::prompt(pa, c, 16., ink)).on_hover_text(&shell.path).clicked() {
                    pick = Some(PaneChoice::Terminal(Some(shell.clone())));
                    ui.close();
                }
            }
        });
    }
    if pick.is_some() {
        ui.close();
    }
    pick
}

/// Act on the + menu's choice for a thread.
pub(crate) fn add_pane_tab(app: &mut YaacApp, key: &ThreadKey, choice: PaneChoice) {
    match choice {
        PaneChoice::Files => {
            let _ = app.browsers.open_file_tab(&mut app.client, key, TabKind::Files, false);
        }
        PaneChoice::Browser => app.preview_ui.focus_address = app.browsers.new_tab(&app.client, key).ok(),
        PaneChoice::Terminal(shell) => {
            if let Err(e) = app.browsers.open_terminal(&app.client, key, shell) {
                app.browsers.notice = Some(e);
            }
        }
    }
}

enum StripAction {
    Select(Entry),
    Close(u64),
    Add(PaneChoice),
    FullScreen(bool),
}

/// The tab strip: equal-width tabs that shrink together and then scroll, + after the last
/// tab, Full screen / Restore at the right end (docs/12-files.md#tab-strip).
fn strip_ui(ui: &mut egui::Ui, app: &mut YaacApp, map: &AttentionMap, key: &ThreadKey, strip: egui::Rect) {
    let p = app.p;
    let Some(area) = app.browsers.areas.get(key) else {
        return;
    };
    let full = area.full_screen;
    let mut infos = Vec::new();
    if full {
        let title = app
            .client
            .state
            .summary(key)
            .map(|s| s.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Chat".to_owned());
        let kind = app.client.state.provider_of(key).map(|d| d.kind.as_str().to_owned());
        infos.push(TabInfo { entry: Entry::Chat, hover: format!("{title}\nThe chat"), title, icon: Icon::Harness(kind), agent: None, dot: false, flash: false });
    }
    infos.extend(area.tabs.iter().map(|t| tab_info(app, area, t)));
    let current = if full && area.chat { Entry::Chat } else { Entry::Tab(area.active) };
    let marked = full && app.browsers.chat_marked(&app.client, map, key);
    let hover = if full { "Restore" } else { "Full screen" };
    let right: [StripButton<'_>; 1] = [(hover, &move |pa: &egui::Painter, c, d, col| icons::maximize(pa, c, d, col, full), false)];
    // A terminal tab being renamed in place (P343): its entry here, and the edit itself.
    let terminal_tabs: Vec<(u64, u64)> = area.tabs.iter().filter_map(|t| if let TabKind::Terminal(s) = t.kind { Some((t.id, s)) } else { None }).collect();
    let mut renaming = app.browsers.terms.renaming.take();
    let rename_entry = renaming.as_ref().and_then(|r| terminal_tabs.iter().find(|(_, s)| *s == r.session)).map(|(t, _)| Entry::Tab(*t));
    let chat_mark_fn = |painter: &egui::Painter, c: egui::Pos2, under: egui::Color32| chat_mark(painter, c, app, map, key, under);
    let out = tab_strip(
        ui,
        &p,
        strip,
        &infos,
        current,
        marked.then_some(&chat_mark_fn as &dyn Fn(&egui::Painter, egui::Pos2, egui::Color32)),
        &right,
        "New tab",
        rename_entry.zip(renaming.as_mut()),
    );
    app.browsers.terms.renaming = renaming;
    let session_of = |e: Entry| match e {
        Entry::Tab(t) => terminal_tabs.iter().find(|(tab, _)| *tab == t).map(|(_, s)| *s),
        Entry::Chat => None,
    };
    if let Some(sid) = out.rename.and_then(session_of) {
        app.browsers.terms.begin_rename(sid);
    }
    match out.rename_done {
        Some(true) => app.browsers.terms.commit_rename(&mut app.client),
        Some(false) => app.browsers.terms.cancel_rename(),
        None => {}
    }
    // A terminal tab's right-click menu: Rename, Close.
    if let Some((entry, resp)) = &out.context
        && let Some(sid) = session_of(*entry)
    {
        let mut pick = None;
        widgets::menu_popup(egui::Popup::context_menu(resp), &p).show(|ui| {
            ui.set_min_width(180.);
            for (label, n) in [("Rename", 0), ("Close tab", 1)] {
                let (row, rr) = widgets::menu_row(ui, &p, false, true);
                let g = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), theme::ui(size::UI), p.text));
                ui.painter().galley(egui::pos2(rr.left() + 12., rr.center().y - g.size().y / 2.), g, p.text);
                if row.clicked() {
                    pick = Some(n);
                    ui.close();
                }
            }
        });
        match (pick, entry) {
            (Some(0), _) => app.browsers.terms.begin_rename(sid),
            (Some(1), Entry::Tab(t)) => app.browsers.close(key, *t),
            _ => {}
        }
    }
    let mut action = None;
    if out.right == Some(0) {
        action = Some(StripAction::FullScreen(!full));
    }
    if let Some(id) = out.close {
        action = Some(StripAction::Close(id));
    } else if let Some(e) = out.select {
        action = Some(StripAction::Select(e));
    }
    if let Some(Some(choice)) = widgets::menu_popup(egui::Popup::menu(&out.plus), &p)
        .show(|ui| pane_menu(ui, app, Some(key)))
        .map(|r| r.inner)
    {
        action = Some(StripAction::Add(choice));
    }

    let Some(action) = action else { return };
    match action {
        StripAction::Select(Entry::Chat) => app.browsers.select_chat(key),
        StripAction::Select(Entry::Tab(id)) => app.browsers.select(key, id),
        StripAction::Close(id) => app.browsers.close(key, id),
        StripAction::FullScreen(on) => app.browsers.set_full_screen(key, on),
        StripAction::Add(choice) => add_pane_tab(app, key, choice),
    }
    ui.ctx().request_repaint();
}

/// A button at the strip's right end: hover text, icon, selected.
/// The Chat tab's attention mark painter.
pub(crate) type ChatMark<'a> = &'a dyn Fn(&egui::Painter, egui::Pos2, egui::Color32);

pub(crate) type StripButton<'a> = (&'a str, &'a dyn Fn(&egui::Painter, egui::Pos2, f32, egui::Color32), bool);

/// What happened on a tab strip this frame.
pub(crate) struct StripOut {
    pub(crate) select: Option<Entry>,
    pub(crate) close: Option<u64>,
    /// A right-end button was clicked (its index).
    pub(crate) right: Option<usize>,
    /// A tab was right-clicked: its entry and response (for a context menu).
    pub(crate) context: Option<(Entry, egui::Response)>,
    /// The **+** button, for its menu or action.
    pub(crate) plus: egui::Response,
    /// Double click or F2 on a tab: rename it in place (P343).
    pub(crate) rename: Option<Entry>,
    /// The in-place rename ended: kept (Enter) or cancelled (Esc, click away).
    pub(crate) rename_done: Option<bool>,
}

/// The tab strip shared by the side pane and the terminal panel: equal-width tabs that shrink
/// together and then scroll, + after the last tab, buttons at the right end
/// (docs/12-files.md#tab-strip).
#[allow(clippy::too_many_arguments)]
pub(crate) fn tab_strip(
    ui: &mut egui::Ui,
    p: &Palette,
    strip: egui::Rect,
    infos: &[TabInfo],
    current: Entry,
    chat_mark: Option<ChatMark<'_>>,
    right: &[StripButton<'_>],
    plus_hover: &str,
    mut renaming: Option<(Entry, &mut crate::terminal::Rename)>,
) -> StripOut {
    let p = *p;
    ui.painter().rect_filled(strip, 0., p.panel);
    let rule_y = ui.painter().round_to_pixel_center(strip.bottom() - 0.5);
    ui.painter()
        .hline(strip.x_range(), rule_y, egui::Stroke::new(1., p.border));
    let mut out_select = None;
    let mut out_close = None;
    let mut out_right = None;
    let mut out_context = None;
    let mut out_rename = None;
    let mut out_rename_done = None;

    // Buttons at the right end, always visible.
    let mut right_edge = strip.right() - 8.;
    for (i, (hover, icon, on)) in right.iter().enumerate() {
        let r = egui::Rect::from_center_size(egui::pos2(right_edge - 14., strip.center().y), egui::Vec2::splat(28.));
        if ui.put(r, |ui: &mut egui::Ui| widgets::tool_button(ui, &p, p.panel, hover, true, *on, |pa, c, d, col| icon(pa, c, d, col))).clicked() {
            out_right = Some(i);
        }
        right_edge = r.left() - 2.;
    }
    let fs = egui::Rect::from_min_max(egui::pos2(right_edge, strip.top()), egui::pos2(right_edge + 1., strip.bottom()));
    let left = strip.left() + 6.;
    let room = (fs.left() - 8. - 28. - 4. - left).max(TAB_MIN);
    let n = infos.len().max(1) as f32;
    let font = theme::ui(size::UI);
    // The active tab never loses its title: it widens (up to the preferred width) when the
    // others shrink.
    let active_i = infos.iter().position(|t| t.entry == current);
    let active_need = active_i.map_or(0., |i| {
        let title = ui.fonts_mut(|f| f.layout_no_wrap(infos[i].title.clone(), font.clone(), p.text)).size().x;
        (title + 34. + 30. + 2.).min(TAB_MAX)
    });
    let w = (room / n).clamp(TAB_MIN, TAB_MAX).floor();
    let w = if active_need > w && n > 1. { ((room - active_need) / (n - 1.)).clamp(TAB_MIN, TAB_MAX).floor() } else { w };
    let widths: Vec<f32> = (0..infos.len()).map(|i| if Some(i) == active_i { active_need.max(w) } else { w }).collect();
    let starts: Vec<f32> = widths.iter().scan(0., |x, w| { let s = *x; *x += w; Some(s) }).collect();
    let total: f32 = widths.iter().sum();
    let view_w = total.min(room);
    let view = egui::Rect::from_min_max(egui::pos2(left, strip.top()), egui::pos2(left + view_w, strip.bottom()));
    let sid = ui.scope_id().with("tab-scroll");
    let (mut offset, seen): (f32, Option<Entry>) = ui.data(|d| d.get_temp(sid)).unwrap_or((0., None));
    let max_off = (total - view_w).max(0.);
    if max_off > 0. && ui.rect_contains_pointer(view) {
        // The wheel scrolls the strip sideways.
        let delta = ui.input(|i| i.smooth_scroll_delta);
        if delta != egui::Vec2::ZERO {
            offset -= delta.x + delta.y;
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }
    if seen != Some(current)
        && let Some(i) = active_i
    {
        // The active tab is kept in view.
        let (a, b) = (starts[i], starts[i] + widths[i]);
        if a < offset {
            offset = a;
        } else if b > offset + view_w {
            offset = b - view_w;
        }
    }
    offset = offset.clamp(0., max_off);
    ui.data_mut(|d| d.insert_temp(sid, (offset, Some(current))));

    // Interactions first, then paint (inactive tabs, then the active one on top).
    let mut drawn = Vec::new();
    for (i, info) in infos.iter().enumerate() {
        let x = view.left() + starts[i] - offset;
        let w = widths[i];
        let rect = egui::Rect::from_min_max(egui::pos2(x, strip.bottom() - TAB_H), egui::pos2(x + w, strip.bottom()));
        if rect.right() < view.left() || rect.left() > view.right() {
            continue;
        }
        let hit = rect.intersect(view);
        let id = ui.scope_id().with(("tab", info.entry_id()));
        let resp = ui.interact(hit, id, egui::Sense::click());
        let active = info.entry == current;
        let closable = info.entry != Entry::Chat;
        let close_room = closable;
        let close_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 17., rect.center().y), egui::Vec2::splat(20.));
        let close = close_room.then(|| {
            ui.interact(close_rect.intersect(view), id.with("close"), egui::Sense::click())
                .on_hover_text("Close tab (middle-click)")
        });
        let hovered = resp.hovered() || close.as_ref().is_some_and(|c| c.hovered());
        if close.as_ref().is_some_and(|c| c.clicked()) || (closable && resp.middle_clicked()) {
            if let Entry::Tab(id) = info.entry {
                out_close = Some(id);
            }
        } else if resp.clicked() {
            out_select = Some(info.entry);
        }
        if resp.secondary_clicked() || resp.context_menu_opened() {
            out_context = Some((info.entry, resp.clone()));
        }
        if resp.double_clicked() || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::F2))) {
            out_rename = Some(info.entry);
        }
        let focused = resp.has_focus();
        let resp = resp.on_hover_text(&info.hover);
        drawn.push((info, rect, active, hovered, focused, close.filter(|_| active || hovered)));
        let _ = resp;
    }
    let painter = ui.painter_at(egui::Rect::from_min_max(view.min, egui::pos2(view.right(), strip.bottom() + 1.)));
    let hover_fill = widgets::hover_fill(&p, p.panel);
    for (i, (info, rect, active, hovered, focused, close)) in drawn.iter().enumerate() {
        let under = if *active {
            p.bg
        } else if *hovered {
            hover_fill
        } else {
            p.panel
        };
        if *active {
            let r = egui::Rect::from_min_max(
                egui::pos2(painter.round_to_pixel_center(rect.left()), painter.round_to_pixel_center(rect.top())),
                egui::pos2(painter.round_to_pixel_center(rect.right()), strip.bottom() + 1.),
            );
            painter.rect_filled(r, egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, p.bg);
            painter.add(egui::Shape::line(tab_outline(r, 7.), egui::Stroke::new(1., p.border)));
        } else if *hovered {
            painter.rect_filled(rect.shrink2(egui::vec2(2., 0.)).with_max_y(rect.bottom() - 3.), 6., hover_fill);
        } else {
            // A short divider between two resting tabs.
            let next = drawn.get(i + 1);
            if next.is_some_and(|n| !n.2 && !n.3) {
                let x = painter.round_to_pixel_center(rect.right());
                painter.vline(x, (rect.center().y - 8.)..=(rect.center().y + 7.), egui::Stroke::new(1., p.border));
            }
        }
        if info.flash {
            painter.rect_filled(rect.shrink2(egui::vec2(2., 0.)).with_max_y(rect.bottom() - 3.), 6., p.wash(p.needs));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let c = egui::pos2(rect.left() + 18., rect.center().y - 1.);
        let ink = if *active || *hovered { p.text } else { p.text2 };
        draw_icon(&painter, c, &info.icon, ink, &p);
        if info.dot && !*active {
            painter.circle_filled(c + egui::vec2(7., -6.), 4., under);
            painter.circle_filled(c + egui::vec2(7., -6.), 2.8, p.unseen);
        }
        if let Some(kind) = &info.agent {
            // An agent drives this tab: its harness mark on the icon's corner.
            let corner = c + egui::vec2(6., 6.);
            painter.circle_filled(corner, 6., under);
            crate::agents_ui::harness_mark(&painter, corner, 10., Some(kind.as_str()).filter(|k| !k.is_empty()), None, false, &p);
        }
        if info.entry == Entry::Chat
            && let Some(mark) = chat_mark
        {
            mark(&painter, c + egui::vec2(6., -6.), under);
        }
        let text_left = rect.left() + 34.;
        // Room for the × only where it shows (the active tab, a hovered one).
        let text_right = if close.is_some() { rect.right() - 30. } else { rect.right() - 10. };
        if let Some((_, r)) = renaming.as_mut().filter(|(e, _)| *e == info.entry) {
            // The title becomes a text field in place, all of it selected at first.
            let field = egui::Rect::from_min_max(egui::pos2(text_left - 4., rect.top() + 4.), egui::pos2(text_right.max(text_left + 40.), rect.bottom() - 4.));
            let id = ui.scope_id().with(("tab-rename", info.entry_id()));
            let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            let resp = ui.put(field, egui::TextEdit::singleline(&mut r.text).id(id).font(font.clone()).margin(egui::Margin::symmetric(4, 2)));
            if r.fresh {
                r.fresh = false;
                resp.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let n = r.text.chars().count();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                    state.store(ui.ctx(), id);
                }
            } else if resp.lost_focus() {
                out_rename_done = Some(enter && !esc);
            }
        } else if text_right > text_left + 8. {
            let g = widgets::elided(ui, &info.title, &font, ink, text_right - text_left);
            painter.galley(egui::pos2(text_left, rect.center().y - g.size().y / 2. - 1.), g, ink);
        }
        if let Some(close) = close {
            let r = close.rect;
            if close.hovered() {
                painter.rect_filled(r, 4., widgets::hover_fill(&p, under));
            }
            icons::close(&painter, r.center(), 14., if close.hovered() { p.text } else { p.text2 });
        }
        if *focused {
            painter.rect_stroke(rect.shrink(3.), 5., egui::Stroke::new(2., p.focus), egui::StrokeKind::Inside);
        }
    }

    // Soft edges where the strip scrolls: more tabs that way.
    let fade = |x0: f32, x1: f32, left_solid: bool| {
        let mut mesh = egui::Mesh::default();
        let (a, b) = if left_solid { (p.panel, egui::Color32::TRANSPARENT) } else { (egui::Color32::TRANSPARENT, p.panel) };
        let r = egui::Rect::from_min_max(egui::pos2(x0, strip.top()), egui::pos2(x1, strip.bottom() - 1.));
        mesh.colored_vertex(r.left_top(), a);
        mesh.colored_vertex(r.right_top(), b);
        mesh.colored_vertex(r.right_bottom(), b);
        mesh.colored_vertex(r.left_bottom(), a);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
    };
    if offset > 0.5 {
        fade(view.left(), view.left() + 24., true);
    }
    if offset < max_off - 0.5 {
        fade(view.right() - 24., view.right(), false);
    }

    // + right after the last tab.
    let plus = egui::Rect::from_center_size(egui::pos2(view.right() + 4. + 14., strip.center().y), egui::Vec2::splat(28.));
    let plus = ui.put(plus, |ui: &mut egui::Ui| {
        widgets::tool_button(ui, &p, p.panel, plus_hover, true, false, icons::plus)
    });
    StripOut { select: out_select, close: out_close, right: out_right, context: out_context, plus, rename: out_rename, rename_done: out_rename_done }
}

impl TabInfo {
    fn entry_id(&self) -> u64 {
        match self.entry {
            Entry::Chat => 0,
            Entry::Tab(id) => id,
        }
    }
}

/// A toolbar row under the strip, on the content's background (so the active tab joins it)
/// with a rule under it; one line, never wrapping. Returns what `add` returned and the rest.
pub(crate) fn toolbar<R>(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, egui::Rect) {
    let all = ui.max_rect();
    let bar = egui::Rect::from_min_size(all.min, egui::vec2(all.width(), BAR_H));
    ui.painter().rect_filled(bar, 0., p.bg);
    let y = ui.painter().round_to_pixel_center(bar.bottom() - 0.5);
    ui.painter().hline(bar.x_range(), y, egui::Stroke::new(1., p.border));
    let inner = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(egui::vec2(6., 0.)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.;
                add(ui)
            },
        )
        .inner;
    (inner, egui::Rect::from_min_max(egui::pos2(all.left(), bar.bottom()), all.max))
}

/// A rounded address field: a leading globe (the spinner while loading), the URL, a 2 px
/// progress line along its bottom while loading. Focusing it selects the whole text.
#[allow(clippy::too_many_arguments)]
fn address_field(ui: &mut egui::Ui, p: &Palette, edit: &mut String, id: egui::Id, size: egui::Vec2, font: egui::FontId, hint: &str, loading: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let radius = size.y / 2.;
    let focused = ui.memory(|m| m.has_focus(id));
    let fill = if focused { p.bg } else if p.dark { p.raised } else { widgets::blend(p.bg, p.selected, 0.55) };
    ui.painter().rect_filled(rect, radius, fill);
    if focused {
        ui.painter().rect_stroke(rect, radius, egui::Stroke::new(2., p.focus), egui::StrokeKind::Inside);
    }
    let ic = egui::pos2(rect.left() + radius.max(14.), rect.center().y);
    if loading {
        icons::spinner(ui.painter(), ic, 15., p.text2);
    } else {
        icons::globe(ui.painter(), ic, 15., p.text2);
    }
    let text_h = font.size + 6.;
    let inner = egui::Rect::from_min_max(
        egui::pos2(ic.x + 14., rect.center().y - text_h / 2.),
        egui::pos2(rect.right() - radius.max(10.), rect.center().y + text_h / 2.),
    );
    let r = ui.put(
        inner,
        egui::TextEdit::singleline(edit)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(font)
            .hint_text(widgets::secondary(hint, p))
            .align(egui::Align2::LEFT_CENTER)
            .desired_width(inner.width()),
    );
    if r.gained_focus()
        && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id)
    {
        let end = edit.chars().count();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(end),
        )));
        state.store(ui.ctx(), id);
    }
    if loading {
        // An indeterminate 2 px progress line in the primary colour.
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.left() + radius, rect.bottom() - 2.),
            egui::pos2(rect.right() - radius, rect.bottom()),
        );
        let t = ui.input(|i| i.time);
        #[allow(clippy::cast_possible_truncation, reason = "a phase in 0..1")]
        let phase = ((t / 1.4).fract()) as f32;
        let seg = track.width() * 0.3;
        let x0 = track.left() - seg + (track.width() + seg) * phase;
        let bar = egui::Rect::from_min_max(egui::pos2(x0.max(track.left()), track.top()), egui::pos2((x0 + seg).min(track.right()), track.bottom()));
        if bar.width() > 0. {
            ui.painter().rect_filled(bar, 1., p.primary);
        }
        // The leading spinner already asks for the next frame while it is drawn.
    }
    r
}

fn draw(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey) {
    let p = app.p;
    let area = app.browsers.areas.get_mut(key).unwrap();
    let id = area.active;
    let Some(tab) = area.tabs.iter_mut().find(|t| t.id == id) else {
        return;
    };
    if let TabKind::Terminal(sid) = tab.kind {
        ui.push_id(id, |ui| crate::terminal_ui::tab(ui, app, sid));
        return;
    }
    if tab.kind != TabKind::Browser {
        ui.push_id(id, |ui| crate::file_pane_ui::draw(ui, app, key, id));
        return;
    }
    #[cfg(feature = "preview")]
    let page = app.preview_ui.pages.get(&id);
    #[cfg(not(feature = "preview"))]
    let page: Option<&()> = None;
    let mut navigate = None;
    let mut external = false;
    let address_id = ui.scope_id().with(("address", id));
    let big_id = ui.scope_id().with(("address-big", id));
    let edit = app.preview_ui.edits.entry(id).or_default();
    let editing = ui.memory(|m| m.has_focus(address_id) || m.has_focus(big_id));
    if !editing {
        edit.clone_from(&tab.url);
    }
    let new_tab = tab.url.is_empty();
    // Shortcuts while the page or its address field has focus.
    let ours = editing || app.preview_ui.page_focus.is_some_and(|f| ui.memory(|m| m.has_focus(f)));
    let (mut back, mut forward, mut reload, mut inspect) = (false, false, false, false);
    if ours {
        ui.input_mut(|i| {
            back = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft);
            forward = i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight);
            reload = i.consume_key(egui::Modifiers::NONE, egui::Key::F5) | i.consume_key(egui::Modifiers::COMMAND, egui::Key::R);
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::L) {
                app.preview_ui.focus_address = Some(id);
            }
        });
    }
    let (address, rest) = toolbar(ui, &p, |ui| {
        let has_page = page.is_some() || app.preview_ui.demo_page;
        back |= widgets::tool_button(ui, &p, p.bg, "Back (Alt+Left)", tab.back && has_page, false, |pa, c, d, col| icons::nav(pa, c, d, false, col)).clicked();
        forward |= widgets::tool_button(ui, &p, p.bg, "Forward (Alt+Right)", tab.forward && has_page, false, |pa, c, d, col| icons::nav(pa, c, d, true, col)).clicked();
        let stop = tab.loading;
        reload |= widgets::tool_button(ui, &p, p.bg, if stop { "Stop" } else { "Reload (F5)" }, has_page, false, |pa, c, d, col| {
            if stop {
                icons::stop_square(pa, c, d * 0.95, col);
            } else {
                icons::reload(pa, c, d, col);
            }
        })
        .clicked();
        ui.add_space(4.);
        let w = (ui.available_width() - 2. * 28. - 2. * 2. - 6.).max(40.);
        // On the New tab page the hint is in the large field only.
        let hint = if new_tab { "" } else { "Enter a URL or a port" };
        let r = address_field(ui, &p, edit, address_id, egui::vec2(w, 28.), theme::ui(size::UI), hint, tab.loading);
        ui.add_space(4.);
        external = widgets::tool_button(ui, &p, p.bg, "Open in browser", !tab.url.is_empty(), false, icons::open_external).clicked();
        inspect |= widgets::tool_button(ui, &p, p.bg, "Inspect (F12)", has_page, false, icons::code).clicked();
        r
    });
    if app.preview_ui.focus_address == Some(id) && !new_tab {
        address.request_focus();
        app.preview_ui.focus_address = None;
    }
    if address.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        navigate = Some(edit.clone());
    }
    #[cfg(feature = "preview")]
    if let Some(pg) = page {
        if back {
            pg.back();
        }
        if forward {
            pg.forward();
        }
        if reload {
            if tab.loading { pg.stop() } else { pg.reload() }
        }
        if inspect {
            pg.inspect(None);
        }
    }
    #[cfg(not(feature = "preview"))]
    let _ = (back, forward, reload, inspect);
    let mut rest = rest;
    // An error: one line under the toolbar, with a close ×.
    if tab.error.is_some() {
        let line = egui::Rect::from_min_size(rest.min, egui::vec2(rest.width(), 30.));
        rest.min.y = line.bottom();
        let mut dismiss = false;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(line.shrink2(egui::vec2(10., 0.))).layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.painter().rect_filled(line, 0., p.wash(p.failed));
                widgets::icon(ui, 14., |pa, c, d| icons::warning(pa, c, d, p.failed, p.bg));
                let text = tab.error.as_deref().unwrap_or_default();
                let g = widgets::elided(ui, text, &theme::ui(size::SMALL), p.failed, ui.available_width() - 34.);
                ui.add(egui::Label::new(g).selectable(false)).on_hover_text(text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dismiss = widgets::icon_button(ui, &p, 12., "Dismiss", icons::close).clicked();
                });
            },
        );
        if dismiss {
            tab.error = None;
        }
    }
    if new_tab {
        let big = egui::Rect::from_min_max(rest.min, rest.max);
        if let Some(input) = new_tab_page(ui, app, key, id, big, big_id) {
            navigate = Some(input);
        }
    }
    if let Some(input) = navigate {
        match app.browsers.navigate(&mut app.client, key, id, &input) {
            Ok(()) =>
            {
                #[cfg(feature = "preview")]
                if let Some(p) = app.preview_ui.pages.get(&id) {
                    p.navigate(&app.browsers.tab_mut(key, id).unwrap().url);
                }
            }
            Err(e) => app.browsers.tab_mut(key, id).unwrap().error = Some(e),
        }
        ui.ctx().request_repaint();
    }
    if external && let Err(e) = app.browsers.external(&mut app.client, key, id) {
        app.browsers.tab_mut(key, id).unwrap().error = Some(e);
    }
    #[cfg(feature = "preview")]
    if !new_tab && !app.preview_ui.demo_page {
        ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| page_ui(ui, app, key, id));
    }
    if !new_tab && app.preview_ui.demo_page {
        demo_page(ui, rest);
    }
}

/// The demo's stand-in page (the same as the WPF renderer's screenshots).
fn demo_page(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    let ink = egui::Color32::from_rgb(0x27, 0x33, 0x45);
    painter.rect_filled(rect, 0., egui::Color32::from_rgb(0xFC, 0xFC, 0xFD));
    painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 38.)), 0., egui::Color32::from_rgb(0xEE, 0xF1, 0xF5));
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y);
    painter.text(at(16., 19.), egui::Align2::LEFT_CENTER, "Project documentation", egui::FontId::proportional(12.), ink);
    painter.text(at(22., 86.), egui::Align2::LEFT_CENTER, "Authentication", theme::bold(20.), ink);
    painter.text(at(22., 117.), egui::Align2::LEFT_CENTER, "Development environment", egui::FontId::proportional(11.), ink);
    painter.hline((rect.left() + 22.)..=(rect.right() - 22.), rect.top() + 146., egui::Stroke::new(1., egui::Color32::from_rgb(0xD5, 0xDA, 0xE0)));
    painter.text(at(22., 180.), egui::Align2::LEFT_CENTER, "Getting started", egui::FontId::proportional(15.), ink);
    painter.text(at(22., 208.), egui::Align2::LEFT_CENTER, "Run the project and open the local preview.", egui::FontId::proportional(11.), ink);
}

/// The New tab page: a large address field, then the servers listening on the target.
fn new_tab_page(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, id: u64, rect: egui::Rect, big_id: egui::Id) -> Option<String> {
    let p = app.p;
    let width = 520_f32.min(rect.width() - 32.).max(120.);
    let left = rect.center().x - width / 2.;
    let mut y = rect.top() + rect.height() * 0.15;
    let mut navigate = None;
    let machine = app.client.state.target_name(&key.target);
    let folder = app
        .client
        .state
        .project_of(key)
        .map(|p| p.folder.as_str().to_owned())
        .or_else(|| app.client.state.summary(key).map(|s| s.cwd.as_str().to_owned()));
    let column = |ui: &mut egui::Ui, y: &mut f32, h: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
        let r = egui::Rect::from_min_size(egui::pos2(left, *y), egui::vec2(width, h));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| add(ui));
        *y += h;
    };
    let edit = app.preview_ui.edits.entry(id).or_default();
    let mut field = None;
    column(ui, &mut y, 36., &mut |ui| {
        field = Some(address_field(ui, &p, edit, big_id, egui::vec2(width, 36.), theme::ui(size::PROSE), "Enter a URL or a port", false));
    });
    let field = field.unwrap();
    if app.preview_ui.focus_address == Some(id) {
        field.request_focus();
        app.preview_ui.focus_address = None;
    }
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        navigate = Some(edit.clone());
    }
    y += 28.;
    let area = app.browsers.areas.get(key).unwrap();
    column(ui, &mut y, 22., &mut |ui| {
        ui.add(egui::Label::new(egui::RichText::new(format!("Servers on {machine}")).font(theme::bold(size::SMALL)).color(p.text2)).selectable(false));
        if !area.ports_discovered {
            ui.add_space(6.);
            widgets::icon(ui, 12., |pa, c, d| icons::spinner(pa, c, d, p.text2));
        }
    });
    y += 4.;
    if let Some(e) = &area.ports_error {
        column(ui, &mut y, 22., &mut |ui| {
            ui.add(egui::Label::new(egui::RichText::new(e).font(theme::ui(size::SMALL)).color(p.failed)).truncate());
        });
    }
    if area.ports.is_empty() && area.ports_discovered {
        y += 6.;
        column(ui, &mut y, 22., &mut |ui| {
            ui.add(egui::Label::new(format!("No servers are listening on {machine}.")).selectable(false));
        });
        column(ui, &mut y, 20., &mut |ui| {
            ui.add(egui::Label::new(widgets::secondary("Start your dev server — it shows up here.", &p)).selectable(false));
        });
    }
    let rows_rect = egui::Rect::from_min_max(egui::pos2(left - 8., y), egui::pos2(left + width + 8., rect.bottom() - 8.));
    let ports = area.ports.clone();
    ui.scope_builder(egui::UiBuilder::new().max_rect(rows_rect), |ui| {
        egui::ScrollArea::vertical().id_salt(("ports", id)).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            for port in &ports {
                let url = crate::preview_controller::listener_url(port);
                let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.), egui::Sense::click());
                let resp = resp.on_hover_text(format!("Open {url}"));
                if resp.hovered() || resp.has_focus() {
                    ui.painter().rect_filled(r, 6., widgets::hover_fill(&p, p.bg));
                }
                if resp.has_focus() {
                    ui.painter().rect_stroke(r, 6., egui::Stroke::new(2., p.focus), egui::StrokeKind::Inside);
                }
                let ink = if resp.hovered() { p.text } else { p.text2 };
                icons::globe(ui.painter(), egui::pos2(r.left() + 18., r.center().y), 16., ink);
                let host = url.trim_start_matches("http://").trim_end_matches('/').to_owned();
                let g = ui.fonts_mut(|f| f.layout_no_wrap(host, theme::mono(size::MONO), p.text));
                let mut x = r.left() + 36.;
                let gw = g.size().x;
                ui.painter().galley(egui::pos2(x, r.center().y - g.size().y / 2.), g, p.text);
                x += gw + 12.;
                let cwd = port.cwd.as_deref().map(|cwd| relative(folder.as_deref(), cwd));
                let detail = match (port.command.as_deref(), cwd) {
                    (Some(c), Some(d)) => format!("{c}  ·  {d}"),
                    (Some(c), None) => c.to_owned(),
                    (None, Some(d)) => d,
                    (None, None) => String::new(),
                };
                let room = r.right() - 30. - x;
                if room > 20. && !detail.is_empty() {
                    let g = widgets::elided(ui, &detail, &theme::ui(size::SMALL), p.text2, room);
                    ui.painter().galley(egui::pos2(x, r.center().y - g.size().y / 2.), g, p.text2);
                }
                icons::chevron(ui.painter(), egui::pos2(r.right() - 16., r.center().y), 12., false, ink);
                if resp.clicked() {
                    navigate = Some(url);
                }
            }
        });
    });
    navigate
}

/// A server's folder relative to the project when inside it.
fn relative(root: Option<&str>, cwd: &str) -> String {
    match root.filter(|root| yaac_core::projects::contains(root, cwd)) {
        Some(root) => {
            let rest = cwd[root.len().min(cwd.len())..].trim_start_matches(['/', '\\']);
            if rest.is_empty() { "project root".to_owned() } else { rest.to_owned() }
        }
        None => cwd.to_owned(),
    }
}

#[cfg(feature = "preview")]
fn page_ui(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey, id: u64) {
    if let Some(handle) = app.browsers.open_ready(key, id, || {}) {
        let tab = app.browsers.tab_mut(key, id).unwrap();
        app.preview_ui.pages.insert(
            id,
            yaac_preview::Preview::from_tab(handle, &tab.url, ui.ctx()),
        );
    }
    if let Some(p) = app.preview_ui.pages.get_mut(&id) {
        p.devtools_title = format!(
            "DevTools — {} · {}",
            p.state.title,
            app.client.state.target_name(&key.target)
        );
        let frame = p.ui(ui);
        app.preview_ui.page_focus = Some(frame.id);
        let png = std::mem::take(&mut p.state.favicon);
        if !png.is_empty()
            && let Some(image) = favicon(&png)
        {
            app.preview_ui.favicons.insert(
                id,
                ui.ctx()
                    .load_texture(format!("favicon-{id}"), image, Default::default()),
            );
            ui.ctx().request_repaint();
        }
        let tab = app.browsers.tab_mut(key, id).unwrap();
        if tab.url != p.state.url
            || tab.title != p.state.title
            || tab.loading != p.state.loading
            || tab.back != p.state.can_go_back
            || tab.forward != p.state.can_go_forward
            || p.state
                .error
                .as_ref()
                .is_some_and(|error| tab.error.as_ref() != Some(error))
        {
            ui.ctx().request_repaint();
        }
        tab.url.clone_from(&p.state.url);
        tab.title.clone_from(&p.state.title);
        tab.loading = p.state.loading;
        tab.back = p.state.can_go_back;
        tab.forward = p.state.can_go_forward;
        if let Some(e) = &p.state.error {
            tab.error = Some(e.clone());
            app.browsers.host_error = Some(e.clone());
        }
    }
}

#[cfg(feature = "preview")]
fn favicon(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    limits.max_alloc = Some(1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?.thumbnail(24, 24).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    ))
}
