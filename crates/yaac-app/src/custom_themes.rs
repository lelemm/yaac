//! The same miniature chat is drawn in both Appearance grids and in a tool preview.
use super::{
    app::YaacApp,
    theme::{Palette, ThemeChoice},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui::{self, Color32};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::{Arc, mpsc},
};
use yaac_core::themes::{Base, MAX_BYTES, Theme};
use yaac_protocol::{
    ThemeAction, ThemeCallResult,
    rpc::{ErrorCode, ErrorObject, RpcId},
};

impl Palette {
    pub(crate) fn custom(theme: &Theme) -> Self {
        let mut p = if theme.base == Base::Light {
            Self::LIGHT
        } else {
            Self::DARK
        };
        macro_rules! overrides { ($($field:ident),*) => {$(
            if let Some(rgb) = theme.colors.get(stringify!($field)).and_then(|s| s.strip_prefix('#')).and_then(|s| u32::from_str_radix(s,16).ok()) {
                let [_,r,g,b] = rgb.to_be_bytes(); p.$field = Color32::from_rgb(r,g,b);
            }
        )*}; }
        overrides!(
            bg, panel, raised, faint, border, text, text2, primary, on_primary, selected, needs,
            done, failed, user_bg, fast, ultrafast, shell
        );
        p.running = p.text;
        p.code_bg = p.faint;
        p
    }
    pub(crate) fn wpf_colors(self) -> Value {
        let rgb = |c: Color32| u32::from_be_bytes([0, c.r(), c.g(), c.b()]);
        json!({"dark":self.dark,"YaacCanvas":rgb(self.bg),"YaacPanel":rgb(self.panel),"YaacCard":rgb(self.raised),
        "YaacRaised":rgb(self.raised),"YaacInput":rgb(self.faint),"YaacRule":rgb(self.border),"YaacText":rgb(self.text),
        "YaacMuted":rgb(self.text2),"YaacAccent":rgb(self.primary),"YaacOnAccent":rgb(self.on_primary),"YaacLink":rgb(self.text),
        "YaacSelected":rgb(self.selected),"YaacHover":rgb(self.faint),"YaacWarning":rgb(self.needs),"YaacFailure":rgb(self.failed),
        "YaacSuccess":rgb(self.done),"YaacUser":rgb(self.user_bg),"YaacFast":rgb(self.fast),"YaacUltrafast":rgb(self.ultrafast),"YaacShell":rgb(self.shell)})
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Piece {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: u32,
    text: &'static str,
}
const WIDTH: u32 = 264;
const HEIGHT: u32 = 144;
pub(crate) fn scene(p: &Palette) -> Vec<Piece> {
    let piece = |x, y, w, h, c: Color32, text| Piece {
        x,
        y,
        w,
        h,
        color: u32::from_be_bytes([0, c.r(), c.g(), c.b()]),
        text,
    };
    vec![
        piece(0, 0, WIDTH, HEIGHT, p.bg, ""),
        piece(0, 0, 70, HEIGHT, p.panel, ""),
        piece(70, 0, 1, HEIGHT, p.border, ""),
        piece(5, 8, 0, 12, p.text, "Threads"),
        piece(3, 30, 64, 20, p.selected, ""),
        piece(8, 33, 0, 11, p.text, "Workspace"),
        piece(8, 58, 0, 11, p.text2, "Review"),
        piece(81, 8, 0, 13, p.text, "Workspace"),
        piece(71, 29, 193, 1, p.border, ""),
        piece(119, 39, 136, 23, p.user_bg, ""),
        piece(127, 43, 0, 12, p.text, "Run the tests"),
        piece(81, 77, 5, 5, p.done, ""),
        piece(92, 71, 0, 12, p.text, "All tests passed."),
        piece(80, 111, 175, 24, p.faint, ""),
        piece(88, 116, 0, 11, p.text2, "Message…"),
        piece(211, 114, 40, 18, p.primary, ""),
        piece(217, 116, 0, 11, p.on_primary, "Send"),
    ]
}

fn color(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

pub(crate) fn card(ui: &mut egui::Ui, t: &Theme) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(WIDTH as f32, HEIGHT as f32),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    for piece in scene(&Palette::custom(t)) {
        let pos = rect.min + egui::vec2(piece.x as f32, piece.y as f32);
        if piece.text.is_empty() {
            painter.rect_filled(
                egui::Rect::from_min_size(pos, egui::vec2(piece.w as f32, piece.h as f32)),
                0,
                color(piece.color),
            );
        } else {
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                piece.text,
                egui::FontId::proportional(piece.h as f32),
                color(piece.color),
            );
        }
    }
    response
}

#[derive(Default)]
pub(crate) struct Import {
    file: Option<mpsc::Receiver<Result<Option<Theme>, String>>>,
    previews: Vec<(RpcId, mpsc::Receiver<Result<ThemeCallResult, String>>)>,
    pub(crate) error: Option<String>,
}

impl YaacApp {
    pub(crate) fn import_theme(&mut self) {
        if self.theme_import.file.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.theme_import.file = Some(rx);
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("YAAC theme", &["json"])
                    .pick_file()
                else {
                    return Ok(None);
                };
                let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err("Choose a regular JSON file.".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                Theme::parse(&bytes).map(Some)
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(crate) fn select_theme(&mut self, name: &str) -> Result<(), String> {
        self.ui.theme = match name {
            "dark" => ThemeChoice::Dark,
            "light" => ThemeChoice::Light,
            "system" => ThemeChoice::System,
            _ if self.ui.themes.entries.contains_key(name) => {
                Arc::make_mut(&mut self.ui.themes).selected = name.to_owned();
                ThemeChoice::Custom
            }
            _ => return Err(format!("Unknown theme {name}.")),
        };
        self.ctx.request_repaint();
        Ok(())
    }

    fn theme_result(&self) -> ThemeCallResult {
        let builtins = ["system", "dark", "light"];
        let selected = match self.ui.theme {
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
            ThemeChoice::System => "system",
            ThemeChoice::Custom => &self.ui.themes.selected,
        };
        ThemeCallResult {
            selected: selected.into(),
            themes: builtins
                .into_iter()
                .map(String::from)
                .chain(self.ui.themes.entries.keys().cloned())
                .collect(),
            png: None,
        }
    }

    fn handle_theme(
        &mut self,
        id: &RpcId,
        action: ThemeAction,
    ) -> Result<Option<ThemeCallResult>, String> {
        match action {
            ThemeAction::List(_) => {}
            ThemeAction::Select(p) => self.select_theme(&p.name)?,
            ThemeAction::Import(p) => Arc::make_mut(&mut self.ui.themes)
                .import(Theme::parse(p.json.as_bytes())?, p.overwrite)?,
            ThemeAction::Preview(p) => {
                let palette = match p.name.as_str() {
                    "dark" => Palette::DARK,
                    "light" => Palette::LIGHT,
                    "system" => {
                        if self.ctx.system_theme() == Some(egui::Theme::Light) {
                            Palette::LIGHT
                        } else {
                            Palette::DARK
                        }
                    }
                    name => {
                        Palette::custom(self.ui.themes.entries.get(name).ok_or("Unknown theme.")?)
                    }
                };
                if self.theme_import.previews.len() >= 4 {
                    return Err("Theme preview is busy.".into());
                }
                let (tx, rx) = mpsc::channel();
                self.theme_import.previews.push((id.clone(), rx));
                let ctx = self.ctx.clone();
                let mut result = self.theme_result();
                std::thread::spawn(move || {
                    let image = preview_png(&palette).map(|bytes| {
                        result.png = Some(STANDARD.encode(bytes));
                        result
                    });
                    let _ = tx.send(image);
                    ctx.request_repaint();
                });
                return Ok(None);
            }
            ThemeAction::Unknown(_) => return Err("Unknown theme operation.".into()),
        }
        Ok(Some(self.theme_result()))
    }

    pub(crate) fn theme_upkeep(&mut self) {
        if let Some(result) = self
            .theme_import
            .file
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.theme_import.file = None;
            self.theme_import.error = result
                .and_then(|t| {
                    if let Some(t) = t {
                        let name = t.name.clone();
                        Arc::make_mut(&mut self.ui.themes).import(t, false)?;
                        self.select_theme(&name)?;
                    }
                    Ok(())
                })
                .err();
        }
        let reply = |result: Result<ThemeCallResult, String>| {
            result
                .map(|r| json!(r))
                .map_err(|e| ErrorObject::new(ErrorCode::InvalidParams, e))
        };
        for (id, call) in std::mem::take(&mut self.client.theme_calls) {
            match self.handle_theme(&id, call.action) {
                Ok(None) => {}
                Ok(Some(result)) => {
                    self.persist_prefs();
                    self.client.answer_window_call(id, Ok(json!(result)));
                }
                Err(message) => self.client.answer_window_call(id, reply(Err(message))),
            }
        }
        self.theme_import
            .previews
            .retain(|(id, rx)| match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => true,
                result => {
                    self.client.answer_window_call(
                        id.clone(),
                        reply(result.unwrap_or_else(|_| Err("Preview renderer stopped.".into()))),
                    );
                    false
                }
            });
    }
}

#[derive(Default)]
struct Outline(Vec<zeno::Command>);
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(zeno::Command::MoveTo((x, y).into()));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(zeno::Command::LineTo((x, y).into()));
    }
    fn quad_to(&mut self, x: f32, y: f32, x1: f32, y1: f32) {
        self.0
            .push(zeno::Command::QuadTo((x, y).into(), (x1, y1).into()));
    }
    fn curve_to(&mut self, x: f32, y: f32, x1: f32, y1: f32, x2: f32, y2: f32) {
        self.0.push(zeno::Command::CurveTo(
            (x, y).into(),
            (x1, y1).into(),
            (x2, y2).into(),
        ));
    }
    fn close(&mut self) {
        self.0.push(zeno::Command::Close);
    }
}

fn preview_png(p: &Palette) -> Result<Vec<u8>, String> {
    let face = ttf_parser::Face::parse(
        include_bytes!("../assets/fonts/UbuntuSansNerdFontPropo-Regular.ttf"),
        0,
    )
    .map_err(|e| e.to_string())?;
    let mut image = image::RgbImage::new(WIDTH * 2, HEIGHT * 2);
    for piece in scene(p) {
        let [_, r, g, b] = piece.color.to_be_bytes();
        let ink = [r, g, b];
        if piece.text.is_empty() {
            for y in piece.y * 2..(piece.y + piece.h) * 2 {
                for x in piece.x * 2..(piece.x + piece.w) * 2 {
                    image.put_pixel(x, y, image::Rgb(ink));
                }
            }
            continue;
        }
        let scale = (piece.h * 2) as f32 / f32::from(face.units_per_em());
        let mut pen = (piece.x * 2) as f32;
        for c in piece.text.chars() {
            let Some(id) = face.glyph_index(c) else {
                continue;
            };
            let mut path = Outline::default();
            if face.outline_glyph(id, &mut path).is_some() {
                let transform = zeno::Transform::scale(scale, -scale)
                    .then_translate(pen, ((piece.y + piece.h) * 2) as f32);
                let (mask, pos) = zeno::Mask::new(path.0.as_slice())
                    .transform(Some(transform))
                    .render();
                for y in 0..pos.height {
                    for x in 0..pos.width {
                        let (xx, yy) = (pos.left + x as i32, pos.top + y as i32);
                        if xx < 0 || yy < 0 {
                            continue;
                        }
                        if let Some(pixel) = image.get_pixel_mut_checked(xx as u32, yy as u32) {
                            let alpha = u32::from(mask[(y * pos.width + x) as usize]);
                            for (bg, fg) in pixel.0.iter_mut().zip(ink) {
                                *bg = ((u32::from(*bg) * (255 - alpha) + u32::from(fg) * alpha)
                                    / 255) as u8;
                            }
                        }
                    }
                }
            }
            pen += f32::from(face.glyph_hor_advance(id).unwrap_or(0)) * scale;
        }
    }
    let mut data = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut data, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(data.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_core::prefs::{UiPrefs, keys};
    #[test]
    fn imported_colors_apply_in_both_renderers_and_survive_restart() {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::demo_app(&ctx);
        let action = |v| serde_json::from_value::<ThemeAction>(v).unwrap();
        let id = RpcId::Number(1);
        let theme =
            r##"{"name":"Oasis","base":"light","colors":{"bg":"#cceedd","primary":"#334433"}}"##;
        let import = action(json!({"kind":"import","json":theme,"overwrite":false,"opId":"1"}));
        app.handle_theme(&id, import.clone()).unwrap();
        assert!(app.handle_theme(&id, import).is_err());
        app.handle_theme(
            &id,
            action(json!({"kind":"select","name":"Oasis","opId":"2"})),
        )
        .unwrap();
        crate::app::tests::window_frame(&ctx, &mut app, Default::default());
        let p = app.p;
        assert!(!p.dark);
        assert_eq!(p.bg, color(0xcceedd));
        assert_eq!(p.wpf_colors()["YaacCanvas"], 0xcceedd);
        assert_eq!(crate::theme::visuals(&p).panel_fill, p.bg);
        let mut loaded = UiPrefs::default();
        let saved = app.ui.prefs().value(keys::THEME);
        #[cfg(feature = "hub")]
        let saved = {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cache.sqlite");
            yaac_hub::cache::Cache::open(&path)
                .unwrap()
                .set_ui_state(keys::THEME, &saved)
                .unwrap();
            yaac_hub::cache::Cache::open_read_only(&path)
                .unwrap()
                .ui_state(keys::THEME)
                .unwrap()
                .unwrap()
        };
        loaded.load(keys::THEME, &saved);
        assert_eq!(loaded.theme, ThemeChoice::Custom);
        assert_eq!(loaded.themes, app.ui.themes);
        let updated = theme.replace("cceedd", "123456");
        app.handle_theme(
            &id,
            action(json!({"kind":"import","json":updated,"overwrite":true,"opId":"3"})),
        )
        .unwrap();
        crate::app::tests::window_frame(&ctx, &mut app, Default::default());
        assert_eq!(ctx.global_style().visuals.panel_fill, color(0x123456));
        app.handle_theme(&id, action(json!({"kind":"preview","name":"dark"})))
            .unwrap();
        let (_, rx) = app.theme_import.previews.pop().unwrap();
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap()
            .unwrap();
        assert_eq!(app.theme_result().selected, "Oasis");
        let png = STANDARD.decode(result.png.unwrap()).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (WIDTH * 2, HEIGHT * 2));
        assert_eq!(img.get_pixel(0, 0).0, Palette::DARK.panel.to_array()[..3]);
        assert!(
            img.rows()
                .skip(16)
                .take(24)
                .flatten()
                .take(5000)
                .any(|px| px[0] > 128)
        );
    }
    #[test]
    fn validation_rejects_invalid_theme_files() {
        for s in [
            r##"{"name":" x","colors":{}}"##,
            r##"{"name":"light","colors":{}}"##,
            r##"{"name":"x","colors":{"typo":"#ffffff"}}"##,
            r#"{"name":"x","colors":{"text":"red"}}"#,
            r#"{"name":"x","base":"auto","colors":{}}"#,
        ] {
            assert!(Theme::parse(s.as_bytes()).is_err());
        }
        assert!(Theme::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
}
