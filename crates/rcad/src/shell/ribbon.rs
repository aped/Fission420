//! Application bar (File ▾, quick-access document buttons, Export/View
//! menus, Help, Preferences) and the ribbon (workspace tabs, groups with
//! promoted icon buttons and a dropdown per group; a contextual SKETCH tab
//! with FINISH SKETCH at its right end while a sketch is open). Everything is
//! drawn from the command registry; each icon button's accessible name and
//! probe key is its command label.

use eframe::egui::{
    self, Color32, Response, RichText, Sense, Ui, Vec2, Widget, WidgetInfo, WidgetType, vec2,
};

use super::icons;
use super::registry::{self, CmdId, Group, Icon, Place, Tab};
use crate::app::RcadApp;
use crate::probe::Probe as _;

const BTN_H: f32 = 36.0;
const BTN_MIN_W: f32 = 40.0;
const BTN_MAX_W: f32 = 66.0;
const ICON: f32 = 18.0;

/// Short visible captions for long labels (the accessible name stays the
/// full label).
fn caption(label: &str) -> &str {
    match label {
        "Sketch on offset plane…" => "Offset Plane",
        "Cut through all…" => "Cut All",
        "Create Sketch" => "Sketch",
        "Clear Selection" => "Clear",
        "Section view…" => "Section",
        "Physical Material" => "Material",
        "Export STL…" => "STL",
        "Export 3MF…" => "3MF",
        "New Component" => "Component",
        l => l.trim_end_matches('…'),
    }
}

/// Icon-over-caption ribbon button (or a small icon-only app-bar button).
pub struct IconButton<'a> {
    pub icon: Option<Icon>,
    pub caption: &'a str,
    pub name: &'a str,
    pub small: bool,
    pub dropdown: bool,
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = egui::FontId::proportional(if self.small { 13.0 } else { 10.0 });
        let galley = ui.painter().layout_no_wrap(
            self.caption.to_string(),
            font.clone(),
            Color32::PLACEHOLDER,
        );
        let size = if self.small {
            let w = if self.icon.is_some() {
                26.0
            } else {
                galley.size().x + 14.0
            };
            vec2(w, 24.0)
        } else {
            vec2((galley.size().x + 10.0).clamp(BTN_MIN_W, BTN_MAX_W), BTN_H)
        };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        let name = self.name.to_string();
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &name));
        if ui.is_rect_visible(rect) {
            let vis = ui.style().interact(&resp);
            if resp.hovered() || resp.has_focus() {
                ui.painter().rect_filled(rect, 3.0, vis.weak_bg_fill);
            }
            let color = if enabled {
                vis.fg_stroke.color
            } else {
                ui.visuals().weak_text_color().gamma_multiply(0.7)
            };
            match (self.icon, self.small) {
                (Some(icon), true) => {
                    let r = egui::Rect::from_center_size(rect.center(), Vec2::splat(18.0));
                    icons::paint(icon, ui.painter(), r, color);
                }
                (Some(icon), false) => {
                    let r = egui::Rect::from_center_size(
                        egui::pos2(rect.center().x, rect.min.y + 2.0 + ICON / 2.0),
                        Vec2::splat(ICON),
                    );
                    icons::paint(icon, ui.painter(), r, color);
                    let mut text = self.caption.to_string();
                    if self.dropdown {
                        text.push_str(" ⏷");
                    }
                    let job = egui::text::LayoutJob {
                        wrap: egui::text::TextWrapping::truncate_at_width(rect.width() - 2.0),
                        ..egui::text::LayoutJob::simple_singleline(text, font, color)
                    };
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    ui.painter().galley(
                        egui::pos2(
                            rect.center().x - g.size().x / 2.0,
                            rect.max.y - g.size().y - 1.0,
                        ),
                        g,
                        color,
                    );
                }
                (None, _) => {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        self.caption,
                        font,
                        color,
                    );
                }
            }
        }
        resp
    }
}

/// A group title drawn vertically beside the group's buttons; clicking it
/// opens the group dropdown. Accessible name: the group label.
pub struct GroupTitle<'a>(pub &'a str);

impl Widget for GroupTitle<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(15.0, BTN_H), Sense::click());
        let name = self.0.to_string();
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
        if ui.is_rect_visible(rect) {
            let vis = ui.style().interact(&resp);
            if resp.hovered() {
                ui.painter().rect_filled(rect, 2.0, vis.weak_bg_fill);
            }
            let color = ui.visuals().weak_text_color();
            let galley = ui.painter().layout_no_wrap(
                format!("{} ⏷", self.0),
                egui::FontId::proportional(8.5),
                color,
            );
            // Rotated a quarter turn: reads bottom to top.
            let w = galley.size().x.min(BTN_H);
            let pos = egui::pos2(
                rect.center().x - galley.size().y / 2.0,
                rect.center().y + w / 2.0,
            );
            ui.painter().add(
                egui::epaint::TextShape::new(pos, galley, color)
                    .with_angle(-std::f32::consts::FRAC_PI_2),
            );
        }
        resp
    }
}

impl RcadApp {
    /// A registry command as an icon button (`small`: the application bar).
    pub(crate) fn cmd_icon_button(&mut self, ui: &mut Ui, id: CmdId, small: bool) -> bool {
        let d = registry::def(id);
        let state = (d.enabled)(self);
        let r = ui
            .add_enabled(
                state.is_ok(),
                IconButton {
                    icon: Some(d.icon),
                    caption: caption(d.label),
                    name: d.label,
                    small,
                    dropdown: false,
                },
            )
            .probe(d.label);
        let r = match &state {
            Ok(()) => r.on_hover_text(format!("{}\n{}", d.label, self.command_tooltip(d))),
            // A disabled command still shows its key (learnt before it is
            // needed).
            Err(why) => {
                let keys = d.key_text(self.shell.prefs.keymap);
                r.on_disabled_hover_text(if keys.is_empty() {
                    format!("{}\n{why}", d.label)
                } else {
                    format!("{} ({keys})\n{why}", d.label)
                })
            }
        };
        let clicked = r.clicked();
        if clicked {
            (d.run)(self);
        }
        clicked
    }

    /// The application bar.
    pub(crate) fn app_bar(&mut self, ui: &mut Ui, busy: bool) {
        use CmdId as C;
        ui.horizontal(|ui| {
            ui.strong("R-CAD");
            let file = ui.menu_button("File", |ui| self.file_menu(ui));
            file.response.named("File menu");
            ui.separator();
            for id in [C::New, C::Open, C::Save, C::SaveAs, C::Close] {
                self.cmd_icon_button(ui, id, true);
            }
            ui.separator();
            self.cmd_icon_button(ui, C::Undo, true);
            self.cmd_icon_button(ui, C::Redo, true);
            if busy {
                self.regenerating_chip(ui);
            }
            ui.separator();
            self.ribbon_tabs(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let prefs = ui
                    .add(IconButton {
                        icon: None,
                        caption: "⚙",
                        name: "Preferences",
                        small: true,
                        dropdown: false,
                    })
                    .probe("Preferences")
                    .on_hover_text("Preferences: key layer, theme, dialog width");
                if prefs.clicked() {
                    self.shell.show_prefs = !self.shell.show_prefs;
                }
                if ui
                    .add(IconButton {
                        icon: None,
                        caption: "Search",
                        name: "Command search",
                        small: true,
                        dropdown: false,
                    })
                    .probe("Command search")
                    .on_hover_text(format!(
                        "Find any command by name ({})",
                        registry::def(C::CommandSearch).key_text(self.shell.prefs.keymap)
                    ))
                    .clicked()
                {
                    self.open_command_search();
                }
                // The sketch bar has its own Help button while sketching.
                if self.sketch.is_none() {
                    self.cmd_icon_button(ui, C::Help, true);
                }
                self.view_menu(ui);
                let export = ui.menu_button("Export", |ui| {
                    for d in registry::at(Place::FileExport) {
                        if self.cmd_button(ui, d.id) {
                            ui.close();
                        }
                    }
                });
                crate::probe::note("Export", export.response.rect);
                if self.sketch.is_some() {
                    ui.separator();
                    self.finish_sketch_button(ui);
                }
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        use CmdId as C;
        for id in [C::New, C::Open, C::OpenSafe] {
            if self.cmd_button(ui, id) {
                ui.close();
            }
        }
        let recent = self.shell.prefs.recent.clone();
        ui.menu_button("Open Recent", |ui| {
            if recent.is_empty() {
                ui.weak("No recent documents");
            }
            for p in recent {
                let label = p.file_name().map_or_else(
                    || p.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if ui
                    .button(&label)
                    .on_hover_text(p.display().to_string())
                    .named(&format!("Recent {label}"))
                    .clicked()
                {
                    self.guarded(crate::app::Guarded::Open(p));
                    ui.close();
                }
            }
        });
        let n = self.autosave.offered.len();
        if ui
            .add_enabled(n > 0, egui::Button::new(format!("Recover Documents ({n})")))
            .on_disabled_hover_text("No recovery copies of unsaved work were found at startup")
            .on_hover_text("Recovery copies are listed in the Recover window")
            .clicked()
        {
            ui.close();
        }
        ui.separator();
        for id in [C::Save, C::SaveAs] {
            if self.cmd_button(ui, id) {
                ui.close();
            }
        }
        ui.separator();
        if self.cmd_button(ui, C::Import) {
            ui.close();
        }
        ui.menu_button("Export", |ui| {
            for d in registry::at(Place::FileExport) {
                if self.cmd_button(ui, d.id) {
                    ui.close();
                }
            }
        });
        if self.cmd_button(ui, C::NewDrawing) {
            ui.close();
        }
        ui.separator();
        if self.cmd_button(ui, C::Close) {
            ui.close();
        }
        ui.separator();
        if self.cmd_button(ui, C::About) {
            ui.close();
        }
    }

    /// Workspace tabs (plus the contextual SKETCH tab while a sketch is open).
    pub(crate) fn ribbon_tabs(&mut self, ui: &mut Ui) {
        let sketching = self.sketch.is_some();
        {
            for t in Tab::ALL {
                let on = !sketching && self.shell.tab == t;
                let r = ui
                    .add_enabled(
                        !sketching,
                        egui::Button::selectable(on, RichText::new(t.label()).small().strong()),
                    )
                    .named(&format!("{} tab", t.label()));
                if r.clicked() {
                    self.shell.tab = t;
                }
            }
            if sketching {
                let _ = ui
                    .add(egui::Button::selectable(
                        true,
                        RichText::new("SKETCH").small().strong(),
                    ))
                    .named("SKETCH tab");
            }
        }
    }

    /// FINISH SKETCH at the right end of the contextual SKETCH tab.
    fn finish_sketch_button(&mut self, ui: &mut Ui) {
        let committing = self.sketch.as_ref().is_some_and(|s| s.committing);
        if ui
            .add_enabled(
                !committing,
                egui::Button::new(RichText::new("FINISH SKETCH").small().strong())
                    .fill(Color32::from_rgb(40, 90, 50)),
            )
            .named("FINISH SKETCH")
            .on_hover_text("Commit the sketch as one undoable document step")
            .clicked()
        {
            self.finish_sketch();
        }
    }

    /// The active tab's groups: promoted buttons over a group dropdown.
    pub(crate) fn ribbon(&mut self, ui: &mut Ui) {
        let tab = self.shell.tab;
        {
            {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                    ui.spacing_mut().button_padding.y = 0.0;
                    // One row: each group's promoted buttons, then its title
                    // as a narrow dropdown (the group's full command list).
                    for g in registry::groups(tab) {
                        for d in registry::in_group(tab, g, true) {
                            self.cmd_icon_button(ui, d.id, false);
                        }
                        self.ribbon_extras(ui, tab, g);
                        let title = ui
                            .add(GroupTitle(g.label()))
                            .on_hover_text(format!("{} commands", g.label()));
                        egui::Popup::menu(&title).show(|ui| {
                            for d in registry::in_group(tab, g, false) {
                                if self.cmd_button(ui, d.id) {
                                    ui.close();
                                }
                            }
                        });
                        ui.separator();
                    }
                });
            }
        }
    }

    /// Dropdown buttons that sit on the ribbon face (they keep the menus
    /// scripts and tests open by name).
    fn ribbon_extras(&mut self, ui: &mut Ui, tab: Tab, g: Group) {
        use CmdId as C;
        if tab != Tab::Solid {
            return;
        }
        match g {
            Group::Create => {
                let r = ui.add(IconButton {
                    icon: Some(Icon::Loft),
                    caption: "Loft/Sweep",
                    name: "Loft / Sweep",
                    small: false,
                    dropdown: true,
                });
                let r = r.probe("Loft / Sweep");
                egui::Popup::menu(&r).show(|ui| {
                    for id in [C::Loft, C::Sweep] {
                        if self.cmd_button(ui, id) {
                            ui.close();
                        }
                    }
                });
            }
            Group::Construct => {
                let open = self.summary.open;
                let r = ui
                    .add_enabled(
                        open,
                        IconButton {
                            icon: Some(Icon::Plane),
                            caption: "Planes",
                            name: "Sketch plane menu",
                            small: false,
                            dropdown: true,
                        },
                    )
                    .probe("Planes")
                    .on_hover_text("Create a sketch on a world origin plane (XY, XZ or YZ)");
                egui::Popup::menu(&r).show(|ui| {
                    for id in [C::SketchXY, C::SketchXZ, C::SketchYZ] {
                        if self.cmd_button(ui, id) {
                            ui.close();
                        }
                    }
                });
            }
            Group::Insert => {
                let r = ui
                    .add(IconButton {
                        icon: Some(Icon::Template),
                        caption: "Templates",
                        name: "Templates",
                        small: false,
                        dropdown: true,
                    })
                    .probe("Templates");
                egui::Popup::menu(&r).show(|ui| self.templates_menu(ui));
                self.placement_controls(ui);
            }
            _ => {}
        }
    }

    /// Preferences window (key layer, theme, dialog width).
    pub(crate) fn prefs_ui(&mut self, ctx: &egui::Context) {
        use super::KeymapLayer;
        use super::prefs::Theme;
        if !self.shell.show_prefs {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Preferences")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Keyboard").strong());
                for l in [KeymapLayer::Fusion, KeymapLayer::Classic] {
                    changed |= ui
                        .radio_value(&mut self.shell.prefs.keymap, l, l.label())
                        .named(&format!("Keys {}", l.label()))
                        .changed();
                }
                ui.label(RichText::new("Theme").strong());
                for t in [Theme::FollowOs, Theme::Light, Theme::Dark] {
                    changed |= ui
                        .radio_value(&mut self.shell.prefs.theme, t, t.label())
                        .named(&format!("Theme {}", t.label()))
                        .changed();
                }
                ui.small(
                    "The help sheet (F1) lists every command and its key in the active layer.",
                );
                ui.label(RichText::new("Sketch").strong());
                changed |= ui
                    .checkbox(
                        &mut self.shell.prefs.auto_project_face_edges,
                        "Auto-project face edges",
                    )
                    .named("Pref auto-project face edges")
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.shell.prefs.auto_project_on_reference,
                        "Auto-project edges on reference",
                    )
                    .named("Pref auto-project edges on reference")
                    .on_hover_text(
                        "Dimensioning a model edge in the sketch editor projects that edge first",
                    )
                    .changed();
            });
        self.shell.show_prefs = open;
        if changed {
            self.apply_theme(ctx);
            self.save_prefs();
        }
    }

    pub(crate) fn apply_theme(&self, ctx: &egui::Context) {
        use super::prefs::Theme;
        ctx.set_theme(match self.shell.prefs.theme {
            Theme::FollowOs => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });
    }
}
