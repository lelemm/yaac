//! egui presentation of the controller's currently visible tab.
use crate::{app::YaacApp, preview_controller::Browsers};
use eframe::egui;
use yaac_core::ThreadKey;

#[derive(Default)]
pub(crate) struct Renderer {
    #[cfg(feature = "preview")]
    pages: std::collections::HashMap<u64, yaac_preview::Preview>,
    #[cfg(feature = "preview")]
    favicons: std::collections::HashMap<u64, egui::TextureHandle>,
    pub(crate) focus_address: Option<u64>,
    pub(crate) page_focus: Option<egui::Id>,
    edits: std::collections::HashMap<u64, String>,
    pub(crate) external_address: String,
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
            let active = visible
                .and_then(|k| browsers.areas.get(k))
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
                    p.set_hidden(!agent)
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
        let active = visible.and_then(|k| browsers.areas.get(k)).map(|a| a.active);
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
    fn engine(&mut self, id: u64) -> Option<&mut yaac_preview::Tab> {
        self.pages.get_mut(&id).map(|p| &mut p.tab)
    }

    fn take_replies(&mut self) -> Vec<(u64, serde_json::Value)> {
        self.pages
            .iter_mut()
            .flat_map(|(id, p)| p.responses.drain(..).filter(|v| v.get("id").is_some()).map(|v| (*id, v)))
            .collect()
    }
}

/// Draw the browser and return the chat rectangle (None when full screen).
pub(crate) fn layout(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey) -> Option<egui::Rect> {
    let full = ui.available_rect_before_wrap();
    let Some(area) = app.browsers.areas.get_mut(key) else {
        return Some(full);
    };
    let mut page = full;
    let chat = if area.full_screen {
        None
    } else {
        let x = full.left() + full.width() * area.ratio;
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
        page.min.x = divider.right().min(full.right());
        let mut chat = full;
        chat.max.x = divider.left().max(full.left());
        Some(chat)
    };
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("preview", key))
            .max_rect(page),
        |ui| {
            ui.set_clip_rect(page);
            draw(ui, app, key);
        },
    );
    chat
}

fn draw(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey) {
    let area = app.browsers.areas.get_mut(key).unwrap();
    let mut close = None;
    let mut add = false;
    egui::ScrollArea::horizontal()
        .id_salt("tabs")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for tab in &area.tabs {
                    let label = if tab.title.is_empty() {
                        if tab.url.is_empty() {
                            "New tab"
                        } else {
                            &tab.url
                        }
                    } else {
                        &tab.title
                    };
                    #[cfg(feature = "preview")]
                    if let Some(tex) = app.preview_ui.favicons.get(&tab.id) {
                        ui.image((tex.id(), egui::vec2(16., 16.)));
                    }
                    let r = ui.add(egui::Button::selectable(
                        area.active == tab.id,
                        if label.chars().count() > 24 {
                            format!("{}…", label.chars().take(23).collect::<String>())
                        } else {
                            label.to_owned()
                        },
                    ));
                    if r.clicked() {
                        area.active = tab.id
                    }
                    if r.middle_clicked()
                        || ui.small_button("×").on_hover_text("Close tab").clicked()
                    {
                        close = Some(tab.id)
                    }
                }
                add = ui.small_button("+").on_hover_text("New tab").clicked();
            })
        });
    if let Some(id) = close {
        app.browsers.close(key, id);
        ui.ctx().request_repaint();
        return;
    }
    if add {
        app.preview_ui.focus_address = app.browsers.new_tab(&app.client, key).ok();
        ui.ctx().request_repaint();
        return;
    }
    let id = area.active;
    let Some(tab) = area.tabs.iter_mut().find(|t| t.id == id) else {
        return;
    };
    #[cfg(feature = "preview")]
    let page = app.preview_ui.pages.get(&id);
    let mut navigate = None;
    let mut external = false;
    let address_id = ui.scope_id().with(("address", id));
    let edit = app.preview_ui.edits.entry(id).or_default();
    if !ui.memory(|m| m.has_focus(address_id)) {
        edit.clone_from(&tab.url);
    }
    ui.horizontal_wrapped(|ui| {
        #[cfg(feature = "preview")]
        {
            if ui
                .add_enabled(tab.back, egui::Button::new("←"))
                .on_hover_text("Back")
                .clicked()
                && let Some(p) = page
            {
                p.back()
            }
            if ui
                .add_enabled(tab.forward, egui::Button::new("→"))
                .on_hover_text("Forward")
                .clicked()
                && let Some(p) = page
            {
                p.forward()
            }
            if ui
                .add_enabled(
                    page.is_some(),
                    egui::Button::new(if tab.loading { "Stop" } else { "Reload" }),
                )
                .clicked()
                && let Some(p) = page
            {
                if tab.loading { p.stop() } else { p.reload() }
            }
        }
        let r = ui.add(
            egui::TextEdit::singleline(edit)
                .id(address_id)
                .desired_width(160.)
                .hint_text("URL or port"),
        );
        if app.preview_ui.focus_address == Some(id) {
            r.request_focus();
            app.preview_ui.focus_address = None;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            navigate = Some(edit.clone());
        }
        external = ui
            .add_enabled(!tab.url.is_empty(), egui::Button::new("Open in browser"))
            .clicked();
        #[cfg(feature = "preview")]
        if ui
            .add_enabled(page.is_some(), egui::Button::new("Inspect"))
            .clicked()
            && let Some(p) = page
        {
            p.inspect(None)
        }
        if ui
            .button(if area.full_screen {
                "Restore"
            } else {
                "Full screen"
            })
            .clicked()
        {
            area.full_screen = !area.full_screen;
        }
    });
    if let Some(e) = &tab.error {
        ui.label(e);
    }
    if tab.url.is_empty() {
        if let Some(e) = &area.ports_error {
            ui.label(e);
        }
        ui.label("Listening ports");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for port in &area.ports {
                if ui
                    .button(format!(
                        "{}  {}",
                        port.port,
                        port.command.as_deref().unwrap_or("")
                    ))
                    .clicked()
                {
                    navigate = Some(crate::preview_controller::listener_url(port));
                }
            }
        });
    }
    if let Some(input) = navigate {
        match app.browsers.navigate(&mut app.client, key, id, &input) {
            Ok(()) =>
            {
                #[cfg(feature = "preview")]
                if let Some(p) = page {
                    p.navigate(&app.browsers.tab_mut(key, id).unwrap().url);
                }
            }
            Err(e) => app.browsers.tab_mut(key, id).unwrap().error = Some(e),
        }
    }
    if external && let Err(e) = app.browsers.external(&mut app.client, key, id) {
        app.browsers.tab_mut(key, id).unwrap().error = Some(e);
    }
    #[cfg(feature = "preview")]
    {
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
