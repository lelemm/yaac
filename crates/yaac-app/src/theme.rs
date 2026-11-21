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
    /// The one filled button on a screen.
    pub primary: Color32,
    /// Text on the primary button (and on the dangerous-action button).
    pub on_primary: Color32,
    /// Selected row fill, and selected text: a neutral tint.
    pub selected: Color32,
    /// A row in the sidebar's multi-selection (D138): a neutral tint at least as strong as
    /// `selected`, with a bar in the primary text colour at its left edge.
    pub multi: Color32,
    /// The 2 px focus ring.
    pub focus: Color32,
    /// Running: no hue, the primary text colour.
    pub running: Color32,
    pub needs: Color32,
    pub failed: Color32,
    pub done: Color32,
    pub git_added: Color32,
    pub git_modified: Color32,
    pub git_deleted: Color32,
    /// A finished turn nobody has looked at (D114): the one hue beside the failed red and
    /// the git colours (an exception to D83). Drawn as an outline, never as text.
    pub unseen: Color32,
    /// Fast mode (D125): the lightning toggle when on, and the running spinner of a turn in
    /// fast mode. A graphic, never text: 3:1 against every surface it is drawn on.
    pub fast: Color32,
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
        primary: hex(0xE6E8EC),
        on_primary: hex(0x16181D),
        selected: hex(0x2A2E37),
        multi: hex(0x30353F),
        focus: hex(0xC9CED6),
        running: hex(0xE6E8EC),
        needs: hex(0xF2B24C),
        failed: hex(0xFF7C87),
        done: hex(0x4CC38A),
        git_added: hex(0x73C991),
        git_modified: hex(0xE2C08D),
        git_deleted: hex(0xF47C7C),
        unseen: hex(0x6495ED),
        fast: hex(0xF5C518),
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
        primary: hex(0x1A1D23),
        on_primary: hex(0xFFFFFF),
        selected: hex(0xE6E8EC),
        multi: hex(0xE6E8EC),
        focus: hex(0x3A4050),
        running: hex(0x1A1D23),
        needs: hex(0x8F5600),
        failed: hex(0xBF2636),
        done: hex(0x17754D),
        git_added: hex(0x166F31),
        git_modified: hex(0x855D00),
        git_deleted: hex(0xC22E29),
        unseen: hex(0x4A7FE0),
        fast: hex(0x9A7000),
        scrim: Color32::from_black_alpha(89),
        code_bg: hex(0xF3F4F7),
        user_bg: hex(0xF3F4F7),
    };

    /// A mark's colour.
    pub(crate) fn mark(&self, mark: yaac_core::attention::Mark) -> Color32 {
        use yaac_core::attention::Mark as M;
        match mark {
            M::Running => self.running,
            // The needs-you mark is the red exclamation (D113).
            M::NeedsYou => self.failed,
            M::Paused => self.needs,
            M::Failed => self.failed,
            M::Done => self.done,
            M::Waiting | M::Idle | M::Offline | M::Off => self.text2,
        }
    }

    /// Selected text's background: the neutral selected-row tint, a step stronger so a
    /// selection over a faint fill (a user message, code) still reads.
    pub(crate) fn text_selection(&self) -> Color32 {
        if self.dark { hex(0x343945) } else { hex(0xD5D9E0) }
    }

    /// A faint ground for an added (green) or removed (red) diff line.
    pub(crate) fn diff_ground(&self, added: bool) -> Color32 {
        let c = if added { self.git_added } else { self.git_deleted };
        let a = if self.dark { 26 } else { 24 };
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    }

    /// Every token, for tests.
    #[cfg(test)]
    fn all(&self) -> Vec<Color32> {
        vec![
            self.bg, self.panel, self.raised, self.faint, self.border, self.text, self.text2, self.unavailable, self.primary,
            self.on_primary, self.selected, self.multi, self.focus, self.running, self.needs, self.failed, self.done, self.git_added,
            self.git_modified, self.git_deleted, self.unseen, self.fast, self.code_bg, self.user_bg, self.text_selection(),
        ]
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
    let v = visuals(p);
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
        // Text can be selected and copied everywhere (docs/design/screens.md, "Text can be
        // selected and copied"); a drag may run across several labels.
        s.interaction.selectable_labels = true;
        s.interaction.multi_widget_text_select = true;
    });
}

/// egui's visuals from a palette: no accent hue anywhere — selection, links, the text cursor,
/// slider and checkbox fills are all neutrals.
pub(crate) fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.raised;
    v.extreme_bg_color = p.raised;
    v.text_edit_bg_color = Some(p.raised);
    v.faint_bg_color = p.panel;
    v.code_bg_color = p.code_bg;
    v.hyperlink_color = p.text;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text2);
    v.selection.bg_fill = p.text_selection();
    // The colour of selected text: it stays primary.
    v.selection.stroke = Stroke::new(1.0, p.text);
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
    v.text_cursor.stroke = Stroke::new(2.0, p.text);
    v
}

/// Fonts: egui's bundled faces, plus a regular and bold system sans when one is installed in a
/// well-known place (egui bundles no bold face). Nothing is shipped with the binary. On Windows
/// that is Segoe UI from the system font folder, `%WINDIR%\Fonts`, wherever Windows lives.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    use std::path::PathBuf;
    let mut fonts = egui::FontDefinitions::default();
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let Some(windir) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).filter(|_| cfg!(windows)) {
        let dir = PathBuf::from(windir).join("Fonts");
        candidates.push((dir.join("segoeui.ttf"), dir.join("segoeuib.ttf")));
    }
    for (regular, bold) in [
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
        ("/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"),
        ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf"),
        ("/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ] {
        candidates.push((regular.into(), bold.into()));
    }
    let mut bold_family = Vec::new();
    for (regular, boldf) in &candidates {
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

/// WCAG contrast ratio (tests).
#[cfg(test)]
pub(crate) fn contrast(a: Color32, b: Color32) -> f64 {
    tests::contrast(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f64 {
        let f = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    /// `fg` drawn at opacity `a` over `bg`.
    fn blend(fg: Color32, bg: Color32, a: f32) -> Color32 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a channel in 0..=255")]
        let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
        Color32::from_rgb(mix(fg.r(), bg.r()), mix(fg.g(), bg.g()), mix(fg.b(), bg.b()))
    }

    pub(crate) fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The old blue accents, as RGB bytes (written in decimal so this file never contains
    /// their hex spelling, which the source scan below looks for).
    const OLD_BLUES: &[[u8; 3]] = &[
        [108, 140, 255],
        [59, 91, 219],
        [143, 168, 255],
        [47, 74, 192],
        [76, 154, 255],
        [26, 102, 199],
        [38, 48, 82],
        [228, 234, 253],
    ];

    fn is_old_blue(c: Color32) -> bool {
        OLD_BLUES.iter().any(|b| c.r() == b[0] && c.g() == b[1] && c.b() == b[2])
    }

    #[test]
    fn every_text_colour_reads_on_every_surface() {
        for p in [Palette::DARK, Palette::LIGHT] {
            let texts = [
                ("text", p.text),
                ("text2", p.text2),
                ("running", p.running),
                ("needs", p.needs),
                ("failed", p.failed),
                ("done", p.done),
                ("git_added", p.git_added),
                ("git_modified", p.git_modified),
                ("git_deleted", p.git_deleted),
            ];
            for (name, c) in texts {
                for (sname, s) in [("bg", p.bg), ("panel", p.panel), ("raised", p.raised), ("selected", p.selected), ("multi", p.multi)] {
                    let r = contrast(c, s);
                    assert!(r >= 4.5, "{} {name} on {sname}: {r:.2}", if p.dark { "dark" } else { "light" });
                }
            }
            // Unavailable-control text is never used inside rows, so not on the selected tint.
            for s in [p.bg, p.panel, p.raised] {
                assert!(contrast(p.unavailable, s) >= 4.5);
            }
            assert!(contrast(p.on_primary, p.primary) >= 4.5, "primary button label");
            assert!(contrast(p.on_primary, p.failed) >= 4.5 || contrast(Color32::WHITE, p.failed) >= 4.5, "danger button label");
            assert!(contrast(p.text, p.text_selection()) >= 4.5, "selected text");
            // The unseen outline is a graphic, not text: 3:1 against every surface it is drawn on.
            for (sname, s) in [("bg", p.bg), ("panel", p.panel), ("raised", p.raised), ("selected", p.selected), ("faint", p.faint)] {
                let r = contrast(p.unseen, s);
                assert!(r >= 3.0, "{} unseen on {sname}: {r:.2}", if p.dark { "dark" } else { "light" });
            }
            // The fast-mode yellow is a graphic too: 3:1 on every surface, even at the icon
            // opacity (87%) it is drawn with.
            for (sname, s) in [("bg", p.bg), ("panel", p.panel), ("raised", p.raised), ("selected", p.selected), ("faint", p.faint)] {
                let soft = blend(p.fast, s, crate::icons::ICON_OPACITY);
                let r = contrast(soft, s);
                assert!(r >= 3.0, "{} fast on {sname}: {r:.2}", if p.dark { "dark" } else { "light" });
            }
        }
    }

    #[test]
    fn running_links_and_the_primary_button_have_no_hue() {
        for p in [Palette::DARK, Palette::LIGHT] {
            assert_eq!(p.running, p.text);
            assert_eq!(p.primary, p.text);
            let v = visuals(&p);
            assert_eq!(v.hyperlink_color, p.text);
            for c in [p.selected, p.multi, p.focus, p.primary, p.text_selection(), v.selection.bg_fill] {
                let spread = c.r().max(c.g()).max(c.b()) - c.r().min(c.g()).min(c.b());
                assert!(spread <= 24, "{c:?} is not a neutral");
            }
        }
    }

    #[test]
    fn no_old_blue_in_the_tokens_or_egui_visuals() {
        for p in [Palette::DARK, Palette::LIGHT] {
            for c in p.all() {
                assert!(!is_old_blue(c), "{c:?} in the palette");
            }
            let v = visuals(&p);
            let w = &v.widgets;
            let mut cs = vec![v.hyperlink_color, v.selection.bg_fill, v.selection.stroke.color, v.text_cursor.stroke.color, v.panel_fill, v.window_fill, v.extreme_bg_color, v.faint_bg_color, v.code_bg_color];
            for s in [&w.noninteractive, &w.inactive, &w.hovered, &w.active, &w.open] {
                cs.extend([s.bg_fill, s.weak_bg_fill, s.bg_stroke.color, s.fg_stroke.color]);
            }
            for c in cs {
                assert!(!is_old_blue(c), "{c:?} in egui's visuals");
            }
        }
    }

    #[test]
    fn no_old_blue_in_the_source() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut needles = Vec::new();
        for b in OLD_BLUES {
            needles.push(format!("{:02X}{:02X}{:02X}", b[0], b[1], b[2]));
            needles.push(format!("0x{:02X}, 0x{:02X}, 0x{:02X}", b[0], b[1], b[2]));
        }
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap_or_default().to_uppercase();
            for n in &needles {
                assert!(!text.contains(&n.to_uppercase()), "{} contains an old blue ({n})", path.display());
            }
        }
    }
}
