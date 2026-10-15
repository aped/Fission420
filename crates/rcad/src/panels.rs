//! Panels around the 3D view: command bar, browser (feature tree), contextual
//! inspector, status bar, dialogs, confirmation, rename and help windows.
//!
//! Commands that cannot run are disabled with the reason as hover text (and,
//! in the inspector, as visible text). Internal ids and solver diagnostics
//! live in collapsible "Details" sections.

use crate::file_pick::{self, PickMode};
use eframe::egui::{self, Color32, RichText};
use rcad_commands::{Command, FeatureSummary};
use rcad_document::{BodyId, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::SolveStatus;

use crate::app::{ConfirmAnswer, Dialog, Guarded, RcadApp, SketchTarget, build_command};
use crate::place::Tool;
use crate::probe::Probe as _;
use crate::sketch_edit::{Item, SketchTool};

const OK_COLOR: Color32 = Color32::from_rgb(120, 220, 120);
const UNDER_COLOR: Color32 = Color32::from_rgb(120, 190, 255);
const BAD_COLOR: Color32 = Color32::from_rgb(255, 110, 110);
const WARN_COLOR: Color32 = Color32::from_rgb(240, 190, 80);

/// Hover text of a feature's health: message, cause and repair hint.
fn health_text(f: &FeatureSummary) -> String {
    let h = &f.health;
    let mut t = h
        .message
        .clone()
        .unwrap_or_else(|| format!("{:?}", h.state));
    if let Some(up) = h.upstream {
        t.push_str(&format!("\nCaused by the failure of feature {}", up.0));
    }
    if let Some(hint) = &h.hint {
        t.push_str(&format!("\nTo repair: {hint}"));
    }
    t
}

/// Button that is disabled with a visible reason on hover.
fn action(ui: &mut egui::Ui, label: &str, blocker: Option<&str>) -> bool {
    let r = ui
        .add_enabled(blocker.is_none(), egui::Button::new(label))
        .probe(label);
    let r = match blocker {
        Some(why) => r.on_disabled_hover_text(why),
        None => r,
    };
    r.clicked()
}

/// Constraint state as symbol + words (never colour alone).
pub(crate) fn state_text(f: &FeatureSummary) -> Option<(String, Color32)> {
    let st = f.solve_status?;
    Some(match st {
        SolveStatus::FullyConstrained => ("✔ Fully constrained".into(), OK_COLOR),
        SolveStatus::UnderConstrained => (
            format!("~ Under-constrained ({} DOF)", f.solve_dof.unwrap_or(0)),
            UNDER_COLOR,
        ),
        SolveStatus::OverConstrained => ("✖ Over-constrained (redundant)".into(), BAD_COLOR),
        _ => (format!("✖ {st:?}"), BAD_COLOR),
    })
}

impl RcadApp {
    // ----- command bars -----

    /// The View menu: fit, section view, standard views.
    pub(crate) fn view_menu(&mut self, ui: &mut egui::Ui) {
        use crate::shell::{CmdId as C, registry};
        ui.menu_button("View", |ui| {
            let fit = registry::def(C::Fit);
            if ui
                .button(format!("Fit ({})", fit.key_text(self.shell.prefs.keymap)))
                .probe("Fit (F)")
                .clicked()
            {
                self.fit_view();
                ui.close();
            }
            let label = if self.section.is_some() {
                "Section view off"
            } else {
                "Section view…"
            };
            if ui
                .button(label)
                .probe("Section view…")
                .on_hover_text(registry::def(C::Section).tooltip)
                .clicked()
            {
                self.toggle_section();
                ui.close();
            }
            for id in [
                C::ViewTop,
                C::ViewFront,
                C::ViewRight,
                C::ViewLeft,
                C::ViewBack,
                C::ViewIso,
            ] {
                let d = registry::def(id);
                if ui
                    .button(format!(
                        "{} ({})",
                        d.label,
                        d.key_text(self.shell.prefs.keymap)
                    ))
                    .clicked()
                {
                    (d.run)(self);
                    ui.close();
                }
            }
        })
        .response
        .probe("View");
    }

    /// Spinner + Cancel while a regeneration runs.
    pub(crate) fn regenerating_chip(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.spinner();
        ui.label("Regenerating…");
        // Stops the running regeneration at the next feature; the
        // edit is then not committed (the model keeps its last state).
        if ui
            .small_button("Cancel")
            .named("Cancel regeneration")
            .on_hover_text(
                "Stop this regeneration at the next feature: the edit is not applied \
                 and the model stays as it was (a kernel operation already running \
                 finishes first)",
            )
            .clicked()
        {
            self.engine.cancel_running();
        }
    }

    /// Templates: printable parts, dimensioned sketch templates, placement tools.
    pub(crate) fn templates_menu(&mut self, ui: &mut egui::Ui) {
        use crate::shell::CmdId as C;
        ui.weak("Printable parts (new document with editable parameters)");
        for id in [
            C::TemplateEnclosure,
            C::TemplateCoupon,
            C::TemplateBracket,
            C::TemplateTray,
        ] {
            if self.cmd_button(ui, id) {
                ui.close();
            }
        }
        ui.separator();
        ui.weak("Dimensioned sketch templates (dialogs)");
        for id in [C::RectangleXY, C::RectangleXZ, C::CircleOnFace] {
            if self.cmd_button(ui, id) {
                ui.close();
            }
        }
        ui.separator();
        ui.weak("Drag to place");
        for id in [C::PlaceRectangle, C::PlaceHole] {
            if self.cmd_button(ui, id) {
                ui.close();
            }
        }
    }

    /// While a placement tool runs: its target and a cancel button.
    pub(crate) fn placement_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(Tool::Hole { extrude }) = self.placement.as_ref().map(|p| p.tool) {
            let extrudes: Vec<FeatureSummary> = self
                .summary
                .features
                .iter()
                .filter(|f| f.kind == "extrude")
                .cloned()
                .collect();
            let refs: Vec<&FeatureSummary> = extrudes.iter().collect();
            let mut sel = Some(extrude);
            feature_combo(ui, "Hole on end cap of", &mut sel, &refs);
            if let Some(e) = sel
                && e != extrude
            {
                self.start_tool(Tool::Hole { extrude: e });
            }
        }
        if self.placement.is_some()
            && ui
                .button("Cancel placement")
                .probe("Cancel placement")
                .clicked()
        {
            self.cancel_tool();
        }
    }

    pub(crate) fn sketch_toolbar(&mut self, ui: &mut egui::Ui) {
        let layer = self.shell.prefs.keymap;
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let committing = s.committing;
        let mut finish = false;
        let mut cancel = false;
        let mut unlink = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Sketch");
            ui.separator();
            ui.add_enabled_ui(!committing, |ui| {
                let ed = &mut s.editor;
                for t in SketchTool::TOOLBAR {
                    if ui
                        .add(egui::Button::new(t.label()).selected(ed.tool == t))
                        .probe(t.label())
                        .on_hover_text(match crate::shell::keymap::sketch_key_for(layer, t) {
                            Some(k) => format!("{} ({k})", t.hint()),
                            None => t.hint().to_string(),
                        })
                        .clicked()
                    {
                        ed.set_tool(t);
                    }
                }
                if ed.tool == SketchTool::Polygon {
                    let l = ui.label("Sides");
                    ui.add(egui::DragValue::new(&mut ed.polygon_sides).range(3..=64))
                        .labelled_by(l.id);
                    let mut circ = ed.polygon_fit == rcad_sketch::PolygonFit::Circumscribed;
                    if ui
                        .checkbox(&mut circ, "Across flats")
                        .on_hover_text("Circumscribed: the dimension is the diameter across flats")
                        .changed()
                    {
                        ed.polygon_fit = if circ {
                            rcad_sketch::PolygonFit::Circumscribed
                        } else {
                            rcad_sketch::PolygonFit::Inscribed
                        };
                    }
                }
                ui.separator();
                // Constraint actions live in the inspector ("Constrain selection").
                if action(ui, "Pattern…", ed.pattern_disabled_reason()) {
                    ed.begin_pattern();
                }
                if action(ui, "Construction", ed.construction_disabled_reason()) {
                    ed.toggle_construction();
                }
                let del = ed
                    .selection
                    .is_empty()
                    .then_some("Select geometry, a dimension or a constraint glyph first");
                if action(ui, "Delete", del) {
                    ed.delete_selection();
                }
            });
            ui.separator();
            if ui
                .add_enabled(
                    !committing,
                    egui::Button::new(RichText::new("Finish Sketch").strong())
                        .fill(Color32::from_rgb(40, 90, 50)),
                )
                .probe("Finish Sketch")
                .on_hover_text("Commit the sketch as one undoable document step")
                .clicked()
            {
                finish = true;
            }
            if ui
                .add_enabled(!committing, egui::Button::new("Cancel Sketch"))
                .probe("Cancel Sketch")
                .on_hover_text("Discard all changes made in this sketch session")
                .clicked()
            {
                cancel = true;
            }
            if ui.button("Help").probe("Help").clicked() {
                self.show_help = !self.show_help;
            }
            // Undo/Redo sit on this row so the tool row stays one line at
            // 1280 px (a wrapped tool row moves the viewport between frames).
            ui.separator();
            if ui
                .add_enabled(
                    !committing && s.editor.can_undo(),
                    egui::Button::new("Undo step"),
                )
                .probe("Undo step")
                .on_hover_text("Undo inside this sketch (Ctrl+Z)")
                .clicked()
            {
                s.editor.undo();
            }
            if ui
                .add_enabled(
                    !committing && s.editor.can_redo(),
                    egui::Button::new("Redo step"),
                )
                .probe("Redo step")
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                s.editor.redo();
            }
            // Second-row sketch tools (Break, Project, Intersect) and the
            // Project options.
            ui.separator();
            ui.add_enabled_ui(!committing, |ui| {
                let ed = &mut s.editor;
                for t in SketchTool::SECOND_ROW {
                    if ui
                        .add(egui::Button::new(t.label()).selected(ed.tool == t))
                        .probe(t.label())
                        .on_hover_text(format!("{} ({})", t.hint(), t.shortcut()))
                        .clicked()
                    {
                        ed.set_tool(t);
                    }
                }
                if crate::sketch_project::options_ui(ui, ed) {
                    unlink = true;
                }
            });
            if committing {
                ui.spinner();
                ui.label("Committing…");
            }
        });
        if unlink {
            self.unlink_selected_projection();
        }
        if finish {
            self.finish_sketch();
        } else if cancel {
            self.cancel_sketch();
        }
    }

    // ----- browser -----

    pub(crate) fn history(&mut self, ui: &mut egui::Ui) {
        // The Browser heading shares its row with the first tree nodes.
        ui.horizontal_wrapped(|ui| {
            ui.heading("Browser");
            if self.summary.open {
                self.browser_top_nodes(ui);
            }
        });
        if !self.summary.open {
            ui.weak("No document open. Use New or Open….");
            return;
        }
        self.parameters_panel(ui);
        self.configurations_panel(ui);
        self.configuration_table(ui);
        self.drawings_panel(ui);
        ui.separator();
        self.bodies_browser(ui);
        ui.add_space(4.0);
        ui.separator();
        ui.label(RichText::new("History").strong());
        self.health_banner(ui);
        if self.summary.features.is_empty() {
            ui.weak("Empty document. Use Create Sketch to start.");
        }
        let editing = self.sketch.as_ref().and_then(|s| match s.target {
            SketchTarget::Existing(id) => Some(id),
            _ => None,
        });
        let mut clicked = None;
        let mut edit = None;
        let mut edit_feature = None;
        let mut rename = None;
        let mut delete = None;
        let feats = self.summary.features.clone();
        let first_rolled = feats.iter().position(|f| f.rolled_back);
        for (i, f) in feats.iter().enumerate() {
            if Some(i) == first_rolled {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("▶ timeline marker (later features rolled back)")
                            .color(Color32::from_rgb(255, 200, 90))
                            .small(),
                    );
                    if ui
                        .small_button("Roll to end")
                        .probe("Roll to end")
                        .clicked()
                    {
                        self.request(Command::SetRollback { at: None });
                    }
                });
            }
            let sel = self.selected == Some(f.id);
            let kind = match f.kind.as_str() {
                "sketch" => "Sketch",
                "extrude" => "Extrude",
                "revolve" => "Revolve",
                "cut_through_all" => "Cut",
                "pocket" => "Pocket",
                "linear_pattern" | "circular_pattern" => "Pattern",
                "mirror" => "Mirror",
                "chamfer" => "Chamfer",
                "fillet" => "Fillet",
                "hole" => "Hole",
                "import_step" | "import_mesh" | "import_surfaces" => "Import",
                "sheet_base" | "sheet_flange" | "sheet_flat" => "Sheet",
                "surface_stitch" | "surface_fill" | "thicken" | "surface_patch"
                | "surface_extrude" => "Surface",
                k => k,
            };
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{kind:<7}"))
                        .monospace()
                        .weak()
                        .small(),
                );
                use rcad_document::HealthState as H;
                let label = if f.suppressed {
                    RichText::new(&f.name).weak().strikethrough()
                } else if f.rolled_back {
                    RichText::new(&f.name).weak().italics()
                } else {
                    match f.health.state {
                        H::Error => RichText::new(&f.name).color(BAD_COLOR),
                        H::Warning => RichText::new(&f.name).color(WARN_COLOR),
                        _ => RichText::new(&f.name),
                    }
                };
                let r = ui.selectable_label(sel, label).probe(&f.name);
                if f.suppressed {
                    ui.label(RichText::new("suppressed").weak().small());
                } else if f.rolled_back {
                    ui.label(RichText::new("rolled back").weak().small());
                } else if matches!(f.health.state, H::Error | H::Warning) {
                    let (sym, c) = if f.health.state == H::Error {
                        ("⚠ error", BAD_COLOR)
                    } else {
                        ("⚠ warning", WARN_COLOR)
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(sym).color(c).small()).frame(false))
                        .named(&format!("Health {}", f.name))
                        .on_hover_text(health_text(f))
                        .clicked()
                    {
                        clicked = Some(f.id);
                    }
                }
                if r.clicked() {
                    clicked = Some(f.id);
                }
                if r.double_clicked() {
                    if f.kind == "sketch" {
                        edit = Some(f.id);
                    } else {
                        edit_feature = Some(f.id);
                    }
                }
                r.context_menu(|ui| {
                    if f.kind == "sketch" && ui.button("Edit Sketch").probe("Edit Sketch").clicked()
                    {
                        edit = Some(f.id);
                        ui.close();
                    }
                    if f.kind != "sketch"
                        && ui
                            .button("Edit Feature")
                            .named(&format!("Edit Feature {}", f.name))
                            .clicked()
                    {
                        edit_feature = Some(f.id);
                        ui.close();
                    }
                    if ui.button("Rename…").probe("Rename…").clicked() {
                        rename = Some(f.id);
                        ui.close();
                    }
                    if ui.button("Delete feature").clicked() {
                        delete = Some(f.id);
                        ui.close();
                    }
                });
                if editing == Some(f.id) {
                    ui.label(RichText::new("editing").color(Color32::from_rgb(255, 200, 90)));
                } else if let Some((t, c)) = state_text(f) {
                    let sym: String = t.chars().take(1).collect();
                    ui.label(RichText::new(sym).color(c)).on_hover_text(t);
                }
                if f.profile_error.is_some() {
                    // Several loops (e.g. text): regions are extrudable one by
                    // one or all together; only a sketch with none is "open".
                    let n = crate::extrude::sketch_regions(f).len();
                    if n > 0 {
                        ui.label(RichText::new(format!("{n} regions")).weak().small())
                            .on_hover_text(
                                "No single profile: extrude picks a region (or all \
                                 outermost regions)",
                            );
                    } else {
                        ui.label(RichText::new("open").color(BAD_COLOR).small())
                            .on_hover_text(f.profile_error.clone().unwrap_or_default());
                    }
                }
            });
            if let Some(p) = &f.plane
                && matches!(p, SketchPlane::Face(_))
            {
                ui.indent(("feat", f.id.0), |ui| {
                    ui.small(self.plane_text(p));
                });
            }
        }
        if self.sketch.is_none() {
            if let Some(id) = clicked {
                self.select(Some(id));
            }
            if let Some(id) = edit {
                self.select(Some(id));
                self.begin_sketch(SketchTarget::Existing(id));
            }
            if let Some(id) = edit_feature {
                self.edit_feature(id);
            }
            if let Some(id) = rename {
                self.rename = Some((id, self.feature_name(id)));
            }
            if let Some(id) = delete {
                self.select(Some(id));
                self.delete_selected_feature();
            }
        }
    }

    /// "Opened with errors" banner above History: how many features fail,
    /// the first one's reason, and Roll back before it (safe mode in place).
    fn health_banner(&mut self, ui: &mut egui::Ui) {
        use rcad_document::HealthState as H;
        let failed: Vec<FeatureSummary> = self
            .summary
            .features
            .iter()
            .filter(|f| f.health.state == H::Error)
            .cloned()
            .collect();
        let Some(first) = failed.first() else {
            return;
        };
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(70, 30, 30))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "⚠ {} feature{} with errors: not evaluated",
                        failed.len(),
                        if failed.len() == 1 { "" } else { "s" }
                    ))
                    .color(BAD_COLOR)
                    .strong(),
                )
                .named("Document has errors");
                ui.label(RichText::new(format!("{}: {}", first.name, health_text(first))).small());
                ui.horizontal(|ui| {
                    if ui
                        .small_button("Review")
                        .named("Review first error")
                        .on_hover_text("Select the first failing feature")
                        .clicked()
                    {
                        self.select(Some(first.id));
                    }
                    let before = self
                        .summary
                        .features
                        .iter()
                        .position(|f| f.id == first.id)
                        .and_then(|i| i.checked_sub(1))
                        .map(|i| self.summary.features[i].id);
                    if let Some(at) = before
                        && ui
                            .small_button("Roll back before it")
                            .named("Roll back before the first error")
                            .on_hover_text("Move the timeline marker before the first failing feature (one undo step)")
                            .clicked()
                    {
                        self.request(Command::SetRollback { at: Some(at) });
                    }
                });
            });
    }

    /// Canvas picks feed the dialog's active selection input: a face picked
    /// while the dialog is open replaces the dialog's face (Hole re-centres
    /// on it). The preselection already filled it when the dialog opened.
    fn feed_selection_input(&mut self, dlg: &mut Dialog) {
        let pick = self.selected_face;
        let changed = pick != self.shell.last_pick;
        self.shell.last_pick = pick;
        if !changed || self.last_dialog != Some(dlg.title()) {
            return;
        }
        let Some(info) = self.selected_face().cloned() else {
            return;
        };
        let Some(r) = info.reference else {
            return;
        };
        let centre = self.face_centre_local(&info, &r);
        match dlg {
            Dialog::Hole { face, cx, cy, .. } => {
                *face = r;
                if let Some([x, y]) = centre {
                    *cx = crate::params_ui::fmt_value(x);
                    *cy = crate::params_ui::fmt_value(y);
                }
            }
            Dialog::ShellFace { face, .. } | Dialog::FilletLoop { face, .. } => *face = r,
            Dialog::Align { from, to } if r != *from => *to = Some(r),
            _ => return,
        }
        self.shell.pick_armed = false;
    }

    /// A selection input row: "<what>: 1 selected (<item>)" with ✕ to clear
    /// it; while cleared, the next face picked in the view fills it and OK
    /// stays disabled. Returns false while it is empty.
    fn selection_row(&mut self, ui: &mut egui::Ui, what: &str, item: &str) -> bool {
        let armed = self.shell.pick_armed;
        ui.horizontal(|ui| {
            ui.label(format!("{what}:"));
            if armed {
                ui.colored_label(UNDER_COLOR, "Select a face in the view");
            } else {
                ui.label(RichText::new("1 selected").strong())
                    .on_hover_text(item);
                if ui
                    .small_button("✕")
                    .named(&format!("Clear {what}"))
                    .on_hover_text("Clear: pick another face in the view")
                    .clicked()
                {
                    self.shell.pick_armed = true;
                }
            }
        });
        if !armed {
            ui.small(item);
        }
        !armed
    }

    // ----- inspector -----

    pub(crate) fn inspector_ui(&mut self, ui: &mut egui::Ui, _busy: bool) {
        // The "Inspector" heading is the right column's tab (shell::properties).
        if let Some(info) = self.selected_face().cloned() {
            ui.label(RichText::new("Selected face").strong());
            ui.label(&info.description);
            self.face_measure_ui(ui, &info);
            self.exact_face_ui(ui, &info);
            match (info.reference, info.sketch_blocker()) {
                (Some(r), _) => {
                    if ui
                        .button("Create Sketch on this face")
                        .probe("Create Sketch on this face")
                        .clicked()
                    {
                        self.begin_sketch(SketchTarget::New {
                            plane: SketchPlane::Face(r),
                        });
                    }
                    if ui
                        .button("Hole…")
                        .probe("Hole…")
                        .on_hover_text("Drill a round hole (through or blind, optional counterbore) into this face's body")
                        .clicked()
                    {
                        let [cx, cy] = self.face_centre_local(&info, &r).unwrap_or([0.0, 0.0]);
                        self.dialog = Some(Dialog::Hole {
                            face: r,
                            cx: crate::params_ui::fmt_value(cx),
                            cy: crate::params_ui::fmt_value(cy),
                            diameter: "5.5".into(),
                            through: true,
                            depth: "8".into(),
                            counterbore: false,
                            cb_diameter: "10".into(),
                            cb_depth: "5".into(),
                            offset: "0".into(),
                        });
                    }
                    // Align this face's body flush onto a face of another body.
                    let mine = self.selected_face().and_then(|f| f.body);
                    let targets: Vec<(String, rcad_document::FaceRef)> = self
                        .summary
                        .features
                        .iter()
                        .flat_map(|g| {
                            g.faces
                                .iter()
                                .filter(|fc| Some(fc.body) != mine)
                                .map(move |fc| {
                                    (format!("{} {:?}", g.name, fc.reference.role), fc.reference)
                                })
                        })
                        .collect();
                    let mut align = None;
                    ui.menu_button("Align onto…", |ui| {
                        for (label, to) in &targets {
                            if ui
                                .button(label)
                                .named(&format!("Align onto {label}"))
                                .clicked()
                            {
                                align = Some(*to);
                                ui.close();
                            }
                        }
                        if targets.is_empty() {
                            ui.weak("No face of another body.");
                        }
                    })
                    .response
                    .probe("Align onto…");
                    if let Some(to) = align {
                        self.request(Command::AlignBody {
                            name: None,
                            from: r,
                            to,
                            offset: 0.0,
                        });
                    }
                    // Shell the body under this face, leaving it open.
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shell_thickness).desired_width(40.0),
                    )
                    .named("Shell thickness (mm)");
                    if ui
                        .button("Shell from this face")
                        .probe("Shell from this face")
                        .on_hover_text("Hollow the body under this face (left open) to walls and floor of the given thickness; bodies prismatic along the face normal")
                        .clicked()
                        && let Ok(t) = self.shell_thickness.trim().parse::<f64>()
                    {
                        self.request(Command::ShellFace {
                            name: None,
                            face: r,
                            thickness: t,
                        });
                    }
                    if ui
                        .button("Chamfer edge…")
                        .probe("Chamfer edge…")
                        .on_hover_text("Chamfer the straight edge where this face meets another planar face of the same body")
                        .clicked()
                    {
                        self.dialog = Some(Dialog::Chamfer {
                            a: r,
                            b: None,
                            distance: "1".into(),
                            fillet: false,
                        });
                    }
                    if ui
                        .button("Fillet edge…")
                        .probe("Fillet edge…")
                        .on_hover_text("Round the straight edge where this face meets another planar face with an exact cylinder (bodies prismatic along the edge)")
                        .clicked()
                    {
                        self.dialog = Some(Dialog::Chamfer {
                            a: r,
                            b: None,
                            distance: "2".into(),
                            fillet: true,
                        });
                    }
                    if ui
                        .button("Fillet edge loop…")
                        .probe("Fillet edge loop…")
                        .on_hover_text("Round every edge around this face's outer boundary (e.g. the top edge of a box or rounded rectangle) with exact cylinders, tori and sphere corners")
                        .clicked()
                    {
                        self.dialog = Some(Dialog::FilletLoop {
                            face: r,
                            radius: "2".into(),
                            chamfer: false,
                            holes: false,
                        });
                    }
                    if ui
                        .button("Chamfer edge loop…")
                        .probe("Chamfer edge loop…")
                        .on_hover_text("Chamfer every edge around this face's outer boundary at 45° (planes along straight edges, cones along arcs, mitered corners)")
                        .clicked()
                    {
                        self.dialog = Some(Dialog::FilletLoop {
                            face: r,
                            radius: "1".into(),
                            chamfer: true,
                            holes: false,
                        });
                    }
                }
                (None, Some(why)) => {
                    ui.add_enabled(false, egui::Button::new("Create Sketch on this face"))
                        .probe("Create Sketch on this face");
                    ui.label(RichText::new(why).weak().small());
                }
                _ => {}
            }
            self.direct_edit_actions(ui, info.group);
            ui.separator();
        }
        if self.selected.is_none()
            && let Some(b) = self.selected_body
        {
            self.body_inspector(ui, b);
            return;
        }
        let Some(id) = self.selected else {
            ui.weak("Select a feature in the browser, or a face in the view.");
            if self.summary.open && !self.summary.features.is_empty() {
                ui.add_space(6.0);
                ui.weak("Tip: double-click a sketch in the browser to edit it.");
            }
            return;
        };
        let Some(f) = self.summary.features.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        // Wrapping row: a long feature name/kind must not widen the
        // inspector panel (that moved the viewport by 26 px at 1280 after an
        // STL import, "Import1 (import_mesh)"; layout_1280).
        ui.horizontal_wrapped(|ui| {
            let kind = match f.kind.as_str() {
                "cut_through_all" => "cut",
                k => k,
            };
            ui.label(
                RichText::new(format!("{} ({kind})", f.name))
                    .strong()
                    .size(16.0),
            );
            if ui.small_button("Rename…").probe("Rename…").clicked() {
                self.rename = Some((f.id, f.name.clone()));
            }
            let users = self.dependents_of(f.id);
            let why = (!users.is_empty())
                .then(|| format!("Used by {}; delete those first", users.join(", ")));
            let r = ui
                .add_enabled(why.is_none(), egui::Button::new("Delete").small())
                .probe("Delete feature");
            let r = match &why {
                Some(w) => r.on_disabled_hover_text(w),
                None => r.on_hover_text("Delete this feature (Del); Undo restores it"),
            };
            if r.clicked() {
                self.delete_selected_feature();
            }
        });
        // Suppression (kept in history, not evaluated). Dependents are
        // never silently rebound: suppress them too, or the change is refused.
        ui.horizontal_wrapped(|ui| {
            let users = self.dependents_of(f.id);
            if f.suppressed {
                ui.label(RichText::new("Suppressed: not evaluated").weak());
                if ui
                    .small_button("Restore")
                    .probe("Restore feature")
                    .on_hover_text("Evaluate this feature again (refused while something it uses is suppressed)")
                    .clicked()
                {
                    self.request(Command::SetSuppressed {
                        feature: f.id,
                        suppressed: false,
                        with_dependents: false,
                    });
                }
                if ui
                    .small_button("Restore with dependents")
                    .probe("Restore with dependents")
                    .clicked()
                {
                    self.request(Command::SetSuppressed {
                        feature: f.id,
                        suppressed: false,
                        with_dependents: true,
                    });
                }
            } else {
                let why = (!users.is_empty()).then(|| {
                    format!(
                        "Used by {}: use \"Suppress with dependents\"",
                        users.join(", ")
                    )
                });
                let r = ui
                    .add_enabled(why.is_none(), egui::Button::new("Suppress").small())
                    .probe("Suppress feature");
                let r = match &why {
                    Some(w) => r.on_disabled_hover_text(w),
                    None => r.on_hover_text("Keep in history but do not evaluate (one Undo step)"),
                };
                if r.clicked() {
                    self.request(Command::SetSuppressed {
                        feature: f.id,
                        suppressed: true,
                        with_dependents: false,
                    });
                }
                if !users.is_empty()
                    && ui
                        .small_button("Suppress with dependents")
                        .probe("Suppress with dependents")
                        .on_hover_text(format!("Also suppresses {}", users.join(", ")))
                        .clicked()
                {
                    self.request(Command::SetSuppressed {
                        feature: f.id,
                        suppressed: true,
                        with_dependents: true,
                    });
                }
            }
        });
        // Timeline marker and reorder (each one Undo step; a move that
        // breaks a dependency or does not regenerate is refused unchanged).
        ui.horizontal_wrapped(|ui| {
            let ids: Vec<FeatureId> = self.summary.features.iter().map(|x| x.id).collect();
            let pos = ids.iter().position(|x| *x == f.id);
            if ui
                .small_button("Roll back here")
                .probe("Roll back here")
                .on_hover_text(
                    "Evaluate history up to this feature; later ones are kept but not \
                     evaluated, and new features are inserted here",
                )
                .clicked()
            {
                self.request(Command::SetRollback { at: Some(f.id) });
            }
            if let Some(p) = pos {
                if p > 0 && ui.small_button("Move up").probe("Move up").clicked() {
                    self.request(Command::MoveFeature {
                        feature: f.id,
                        before: Some(ids[p - 1]),
                    });
                }
                if p + 1 < ids.len() && ui.small_button("Move down").probe("Move down").clicked() {
                    self.request(Command::MoveFeature {
                        feature: f.id,
                        before: ids.get(p + 2).copied(),
                    });
                }
            }
        });
        if let Some((t, c)) = state_text(&f) {
            ui.label(RichText::new(t).color(c));
        }
        // Contextual actions.
        match f.kind.as_str() {
            "sketch" => {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Edit Sketch").probe("Edit Sketch").clicked() {
                        self.begin_sketch(SketchTarget::Existing(f.id));
                    }
                    let eb = self.extrude_blocker(&f);
                    if action(ui, "Extrude this sketch…", eb.as_deref()) {
                        self.dialog = Some(Dialog::Extrude(crate::extrude::ExtrudeForm::new(
                            self,
                            Some(f.id),
                        )));
                    }
                    if ui
                        .button("Re-attach…")
                        .probe("Re-attach…")
                        .on_hover_text("Move this sketch to another face or plane (repair a broken support); one undo step")
                        .clicked()
                    {
                        self.start_reattach(f.id);
                    }
                    let cb = self.cut_blocker(&f);
                    if action(ui, "Cut with this sketch…", cb.as_deref()) {
                        self.dialog = Some(Dialog::Cut {
                            sketch: Some(f.id),
                            blind: false,
                            depth: "5".into(),
                        });
                    }
                });
                self.projection_ui(ui, &f);
                self.sheet_ui(ui, &f);
                self.sheet_base_ui(ui, &f);
                self.surface_sketch_ui(ui, &f);
                match &f.profile_error {
                    None => {
                        ui.label(RichText::new("Profile: closed").color(OK_COLOR).small());
                    }
                    Some(e) => {
                        ui.label(
                            RichText::new(format!("Profile not usable: {e}"))
                                .color(BAD_COLOR)
                                .small(),
                        );
                    }
                }
                if let Some(c) = self.consumer_of(f.id) {
                    ui.small(format!("Used by {}", c.name));
                }
            }
            "sheet_metal" => self.flat_pattern_ui(ui, &f),
            "sheet_base" | "sheet_flange" | "sheet_flat" => self.sheet_feature_ui(ui, &f),
            "import_surfaces" | "surface_stitch" | "surface_fill" | "thicken" | "surface_patch"
            | "surface_extrude" => self.surface_feature_ui(ui, &f),
            "extrude" => {
                // Scoped shell of a New Body extrusion (dialog: extrude.rs).
                if matches!(f.operation, Some(rcad_document::Operation::NewBody { .. }))
                    && ui
                        .button("Shell…")
                        .probe("Shell…")
                        .on_hover_text("Hollow this extrusion: walls and floor of one thickness, open at one cap (scoped shell, not a general shell)")
                        .clicked()
                {
                    self.dialog = Some(Dialog::Shell {
                        extrude: f.id,
                        open_end: true,
                        thickness: "2".into(),
                    });
                }
                if matches!(
                    f.operation,
                    Some(
                        rcad_document::Operation::Cut { .. } | rcad_document::Operation::Join { .. }
                    )
                ) && ui
                    .button("Pattern…")
                    .probe("Pattern…")
                    .on_hover_text("Repeat this join/cut in a row or around a centre, or mirror it across a world plane (editable parameters)")
                    .clicked()
                {
                    self.dialog = Some(Dialog::Pattern {
                        feature: f.id,
                        kind: crate::app::PatternKind::Linear,
                        count: "3".into(),
                        spacing: "10".into(),
                        along_y: false,
                        reverse: false,
                        count2: "1".into(),
                        spacing2: "10".into(),
                        angle: "60".into(),
                        center_x: "0".into(),
                        center_y: "0".into(),
                        mirror: self
                            .selected_face()
                            .and_then(|i| i.reference)
                            .map_or(SketchPlane::WorldYZ, SketchPlane::Face),
                        mirror_face: self.selected_face().and_then(|i| i.reference),
                        mirror_offset: "0".into(),
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    // Text glyph walls are many small faces: not listed
                    // as buttons (pick them in the view instead).
                    let text_side = |r: &rcad_document::FaceRole| match r {
                        rcad_document::FaceRole::ExtrudeSide { entity } => {
                            rcad_sketch::text::text_edge_parts(rcad_sketch::EntityId(*entity))
                                .is_some()
                        }
                        _ => false,
                    };
                    let hidden = f
                        .faces
                        .iter()
                        .filter(|fs| text_side(&fs.reference.role))
                        .count();
                    for fs in f.faces.iter().filter(|fs| !text_side(&fs.reference.role)) {
                        let label =
                            format!("Sketch on {}", crate::face_role_label(&fs.reference.role));
                        if ui.button(label).clicked() {
                            self.begin_sketch(SketchTarget::New {
                                plane: SketchPlane::Face(fs.reference),
                            });
                        }
                    }
                    if hidden > 0 {
                        ui.small(format!(
                            "{hidden} text side faces not listed (pick one in the view)"
                        ));
                    }
                });
            }
            "hole" => self.countersink_action(ui, f.id),
            _ => {}
        }
        ui.separator();
        self.parameter_editor(ui);
        ui.collapsing("Details", |ui| {
            ui.small(format!("id {} · kind {}", f.id, f.kind));
            if !f.depends_on.is_empty() {
                let deps: Vec<String> = f
                    .depends_on
                    .iter()
                    .map(|d| format!("{} {}", d, self.feature_name(*d)))
                    .collect();
                ui.small(format!("uses {}", deps.join(", ")));
            }
            if let Some(p) = &f.plane {
                ui.small(self.plane_text(p));
            }
            if let Some(st) = f.solve_status {
                let dof = f.solve_dof.map_or("?".into(), |d| d.to_string());
                let res = f.solve_residual.map_or("?".into(), |r| format!("{r:.1e}"));
                ui.small(format!("{st:?} · DOF {dof} · residual {res}"));
            }
            for c in &f.solve_conflicts {
                ui.small(RichText::new(format!("conflict: {c}")).color(BAD_COLOR));
            }
            if let Some(fr) = f.frame {
                ui.small(format!(
                    "frame origin ({:.3}, {:.3}, {:.3}) normal ({:.3}, {:.3}, {:.3})",
                    fr.origin[0], fr.origin[1], fr.origin[2], fr.z[0], fr.z[1], fr.z[2]
                ));
            }
        });
    }

    /// Two-face measurement: exact distance between parallel planar faces
    /// (walls, floor, cavity, mating clearance) or the angle between them,
    /// from the B-rep planes in the document summary (not the display mesh).
    fn face_measure_ui(&mut self, ui: &mut egui::Ui, info: &crate::view3d::FaceInfo) {
        let Some(p) = info.plane else {
            if !info.planar {
                ui.small("Curved face: no plane to measure from.");
            }
            return;
        };
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        ui.small(format!(
            "Plane: normal ({:.4}, {:.4}, {:.4}), offset {:.4} mm",
            p.normal[0],
            p.normal[1],
            p.normal[2],
            dot(p.normal, p.point)
        ));
        match &self.measure_from {
            Some(a) if a.group != info.group => {
                if let Some((parallel, v)) = self.face_measurement() {
                    let text = if parallel {
                        format!(
                            "Distance between parallel faces: {} mm",
                            crate::params_ui::fmt_value(v)
                        )
                    } else {
                        format!(
                            "Angle between the faces' normals: {}°",
                            crate::params_ui::fmt_value(v)
                        )
                    };
                    ui.label(RichText::new(text).strong());
                    ui.small(format!("from: {}", a.description));
                }
                if ui.small_button("Clear measurement").clicked() {
                    self.measure_from = None;
                }
            }
            Some(_) => {
                ui.small("Measuring from this face: select another planar face.");
                if ui.small_button("Clear measurement").clicked() {
                    self.measure_from = None;
                }
            }
            None => {
                if ui
                    .small_button("Measure from this face")
                    .named("Measure from this face")
                    .on_hover_text("Then select a second planar face: exact distance (parallel faces) or angle")
                    .clicked()
                {
                    self.measure_from = Some(info.clone());
                }
            }
        }
    }

    fn parameter_editor(&mut self, ui: &mut egui::Ui) {
        let Some(ins) = self.inspector.as_mut() else {
            return;
        };
        ui.label(RichText::new("Parameters").strong());
        if ins.fields.is_empty() {
            ui.weak("No editable parameters.");
            return;
        }
        let feature = ins.feature;
        let fsum = self
            .summary
            .features
            .iter()
            .find(|f| f.id == feature)
            .cloned();
        let bound: Vec<String> = fsum
            .as_ref()
            .map(|f| f.expressions.keys().cloned().collect())
            .unwrap_or_default();
        let unit_of = |pname: &str| -> &'static str {
            let angle_dim = fsum
                .as_ref()
                .and_then(|f| f.sketch.as_ref())
                .is_some_and(|sk| {
                    sk.constraints.iter().any(|c| {
                    matches!(c, rcad_sketch::Constraint::Angle { dim, .. } if dim.name == pname)
                })
                });
            let mesh_edit = fsum.as_ref().is_some_and(|f| f.kind == "mesh_edit");
            if angle_dim || pname.ends_with("_deg") {
                "deg"
            } else if mesh_edit && matches!(pname, "sx" | "sy" | "sz") {
                "ratio"
            } else if pname == "count" || (mesh_edit && pname == "target_triangles") {
                "count"
            } else if pname == "k_factor" {
                "ratio"
            } else {
                "mm"
            }
        };
        for (pname, v, _, buf) in &mut ins.fields {
            let unit = unit_of(pname);
            let l = ui.horizontal(|ui| {
                let l = ui.label(format!("{pname} ({unit})"));
                if bound.contains(pname) {
                    ui.label(
                        RichText::new(format!("= {}", crate::params_ui::fmt_value(*v))).weak(),
                    );
                }
                l
            });
            ui.add(egui::TextEdit::singleline(buf).desired_width(ui.available_width()))
                .labelled_by(l.inner.id)
                .probe(&format!("{pname} ({unit})"))
                .on_hover_text("A number, or an expression of named parameters (e.g. outer_length - 2 * wall). A plain number removes a binding.");
        }
        let modified = ins.fields.iter().any(|(_, _, t, b)| b.trim() != t.trim());
        let mut apply = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            apply = ui
                .add_enabled(modified, egui::Button::new("Apply"))
                .probe("Apply")
                .clicked();
            cancel = ui
                .add_enabled(modified, egui::Button::new("Cancel edits"))
                .probe("Cancel edits")
                .clicked();
        });
        if modified {
            ui.small("Preview only: not in the document until Apply.");
        }
        if cancel {
            self.reset_inspector();
        } else if apply {
            let ins = ins.clone();
            let mut cmds = vec![];
            let mut bad = vec![];
            for (pname, v, t, b) in &ins.fields {
                if b.trim() == t.trim() {
                    continue;
                }
                let plain = rcad_document::params::plain_number(b.trim());
                match plain {
                    // Unbound numeric edits keep the coalescing SetParameter path.
                    Some(x) if !bound.contains(pname) => {
                        if x != *v {
                            cmds.push(Command::SetParameter {
                                feature: ins.feature,
                                name: pname.clone(),
                                value: x,
                            })
                        }
                    }
                    _ if b.trim().is_empty() => {
                        bad.push(format!("{pname}: empty; type a number or an expression"))
                    }
                    _ => cmds.push(Command::SetExpression {
                        feature: ins.feature,
                        name: pname.clone(),
                        expression: b.trim().to_string(),
                    }),
                }
            }
            if !bad.is_empty() {
                self.diag(true, "Input", bad.join("; "));
                self.reset_inspector();
            }
            for c in cmds {
                self.request(c);
            }
        }
    }

    pub(crate) fn sketch_inspector(&mut self, ui: &mut egui::Ui) {
        if let Some(b) = self.broken_sketch {
            ui.colored_label(
                BAD_COLOR,
                format!(
                    "{} lost its support face (edit refused). Re-attach it, then Finish again.",
                    self.feature_name(b)
                ),
            );
            if ui
                .button("Re-attach sketch…")
                .probe("Re-attach sketch…")
                .clicked()
            {
                self.start_reattach(b);
            }
            ui.separator();
        }
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        ui.heading("Sketch");
        ui.label(&s.title);
        let (_, status) = s.editor.status();
        let col = match s.editor.status().0 {
            crate::sketch_edit::StateKind::Full => OK_COLOR,
            crate::sketch_edit::StateKind::Under => UNDER_COLOR,
            crate::sketch_edit::StateKind::Conflict => BAD_COLOR,
        };
        ui.label(RichText::new(status).color(col));
        match s.editor.profile_state() {
            Ok(t) => ui.label(RichText::new(format!("Profile: {t}")).small()),
            Err(e) => ui.label(
                RichText::new(format!("Profile not extrudable yet: {e}"))
                    .small()
                    .color(Color32::from_rgb(255, 200, 120)),
            ),
        };
        ui.separator();
        ui.label(RichText::new("Dimensions").strong());
        let dims: Vec<(usize, String, f64)> = s
            .editor
            .sketch
            .constraints
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.dimension().map(|d| (i, d.name.clone(), d.value)))
            .collect();
        if dims.is_empty() {
            ui.weak("None yet. Use Dimension (D) and click a line, circle or two points.");
        }
        let mut edit = None;
        for (i, name, v) in &dims {
            ui.horizontal(|ui| {
                let sel = s.editor.selection.contains(&Item::Constraint(*i));
                if ui
                    .selectable_label(sel, format!("{name} = {} mm", rcad_sketch::fmt_mm(*v)))
                    .on_hover_text("Click to edit the value")
                    .clicked()
                {
                    edit = Some(*i);
                }
            });
        }
        if let Some(i) = edit {
            s.editor.begin_dim_edit(i, false);
        }
        ui.separator();
        ui.label(RichText::new("Selection").strong());
        if s.editor.selection.is_empty() {
            ui.weak("Nothing selected.");
        }
        for it in s.editor.selection.clone() {
            let text = match it {
                Item::Point(p) => {
                    let xy = s.editor.sketch.point_xy(p).unwrap_or([f64::NAN; 2]);
                    format!(
                        "Point P{} ({}, {}) mm",
                        p.0,
                        rcad_sketch::fmt_mm(xy[0]),
                        rcad_sketch::fmt_mm(xy[1])
                    )
                }
                Item::Entity(e) => match s.editor.sketch.entity(e) {
                    Some(rcad_sketch::Entity::Line { a, b, .. }) => {
                        let (pa, pb) = (
                            s.editor.sketch.point_xy(*a).unwrap_or_default(),
                            s.editor.sketch.point_xy(*b).unwrap_or_default(),
                        );
                        format!(
                            "Line L{} length {} mm",
                            e.0,
                            rcad_sketch::fmt_mm((pb[0] - pa[0]).hypot(pb[1] - pa[1]))
                        )
                    }
                    Some(rcad_sketch::Entity::Circle { radius, .. }) => {
                        format!("Circle C{} Ø{} mm", e.0, rcad_sketch::fmt_mm(2.0 * radius))
                    }
                    Some(rcad_sketch::Entity::Arc { .. }) => {
                        match s.editor.sketch.arc_geometry(e) {
                            Some(g) => format!(
                                "Arc A{} R{} mm, sweep {}°",
                                e.0,
                                rcad_sketch::fmt_mm(g.radius),
                                rcad_sketch::fmt_mm(g.sweep.to_degrees())
                            ),
                            None => format!("Arc A{}", e.0),
                        }
                    }
                    Some(rcad_sketch::Entity::Ellipse {
                        center,
                        major,
                        minor,
                        ..
                    }) => {
                        let (c, m) = (
                            s.editor.sketch.point_xy(*center).unwrap_or_default(),
                            s.editor.sketch.point_xy(*major).unwrap_or_default(),
                        );
                        format!(
                            "Ellipse El{} radii {} × {} mm",
                            e.0,
                            rcad_sketch::fmt_mm((m[0] - c[0]).hypot(m[1] - c[1])),
                            rcad_sketch::fmt_mm(*minor)
                        )
                    }
                    Some(rcad_sketch::Entity::Spline { points, closed, .. }) => format!(
                        "Spline Sp{} ({} control points, {})",
                        e.0,
                        points.len(),
                        if *closed { "closed" } else { "open" }
                    ),
                    None => continue,
                },
                Item::Constraint(i) => s.editor.sketch.constraint_label(i),
            };
            ui.small(text);
        }
        ui.separator();
        ui.label(RichText::new("Constrain selection").strong());
        {
            let ed = &mut s.editor;
            let hv = ed.hv_disabled_reason();
            egui::Grid::new("constrain_grid")
                .num_columns(3)
                .spacing([4.0, 4.0])
                .show(ui, |ui| {
                    if action(ui, "Horizontal", hv) {
                        ed.constrain_lines(false);
                    }
                    if action(ui, "Vertical", hv) {
                        ed.constrain_lines(true);
                    }
                    if action(ui, "Coincident", ed.coincident_disabled_reason()) {
                        ed.constrain_coincident();
                    }
                    ui.end_row();
                    if action(ui, "Parallel", ed.two_lines_disabled_reason()) {
                        ed.constrain_parallel(false);
                    }
                    if action(ui, "Perpendicular", ed.two_lines_disabled_reason()) {
                        ed.constrain_parallel(true);
                    }
                    if action(ui, "Equal", ed.equal_disabled_reason()) {
                        ed.constrain_equal_radius();
                    }
                    ui.end_row();
                    if action(ui, "Angle", ed.two_lines_disabled_reason()) {
                        ed.constrain_angle();
                    }
                    ui.end_row();
                    if action(ui, "Midpoint", ed.midpoint_disabled_reason()) {
                        ed.constrain_midpoint();
                    }
                    if action(ui, "On line", ed.on_line_disabled_reason()) {
                        ed.constrain_on_line();
                    }
                    if action(ui, "Tangent", ed.tangent_disabled_reason()) {
                        ed.constrain_tangent();
                    }
                    ui.end_row();
                    if action(ui, "Fix / Unfix", ed.fix_disabled_reason()) {
                        ed.toggle_fix();
                    }
                    if action(ui, "Concentric", ed.concentric_disabled_reason()) {
                        ed.constrain_concentric();
                    }
                    if action(ui, "Collinear", ed.two_lines_disabled_reason()) {
                        ed.constrain_collinear();
                    }
                    ui.end_row();
                    if action(ui, "Symmetric", ed.symmetry_disabled_reason()) {
                        ed.constrain_symmetric();
                    }
                    let driven = ed
                        .selection
                        .first()
                        .and_then(|i| match i {
                            Item::Constraint(c) => ed.sketch.constraints.get(*c),
                            _ => None,
                        })
                        .and_then(|c| c.dimension())
                        .is_some();
                    if action(
                        ui,
                        "Driven / Driving",
                        (!driven).then_some("Select a dimension"),
                    ) {
                        ed.toggle_driven();
                    }
                    ui.end_row();
                });
            ui.weak("Hover a disabled button to see what to select.");
        }
        ui.collapsing("Constraints", |ui| {
            for i in 0..s.editor.sketch.constraints.len() {
                ui.small(s.editor.sketch.constraint_label(i));
            }
        });
        ui.collapsing("Details", |ui| {
            let r = &s.editor.report;
            ui.small(format!(
                "{:?} · DOF {} · residual {:.1e} · {} iterations",
                r.status, r.dof, r.residual, r.iterations
            ));
            ui.small(format!(
                "{} points · {} curves · {} constraints",
                s.editor.sketch.points.len(),
                s.editor.sketch.entities.len(),
                s.editor.sketch.constraints.len()
            ));
        });
    }

    pub(crate) fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Messages").strong());
            if ui.small_button("Clear").clicked() {
                self.diagnostics.clear();
                self.banner = None;
            }
        });
        // Wrapped: a long message must not widen the browser panel (a
        // refused edit would squeeze the 3D view to ~220 px at 1280x800).
        if let Some(e) = self.diagnostics.iter().rev().find(|d| d.error) {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Last error [{}]: {}", e.kind, e.text))
                        .small()
                        .color(ui.visuals().error_fg_color),
                )
                .wrap(),
            );
        }
        ui.collapsing("Log", |ui| {
            for d in self.diagnostics.iter().rev().take(12) {
                let t = RichText::new(format!("[{}] {}", d.kind, d.text)).small();
                let t = if d.error {
                    t.color(ui.visuals().warn_fg_color)
                } else {
                    t
                };
                ui.add(egui::Label::new(t).wrap());
            }
        });
    }

    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.horizontal_wrapped(|ui| {
            let stale = busy;
            if !busy && self.summary.measuring {
                ui.spinner();
                ui.label(
                    RichText::new("Measuring volume… (geometry is current)")
                        .italics()
                        .weak(),
                );
            } else if self.summary.bodies.len() > 1
                || self.summary.bodies.iter().any(|b| b.kind == "mesh")
            {
                // Several bodies: one compact entry per body (never a fused total).
                let n_vis = self.summary.bodies.iter().filter(|b| b.visible).count();
                let parts: Vec<String> = self
                    .summary
                    .bodies
                    .iter()
                    .map(|b| {
                        let v = match (&b.measurements, &b.mesh) {
                            (Some(m), _) => match &m.volume {
                                VolumeResult::Value { volume, .. } => format!("{volume:.2} mm³"),
                                VolumeResult::Unavailable { .. } => "volume n/a".into(),
                            },
                            (None, Some(d)) => format!("mesh {} tris", d.triangles),
                            (None, None) => "…".into(),
                        };
                        format!("{}{} {v}", b.name, if b.visible { "" } else { " (hidden)" })
                    })
                    .collect();
                let text = format!(
                    "Bodies {} ({n_vis} visible) · {}",
                    self.summary.bodies.len(),
                    parts.join(" · ")
                );
                if stale {
                    ui.label(RichText::new(format!("(pending) {text}")).weak().italics());
                } else {
                    ui.label(text);
                }
            } else {
            match &self.measurements {
                Some(m) => {
                    let mut vol = match &m.volume {
                        VolumeResult::Value {
                            volume,
                            error_estimate,
                            ..
                        } => format!("Volume {volume:.4} mm³ (±{error_estimate:.0e})"),
                        VolumeResult::Unavailable { reason } => {
                            format!("Volume unavailable: {reason}")
                        }
                    };
                    let (a, b) = (m.bounds.min, m.bounds.max);
                    vol += &format!(
                        " · Bounds X[{:.3}, {:.3}] Y[{:.3}, {:.3}] Z[{:.3}, {:.3}] · Solids {} {}",
                        a[0],
                        b[0],
                        a[1],
                        b[1],
                        a[2],
                        b[2],
                        m.shell_count,
                        if m.all_shells_closed {
                            "closed"
                        } else {
                            "OPEN"
                        }
                    );
                    if stale {
                        ui.label(RichText::new(format!("(pending) {vol}")).weak().italics())
                            .on_hover_text(
                                "Regenerating: these values belong to the previous model",
                            );
                    } else {
                        ui.label(vol);
                    }
                }
                None => {
                    ui.label(if stale { "Regenerating…" } else { "No body" });
                }
            }
            }
            ui.separator();
            if self.summary.open {
                // One reading of the save state: a new document is
                // "Untitled · not saved yet", never "saved … unsaved".
                let path = self
                    .summary
                    .path
                    .as_ref()
                    .map_or("Untitled".into(), |p| p.display().to_string());
                let state = if self.has_unsaved_changes() {
                    RichText::new("● unsaved changes").color(Color32::from_rgb(255, 200, 90))
                } else if self.summary.path.is_none() {
                    RichText::new("not saved yet").weak()
                } else {
                    RichText::new("saved").weak()
                };
                ui.label(RichText::new(path).weak());
                ui.label(state).probe("Save state");
                ui.label(RichText::new(format!("rev {}", self.summary.revision)).weak().small());
            } else {
                ui.label("no document");
            }
            ui.separator();
            if let Some((name, d)) = self.latencies.back() {
                ui.label(
                    RichText::new(format!("last {name}: {:.0} ms", d.as_secs_f64() * 1e3))
                        .weak()
                        .small(),
                );
            }
            if let Some(ft) = self.frame_ema {
                ui.label(
                    RichText::new(format!("UI {:.1} ms/frame", ft.as_secs_f64() * 1e3))
                        .weak()
                        .small(),
                );
            }
        });
    }

    // ----- windows -----

    /// The docked command dialog: one panel at the right of the canvas
    /// (its width is remembered in the preferences). Number fields take
    /// expressions with units and parameter names (red with the reason when
    /// invalid; OK is then disabled). Enter = OK, Esc = Cancel.
    pub(crate) fn dialog_ui(&mut self, outer: &mut egui::Ui, busy: bool) {
        let ctx = outer.ctx().clone();
        let _ = outer;
        let Some(mut dlg) = self.dialog.take() else {
            return;
        };
        crate::dialogs::set_expression_env(&self.summary.parameters);
        self.feed_selection_input(&mut dlg);
        let features = self.summary.features.clone();
        let sketches: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "sketch").collect();
        let extrudes: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "extrude").collect();
        let mut ok = false;
        let mut ok_allowed = true;
        let mut cancel = false;
        let mut error: Option<String> = None;
        let opened = self.last_dialog != Some(dlg.title());
        self.last_dialog = Some(dlg.title());
        AUTOFOCUS.with(|a| a.set(opened));
        if opened {
            // The docked window lays itself out over two frames (sizing pass,
            // then shown); ask for both even without further input.
            ctx.request_repaint();
        }
        // Enter confirms and Escape cancels (also while a field has focus).
        let (enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let width = self.shell.prefs.dialog_width.unwrap_or(360.0);
        let title = match self.shell.editing {
            Some(f) => format!("Edit {}", self.feature_name(f)),
            None => dlg.title().to_string(),
        };
        // Docked at the right edge of the canvas, under the ribbon (Fusion's
        // command-dialog position); one dialog at a time.
        let screen = ctx.content_rect();
        let view = self.view_rect.unwrap_or(screen);
        let max_h = (view.height() - 90.0).max(120.0);
        let dock = egui::Window::new(&title)
            .id(egui::Id::new("rcad_command_dialog"))
            .collapsible(false)
            .resizable([true, false])
            .default_width(width)
            .min_width(260.0)
            .anchor(
                egui::Align2::RIGHT_TOP,
                [view.max.x - screen.max.x - 8.0, view.min.y - screen.min.y + 8.0],
            )
            .show(&ctx, |ui| {
                crate::dialogs::begin_fields();
                // Fields scroll; OK / Cancel stay at the bottom in view.
                egui::ScrollArea::vertical()
                    .id_salt("dialog_scroll")
                    .max_height(max_h)
                    .show(ui, |ui| {
                match &mut dlg {
                    Dialog::Rectangle {
                        width,
                        height,
                        cx,
                        cy,
                    } => {
                        num_field(ui, "Rectangle width (mm)", width);
                        num_field(ui, "Rectangle height (mm)", height);
                        num_field(ui, "Rectangle center X (mm)", cx);
                        num_field(ui, "Rectangle center Y (mm)", cy);
                        ui.small("Centre in world XY coordinates.");
                    }
                    Dialog::Extrude(form) => {
                        self.extrude_form_ui(ui, form);
                        ok_allowed &= !self.extrude_needs_region(form);
                    }
                    Dialog::RectangleXz {
                        width,
                        height,
                        cx,
                        cz,
                    } => {
                        num_field(ui, "XZ rectangle width (mm)", width);
                        num_field(ui, "XZ rectangle height (mm)", height);
                        num_field(ui, "XZ rectangle center X (mm)", cx);
                        num_field(ui, "XZ rectangle center Z (mm)", cz);
                        ui.small("Must lie entirely at X > 0 to be revolved about world Z.");
                    }
                    Dialog::Thread {
                        diameter,
                        pitch,
                        length,
                        nut,
                        nut_width,
                        left_hand,
                        plane,
                        cx,
                        cy,
                    } => {
                        let place = match plane {
                            SketchPlane::Face(r) => format!(
                                "On {} {}",
                                self.feature_name(r.feature),
                                crate::face_role_label(&r.role)
                            ),
                            _ => "On world XY".to_string(),
                        };
                        ui.horizontal(|ui| {
                            ui.label(place);
                            if matches!(plane, SketchPlane::Face(_))
                                && ui.small_button("Use world XY").probe("Thread on world XY").clicked()
                            {
                                *plane = SketchPlane::WorldXY;
                            }
                        });
                        num_field(ui, "Thread axis X (mm)", cx);
                        num_field(ui, "Thread axis Y (mm)", cy);
                        num_field(ui, "Thread diameter (mm)", diameter);
                        num_field(ui, "Thread pitch (mm)", pitch);
                        num_field(ui, "Thread length (mm)", length);
                        ui.checkbox(left_hand, "Left hand").probe("Left hand");
                        ui.checkbox(nut, "Square nut (internal thread, 0.1 mm radial clearance)")
                            .probe("Square nut");
                        if *nut {
                            num_field(ui, "Nut width (mm)", nut_width);
                        }
                        ui.small("Faceted solid (planar facets within 0.01 mm of the ideal helicoid), stated 60° symmetric V test profile with one-turn lead-in/runout; not a certified standard thread. Length: a whole number of pitches. Printable geometry, not cosmetic. ADR 0011.");
                    }
                    Dialog::Revolve {
                        sketch,
                        angle,
                        axis,
                        reverse,
                        op,
                        target,
                    } => {
                        feature_combo(ui, "Revolve profile sketch", sketch, &sketches);
                        let bodies = self.summary.bodies.clone();
                        ok_allowed &= crate::dialogs::operation_ui(ui, "Revolve", op, target, &bodies);
                        num_field(ui, "Revolve angle (deg)", angle);
                        // Lines of the chosen sketch can be the axis.
                        let lines: Vec<rcad_sketch::EntityId> = sketch
                            .and_then(|id| self.summary.features.iter().find(|f| f.id == id))
                            .and_then(|f| f.sketch.as_ref())
                            .map(|s| {
                                s.entities
                                    .iter()
                                    .filter(|e| matches!(e, rcad_sketch::Entity::Line { .. }))
                                    .map(|e| e.id())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let label = |a: &rcad_document::RevolveAxis| match a {
                            rcad_document::RevolveAxis::SketchY => "Sketch Y axis".to_string(),
                            rcad_document::RevolveAxis::SketchX => "Sketch X axis".to_string(),
                            rcad_document::RevolveAxis::Line(e) => format!("Sketch line L{}", e.0),
                        };
                        egui::ComboBox::from_label("Revolve axis")
                            .selected_text(label(axis))
                            .show_ui(ui, |ui| {
                                for a in [
                                    rcad_document::RevolveAxis::SketchY,
                                    rcad_document::RevolveAxis::SketchX,
                                ]
                                .into_iter()
                                .chain(lines.iter().map(|e| rcad_document::RevolveAxis::Line(*e)))
                                {
                                    ui.selectable_value(axis, a, label(&a)).probe(&label(&a));
                                }
                            })
                            .response
                            .probe("Revolve axis");
                        ui.checkbox(reverse, "Reverse direction").probe("Revolve reverse");
                        ui.small("The profile must lie on one side of the axis (either side). Partial angles sweep toward −normal, or +normal with Reverse. A construction line is the usual axis.");
                    }
                    Dialog::LoftSweep { a, b, sweep, via } => {
                        if !*sweep {
                            feature_combo(ui, "Middle section (optional)", via, &sketches);
                        }
                        if *sweep {
                            feature_combo(ui, "Profile sketch", a, &sketches);
                            feature_combo(ui, "Path sketch", b, &sketches);
                            ui.small("The path sketch's lines and arcs form one open chain that starts at the profile sketch's origin, leaving along its normal, with tangent joints. Exact faces; refused otherwise with the reason.");
                        } else {
                            feature_combo(ui, "First profile sketch", a, &sketches);
                            feature_combo(ui, "Second profile sketch", b, &sketches);
                            ui.small("Parallel planes; polygons with the same number of corners whose corresponding edges are parallel (planar sides), or two coaxial circles (cone frustum). Corner 0 of the second profile is the one nearest corner 0 of the first (ADR 0008).");
                        }
                    }
                    Dialog::CircleOnFace {
                        extrude,
                        diameter,
                        cx,
                        cy,
                    } => {
                        feature_combo(ui, "End cap of extrusion", extrude, &extrudes);
                        num_field(ui, "Circle diameter (mm)", diameter);
                        num_field(ui, "Circle center X (mm)", cx);
                        num_field(ui, "Circle center Y (mm)", cy);
                        ui.small("Centre in the end cap's frame (the profile sketch's X/Y).");
                        ui.small("Attached semantically to the extrusion's end cap face.");
                    }
                    Dialog::Cut {
                        sketch,
                        blind,
                        depth,
                    } => {
                        feature_combo(ui, "Cut profile sketch", sketch, &sketches);
                        ui.horizontal(|ui| {
                            ui.radio_value(blind, false, "Through all")
                                .probe("Through all");
                            ui.radio_value(blind, true, "Blind depth")
                                .probe("Blind depth");
                        });
                        if *blind {
                            num_field(ui, "Pocket depth (mm)", depth);
                            ui.small("Cut from the sketch's cap face into the material; must be shallower than the part.");
                        } else {
                            ui.small("Removes material in both directions along the sketch normal.");
                        }
                    }
                    Dialog::SaveAs { path } => {
                        path_field(ui, "File path", path, PickMode::Save, file_pick::DOCUMENT);
                    }
                    Dialog::Open { path, safe_mode } => {
                        path_field(ui, "File path", path, PickMode::Open, file_pick::DOCUMENT);
                        ui.checkbox(safe_mode, "Safe mode (roll back before the first failing feature)")
                            .probe("Safe mode");
                    }
                    Dialog::ExportAssembly { path } => {
                        path_field(ui, "STEP path", path, PickMode::Save, file_pick::STEP);
                        ui.small("One STEP product per component (reused definitions written once), occurrences with their placements.");
                    }
                    Dialog::ShellFace { face, thickness } => {
                        let text = format!(
                            "{} {}",
                            self.feature_name(face.feature),
                            crate::face_role_label(&face.role)
                        );
                        ui.label(format!("Open face: {text}"));
                        ok_allowed &= self.selection_row(ui, "Faces to remove", &text);
                        num_field(ui, "Shell thickness (mm)", thickness);
                        ui.small("Hollows the body under this face (left open) to walls and floor of this thickness; bodies prismatic along the face normal.");
                    }
                    Dialog::Align { from, to } => {
                        ui.label(format!(
                            "Move the body of {} {}",
                            self.feature_name(from.feature),
                            crate::face_role_label(&from.role)
                        ));
                        let mine = self.selected_face().and_then(|f| f.body);
                        let targets: Vec<(String, rcad_document::FaceRef)> = self
                            .summary
                            .features
                            .iter()
                            .flat_map(|g| {
                                g.faces
                                    .iter()
                                    .filter(|fc| Some(fc.body) != mine)
                                    .map(move |fc| {
                                        (format!("{} {}", g.name, crate::face_role_label(&fc.reference.role)), fc.reference)
                                    })
                            })
                            .collect();
                        let current = targets
                            .iter()
                            .find(|(_, r)| Some(*r) == *to)
                            .map_or("(choose)".to_string(), |(l, _)| l.clone());
                        egui::ComboBox::from_label("Align onto face")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (l, r) in &targets {
                                    ui.selectable_value(to, Some(*r), l).probe(&format!("Align onto {l}"));
                                }
                            })
                            .response
                            .probe("Align onto face");
                        ok_allowed &= to.is_some();
                    }
                    Dialog::RigidGroup { occurrences } => {
                        ui.label("Occurrences to lock together:");
                        for (_, name, on) in occurrences.iter_mut() {
                            ui.checkbox(on, name.as_str()).probe(&format!("Rigid {name}"));
                        }
                        ok_allowed &= occurrences.iter().filter(|o| o.2).count() >= 2;
                    }
                    Dialog::DriveJoints { joints } => {
                        for (_, name, values) in joints.iter_mut() {
                            for (i, v) in values.iter_mut().enumerate() {
                                num_field(ui, &format!("{name} value {}", i + 1), v);
                            }
                        }
                        ui.small("Degrees for rotations, mm for slides; each joint is one undo step.");
                    }
                    Dialog::ExportStl(form) | Dialog::Export3mf(form) => {
                        ok_allowed &= self.print_export_ui(ui, form);
                    }
                    Dialog::ExportStep { path, scope } => {
                        path_field(ui, "STEP path", path, PickMode::Save, file_pick::STEP);
                        ok_allowed &= self.export_scope_ui(ui, scope, true).is_none();
                        ui.label(
                            "Exact B-rep (AP214); refused if an edge cannot be written exactly.",
                        );
                        ui.small("Exports the last committed model revision (queued after pending edits).");
                    }
                    Dialog::Hole {
                        face,
                        cx,
                        cy,
                        diameter,
                        through,
                        depth,
                        counterbore,
                        cb_diameter,
                        cb_depth,
                        offset,
                    } => {
                        let text = format!(
                            "{} {}",
                            self.feature_name(face.feature),
                            crate::face_role_label(&face.role)
                        );
                        ok_allowed &= self.selection_row(ui, "Face", &text);
                        num_field(ui, "Hole centre X (face mm)", cx);
                        num_field(ui, "Hole centre Y (face mm)", cy);
                        ui.small("Face coordinates: origin and axes of the face's frame (for a face of an XY-sketched extrusion: world X/Y).");
                        num_field(ui, "Hole diameter (mm)", diameter);
                        ui.checkbox(through, "Through all").probe("Hole through all");
                        if !*through {
                            num_field(ui, "Hole depth (mm)", depth);
                        }
                        ui.checkbox(counterbore, "Counterbore").probe("Counterbore");
                        if *counterbore {
                            num_field(ui, "Counterbore diameter (mm)", cb_diameter);
                            num_field(ui, "Counterbore depth (mm)", cb_depth);
                        }
                        num_field(ui, "Start plane offset (mm)", offset);
                        ui.small("0 starts on the face. A non-zero offset starts on a construction plane moved along the face's outward normal; that plane must hold a face of the same body (e.g. a step's other level).");
                        ui.small("Built as exact cuts into the material, each checked against the nominal shape; countersinks are not supported. Sizes and the offset (plane_offset) can be bound to parameters afterwards in the inspector.");
                    }
                    Dialog::OffsetPlane {
                        base,
                        face,
                        distance,
                        angle,
                    } => {
                        self.offset_plane_form_ui(ui, base, face, distance);
                        crate::panels::num_field(ui, "Plane angle (deg)", angle);
                        ui.small("Optional tilt about the base plane's local X axis through its origin (for a face: its frame's X axis), applied before the offset; 0 = parallel. Editable and bindable afterwards as plane_angle_deg.");
                    }
                    Dialog::Shell {
                        extrude,
                        open_end,
                        thickness,
                    } => {
                        self.shell_form_ui(ui, *extrude, open_end, thickness);
                    }
                    Dialog::DirectEdit(f) => f.ui(ui),
                    Dialog::Chamfer {
                        a,
                        b,
                        distance,
                        fillet,
                    } => {
                        let faces: Vec<rcad_commands::FaceSummary> = self
                            .summary
                            .features
                            .iter()
                            .flat_map(|f| f.faces.iter().cloned())
                            .collect();
                        let first = faces.iter().find(|f| f.reference == *a).cloned();
                        let label = |f: &rcad_commands::FaceSummary| {
                            format!(
                                "{} {}",
                                self.feature_name(f.reference.feature),
                                crate::face_role_label(&f.reference.role)
                            )
                        };
                        match &first {
                            Some(fa) => {
                                ui.label(format!("First face: {}", label(fa)));
                                let cross = |p: [f64; 3], q: [f64; 3]| {
                                    let c = [
                                        p[1] * q[2] - p[2] * q[1],
                                        p[2] * q[0] - p[0] * q[2],
                                        p[0] * q[1] - p[1] * q[0],
                                    ];
                                    (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
                                };
                                let others: Vec<_> = faces
                                    .iter()
                                    .filter(|f| {
                                        f.body == fa.body
                                            && f.reference != *a
                                            && cross(f.frame.z, fa.frame.z) > 1e-6
                                    })
                                    .collect();
                                let current = b
                                    .and_then(|r| others.iter().find(|f| f.reference == r))
                                    .map_or("choose…".to_string(), |f| label(f));
                                egui::ComboBox::from_label("Second face")
                                    .selected_text(current)
                                    .show_ui(ui, |ui| {
                                        for f in &others {
                                            ui.selectable_value(b, Some(f.reference), label(f))
                                                .probe(&label(f));
                                        }
                                    })
                                    .response
                                    .probe("Second face");
                            }
                            None => {
                                ui.label("The selected face is no longer available.");
                            }
                        }
                        if *fillet {
                            num_field(ui, "Fillet radius (mm)", distance);
                            ui.small("Exact solid fillet: a cylinder of this radius tangent to both faces. Supported when the body is prismatic along the edge (its planar faces parallel or perpendicular to it) and the edge ends on perpendicular faces; refused otherwise with the reason (concave edge, radius too large, other features within reach).");
                        } else {
                            num_field(ui, "Chamfer distance (mm)", distance);
                            ui.small("Equal-distance chamfer of the straight convex edge where the two faces meet. Refused for parallel or non-adjacent faces, concave edges, or when anything other than the edge wedge would be removed.");
                        }
                    }
                    Dialog::FilletLoop {
                        radius,
                        chamfer: true,
                        holes,
                        ..
                    } => {
                        num_field(ui, "Chamfer distance (mm)", radius);
                        ui.checkbox(holes, "Include hole edges")
                            .probe("Include hole edges");
                        ui.small("Equal-distance chamfer of every edge of this face's outer boundary: 45° planes along straight edges, cones along arcs, mitered sharp corners. The body must be a straight prism under the face; refused otherwise with the reason (distance not smaller than a rounded corner's radius, a hole within reach, the loop already finished).");
                    }
                    Dialog::FilletLoop { radius, holes, .. } => {
                        num_field(ui, "Fillet radius (mm)", radius);
                        ui.checkbox(holes, "Include hole edges")
                            .probe("Include hole edges");
                        ui.small("Exact solid fillet of every edge of this face's outer boundary: cylinders along straight edges, tori along arcs, a sphere corner where a rounded corner's radius equals this radius, mitered sharp corners. The body must be a straight prism under the face; refused otherwise with the reason (radius larger than a rounded corner, a hole within reach, the loop already filleted).");
                    }
                    Dialog::Pattern {
                        feature,
                        kind,
                        count,
                        spacing,
                        along_y,
                        reverse,
                        count2,
                        spacing2,
                        angle,
                        center_x,
                        center_y,
                        mirror,
                        mirror_face,
                        mirror_offset,
                    } => {
                        use crate::app::PatternKind;
                        ui.label(format!("Seed: {}", self.feature_name(*feature)));
                        // Edit Feature keeps the feature type (a stored
                        // linear pattern stays linear).
                        let fixed = self.shell.editing.is_some();
                        ui.add_enabled_ui(!fixed, |ui| {
                            ui.horizontal(|ui| {
                                ui.radio_value(kind, PatternKind::Linear, "Linear");
                                ui.radio_value(kind, PatternKind::Circular, "Circular");
                                ui.radio_value(kind, PatternKind::Mirror, "Mirror");
                            })
                        })
                        .response
                        .on_disabled_hover_text("Edit Feature cannot change the feature type");
                        match kind {
                            PatternKind::Linear => {
                                num_field(ui, "Pattern count", count);
                                num_field(ui, "Pattern spacing (mm)", spacing);
                                ui.horizontal(|ui| {
                                    ui.label("Direction");
                                    ui.radio_value(along_y, false, "Sketch X");
                                    ui.radio_value(along_y, true, "Sketch Y");
                                    ui.checkbox(reverse, "Reverse");
                                });
                                num_field(ui, "Second direction count", count2);
                                num_field(ui, "Second direction spacing (mm)", spacing2);
                                ui.small("Count includes the seed. Direction is in the seed sketch's local axes; a second direction count above 1 makes a rectangular grid along the other sketch axis. Instances act on the same target. Counts and spacings stay editable parameters.");
                            }
                            PatternKind::Circular => {
                                num_field(ui, "Pattern count", count);
                                num_field(ui, "Angle between instances (deg)", angle);
                                num_field(ui, "Centre x (mm)", center_x);
                                num_field(ui, "Centre y (mm)", center_y);
                                ui.small("Counter-clockwise about the seed sketch's normal through the centre (seed sketch coordinates). Count includes the seed; instances may not wrap past a full turn.");
                            }
                            PatternKind::Mirror => {
                                ui.horizontal(|ui| {
                                    ui.label("Mirror plane");
                                    ui.radio_value(mirror, SketchPlane::WorldYZ, "World YZ");
                                    ui.radio_value(mirror, SketchPlane::WorldXZ, "World XZ");
                                    ui.radio_value(mirror, SketchPlane::WorldXY, "World XY");
                                });
                                // Or a planar face of the model (semantic reference).
                                let faces: Vec<rcad_commands::FaceSummary> = self
                                    .summary
                                    .features
                                    .iter()
                                    .flat_map(|f| f.faces.iter().cloned())
                                    .collect();
                                let label = |r: &rcad_document::FaceRef| {
                                    format!(
                                        "{} {}",
                                        self.feature_name(r.feature),
                                        crate::face_role_label(&r.role)
                                    )
                                };
                                let current = match mirror {
                                    SketchPlane::Face(r) => label(r),
                                    _ => "world plane (above)".to_string(),
                                };
                                egui::ComboBox::from_label("Mirror face")
                                    .selected_text(current)
                                    .show_ui(ui, |ui| {
                                        for f in &faces {
                                            if ui
                                                .selectable_label(
                                                    *mirror == SketchPlane::Face(f.reference),
                                                    label(&f.reference),
                                                )
                                                .clicked()
                                            {
                                                *mirror = SketchPlane::Face(f.reference);
                                                *mirror_face = Some(f.reference);
                                            }
                                        }
                                    });
                                num_field(ui, "Mirror plane offset (mm)", mirror_offset);
                                ui.small("Adds the reflected copy of the seed's tool (the seed stays). A joined copy must land on a face of the target. A non-zero offset mirrors across a construction plane parallel to the chosen one (parameter plane_offset, bindable in the inspector). Or mirror across a planar face of the model (Mirror face; the face selected when the dialog opened is preselected).");
                            }
                        }
                    }
                    Dialog::ExportDxf { path, sketch, svg } => {
                        ok_allowed &= self.export_dxf_ui(ui, path, sketch, *svg);
                    }
                    Dialog::Import(form) => {
                        ok_allowed &= self.import_form_ui(ui, form);
                        if form.pending.is_some() {
                            ui.small("Importing… Cancel stops the import: nothing is imported.");
                        }
                    }
                }
                    });
                let invalid = crate::dialogs::invalid_fields();
                if let Some(why) = &invalid {
                    ui.colored_label(BAD_COLOR, why);
                }
                ok_allowed &= invalid.is_none();
                ui.separator();
                ui.horizontal(|ui| {
                    ok = ui
                        .add_enabled(ok_allowed && !busy_blocks(&dlg, busy), egui::Button::new("OK"))
                        .probe("OK")
                        .clicked();
                    cancel = ui.button("Cancel").probe("Cancel").clicked();
                });
                ui.weak("Enter = OK · Esc = Cancel");
            });
        // Remember a width the user dragged (not every layout jitter).
        let w = dock.map_or(width, |d| d.response.rect.width());
        if (w - width).abs() > 4.0 && w <= 640.0 && !ctx.input(|i| i.pointer.any_down()) {
            self.shell.prefs.dialog_width = Some(w.round());
            self.save_prefs();
        }
        for e in crate::dialogs::take_left_invalid() {
            self.diag(true, "Input", e);
        }
        AUTOFOCUS.with(|a| a.set(false));
        // Enter counts on the frame the dialog opens too: under load the click
        // that opened it and the Enter arrive in the same frame (no command
        // opens a dialog from an Enter key, so it cannot confirm itself).
        ok |= enter && ok_allowed && !busy_blocks(&dlg, busy);
        cancel |= esc;
        if cancel {
            // A pending import is cancelled too (stopped while it reads, or
            // answered as cancelled if still queued): nothing is imported.
            if let Dialog::Import(f) = &dlg
                && let Some(token) = f.pending
            {
                self.engine.cancel(token);
            }
            self.last_dialog = None;
            return; // dialog dropped; nothing created
        }
        if ok
            && let Dialog::OffsetPlane {
                base,
                distance,
                angle,
                ..
            } = &dlg
        {
            let parsed = crate::app::parse_num("plane offset", distance).and_then(|d| {
                let a = crate::app::parse_num("plane angle", angle)?;
                if a.abs() < 180.0 {
                    Ok((d, a))
                } else {
                    Err(format!(
                        "plane angle must be in (-180, 180) degrees, got {a}"
                    ))
                }
            });
            match parsed {
                Ok((d, a)) => {
                    self.last_dialog = None;
                    self.begin_sketch(SketchTarget::New {
                        plane: SketchPlane::Offset {
                            base: *base,
                            distance: d,
                            angle: a.to_radians(),
                        },
                    });
                    return;
                }
                Err(e) => {
                    self.diag(true, "Input", e);
                    self.dialog = Some(dlg);
                    return;
                }
            }
        }
        if ok && let Dialog::DriveJoints { joints } = &dlg {
            let mut cmds = vec![];
            for (joint, name, values) in joints {
                let v: Result<Vec<f64>, String> = values
                    .iter()
                    .map(|t| crate::app::parse_num(name, t))
                    .collect();
                match v {
                    Ok(values) => cmds.push(Command::SetJointValues {
                        joint: *joint,
                        values,
                    }),
                    Err(e) => error = Some(e),
                }
            }
            if error.is_none() {
                self.last_dialog = None;
                for c in cmds {
                    self.request(c);
                }
                return;
            }
        } else if ok {
            self.last_dialog = None;
            let built = self
                .redefine_command(&dlg)
                .unwrap_or_else(|| build_command(&dlg));
            match built {
                Ok(Command::Open { path, safe_mode }) => {
                    self.guarded(if safe_mode {
                        Guarded::OpenSafe(path)
                    } else {
                        Guarded::Open(path)
                    });
                    return;
                }
                Ok(cmd) => {
                    let cmd = if crate::app::built_from_pick(&dlg) {
                        self.at_pick_revision(cmd)
                    } else {
                        cmd
                    };
                    let keep = matches!(
                        dlg,
                        Dialog::ExportStl(_) | Dialog::Export3mf(_) | Dialog::Import(_)
                    );
                    let token = self.request_token(cmd);
                    if let Dialog::Import(f) = &mut dlg {
                        f.pending = token;
                        f.error = None;
                    }
                    if let Dialog::ExportStl(f) | Dialog::Export3mf(f) = &mut dlg {
                        f.pending = token;
                        f.error = None;
                    }
                    if keep {
                        self.dialog = Some(dlg);
                    }
                    return;
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            self.diag(true, "Input", e);
        }
        self.dialog = Some(dlg);
    }

    pub(crate) fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(g) = self.confirm.clone() else {
            return;
        };
        let what = match g {
            Guarded::New | Guarded::NewAndSketch | Guarded::NewTemplate(_) => {
                "creating a new document"
            }
            Guarded::Open(_) | Guarded::OpenSafe(_) | Guarded::OpenCopy(_) => {
                "opening another document"
            }
            Guarded::Close => "closing",
            Guarded::Exit => "quitting",
        };
        let sketch_open = self.sketch.as_ref().is_some_and(|s| s.editor.modified());
        let mut answer = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "The document has unsaved changes{}. Save before {what}?",
                    if sketch_open {
                        " (including an unfinished sketch, which would be discarded)"
                    } else {
                        ""
                    }
                ));
                ui.horizontal(|ui| {
                    let can_save = self.summary.path.is_some() && self.sketch.is_none();
                    // Distinct from the toolbar's Save / Save As… labels.
                    let save_label = if can_save {
                        "Save first"
                    } else {
                        "Save As first…"
                    };
                    if ui.button(save_label).clicked() {
                        answer = Some(ConfirmAnswer::Save);
                    }
                    if ui
                        .button("Discard changes")
                        .probe("Discard changes")
                        .clicked()
                    {
                        answer = Some(ConfirmAnswer::Discard);
                    }
                    if ui.button("Keep editing").probe("Keep editing").clicked() {
                        answer = Some(ConfirmAnswer::Cancel);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(ConfirmAnswer::Cancel);
        }
        if let Some(a) = answer {
            let exit = matches!(g, Guarded::Exit);
            self.answer_confirm(a);
            if exit && self.exit_confirmed {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(crate) fn rename_ui(&mut self, ctx: &egui::Context) {
        let Some((id, mut buf)) = self.rename.clone() else {
            return;
        };
        let mut done = None;
        egui::Window::new("Rename feature")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
            .show(ctx, |ui| {
                let l = ui.label("New name");
                let r = ui
                    .add(egui::TextEdit::singleline(&mut buf).desired_width(220.0))
                    .labelled_by(l.id)
                    .probe("New name");
                if !r.has_focus() && !ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = Some(false);
                }
                ui.horizontal(|ui| {
                    if ui.button("Rename").probe("Rename").clicked() {
                        done = Some(true);
                    }
                    if ui.button("Cancel rename").probe("Cancel rename").clicked() {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                self.rename = None;
                self.request(Command::RenameFeature {
                    feature: id,
                    name: buf,
                });
            }
            Some(false) => self.rename = None,
            None => self.rename = Some((id, buf)),
        }
    }

    pub(crate) fn help_ui(&mut self, ctx: &egui::Context) {
        use crate::shell::{KeymapLayer, keymap, registry};
        let mut open = self.show_help;
        let layer = self.shell.prefs.keymap;
        egui::Window::new("Help")
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Keyboard layer:");
                    for l in [KeymapLayer::Fusion, KeymapLayer::Classic] {
                        if ui
                            .selectable_label(layer == l, l.label())
                            .named(&format!("Keymap {}", l.label()))
                            .clicked()
                        {
                            self.set_keymap(l);
                            self.save_prefs();
                        }
                    }
                });
                ui.label(RichText::new("Navigation").strong());
                ui.label(rcad_viewport::NAV_HELP);
                ui.label("In sketch mode the left button draws; use the middle button to orbit and the right button to pan.");
                ui.label(format!(
                    "Views: {}",
                    registry::shortcut_line(layer, |d| matches!(
                        d.places.first(),
                        Some(registry::Place::View)
                    ))
                ));
                ui.separator();
                ui.label(RichText::new("Document").strong());
                ui.label(registry::shortcut_line(layer, |d| {
                    d.places
                        .iter()
                        .any(|p| matches!(p, registry::Place::File | registry::Place::AppBar))
                }));
                ui.separator();
                ui.label(RichText::new("Modeling").strong());
                ui.label(registry::shortcut_line(layer, |d| {
                    d.places
                        .iter()
                        .any(|p| matches!(p, registry::Place::Ribbon { .. }))
                }));
                ui.separator();
                ui.label(RichText::new("Sketch").strong());
                ui.label(keymap::sketch_help(layer));
                ui.label("Constraints: lines for Horizontal/Vertical; two lines for Parallel/Perpendicular/Equal length/Angle (degrees, counter-clockwise); two points for Coincident; a line + a point for Midpoint/On line; a line + arc sharing an end for Tangent; two arcs/circles for Equal radius. X toggles construction (reference) geometry, which profiles ignore.");
                ui.label("Esc cancels the gesture, then the tool · Shift while drawing disables snapping");
                ui.label("Snaps: existing points (shared endpoint), horizontal/vertical alignment (adds an H/V constraint), grid (position only).");
                ui.label("Ctrl+Z inside a sketch undoes sketch steps; Finish Sketch records one document undo step; Cancel Sketch discards the session.");
                ui.separator();
                egui::CollapsingHeader::new("All commands")
                    .id_salt("help_all_commands")
                    .show(ui, |ui| {
                        egui::Grid::new("help_commands").striped(true).show(ui, |ui| {
                            for d in registry::COMMANDS {
                                ui.label(d.label);
                                ui.label(d.key_text(layer));
                                ui.label(registry::placement_text(d));
                                ui.end_row();
                            }
                        });
                    });
                ui.label("Shortcuts are ignored while a text field has focus.");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.weak(format!("R-CAD {}", about::VERSION));
                    if ui.small_button("About R-CAD").clicked() {
                        self.shell.show_about = true;
                    }
                });
            });
        self.show_help = open;
    }

    /// File ▸ About R-CAD: version, license and source of this build.
    pub(crate) fn about_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.shell.show_about;
        egui::Window::new("About R-CAD")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                ui.heading("R-CAD");
                ui.label(format!("Version {}", about::VERSION))
                    .probe("About version");
                ui.label("Local-first parametric mechanical CAD in Rust.");
                ui.label(format!("License: {}", about::LICENSE));
                ui.horizontal(|ui| {
                    ui.label("Source:");
                    ui.hyperlink(about::REPOSITORY);
                });
                ui.small(
                    "Geometry is exact B-rep (f64, millimetres); the display mesh is \
                     derived from it. Third-party components and their licenses are \
                     listed in docs/DEPENDENCIES.md.",
                );
            });
        self.shell.show_about = open;
    }
}

/// Build facts shown by About (from the package manifest).
mod about {
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");
    /// The manifest's `repository`, or the project's public repository
    /// when the manifest names none.
    pub const REPOSITORY: &str = match env!("CARGO_PKG_REPOSITORY").as_bytes() {
        [] => "https://github.com/aped/R-CAD",
        _ => env!("CARGO_PKG_REPOSITORY"),
    };
}

thread_local! {
    /// Set when a dialog opens: the first text field drawn takes keyboard focus.
    static AUTOFOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn field(ui: &mut egui::Ui, label: &str, buf: &mut String, width: f32) {
    field_checked(ui, label, buf, width, false);
}

/// A labelled text field; `numeric`: a value input (expression with units,
/// checked against the document's parameters, red with the reason when it
/// does not evaluate).
fn field_checked(ui: &mut egui::Ui, label: &str, buf: &mut String, width: f32, numeric: bool) {
    // Empty is left to the command (optional fields such as the facet angle).
    let err = if numeric && !buf.trim().is_empty() {
        crate::dialogs::check_value(label, buf).err()
    } else {
        None
    };
    ui.horizontal(|ui| {
        let l = ui.label(label);
        let mut edit = egui::TextEdit::singleline(buf).desired_width(width);
        if err.is_some() {
            edit = edit.text_color(BAD_COLOR);
        }
        let r = ui.add(edit).labelled_by(l.id).probe(label);
        if AUTOFOCUS.with(|a| a.replace(false)) {
            r.request_focus();
        }
        if let Some(e) = &err {
            crate::dialogs::note_invalid(label, e, r.lost_focus());
            r.on_hover_text(e.as_str());
        }
    });
}

pub(crate) fn num_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    field_checked(ui, label, buf, 90.0, true);
}

pub(crate) fn text_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    field(ui, label, buf, 320.0);
}

/// A narrow text field for the Browser column (keeps it from widening and
/// squeezing the 3D view).
pub(crate) fn short_text_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    field(ui, label, buf, 120.0);
}

/// A path text field with a **Browse…** button (native file dialog, [`crate::file_pick`]).
/// The text box keeps the label as its name, so typed input still works.
pub(crate) fn path_field(
    ui: &mut egui::Ui,
    label: &str,
    buf: &mut String,
    mode: crate::file_pick::PickMode,
    filters: crate::file_pick::Filters,
) {
    ui.horizontal(|ui| {
        field(ui, label, buf, 280.0);
        browse_button_text(ui, label, buf, mode, filters, "Browse…");
    });
}

/// [`path_field`] for the narrow Browser column: a compact "…" button and a text box
/// narrowed by its width, so the column is no wider than with the plain field.
pub(crate) fn short_path_field(
    ui: &mut egui::Ui,
    label: &str,
    buf: &mut String,
    mode: crate::file_pick::PickMode,
    filters: crate::file_pick::Filters,
) {
    ui.horizontal(|ui| {
        field(ui, label, buf, 92.0);
        browse_button_text(ui, label, buf, mode, filters, "…");
    });
}

/// A compact "…" browse button, for fields in fixed-width panels.
pub(crate) fn browse_button_compact(
    ui: &mut egui::Ui,
    key: &str,
    buf: &mut String,
    mode: crate::file_pick::PickMode,
    filters: crate::file_pick::Filters,
) {
    browse_button_text(ui, key, buf, mode, filters, "…");
}

/// The **Browse…** button for the path field named `key` (its accessible name is
/// "Browse <key>"); a chosen path replaces `buf`.
fn browse_button_text(
    ui: &mut egui::Ui,
    key: &str,
    buf: &mut String,
    mode: crate::file_pick::PickMode,
    filters: crate::file_pick::Filters,
    text: &str,
) {
    use crate::file_pick as fp;
    if let Some(p) = fp::take(key) {
        *buf = p.to_string_lossy().into_owned();
    }
    let what = match mode {
        fp::PickMode::Open => "Choose a file",
        fp::PickMode::Save => "Choose where to save",
        fp::PickMode::Folder => "Choose a folder",
    };
    let clicked = ui
        .add_enabled(!fp::busy(), egui::Button::new(text).small())
        .on_hover_text(what)
        .on_disabled_hover_text("A file dialog is already open")
        .named(&format!("Browse {key}"))
        .clicked();
    if clicked {
        let (start_dir, file_name) = fp::start_from(buf);
        let file_name = if mode == fp::PickMode::Save {
            file_name
        } else {
            None
        };
        fp::request(
            ui.ctx(),
            key,
            fp::PickRequest {
                mode,
                title: format!("{what}: {key}"),
                filters,
                start_dir,
                file_name,
            },
        );
    }
}

/// Opening a document waits for pending work (other commands just queue).
fn busy_blocks(d: &Dialog, busy: bool) -> bool {
    busy && matches!(d, Dialog::Open { .. })
}

pub(crate) fn feature_combo(
    ui: &mut egui::Ui,
    label: &str,
    sel: &mut Option<FeatureId>,
    options: &[&FeatureSummary],
) {
    let text = sel
        .and_then(|id| options.iter().find(|f| f.id == id))
        .map_or("(choose)".to_string(), |f| format!("{} {}", f.id, f.name));
    egui::ComboBox::from_label(label)
        .selected_text(text)
        .show_ui(ui, |ui| {
            for f in options {
                ui.selectable_value(sel, Some(f.id), format!("{}: {}", f.id, f.name));
            }
        });
}

impl RcadApp {
    /// Project a planar face or another sketch into this
    /// sketch (associative), and unlink existing projections.
    fn projection_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        let Some(z) = f.frame.map(|fr| fr.z) else {
            return;
        };
        let pos = |id: FeatureId| self.summary.features.iter().position(|x| x.id == id);
        let me = pos(f.id);
        let parallel = |w: [f64; 3]| (w[0] * z[0] + w[1] * z[1] + w[2] * z[2]).abs() > 1.0 - 1e-9;
        // Faces of earlier features parallel to the sketch, and earlier
        // sketches on parallel planes.
        let mut choices: Vec<(String, rcad_document::ProjectSource)> = Vec::new();
        for g in &self.summary.features {
            if pos(g.id) >= me {
                continue;
            }
            for fc in &g.faces {
                if parallel(fc.frame.z) {
                    choices.push((
                        format!("{} {:?}", g.name, fc.reference.role),
                        rcad_document::ProjectSource::Face(fc.reference),
                    ));
                }
            }
            if g.kind == "sketch" && g.frame.is_some_and(|fr| parallel(fr.z)) {
                choices.push((
                    format!("Sketch {}", g.name),
                    rcad_document::ProjectSource::Sketch(g.id),
                ));
            }
        }
        // Intersect: bodies that exist before this sketch, at their last
        // change before it.
        let mut bodies: Vec<(BodyId, FeatureId, String)> = Vec::new();
        for g in &self.summary.features {
            if pos(g.id) >= me {
                continue;
            }
            for b in g.creates.iter().copied().chain(g.body) {
                match bodies.iter_mut().find(|(x, _, _)| *x == b) {
                    Some(e) => e.1 = g.id,
                    None => {
                        let name = self
                            .summary
                            .bodies
                            .iter()
                            .find(|x| x.id == b)
                            .map_or_else(|| b.to_string(), |x| x.name.clone());
                        bodies.push((b, g.id, name));
                    }
                }
            }
        }
        let header = egui::CollapsingHeader::new("Project geometry into this sketch")
            .id_salt(("project_into", f.id.0))
            .show(ui, |ui| {
                if choices.is_empty() {
                    ui.weak("No earlier parallel face or sketch to project.");
                }
                for (label, src) in choices {
                    ui.horizontal(|ui| {
                        ui.label(&label);
                        if ui
                            .small_button("Project")
                            .named(&format!("Project {label}"))
                            .clicked()
                        {
                            self.request(Command::Project {
                                sketch: f.id,
                                source: src,
                                construction: false,
                                at: None,
                            });
                        }
                        if ui.small_button("as construction").clicked() {
                            self.request(Command::Project {
                                sketch: f.id,
                                source: src,
                                construction: true,
                                at: None,
                            });
                        }
                    });
                }
                ui.separator();
                let picking = self.edge_pick_for == Some(f.id);
                if ui
                    .add(egui::Button::new("Project edge (pick in view)").selected(picking))
                    .on_hover_text(
                        "Then click a planar face next to the edge to project: the face edge \
                         nearest to the click is projected along the sketch normal (a circle on an \
                         oblique face becomes an exact ellipse)",
                    )
                    .clicked()
                {
                    self.edge_pick_for = if picking { None } else { Some(f.id) };
                }
                for (b, after, name) in bodies {
                    if ui
                        .small_button(format!("Intersect {name}"))
                        .named(&format!("Intersect {name}"))
                        .on_hover_text(
                            "Add the section of this body by the sketch plane (lines, arcs, \
                             circles), kept associative",
                        )
                        .clicked()
                    {
                        self.request(Command::Project {
                            sketch: f.id,
                            source: rcad_document::ProjectSource::Section { body: b, after },
                            construction: false,
                            at: None,
                        });
                    }
                }
                ui.small(
                    "Projected lines/arcs follow their source at every regeneration; if the source \
                 changes shape the sketch reports a broken reference until you unlink it.",
                );
                for (i, p) in f.projections.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Link {}: {p:?}", i + 1));
                        if ui
                            .small_button("Unlink")
                            .named(&format!("Unlink {}", i + 1))
                            .clicked()
                        {
                            self.request(Command::UnlinkProjection {
                                sketch: f.id,
                                link: i,
                            });
                        }
                    });
                }
            });
        header
            .header_response
            .probe("Project geometry into this sketch");
    }
}

impl RcadApp {
    /// Named configurations. "Save
    /// current" stores every user parameter's expression and the features
    /// suppressed now; Activate applies one (one Undo step each).
    fn configurations_panel(&mut self, ui: &mut egui::Ui) {
        let configs = self.summary.configurations.clone();
        let active = self.summary.active_configuration.clone();
        let header = egui::CollapsingHeader::new(format!("Configurations ({})", configs.len()))
            .id_salt("configurations_panel")
            .open(self.shell.take_focus(crate::shell::Focus::Configurations))
            .show(ui, |ui| {
                for c in &configs {
                    ui.horizontal(|ui| {
                        let is_active = active.as_deref() == Some(c.name.as_str());
                        let t = if is_active {
                            RichText::new(format!("● {}", c.name)).strong()
                        } else {
                            RichText::new(&c.name)
                        };
                        ui.label(t);
                        if !c.part_number.is_empty() {
                            ui.label(RichText::new(&c.part_number).weak().small());
                        }
                        if ui
                            .add_enabled(!is_active, egui::Button::new("Activate").small())
                            .named(&format!("Activate {}", c.name))
                            .clicked()
                        {
                            self.request(Command::ActivateConfiguration {
                                name: c.name.clone(),
                            });
                        }
                        if ui
                            .small_button("Delete")
                            .named(&format!("Delete configuration {}", c.name))
                            .clicked()
                        {
                            self.request(Command::DeleteConfiguration {
                                name: c.name.clone(),
                            });
                        }
                    })
                    .response
                    .on_hover_text(format!(
                        "{}; suppresses {}",
                        c.values
                            .iter()
                            .map(|(k, v)| format!("{k} = {v}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if c.suppressed.is_empty() {
                            "nothing".to_string()
                        } else {
                            c.suppressed
                                .iter()
                                .map(|f| f.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ));
                }
                ui.horizontal(|ui| {
                    let f = &mut self.config_form;
                    ui.add(
                        egui::TextEdit::singleline(&mut f.0)
                            .hint_text("name")
                            .desired_width(80.0),
                    )
                    .named("Configuration name");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.1)
                            .hint_text("part number")
                            .desired_width(80.0),
                    )
                    .named("Part number");
                });
                if ui
                    .button("Save current as configuration")
                    .probe("Save current as configuration")
                    .on_hover_text(
                        "Stores every user parameter's current expression and the features \
                         suppressed now (they become configured features)",
                    )
                    .clicked()
                {
                    let (name, part) = self.config_form.clone();
                    let values = self
                        .summary
                        .parameters
                        .iter()
                        .map(|p| (p.name.clone(), p.expression.clone()))
                        .collect();
                    let suppressed = self
                        .summary
                        .features
                        .iter()
                        .filter(|f| f.suppressed)
                        .map(|f| f.id)
                        .collect();
                    self.request(Command::SetConfiguration {
                        name,
                        values,
                        suppressed,
                        part_number: part,
                        // Occurrence exclusion: through the command API.
                        excluded_occurrences: Vec::new(),
                    });
                }
            });
        header.header_response.probe("Configurations header");
    }
}

impl RcadApp {
    /// Sheet-metal flange from this sketch's open line chain, and the
    /// flat-pattern export of a flange feature (docs/SHEET_METAL.md).
    fn sheet_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        let mut send = None;
        let header = egui::CollapsingHeader::new("Sheet metal flange")
            .id_salt(("sheet_ui", f.id.0))
            .open(self.shell.take_focus(crate::shell::Focus::SheetFlange))
            .show(ui, |ui| {
                let b = &mut self.sheet_form;
                for (i, l) in ["Thickness (mm)", "Inner bend radius (mm)", "K-factor", "Width (mm)"]
                    .iter()
                    .enumerate()
                {
                    num_field(ui, l, &mut b[i]);
                }
                let v: Vec<Option<f64>> = b
                    .iter()
                    .map(|t| t.trim().parse::<f64>().ok().filter(|x| x.is_finite()))
                    .collect();
                if ui
                    .add_enabled(v.iter().all(Option::is_some), egui::Button::new("Create flange"))
                    .probe("Create flange")
                    .clicked()
                {
                    send = Some(Command::SheetMetal {
                        name: None,
                        sketch: f.id,
                        thickness: v[0].unwrap_or(1.0),
                        radius: v[1].unwrap_or(1.0),
                        k_factor: v[2].unwrap_or(0.44),
                        width: v[3].unwrap_or(10.0),
                        flip: false,
                        body_name: None,
                    });
                }
                ui.small("Open chain of lines = one sheet surface; material to the right of the chain; bends at the corners (inner radius); flat length by K-factor.");
            });
        header.header_response.probe("Sheet metal flange");
        if let Some(c) = send {
            self.request(c);
        }
    }

    /// Flat-pattern DXF export of a sheet-metal flange feature.
    pub(crate) fn flat_pattern_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        ui.label("Flat pattern (DXF, layers CUT / BEND):");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.flat_path).desired_width(192.0))
                .named("Flat pattern DXF path");
            browse_button_compact(
                ui,
                "Flat pattern DXF path",
                &mut self.flat_path,
                PickMode::Save,
                file_pick::DXF,
            );
        });
        if ui
            .button("Export flat pattern")
            .probe("Export flat pattern")
            .clicked()
        {
            let c = Command::ExportFlatPattern {
                feature: f.id,
                path: self.flat_path.clone().into(),
            };
            self.request(c);
        }
    }
}
