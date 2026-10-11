//! Panels around the 3D view: command bar, browser (feature tree), contextual
//! inspector, status bar, dialogs, confirmation, rename and help windows.
//!
//! Commands that cannot run are disabled with the reason as hover text (and,
//! in the inspector, as visible text). Internal ids and solver diagnostics
//! live in collapsible "Details" sections.

use eframe::egui::{self, Color32, RichText};
use f420_commands::{Command, FeatureSummary};
use f420_document::{FaceRole, FeatureId, SketchPlane};
use f420_kernel::VolumeResult;
use f420_sketch::SolveStatus;

use crate::app::{ConfirmAnswer, Dialog, F420App, Guarded, SketchTarget, build_command};
use crate::place::Tool;
use crate::sketch_edit::{Item, SketchTool};

const OK_COLOR: Color32 = Color32::from_rgb(120, 220, 120);
const UNDER_COLOR: Color32 = Color32::from_rgb(120, 190, 255);
const BAD_COLOR: Color32 = Color32::from_rgb(255, 110, 110);

/// Button that is disabled with a visible reason on hover.
fn action(ui: &mut egui::Ui, label: &str, blocker: Option<&str>) -> bool {
    let r = ui.add_enabled(blocker.is_none(), egui::Button::new(label));
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

impl F420App {
    // ----- command bars -----

    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, busy: bool) {
        let open = self.summary.open;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Fission420");
            ui.separator();
            if ui
                .button("New")
                .on_hover_text("New document (Ctrl+N)")
                .clicked()
            {
                self.guarded(Guarded::New);
            }
            if ui.button("Open…").on_hover_text("Open (Ctrl+O)").clicked() {
                self.dialog = Some(Dialog::Open {
                    path: self.default_path("f420.json"),
                });
            }
            if ui
                .add_enabled(open, egui::Button::new("Save"))
                .on_hover_text("Save (Ctrl+S)")
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(open, egui::Button::new("Save As…"))
                .on_hover_text("Save as (Ctrl+Shift+S)")
                .clicked()
            {
                self.dialog = Some(Dialog::SaveAs {
                    path: self.default_path("f420.json"),
                });
            }
            if ui.add_enabled(open, egui::Button::new("Close")).clicked() {
                self.guarded(Guarded::Close);
            }
            ui.menu_button("Export", |ui| {
                if ui
                    .add_enabled(open, egui::Button::new("Export STL…"))
                    .clicked()
                {
                    self.dialog = Some(Dialog::ExportStl {
                        path: self.default_path("stl"),
                        chord: "0.01".into(),
                    });
                    ui.close();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Export STEP…"))
                    .clicked()
                {
                    self.dialog = Some(Dialog::ExportStep {
                        path: self.default_path("step"),
                    });
                    ui.close();
                }
            });
            ui.separator();
            // Fixed labels so the bar never shifts; the action is in the tooltip.
            let undo_tip = match &self.summary.can_undo {
                Some(l) => format!("Undo: {l} (Ctrl+Z)"),
                None => "Nothing to undo".into(),
            };
            if ui
                .add_enabled(self.summary.can_undo.is_some(), egui::Button::new("Undo"))
                .on_hover_text(&undo_tip)
                .on_disabled_hover_text(&undo_tip)
                .clicked()
            {
                self.request(Command::Undo);
            }
            let redo_tip = match &self.summary.can_redo {
                Some(l) => format!("Redo: {l} (Ctrl+Shift+Z)"),
                None => "Nothing to redo".into(),
            };
            if ui
                .add_enabled(self.summary.can_redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(&redo_tip)
                .on_disabled_hover_text(&redo_tip)
                .clicked()
            {
                self.request(Command::Redo);
            }
            ui.separator();
            self.modeling_buttons(ui, busy);
            ui.separator();
            ui.menu_button("View", |ui| {
                if ui.button("Fit (F)").clicked() {
                    self.fit_view();
                    ui.close();
                }
                for (key, preset) in crate::view3d::VIEW_KEYS {
                    if ui
                        .button(format!("{} ({})", preset.label(), key.name()))
                        .clicked()
                    {
                        self.set_view(*preset);
                        ui.close();
                    }
                }
            });
            if ui.button("Fit view").on_hover_text("F").clicked() {
                self.fit_view();
            }
            if ui.button("Help").on_hover_text("F1").clicked() {
                self.show_help = !self.show_help;
            }
            if busy {
                ui.separator();
                ui.spinner();
                ui.label("Regenerating…");
            }
        });
    }

    fn modeling_buttons(&mut self, ui: &mut egui::Ui, _busy: bool) {
        let open = self.summary.open;
        // Create Sketch: on the selected supported face, otherwise world XY.
        let face = self.selected_face().cloned();
        let (plane, blocker): (Option<SketchPlane>, Option<String>) = if !open {
            (None, Some("Open or create a document first".into()))
        } else {
            match &face {
                Some(f) => match (f.reference, f.sketch_blocker()) {
                    (Some(r), _) => (Some(SketchPlane::Face(r)), None),
                    (None, Some(why)) => (None, Some(format!("Selected face: {why}"))),
                    (None, None) => (None, None),
                },
                None => (Some(SketchPlane::WorldXY), None),
            }
        };
        let hover = match &plane {
            Some(SketchPlane::Face(_)) => "Sketch on the selected face",
            _ => "Sketch on the world XY plane (select a cap face first to sketch on it)",
        };
        let r = ui.add_enabled(blocker.is_none(), egui::Button::new("Create Sketch"));
        let r = match &blocker {
            Some(b) => r.on_disabled_hover_text(b),
            None => r.on_hover_text(hover),
        };
        if r.clicked()
            && let Some(plane) = plane
        {
            self.begin_sketch(SketchTarget::New { plane });
        }
        let sketch_sel = self
            .selected
            .and_then(|id| self.summary.features.iter().find(|f| f.id == id))
            .filter(|f| f.kind == "sketch")
            .cloned();
        let has_sketch = self.summary.features.iter().any(|f| f.kind == "sketch");
        let extrude_block = if !has_sketch {
            Some("Create a sketch with a closed profile first".to_string())
        } else if self.has_body() {
            Some("Only one base body per document is supported (product limit)".to_string())
        } else {
            sketch_sel.as_ref().and_then(|s| self.extrude_blocker(s))
        };
        if action(ui, "Extrude…", extrude_block.as_deref()) {
            let sketch = sketch_sel
                .as_ref()
                .map(|s| s.id)
                .or_else(|| self.unused_sketch());
            self.dialog = Some(Dialog::Extrude {
                sketch,
                distance: "10".into(),
            });
        }
        let cut_block = if !has_sketch {
            Some("Create a circle sketch on a cap face first".to_string())
        } else if !self.has_body() {
            Some("Create a body first (Extrude)".to_string())
        } else {
            sketch_sel.as_ref().and_then(|s| self.cut_blocker(s))
        };
        if action(ui, "Cut through all…", cut_block.as_deref()) {
            let sketch = sketch_sel
                .as_ref()
                .map(|s| s.id)
                .or_else(|| self.unused_sketch());
            self.dialog = Some(Dialog::Cut {
                sketch,
                blind: false,
                depth: "5".into(),
            });
        }
        let revolve_block = if !has_sketch {
            Some("Create a sketch on XZ first (Templates > Rectangle on XZ…)".to_string())
        } else if self.has_body() {
            Some("Only one base body per document is supported (product limit)".to_string())
        } else {
            None
        };
        if action(ui, "Revolve…", revolve_block.as_deref()) {
            self.dialog = Some(Dialog::Revolve {
                sketch: self.unused_sketch(),
                angle: "360".into(),
            });
        }
        let has_extrude = self.summary.features.iter().any(|f| f.kind == "extrude");
        ui.menu_button("Templates", |ui| {
            ui.weak("Dimensioned sketch templates (dialogs)");
            if ui
                .add_enabled(open, egui::Button::new("Rectangle on XY…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Rectangle {
                    width: "80".into(),
                    height: "50".into(),
                    cx: "0".into(),
                    cy: "0".into(),
                });
                ui.close();
            }
            if ui
                .add_enabled(open, egui::Button::new("Rectangle on XZ…"))
                .clicked()
            {
                self.dialog = Some(Dialog::RectangleXz {
                    width: "20".into(),
                    height: "10".into(),
                    cx: "30".into(),
                    cz: "5".into(),
                });
                ui.close();
            }
            if ui
                .add_enabled(has_extrude, egui::Button::new("Circle on face…"))
                .on_disabled_hover_text("Needs an extrusion")
                .clicked()
            {
                let last = self
                    .summary
                    .features
                    .iter()
                    .rev()
                    .find(|f| f.kind == "extrude");
                self.dialog = Some(Dialog::CircleOnFace {
                    extrude: last.map(|f| f.id),
                    diameter: "10".into(),
                    cx: "0".into(),
                    cy: "0".into(),
                });
                ui.close();
            }
            ui.separator();
            ui.weak("Drag to place");
            let current = self.placement.as_ref().map(|p| p.tool);
            if ui
                .add_enabled(
                    open,
                    egui::Button::new("Place rectangle").selected(current == Some(Tool::Rectangle)),
                )
                .clicked()
            {
                self.start_tool(Tool::Rectangle);
                ui.close();
            }
            if ui
                .add_enabled(has_extrude, egui::Button::new("Place hole"))
                .on_disabled_hover_text("Needs an extrusion")
                .clicked()
            {
                if let Some(e) = self
                    .summary
                    .features
                    .iter()
                    .rev()
                    .find(|f| f.kind == "extrude")
                {
                    let extrude = e.id;
                    self.start_tool(Tool::Hole { extrude });
                }
                ui.close();
            }
        });
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
        if self.placement.is_some() && ui.button("Cancel placement").clicked() {
            self.cancel_tool();
        }
    }

    pub(crate) fn sketch_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let committing = s.committing;
        let mut finish = false;
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Sketch");
            ui.separator();
            ui.add_enabled_ui(!committing, |ui| {
                let ed = &mut s.editor;
                for t in [
                    SketchTool::Select,
                    SketchTool::Line,
                    SketchTool::CenterRectangle,
                    SketchTool::CornerRectangle,
                    SketchTool::Circle,
                    SketchTool::Dimension,
                ] {
                    if ui
                        .add(egui::Button::new(t.label()).selected(ed.tool == t))
                        .on_hover_text(format!("{} ({})", t.hint(), t.shortcut()))
                        .clicked()
                    {
                        ed.set_tool(t);
                    }
                }
                ui.separator();
                let hv = ed.hv_disabled_reason();
                if action(ui, "Horizontal", hv) {
                    ed.constrain_lines(false);
                }
                if action(ui, "Vertical", hv) {
                    ed.constrain_lines(true);
                }
                if action(ui, "Coincident", ed.coincident_disabled_reason()) {
                    ed.constrain_coincident();
                }
                let del = ed
                    .selection
                    .is_empty()
                    .then_some("Select geometry, a dimension or a constraint glyph first");
                if action(ui, "Delete", del) {
                    ed.delete_selection();
                }
                ui.separator();
                if ui
                    .add_enabled(ed.can_undo(), egui::Button::new("Undo step"))
                    .on_hover_text("Undo inside this sketch (Ctrl+Z)")
                    .clicked()
                {
                    ed.undo();
                }
                if ui
                    .add_enabled(ed.can_redo(), egui::Button::new("Redo step"))
                    .on_hover_text("Ctrl+Shift+Z")
                    .clicked()
                {
                    ed.redo();
                }
            });
            ui.separator();
            if ui
                .add_enabled(
                    !committing,
                    egui::Button::new(RichText::new("Finish Sketch").strong())
                        .fill(Color32::from_rgb(40, 90, 50)),
                )
                .on_hover_text("Commit the sketch as one undoable document step")
                .clicked()
            {
                finish = true;
            }
            if ui
                .add_enabled(!committing, egui::Button::new("Cancel Sketch"))
                .on_hover_text("Discard all changes made in this sketch session")
                .clicked()
            {
                cancel = true;
            }
            if ui.button("Help").clicked() {
                self.show_help = !self.show_help;
            }
            if committing {
                ui.spinner();
                ui.label("Committing…");
            }
        });
        if finish {
            self.finish_sketch();
        } else if cancel {
            self.cancel_sketch();
        }
    }

    // ----- browser -----

    pub(crate) fn history(&mut self, ui: &mut egui::Ui) {
        ui.heading("Browser");
        if !self.summary.open {
            ui.weak("No document open. Use New or Open….");
            return;
        }
        if self.summary.features.is_empty() {
            ui.weak("Empty document. Use Create Sketch to start.");
        }
        let editing = self.sketch.as_ref().and_then(|s| match s.target {
            SketchTarget::Existing(id) => Some(id),
            _ => None,
        });
        let mut clicked = None;
        let mut edit = None;
        let mut rename = None;
        let feats = self.summary.features.clone();
        for f in &feats {
            let sel = self.selected == Some(f.id);
            let kind = match f.kind.as_str() {
                "sketch" => "Sketch",
                "extrude" => "Extrude",
                "revolve" => "Revolve",
                "cut_through_all" => "Cut",
                "pocket" => "Pocket",
                k => k,
            };
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{kind:<7}"))
                        .monospace()
                        .weak()
                        .small(),
                );
                let r = ui.selectable_label(sel, &f.name);
                if r.clicked() {
                    clicked = Some(f.id);
                }
                if r.double_clicked() && f.kind == "sketch" {
                    edit = Some(f.id);
                }
                r.context_menu(|ui| {
                    if f.kind == "sketch" && ui.button("Edit Sketch").clicked() {
                        edit = Some(f.id);
                        ui.close();
                    }
                    if ui.button("Rename…").clicked() {
                        rename = Some(f.id);
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
                    ui.label(RichText::new("open").color(BAD_COLOR).small())
                        .on_hover_text(f.profile_error.clone().unwrap_or_default());
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
            if let Some(id) = rename {
                self.rename = Some((id, self.feature_name(id)));
            }
        }
    }

    // ----- inspector -----

    pub(crate) fn inspector_ui(&mut self, ui: &mut egui::Ui, _busy: bool) {
        ui.heading("Inspector");
        if let Some(info) = self.selected_face().cloned() {
            ui.label(RichText::new("Selected face").strong());
            ui.label(&info.description);
            match (info.reference, info.sketch_blocker()) {
                (Some(r), _) => {
                    if ui.button("Create Sketch on this face").clicked() {
                        self.begin_sketch(SketchTarget::New {
                            plane: SketchPlane::Face(r),
                        });
                    }
                }
                (None, Some(why)) => {
                    ui.add_enabled(false, egui::Button::new("Create Sketch on this face"));
                    ui.label(RichText::new(why).weak().small());
                }
                _ => {}
            }
            ui.separator();
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
        ui.horizontal(|ui| {
            let kind = match f.kind.as_str() {
                "cut_through_all" => "cut",
                k => k,
            };
            ui.label(
                RichText::new(format!("{} ({kind})", f.name))
                    .strong()
                    .size(16.0),
            );
            if ui.small_button("Rename…").clicked() {
                self.rename = Some((f.id, f.name.clone()));
            }
        });
        if let Some((t, c)) = state_text(&f) {
            ui.label(RichText::new(t).color(c));
        }
        // Contextual actions.
        match f.kind.as_str() {
            "sketch" => {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Edit Sketch").clicked() {
                        self.begin_sketch(SketchTarget::Existing(f.id));
                    }
                    let eb = self.extrude_blocker(&f);
                    if action(ui, "Extrude this sketch…", eb.as_deref()) {
                        self.dialog = Some(Dialog::Extrude {
                            sketch: Some(f.id),
                            distance: "10".into(),
                        });
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
            "extrude" => {
                ui.horizontal_wrapped(|ui| {
                    for fs in &f.faces {
                        let label = match fs.reference.role {
                            FaceRole::ExtrudeEndCap => "Sketch on end cap",
                            FaceRole::ExtrudeStartCap => "Sketch on start cap",
                        };
                        if ui.button(label).clicked() {
                            self.begin_sketch(SketchTarget::New {
                                plane: SketchPlane::Face(fs.reference),
                            });
                        }
                    }
                });
            }
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

    fn parameter_editor(&mut self, ui: &mut egui::Ui) {
        let Some(ins) = self.inspector.as_mut() else {
            return;
        };
        ui.label(RichText::new("Parameters").strong());
        if ins.fields.is_empty() {
            ui.weak("No editable parameters.");
            return;
        }
        egui::Grid::new("inspector_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (pname, _, buf) in &mut ins.fields {
                    let unit = if pname.ends_with("_deg") { "deg" } else { "mm" };
                    let l = ui.label(format!("{pname} ({unit})"));
                    ui.add(egui::TextEdit::singleline(buf).desired_width(90.0))
                        .labelled_by(l.id);
                    ui.end_row();
                }
            });
        let modified = ins
            .fields
            .iter()
            .any(|(_, v, b)| b.trim().parse::<f64>().ok() != Some(*v));
        let mut apply = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            apply = ui
                .add_enabled(modified, egui::Button::new("Apply"))
                .clicked();
            cancel = ui
                .add_enabled(modified, egui::Button::new("Cancel edits"))
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
            for (pname, v, b) in &ins.fields {
                match b.trim().parse::<f64>() {
                    Ok(x) if x != *v => cmds.push(Command::SetParameter {
                        feature: ins.feature,
                        name: pname.clone(),
                        value: x,
                    }),
                    Ok(_) => {}
                    Err(_) => bad.push(format!("{pname}: '{b}' is not a number")),
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
                    .selectable_label(sel, format!("{name} = {} mm", f420_sketch::fmt_mm(*v)))
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
                        f420_sketch::fmt_mm(xy[0]),
                        f420_sketch::fmt_mm(xy[1])
                    )
                }
                Item::Entity(e) => match s.editor.sketch.entity(e) {
                    Some(f420_sketch::Entity::Line { a, b, .. }) => {
                        let (pa, pb) = (
                            s.editor.sketch.point_xy(*a).unwrap_or_default(),
                            s.editor.sketch.point_xy(*b).unwrap_or_default(),
                        );
                        format!(
                            "Line L{} length {} mm",
                            e.0,
                            f420_sketch::fmt_mm((pb[0] - pa[0]).hypot(pb[1] - pa[1]))
                        )
                    }
                    Some(f420_sketch::Entity::Circle { radius, .. }) => {
                        format!("Circle C{} Ø{} mm", e.0, f420_sketch::fmt_mm(2.0 * radius))
                    }
                    None => continue,
                },
                Item::Constraint(i) => s.editor.sketch.constraint_label(i),
            };
            ui.small(text);
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
        if let Some(e) = self.diagnostics.iter().rev().find(|d| d.error) {
            ui.colored_label(
                ui.visuals().error_fg_color,
                RichText::new(format!("Last error [{}]: {}", e.kind, e.text)).small(),
            );
        }
        ui.collapsing("Log", |ui| {
            for d in self.diagnostics.iter().rev().take(12) {
                let t = format!("[{}] {}", d.kind, d.text);
                if d.error {
                    ui.colored_label(ui.visuals().warn_fg_color, RichText::new(t).small());
                } else {
                    ui.small(t);
                }
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
                let path = self
                    .summary
                    .path
                    .as_ref()
                    .map_or("unsaved".into(), |p| p.display().to_string());
                let dirty = if self.summary.dirty {
                    RichText::new("* modified").color(Color32::from_rgb(255, 200, 90))
                } else {
                    RichText::new("saved").weak()
                };
                ui.label(dirty);
                ui.label(RichText::new(format!("rev {} · {path}", self.summary.revision)).weak());
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

    pub(crate) fn dialog_ui(&mut self, ctx: &egui::Context, busy: bool) {
        let Some(mut dlg) = self.dialog.take() else {
            return;
        };
        let features = self.summary.features.clone();
        let sketches: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "sketch").collect();
        let extrudes: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "extrude").collect();
        let mut ok = false;
        let mut cancel = false;
        let mut error: Option<String> = None;
        let opened = self.last_dialog != Some(dlg.title());
        self.last_dialog = Some(dlg.title());
        AUTOFOCUS.with(|a| a.set(opened));
        // Enter confirms and Escape cancels (also while a field has focus).
        let (enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        egui::Window::new(dlg.title())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
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
                    Dialog::Extrude { sketch, distance } => {
                        feature_combo(ui, "Profile sketch", sketch, &sketches);
                        num_field(ui, "Extrude distance (mm)", distance);
                        if let Some(e) = sketch
                            .and_then(|id| sketches.iter().find(|s| s.id == id))
                            .and_then(|s| s.profile_error.clone())
                        {
                            ui.colored_label(BAD_COLOR, format!("This sketch's profile is not usable: {e}"));
                        }
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
                    Dialog::Revolve { sketch, angle } => {
                        feature_combo(ui, "Revolve profile sketch", sketch, &sketches);
                        num_field(ui, "Revolve angle (deg)", angle);
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
                            ui.radio_value(blind, false, "Through all");
                            ui.radio_value(blind, true, "Blind depth");
                        });
                        if *blind {
                            num_field(ui, "Pocket depth (mm)", depth);
                            ui.small("Cut from the sketch's cap face into the material; must be shallower than the part.");
                        } else {
                            ui.small("Removes material in both directions along the sketch normal.");
                        }
                    }
                    Dialog::SaveAs { path } | Dialog::Open { path } => {
                        text_field(ui, "File path", path);
                    }
                    Dialog::ExportStl { path, chord } => {
                        text_field(ui, "STL path", path);
                        num_field(ui, "Chord tolerance (mm)", chord);
                        ui.small("Exports the last committed model revision (queued after pending edits).");
                        if let Some(mc) = &self.last_export {
                            ui.label(format!(
                                "Last export: watertight {} · {} triangles · mesh volume {:.4} mm³",
                                if mc.watertight { "yes" } else { "NO" },
                                mc.triangles,
                                mc.mesh_volume
                            ));
                        }
                    }
                    Dialog::ExportStep { path } => {
                        text_field(ui, "STEP path", path);
                        ui.label(
                            "Exact B-rep (AP214); refused if an edge cannot be written exactly.",
                        );
                        ui.small("Exports the last committed model revision (queued after pending edits).");
                    }
                }
                ui.horizontal(|ui| {
                    ok = ui
                        .add_enabled(!busy_blocks(&dlg, busy), egui::Button::new("OK"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                ui.weak("Enter = OK · Esc = Cancel");
            });
        AUTOFOCUS.with(|a| a.set(false));
        ok |= enter && !opened && !busy_blocks(&dlg, busy);
        cancel |= esc;
        if cancel {
            self.last_dialog = None;
            return; // dialog dropped; nothing created
        }
        if ok {
            self.last_dialog = None;
            match build_command(&dlg) {
                Ok(Command::Open { path }) => {
                    self.guarded(Guarded::Open(path));
                    return;
                }
                Ok(cmd) => {
                    let keep = matches!(dlg, Dialog::ExportStl { .. });
                    self.request(cmd);
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
            Guarded::New | Guarded::NewAndSketch => "creating a new document",
            Guarded::Open(_) => "opening another document",
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
                    let save_label = if can_save { "Save" } else { "Save As…" };
                    if ui.button(save_label).clicked() {
                        answer = Some(ConfirmAnswer::Save);
                    }
                    if ui.button("Discard changes").clicked() {
                        answer = Some(ConfirmAnswer::Discard);
                    }
                    if ui.button("Keep editing").clicked() {
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
                    .labelled_by(l.id);
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
                    if ui.button("Rename").clicked() {
                        done = Some(true);
                    }
                    if ui.button("Cancel rename").clicked() {
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
        let mut open = self.show_help;
        egui::Window::new("Help")
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Navigation").strong());
                ui.label(f420_viewport::NAV_HELP);
                ui.label("In sketch mode the left button draws; use the middle button to orbit and the right button to pan.");
                ui.label("Views: F fit · 7 top · 1 front · 3 right · 0 isometric.");
                ui.separator();
                ui.label(RichText::new("Document").strong());
                ui.label("Ctrl+N new · Ctrl+O open · Ctrl+S save · Ctrl+Shift+S save as · Ctrl+Z undo · Ctrl+Shift+Z / Ctrl+Y redo");
                ui.separator();
                ui.label(RichText::new("Sketch").strong());
                ui.label("S select · L line · R centre rectangle · Shift+R 2-point rectangle · C circle · D dimension");
                ui.label("Del delete · Enter end line chain · Esc cancels the gesture, then the tool · Shift while drawing disables snapping");
                ui.label("Snaps: existing points (shared endpoint), horizontal/vertical alignment (adds an H/V constraint), grid (position only).");
                ui.label("Ctrl+Z inside a sketch undoes sketch steps; Finish Sketch records one document undo step; Cancel Sketch discards the session.");
                ui.separator();
                ui.label("Shortcuts are ignored while a text field has focus.");
            });
        self.show_help = open;
    }
}

thread_local! {
    /// Set when a dialog opens: the first text field drawn takes keyboard focus.
    static AUTOFOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn field(ui: &mut egui::Ui, label: &str, buf: &mut String, width: f32) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        let r = ui
            .add(egui::TextEdit::singleline(buf).desired_width(width))
            .labelled_by(l.id);
        if AUTOFOCUS.with(|a| a.replace(false)) {
            r.request_focus();
        }
    });
}

pub(crate) fn num_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    field(ui, label, buf, 90.0);
}

pub(crate) fn text_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    field(ui, label, buf, 320.0);
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
