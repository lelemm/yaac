//! Colour tokens from the screen brief (docs/design/screens.md), dark and light, and how they
//! become egui's style. Flat: 1 px borders, small radii, no shadows except floating menus.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, Visuals};

/// Theme preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemeChoice {
    Dark,
    Light,
    System,
}

/// The tokens of one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub raised: Color32,
    /// Faint fill: user message, inputs, code, hover.
    pub faint: Color32,
    /// The hairline between regions — the only border.
    pub border: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub unavailable: Color32,
    pub accent: Color32,
    pub selected: Color32,
    pub on_accent: Color32,
    pub link: Color32,
    pub running: Color32,
    pub needs: Color32,
    pub failed: Color32,
    pub done: Color32,
    pub git_added: Color32,
    pub git_modified: Color32,
    pub git_deleted: Color32,
    pub scrim: Color32,
    /// Fill of code blocks.
    pub code_bg: Color32,
    /// The user's message bubble.
    pub user_bg: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

impl Palette {
    pub(crate) const DARK: Palette = Palette {
        dark: true,
        bg: hex(0x16181D),
        panel: hex(0x1B1E24),
        raised: hex(0x22262E),
        faint: hex(0x1E2128),
        border: hex(0x2E333D),
        text: hex(0xE6E8EC),
        text2: hex(0x9AA2AF),
        unavailable: hex(0x8891A0),
        accent: hex(0x6C8CFF),
        selected: hex(0x263052),
        on_accent: hex(0x0E1116),
        link: hex(0x8FA8FF),
        running: hex(0x4C9AFF),
        needs: hex(0xF2B24C),
        failed: hex(0xFF7C87),
        done: hex(0x4CC38A),
        git_added: hex(0x73C991),
        git_modified: hex(0xE2C08D),
        git_deleted: hex(0xF47C7C),
        scrim: Color32::from_black_alpha(140),
        code_bg: hex(0x1E2128),
        user_bg: hex(0x1E2128),
    };

    pub(crate) const LIGHT: Palette = Palette {
        dark: false,
        bg: hex(0xFFFFFF),
        panel: hex(0xF5F6F8),
        raised: hex(0xF7F8FA),
        faint: hex(0xF3F4F7),
        border: hex(0xDADDE3),
        text: hex(0x1A1D23),
        text2: hex(0x5B6472),
        unavailable: hex(0x646E7C),
        accent: hex(0x3B5BDB),
        selected: hex(0xE4EAFD),
        on_accent: hex(0xFFFFFF),
        link: hex(0x2F4AC0),
        running: hex(0x1A66C7),
        needs: hex(0x8F5600),
        failed: hex(0xC92A3A),
        done: hex(0x17754D),
        git_added: hex(0x166F31),
        git_modified: hex(0x855D00),
        git_deleted: hex(0xC4302B),
        scrim: Color32::from_black_alpha(89),
        code_bg: hex(0xF3F4F7),
        user_bg: hex(0xF3F4F7),
    };

    /// A mark's colour.
    pub(crate) fn mark(&self, mark: yaac_core::attention::Mark) -> Color32 {
        use yaac_core::attention::Mark as M;
        match mark {
            M::Running => self.running,
            M::NeedsYou | M::Paused => self.needs,
            M::Failed => self.failed,
            M::Done => self.done,
            M::Waiting | M::Idle | M::Offline | M::Off => self.text2,
        }
    }

    /// A translucent version of a colour, for tag fills.
    pub(crate) fn wash(&self, c: Color32) -> Color32 {
        let a = if self.dark { 46 } else { 30 };
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    }
}

/// Type sizes (points).
pub(crate) mod size {
    pub(crate) const UI: f32 = 13.5;
    pub(crate) const SMALL: f32 = 12.0;
    pub(crate) const PROSE: f32 = 15.0;
    pub(crate) const MONO: f32 = 13.0;
    pub(crate) const TITLE: f32 = 19.0;
    pub(crate) const SECTION: f32 = 14.5;
}

/// The bold family (falls back to the regular face when no bold face is available).
pub(crate) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub(crate) fn ui(size: f32) -> FontId {
    FontId::proportional(size)
}

pub(crate) fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// egui's style from a palette.
pub(crate) fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.raised;
    v.extreme_bg_color = p.raised;
    v.text_edit_bg_color = Some(p.raised);
    v.faint_bg_color = p.panel;
    v.code_bg_color = p.code_bg;
    v.hyperlink_color = p.link;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text2);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 90);
    v.selection.stroke = Stroke::new(2.0, p.accent);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 3], blur: 10, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 36 }) };
    v.striped = false;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, p.bg),
        (&mut v.widgets.inactive, p.faint),
        (&mut v.widgets.hovered, p.faint),
        (&mut v.widgets.active, p.selected),
        (&mut v.widgets.open, p.faint),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = CornerRadius::same(5);
        w.expansion = 0.0;
    }
    // Checkboxes, radios, and number fields need a visible shape: a solid (not outlined) box.
    v.widgets.inactive.bg_fill = if p.dark { hex(0x3A404C) } else { hex(0xC9CED6) };
    v.widgets.hovered.bg_fill = if p.dark { hex(0x454C5A) } else { hex(0xB9BFC9) };
    // The one line egui draws for us: the hairline between regions (panel separators).
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.window_stroke = Stroke::NONE;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    ctx.set_visuals_of(if p.dark { egui::Theme::Dark } else { egui::Theme::Light }, v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.bar_width = 8.0;
        s.text_styles.insert(egui::TextStyle::Body, ui(size::UI));
        s.text_styles.insert(egui::TextStyle::Button, ui(size::UI));
        s.text_styles.insert(egui::TextStyle::Small, ui(size::SMALL));
        s.text_styles.insert(egui::TextStyle::Monospace, mono(size::MONO));
        s.text_styles.insert(egui::TextStyle::Heading, bold(size::TITLE));
        s.interaction.selectable_labels = false;
    });
}

/// Fonts: egui's bundled faces, plus a regular and bold system sans when one is installed in a
/// well-known place (egui bundles no bold face). Nothing is shipped with the binary.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates: &[(&str, &str)] = &[
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
        ("/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"),
        ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf"),
        ("C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\segoeuib.ttf"),
        ("/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ];
    let mut bold_family = Vec::new();
    for (regular, boldf) in candidates {
        if let (Ok(r), Ok(b)) = (std::fs::read(regular), std::fs::read(boldf)) {
            fonts.font_data.insert("sys-regular".into(), std::sync::Arc::new(egui::FontData::from_owned(r)));
            fonts.font_data.insert("sys-bold".into(), std::sync::Arc::new(egui::FontData::from_owned(b)));
            if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
                f.insert(0, "sys-regular".into());
            }
            bold_family.push("sys-bold".to_owned());
            break;
        }
    }
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    bold_family.extend(fallback);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}
