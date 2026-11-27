use crate::{
    BrowserHost, Frame, Tab, Update, paint_patch, virtual_key,
    wire::{Action, Event, Pointer},
};
use egui::{self, Context, Pos2, Rect, Response, TextureHandle, TextureOptions, Ui, ViewportId};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct PageState {
    pub favicon: Vec<u8>,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub error: Option<String>,
}
type Repaint = Arc<Mutex<Option<(Context, ViewportId)>>>;
struct ContextMenu {
    at: [i32; 2],
    items: Vec<(i32, String, bool)>,
}
struct Dialog {
    kind: u32,
    message: String,
    text: String,
}

/// egui adapter for one tab. Call set_hidden(true) when the owner stops drawing it;
/// ui restores visibility. BrowserHost also provides a renderer-neutral tab API.
pub struct Preview {
    pub tab: Tab,
    pub state: PageState,
    pub devtools_title: String,
    repaint: Repaint,
    textures: [Option<TextureHandle>; 2],
    popup: Option<[i32; 4]>,
    bounds: (i32, i32, f32),
    hidden: bool,
    inside: bool,
    buttons: u32,
    last_press: Option<(u8, Pos2, f64, i32)>,
    texture_context: Context,
    cursor: u32,
    cursor_image: Option<egui::CustomCursorImage>,
    caret: [i32; 4],
    menu: Option<ContextMenu>,
    dialog: Option<Dialog>,
    pub responses: std::collections::VecDeque<serde_json::Value>,
    pub external_links: Vec<String>,
    pub host_exit: Option<String>,
}
impl Preview {
    pub fn new(engine: &mut BrowserHost, url: &str, socks: Option<u16>, ctx: &Context) -> Self {
        let tab = engine.open(url, socks, || {});
        Self::from_tab(tab, url, ctx)
    }
    pub fn from_tab(tab: Tab, url: &str, ctx: &Context) -> Self {
        let repaint: Repaint = Arc::new(Mutex::new(Some((ctx.clone(), ctx.viewport_id()))));
        let notify = repaint.clone();
        tab.add_wake(move || {
            let target = notify.lock().unwrap().clone();
            if let Some((ctx, id)) = target {
                ctx.request_repaint_of(id)
            }
        });
        Self {
            tab,
            state: PageState {
                url: url.into(),
                ..Default::default()
            },
            devtools_title: "DevTools".into(),
            repaint,
            textures: Default::default(),
            popup: None,
            bounds: (0, 0, 0.),
            hidden: false,
            inside: false,
            buttons: 0,
            last_press: None,
            texture_context: ctx.clone(),
            cursor: 0,
            cursor_image: None,
            caret: [0, 0, 1, 16],
            menu: None,
            dialog: None,
            responses: Default::default(),
            external_links: Vec::new(),
            host_exit: None,
        }
    }
    pub fn navigate(&self, url: &str) {
        self.tab.send(Action::Navigate(url.into()))
    }
    pub fn back(&self) {
        self.tab.send(Action::Back)
    }
    pub fn forward(&self) {
        self.tab.send(Action::Forward)
    }
    pub fn reload(&self) {
        self.tab.send(Action::Reload)
    }
    pub fn stop(&self) {
        self.tab.send(Action::Stop)
    }
    pub fn inspect(&self, at: Option<[i32; 2]>) {
        self.tab.send(Action::Inspect {
            title: self.devtools_title.clone(),
            at,
        })
    }
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.hidden != hidden {
            self.hidden = hidden;
            self.tab.send(Action::Hidden(hidden));
            if hidden {
                self.repaint.lock().unwrap().take();
                self.buttons = 0;
                self.inside = false;
                self.tab.send(Action::Focus(false));
            }
        }
    }
    /// Drain the tab's updates without drawing it, for a tab an agent drives while the user
    /// looks elsewhere. `ui` does this itself.
    pub fn pump(&mut self, ctx: &Context) {
        self.update(ctx)
    }
    fn update(&mut self, ctx: &Context) {
        while let Some(update) = self.tab.try_update() {
            match update {
                Update::Frame(f) => upload(ctx, &mut self.textures[usize::from(f.popup)], &f),
                Update::HostExited(error) => {
                    self.state.error = Some(error.clone());
                    self.host_exit = Some(error);
                    ctx.request_repaint();
                }
                Update::Event(event) => match event {
                    Event::Favicon(png) => self.state.favicon = png,
                    Event::FileDialog {
                        mode,
                        title,
                        default_path,
                        filters,
                    } => self.tab.choose_files(mode, title, default_path, filters),
                    Event::Url(s) => self.state.url = s,
                    Event::ExternalLink(url) => {
                        self.external_links.push(url);
                        ctx.request_repaint();
                    }
                    Event::Title(s) => self.state.title = s,
                    Event::Loading {
                        busy,
                        back,
                        forward,
                    } => {
                        self.state.loading = busy;
                        self.state.can_go_back = back;
                        self.state.can_go_forward = forward;
                    }
                    Event::CustomCursor {
                        rgba,
                        size,
                        hotspot,
                    } => {
                        self.cursor_image = Some(egui::CustomCursorImage {
                            rgba: rgba.into(),
                            size,
                            hotspot,
                        });
                    }
                    Event::Cursor(c) => {
                        self.cursor = c;
                        self.cursor_image = None;
                    }
                    Event::CompositionCaret(rect) => self.caret = rect,
                    Event::DismissDialog => self.dialog = None,
                    Event::DismissMenu => self.menu = None,
                    Event::Popup(r) => self.popup = r,
                    Event::Menu { at, items } => self.menu = Some(ContextMenu { at, items }),
                    Event::Dialog {
                        kind,
                        message,
                        text,
                    } => {
                        self.dialog = Some(Dialog {
                            kind,
                            message,
                            text,
                        })
                    }
                    Event::Error(e) => self.state.error = Some(e),
                    Event::DevTools(v) => {
                        if self.responses.len() == 1000 {
                            self.responses.pop_front();
                        }
                        self.responses.push_back(v);
                    }
                    Event::Closed => {
                        self.state.error.get_or_insert_with(|| "Tab closed".into());
                    }
                    Event::Paint { .. } | Event::Call(_) => {}
                },
            }
        }
    }
    pub fn ui(&mut self, ui: &mut Ui) -> Response {
        self.set_hidden(false);
        let ctx = ui.ctx().clone();
        *self.repaint.lock().unwrap() = Some((ctx.clone(), ctx.viewport_id()));
        if self.texture_context != ctx {
            self.textures = Default::default();
            self.texture_context = ctx.clone();
            self.tab.send(Action::Invalidate);
        }
        self.update(&ctx);
        let size = ui.available_size().max(egui::vec2(1., 1.));
        let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let bounds = (
            size.x.round() as i32,
            size.y.round() as i32,
            ctx.pixels_per_point(),
        );
        if self.bounds != bounds {
            self.bounds = bounds;
            self.tab.send(Action::Bounds {
                width: bounds.0,
                height: bounds.1,
                scale: bounds.2,
            });
        }
        let painter = ui.painter_at(rect);
        let uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1., 1.));
        if let Some(t) = &self.textures[0] {
            painter.image(t.id(), rect, uv, egui::Color32::WHITE);
        }
        if let (Some(t), Some([x, y, w, h])) = (&self.textures[1], self.popup) {
            painter.image(
                t.id(),
                Rect::from_min_size(
                    rect.min + egui::vec2(x as f32, y as f32),
                    egui::vec2(w as f32, h as f32),
                ),
                uv,
                egui::Color32::WHITE,
            );
        }
        if let Some(e) = &self.state.error {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                e,
                egui::FontId::proportional(14.),
                ui.visuals().text_color(),
            );
        }
        if r.hovered() {
            ctx.set_cursor_image(self.cursor_image.clone());
            use egui::CursorIcon::*;
            ctx.set_cursor_icon(match self.cursor {
                1 => Crosshair,
                2 => PointingHand,
                3 => Text,
                4 => Wait,
                5 => Help,
                6 => ResizeEast,
                7 => ResizeNorth,
                8 => ResizeNorthEast,
                9 => ResizeNorthWest,
                10 => ResizeSouth,
                11 => ResizeSouthEast,
                12 => ResizeSouthWest,
                13 => ResizeWest,
                14 => ResizeVertical,
                15 => ResizeHorizontal,
                16 => ResizeNeSw,
                17 => ResizeNwSe,
                18 => ResizeColumn,
                19 => ResizeRow,
                20..=28 | 43 | 44 => AllScroll,
                29 | 47 => Move,
                30 => VerticalText,
                31 => Cell,
                32 => ContextMenu,
                33 | 49 => Alias,
                34 => Progress,
                35 | 46 => NoDrop,
                36 | 48 => Copy,
                37 => None,
                38 => NotAllowed,
                39 => ZoomIn,
                40 => ZoomOut,
                41 => Grab,
                42 => Grabbing,
                _ => Default,
            });
        }
        if self.dialog.is_none() && self.menu.is_none() {
            self.input(ui, &r);
        }
        if let Some(menu) = &self.menu {
            let at = menu.at;
            let mut open = true;
            let mut selected = None;
            let mut inspect = false;
            egui::Popup::menu(&r)
                .at_position(rect.min + egui::vec2(at[0] as f32, at[1] as f32))
                .open_bool(&mut open)
                .show(|ui| {
                    ui.set_min_width(160.0);
                    for (id, title, enabled) in &menu.items {
                        if ui.add_enabled(*enabled, egui::Button::new(title)).clicked() {
                            selected = Some(*id);
                            ui.close();
                        }
                    }
                    if !menu.items.is_empty() { ui.separator(); }
                    if ui.button("Inspect element").clicked() {
                        inspect = true;
                        ui.close();
                    }
                });
            if !open || inspect || selected.is_some() {
                self.menu = None;
                self.tab.send(Action::Menu(selected));
                if inspect {
                    self.inspect(Some(at))
                }
            }
        }
        if let Some(d) = &mut self.dialog {
            let mut answer = None;
            egui::Window::new("Page dialog")
                .id(r.id.with("dialog"))
                .collapsible(false)
                .show(&ctx, |ui| {
                    ui.label(&d.message);
                    if d.kind == 2 {
                        ui.text_edit_singleline(&mut d.text);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            answer = Some(true)
                        }
                        if d.kind != 0 && ui.button("Cancel").clicked() {
                            answer = Some(false)
                        }
                    });
                });
            if let Some(accept) = answer {
                self.tab.send(Action::Dialog {
                    accept,
                    text: d.text.clone(),
                });
                self.dialog = None;
            }
        }
        r
    }
    fn input(&mut self, ui: &Ui, r: &Response) {
        let rect = r.rect;
        let events = ui.input(|i| i.events.clone());
        let current = ui.input(|i| i.modifiers);
        // egui supplies committed layout text independently of key presses. A printable
        // Ctrl+Alt chord is how Windows keyboards report AltGr in this event stream.
        let layout_text = events
            .iter()
            .any(|e| matches!(e,egui::Event::Text(t) if !t.is_empty()));
        let point = |p: Pos2| {
            (
                (p.x - rect.left()).round() as i32,
                (p.y - rect.top()).round() as i32,
            )
        };
        for event in events {
            use egui::Event as E;
            match event {
                E::PointerMoved(p)
                    if (rect.contains(p) && r.contains_pointer()) || self.buttons != 0 =>
                {
                    self.inside = true;
                    let (x, y) = point(p);
                    self.tab.send(Action::Pointer {
                        x,
                        y,
                        flags: flags(current) | self.buttons,
                        kind: Pointer::Move,
                    })
                }
                E::PointerMoved(_) | E::PointerGone if self.inside => {
                    self.inside = false;
                    self.tab.send(Action::Pointer {
                        x: 0,
                        y: 0,
                        flags: flags(current),
                        kind: Pointer::Leave,
                    });
                }
                E::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } if (rect.contains(pos) && r.contains_pointer())
                    || (!pressed && self.buttons != 0) =>
                {
                    let (index, bit) = match button {
                        egui::PointerButton::Primary => (0, 16),
                        egui::PointerButton::Middle => (1, 32),
                        egui::PointerButton::Secondary => (2, 64),
                        _ => continue,
                    };
                    if pressed {
                        r.request_focus();
                        self.buttons |= bit;
                        let now = ui.input(|i| i.time);
                        let options = ui.ctx().options(|o| o.input_options);
                        let count = self
                            .last_press
                            .filter(|(b, p, t, n)| {
                                *b == index
                                    && *n < 3
                                    && pos.distance(*p) <= options.max_click_dist
                                    && now - t <= options.max_double_click_delay
                            })
                            .map_or(1, |(_, _, _, n)| n + 1);
                        self.last_press = Some((index, pos, now, count));
                    } else {
                        self.buttons &= !bit;
                    }
                    let (x, y) = point(pos);
                    self.tab.send(Action::Pointer {
                        x,
                        y,
                        flags: flags(modifiers) | self.buttons,
                        kind: Pointer::Button {
                            button: index,
                            up: !pressed,
                            count: self.last_press.map_or(1, |(_, _, _, count)| count),
                        },
                    });
                }
                E::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if r.hovered() => {
                    if let Some(p) = r.hover_pos() {
                        let (x, y) = point(p);
                        let unit = match unit {
                            egui::MouseWheelUnit::Point => 1.,
                            egui::MouseWheelUnit::Line => 40.,
                            egui::MouseWheelUnit::Page => rect.height(),
                        };
                        self.tab.send(Action::Pointer {
                            x,
                            y,
                            flags: flags(modifiers) | self.buttons,
                            kind: Pointer::Wheel((delta.x * unit) as i32, (delta.y * unit) as i32),
                        });
                    }
                }
                E::Key {
                    key: egui::Key::F12,
                    pressed: true,
                    ..
                } if r.has_focus() => self.inspect(None),
                E::Key {
                    key,
                    physical_key,
                    repeat,
                    pressed,
                    modifiers,
                    ..
                } if r.has_focus() => {
                    if let Some(vk) =
                        virtual_key(key).or_else(|| physical_key.and_then(virtual_key))
                    {
                        self.tab.send(Action::Key {
                            vk,
                            flags: flags(modifiers)
                                | (u32::from(layout_text && modifiers.ctrl && modifiers.alt) << 12)
                                | (u32::from(repeat) << 13),
                            pressed,
                        });
                        if pressed && key == egui::Key::Enter {
                            self.tab.send(Action::Text("\r".into()))
                        }
                    }
                }
                E::Paste(s) => {
                    if r.has_focus() {
                        self.tab.send(Action::Commit(s))
                    }
                }
                E::Text(s) if r.has_focus() => self.tab.send(Action::Text(s)),
                E::Ime(egui::ImeEvent::Preedit { text, .. }) if r.has_focus() => {
                    self.tab.send(if text.is_empty() {
                        Action::CancelComposition
                    } else {
                        Action::Compose(text)
                    })
                }
                E::Ime(egui::ImeEvent::Commit(s)) if r.has_focus() => {
                    self.tab.send(Action::Commit(s))
                }
                _ => {}
            }
        }
        if r.gained_focus() || r.lost_focus() {
            self.tab.send(Action::Focus(r.has_focus()));
        }
        if r.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    r.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            ui.output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Normal,
                    rect,
                    cursor_rect: Rect::from_min_size(
                        rect.min + egui::vec2(self.caret[0] as f32, self.caret[1] as f32),
                        egui::vec2(self.caret[2].max(1) as f32, self.caret[3].max(1) as f32),
                    ),
                    should_interrupt_composition: false,
                })
            });
        }
    }
}
fn flags(m: egui::Modifiers) -> u32 {
    (u32::from(m.shift) << 1)
        | (u32::from(m.ctrl) << 2)
        | (u32::from(m.alt) << 3)
        | (u32::from(m.mac_cmd) << 7)
}
fn upload(ctx: &Context, texture: &mut Option<TextureHandle>, f: &Frame) {
    if let Some(t) = texture.as_mut().filter(|t| t.size() == f.size) {
        for &d in &f.dirty {
            if let Some(image) = paint_patch(f.bytes(), f.size, d) {
                t.set_partial([d[0], d[1]], image, TextureOptions::LINEAR);
            }
        }
    } else if let Some(image) = paint_patch(f.bytes(), f.size, [0, 0, f.size[0], f.size[1]]) {
        *texture = Some(ctx.load_texture("web-preview", image, TextureOptions::LINEAR));
    }
}
