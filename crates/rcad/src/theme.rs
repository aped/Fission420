//! Colour theme: the egui visuals for the light and dark themes and a small
//! semantic palette (status text, fills, chips over the 3D view) with one set
//! of values per theme. UI code asks for a role (`theme::of(ui).ok`), never a
//! literal colour, so both themes stay legible. Sketch geometry colours live
//! in [`crate::sketch_view::Palette`] (also one per theme); axis and body
//! colours come from the viewport.

use eframe::egui::{self, Color32, Stroke};

/// Semantic colours for one theme.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Success / closed / fully constrained text on a panel.
    pub ok: Color32,
    /// Informational or under-constrained text.
    pub info: Color32,
    /// Errors and failed checks.
    pub bad: Color32,
    /// Warnings, notes and unsaved state.
    pub warn: Color32,
    /// Stronger caution (missing results, broken references).
    pub caution: Color32,
    /// The item being edited right now.
    pub active: Color32,
    /// Fill of a commit-style button (FINISH SKETCH, OK on a sketch).
    pub go_fill: Color32,
    /// Fill of a destructive or error box on a panel.
    pub danger_fill: Color32,
    /// Background of a chip drawn over the 3D view.
    pub chip_bg: Color32,
    /// Normal text on a chip.
    pub chip_text: Color32,
    /// Hint / secondary text on a chip.
    pub chip_hint: Color32,
    /// Warning text on a chip (busy, notices).
    pub chip_warn: Color32,
    /// Success text on a chip.
    pub chip_ok: Color32,
    /// Informational text on a chip.
    pub chip_info: Color32,
    /// Error banner over the 3D view: fill and body text (heading and
    /// border use [`Palette::chip_bad`]).
    pub banner_bg: Color32,
    pub banner_text: Color32,
    pub chip_bad: Color32,
    /// Translucent tool-body previews in the 3D view: cut (red), join
    /// (green), and a picked region or highlighted target (amber).
    pub cut_fill: Color32,
    pub cut_stroke: Color32,
    pub join_fill: Color32,
    pub join_stroke: Color32,
    pub pick_fill: Color32,
    pub pick_stroke: Color32,
    /// A picked edge or loop drawn over the 3D view.
    pub edge_pick: Color32,
}

pub const DARK: Palette = Palette {
    ok: Color32::from_rgb(120, 220, 120),
    info: Color32::from_rgb(120, 190, 255),
    bad: Color32::from_rgb(255, 110, 110),
    warn: Color32::from_rgb(255, 200, 120),
    caution: Color32::from_rgb(255, 160, 120),
    active: Color32::from_rgb(255, 200, 90),
    go_fill: Color32::from_rgb(40, 90, 50),
    danger_fill: Color32::from_rgb(70, 30, 30),
    chip_bg: Color32::from_black_alpha(185),
    chip_text: Color32::from_gray(225),
    chip_hint: Color32::from_gray(220),
    chip_warn: Color32::from_rgb(255, 210, 120),
    chip_ok: Color32::from_rgb(120, 220, 120),
    chip_info: Color32::from_rgb(120, 190, 255),
    banner_bg: Color32::from_rgba_unmultiplied_const(40, 10, 10, 235),
    banner_text: Color32::from_rgb(240, 205, 205),
    chip_bad: Color32::from_rgb(255, 90, 90),
    cut_fill: Color32::from_rgba_unmultiplied_const(255, 90, 90, 55),
    cut_stroke: Color32::from_rgb(255, 120, 120),
    join_fill: Color32::from_rgba_unmultiplied_const(110, 220, 140, 55),
    join_stroke: Color32::from_rgb(130, 230, 160),
    pick_fill: Color32::from_rgba_unmultiplied_const(255, 200, 60, 90),
    pick_stroke: Color32::from_rgb(255, 170, 60),
    edge_pick: Color32::from_rgb(120, 200, 255),
};

pub const LIGHT: Palette = Palette {
    ok: Color32::from_rgb(26, 127, 55),
    info: Color32::from_rgb(9, 105, 218),
    bad: Color32::from_rgb(207, 34, 46),
    warn: Color32::from_rgb(135, 90, 0),
    caution: Color32::from_rgb(188, 76, 0),
    active: Color32::from_rgb(156, 82, 0),
    go_fill: Color32::from_rgb(178, 228, 188),
    danger_fill: Color32::from_rgb(255, 228, 226),
    chip_bg: Color32::from_rgba_premultiplied(232, 232, 232, 232),
    chip_text: Color32::from_gray(35),
    chip_hint: Color32::from_gray(55),
    chip_warn: Color32::from_rgb(140, 90, 0),
    chip_ok: Color32::from_rgb(26, 127, 55),
    chip_info: Color32::from_rgb(9, 105, 218),
    banner_bg: Color32::from_rgba_unmultiplied_const(255, 236, 234, 240),
    banner_text: Color32::from_rgb(110, 25, 25),
    chip_bad: Color32::from_rgb(196, 30, 40),
    cut_fill: Color32::from_rgba_unmultiplied_const(230, 40, 40, 70),
    cut_stroke: Color32::from_rgb(200, 40, 40),
    join_fill: Color32::from_rgba_unmultiplied_const(20, 140, 60, 70),
    join_stroke: Color32::from_rgb(20, 130, 55),
    pick_fill: Color32::from_rgba_unmultiplied_const(230, 140, 0, 90),
    pick_stroke: Color32::from_rgb(200, 110, 0),
    edge_pick: Color32::from_rgb(10, 100, 210),
};

/// The palette for dark (`true`) or light visuals.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// The palette of the theme `ui` is drawn with.
pub fn of(ui: &egui::Ui) -> &'static Palette {
    palette(ui.visuals().dark_mode)
}

/// The palette of the theme `ctx` currently uses.
pub fn of_ctx(ctx: &egui::Context) -> &'static Palette {
    palette(ctx.theme() == egui::Theme::Dark)
}

/// Whether `ctx` currently draws the dark theme.
pub fn is_dark(ctx: &egui::Context) -> bool {
    ctx.theme() == egui::Theme::Dark
}

/// Tune the colours of both themes (the preference only picks which one is
/// shown, so "Follow system" switches cleanly). Dark keeps egui's dark
/// visuals; light gets higher-contrast text, a soft grey panel behind white
/// fields, and a calmer selection blue. Only colours change: other visuals
/// (cursor blink, rounding, shadows) set by the host stay as they are.
pub fn install(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |s| tune_dark(&mut s.visuals));
    ctx.style_mut_of(egui::Theme::Light, |s| tune_light(&mut s.visuals));
}

fn tune_dark(v: &mut egui::Visuals) {
    v.warn_fg_color = DARK.warn;
    v.error_fg_color = DARK.bad;
}

fn tune_light(v: &mut egui::Visuals) {
    v.panel_fill = Color32::from_gray(243);
    v.window_fill = Color32::from_gray(250);
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_gray(235);
    v.code_bg_color = Color32::from_gray(228);
    v.window_stroke = Stroke::new(1.0, Color32::from_gray(200));
    v.warn_fg_color = LIGHT.warn;
    v.error_fg_color = LIGHT.bad;
    v.hyperlink_color = LIGHT.info;
    v.selection.bg_fill = Color32::from_rgb(196, 222, 250);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0, 80, 160));
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = Color32::from_gray(243);
    w.noninteractive.weak_bg_fill = Color32::from_gray(243);
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_gray(208));
    w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_gray(40));
    w.inactive.weak_bg_fill = Color32::from_gray(226);
    w.inactive.bg_fill = Color32::from_gray(222);
    w.inactive.fg_stroke = Stroke::new(1.0, Color32::from_gray(25));
    w.hovered.weak_bg_fill = Color32::from_gray(212);
    w.hovered.bg_fill = Color32::from_gray(212);
    w.active.weak_bg_fill = Color32::from_gray(190);
    w.active.bg_fill = Color32::from_gray(190);
    w.open.weak_bg_fill = Color32::from_gray(214);
    w.open.bg_fill = Color32::from_gray(214);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: Color32) -> f64 {
        let lin = |u: u8| {
            let s = f64::from(u) / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Status text on the panel background meets WCAG AA for normal text
    /// (4.5:1) in the light theme and stays readable (3:1) in the dark one.
    #[test]
    fn status_text_contrasts_with_the_panel() {
        let mut lv = egui::Visuals::light();
        tune_light(&mut lv);
        let light = lv.panel_fill;
        for c in [
            LIGHT.ok,
            LIGHT.info,
            LIGHT.bad,
            LIGHT.warn,
            LIGHT.caution,
            LIGHT.active,
        ] {
            assert!(contrast(c, light) >= 4.5, "{c:?} on {light:?}");
        }
        let dark = egui::Visuals::dark().panel_fill;
        for c in [
            DARK.ok,
            DARK.info,
            DARK.bad,
            DARK.warn,
            DARK.caution,
            DARK.active,
        ] {
            assert!(contrast(c, dark) >= 3.0, "{c:?} on {dark:?}");
        }
        // Chip text over an opaque chip of either theme.
        assert!(contrast(LIGHT.chip_text, Color32::WHITE) >= 7.0);
        assert!(contrast(LIGHT.chip_warn, Color32::WHITE) >= 4.5);
        assert!(contrast(LIGHT.banner_text, Color32::from_rgb(255, 232, 230)) >= 7.0);
    }
}
