//! Enabled predicates and run actions of the registry's commands, as
//! [`RcadApp`] methods. A predicate returns the reason a command cannot run
//! (shown as the disabled button's hover text and, for a key press, as a
//! message); a run action opens the command's dialog or sends its command.

use super::Focus;
use super::registry::{self, CmdId, CommandDef};
use crate::app::{Dialog, PatternKind, RcadApp, SketchTarget};
use crate::probe::Probe as _;
use eframe::egui;
use rcad_commands::{Command, ExtrudeOp, FeatureSummary};
use rcad_document::{FaceRef, FeatureId, SketchPlane};

const NO_DOC: &str = "Open or create a document first";

impl RcadApp {
    // ----- predicates -----

    pub(crate) fn need_open(&self) -> Result<(), String> {
        if self.summary.open {
            Ok(())
        } else {
            Err(NO_DOC.into())
        }
    }

    pub(crate) fn need_sketch(&self) -> Result<(), String> {
        self.need_open()?;
        if self.summary.features.iter().any(|f| f.kind == "sketch") {
            Ok(())
        } else {
            Err("Needs a sketch".into())
        }
    }

    pub(crate) fn need_selected_sketch(&self) -> Result<(), String> {
        self.need_open()?;
        match self.selected_sketch() {
            Some(_) => Ok(()),
            None => Err("Select a sketch in History first".into()),
        }
    }

    pub(crate) fn need_extrude(&self) -> Result<(), String> {
        self.need_open()?;
        if self.summary.features.iter().any(|f| f.kind == "extrude") {
            Ok(())
        } else {
            Err("Needs an extrusion".into())
        }
    }

    pub(crate) fn need_face(&self) -> Result<(), String> {
        self.need_open()?;
        self.selected_face()
            .map(|_| ())
            .ok_or_else(|| "Select a face in the view first".into())
    }

    /// The selected face's semantic reference (planar faces that can host a
    /// sketch, hole, chamfer or fillet).
    pub(crate) fn need_sketchable_face(&self) -> Result<FaceRef, String> {
        self.need_open()?;
        let f = self
            .selected_face()
            .ok_or_else(|| "Select a planar face in the view first".to_string())?;
        match (f.reference, f.sketch_blocker()) {
            (Some(r), _) => Ok(r),
            (None, Some(why)) => Err(format!("Selected face: {why}")),
            (None, None) => Err("The selected face has no stable reference".into()),
        }
    }

    pub(crate) fn need_body(&self) -> Result<(), String> {
        self.need_open()?;
        match self.selected_body {
            Some(_) => Ok(()),
            None => Err("Select a body in the browser or the view first".into()),
        }
    }

    pub(crate) fn need_mesh_body(&self) -> Result<(), String> {
        self.need_body()?;
        let b = self
            .selected_body
            .and_then(|b| self.summary.bodies.iter().find(|x| x.id == b));
        match b {
            Some(b) if b.kind == "mesh" => Ok(()),
            _ => Err("Select a mesh body (STL import) first".into()),
        }
    }

    pub(crate) fn need_selection(&self) -> Result<(), String> {
        if self.selected.is_some() || self.selected_face.is_some() || self.selected_body.is_some() {
            Ok(())
        } else {
            Err("Nothing is selected".into())
        }
    }

    pub(crate) fn need_occurrences(&self, n: usize) -> Result<(), String> {
        self.need_open()?;
        let have = self.summary.assembly.occurrences.len();
        if have >= n {
            Ok(())
        } else if n == 1 {
            Err("Needs a component occurrence (ASSEMBLE › New Component)".into())
        } else {
            Err(format!(
                "Needs at least {n} component occurrences (ASSEMBLE › New Component)"
            ))
        }
    }

    pub(crate) fn need_joints(&self) -> Result<(), String> {
        self.need_open()?;
        if self
            .summary
            .assembly
            .joints
            .iter()
            .any(|j| j.kind.dof() > 0)
        {
            Ok(())
        } else {
            Err("Needs a joint with a free value (ASSEMBLE › Joint)".into())
        }
    }

    pub(crate) fn need_drawing(&self) -> Result<(), String> {
        self.need_open()?;
        if self.summary.drawings.is_empty() {
            Err("Needs a drawing (INSERT › New Drawing)".into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn undo_blocker(&self) -> Result<(), String> {
        if self.sketch.is_some() {
            return Ok(());
        }
        match &self.summary.can_undo {
            Some(_) => Ok(()),
            None => Err("Nothing to undo".into()),
        }
    }

    pub(crate) fn redo_blocker(&self) -> Result<(), String> {
        if self.sketch.is_some() {
            return Ok(());
        }
        match &self.summary.can_redo {
            Some(_) => Ok(()),
            None => Err("Nothing to redo".into()),
        }
    }

    fn selected_feature(&self) -> Option<&FeatureSummary> {
        self.selected
            .and_then(|id| self.summary.features.iter().find(|f| f.id == id))
    }

    fn selected_sketch(&self) -> Option<FeatureSummary> {
        self.selected_feature()
            .filter(|f| f.kind == "sketch")
            .cloned()
    }

    /// Where Create Sketch starts: the selected supported face, else XY.
    pub(crate) fn create_sketch_target(&self) -> Result<SketchPlane, String> {
        self.need_open()?;
        match self.selected_face() {
            Some(f) => match (f.reference, f.sketch_blocker()) {
                (Some(r), _) => Ok(SketchPlane::Face(r)),
                (None, Some(why)) => Err(format!("Selected face: {why}")),
                (None, None) => Ok(SketchPlane::WorldXY),
            },
            None => Ok(SketchPlane::WorldXY),
        }
    }

    pub(crate) fn extrude_cmd_blocker(&self) -> Result<(), String> {
        self.need_open()?;
        if !self.summary.features.iter().any(|f| f.kind == "sketch") {
            return Err("Create a sketch with a closed profile first".into());
        }
        match self.selected_sketch() {
            Some(s) => self.extrude_blocker(&s).map_or(Ok(()), Err),
            None => Ok(()),
        }
    }

    /// Cut Through All: circles keep the hole-style cut dialog; any other
    /// usable closed profile opens Extrude preset to Cut, Through All.
    fn cut_mode(&self) -> Result<bool, String> {
        self.need_open()?;
        let sketch_sel = self.selected_sketch();
        if !self.summary.features.iter().any(|f| f.kind == "sketch") {
            return Err("Create a closed sketch profile first".into());
        }
        if !self.has_body() {
            return Err("Create a body first (Extrude)".into());
        }
        let extrude_ok = self.extrude_cmd_blocker().is_ok();
        let circle_block = sketch_sel.as_ref().and_then(|s| self.cut_blocker(s));
        let general = circle_block.is_some()
            && extrude_ok
            && sketch_sel
                .as_ref()
                .is_some_and(|s| s.profile_error.is_none() && self.consumer_of(s.id).is_none());
        if general {
            Ok(true)
        } else {
            circle_block.map_or(Ok(false), Err)
        }
    }

    pub(crate) fn cut_cmd_blocker(&self) -> Result<(), String> {
        self.cut_mode().map(|_| ())
    }

    pub(crate) fn revolve_cmd_blocker(&self) -> Result<(), String> {
        self.need_open()?;
        if !self.summary.features.iter().any(|f| f.kind == "sketch") {
            return Err("Create a sketch on XZ first (Templates > Rectangle on XZ…)".into());
        }
        match self.selected_sketch().and_then(|s| s.profile_error) {
            Some(e) => Err(format!("Profile is not usable: {e}")),
            None => Ok(()),
        }
    }

    /// The feature Pattern / Mirror repeats: the selected Extrude join/cut
    /// or Hole.
    pub(crate) fn pattern_source(&self) -> Result<FeatureId, String> {
        self.need_open()?;
        let f = self.selected_feature().ok_or_else(|| {
            "Select an Extrude (join or cut) or a Hole in History first".to_string()
        })?;
        let ok = match f.kind.as_str() {
            "extrude" => matches!(
                f.operation,
                Some(rcad_document::Operation::Cut { .. } | rcad_document::Operation::Join { .. })
            ),
            "hole" => true,
            _ => false,
        };
        if ok {
            Ok(f.id)
        } else {
            Err(format!(
                "{} cannot be patterned: select an Extrude join/cut or a Hole",
                f.name
            ))
        }
    }

    pub(crate) fn shell_blocker(&self) -> Result<(), String> {
        self.need_open()?;
        if let Some(f) = self.selected_feature()
            && f.kind == "extrude"
            && matches!(f.operation, Some(rcad_document::Operation::NewBody { .. }))
        {
            return Ok(());
        }
        self.need_sketchable_face()
            .map(|_| ())
            .map_err(|_| "Select a planar face to leave open (or a New Body extrusion)".into())
    }

    pub(crate) fn align_blocker(&self) -> Result<(), String> {
        self.need_sketchable_face()?;
        let mine = self.selected_face().and_then(|f| f.body);
        let other = self
            .summary
            .features
            .iter()
            .flat_map(|g| g.faces.iter())
            .any(|fc| Some(fc.body) != mine);
        if other {
            Ok(())
        } else {
            Err("Needs a face of another body to align onto".into())
        }
    }

    pub(crate) fn direct_edit_blocker(&self) -> Result<(), String> {
        self.need_face()?;
        let group = self.selected_face.unwrap_or_default();
        match crate::direct_edit_ui::imported_face_kind(self, group) {
            Some(rcad_kernel::DirectFace::Other) => {
                Err("This face is neither planar nor a circular cylinder".into())
            }
            Some(_) => Ok(()),
            None => Err("Select a face of an imported body (changed only by direct edits)".into()),
        }
    }

    pub(crate) fn delete_blocker(&self) -> Result<(), String> {
        if self.selected_face.is_some() {
            // Explained when run (faces are not deleted).
            return Ok(());
        }
        let id = self
            .selected
            .ok_or_else(|| "Select a feature in History first".to_string())?;
        let users = self.dependents_of(id);
        if users.is_empty() {
            Ok(())
        } else {
            Err(format!("Used by {}; delete those first", users.join(", ")))
        }
    }

    // ----- actions -----

    pub(crate) fn undo(&mut self) {
        if let Some(s) = self.sketch.as_mut() {
            s.editor.undo();
        } else if self.summary.can_undo.is_some() {
            self.request(Command::Undo);
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(s) = self.sketch.as_mut() {
            s.editor.redo();
        } else if self.summary.can_redo.is_some() {
            self.request(Command::Redo);
        }
    }

    pub(crate) fn open_dialog(&mut self, safe_mode: bool) {
        self.dialog = Some(Dialog::Open {
            path: self.default_path("rcad.json"),
            safe_mode,
        });
    }

    pub(crate) fn save_as_dialog(&mut self) {
        if self.summary.open {
            self.dialog = Some(Dialog::SaveAs {
                path: self.default_path("rcad.json"),
            });
        }
    }

    pub(crate) fn export_step_dialog(&mut self) {
        self.dialog = Some(Dialog::ExportStep {
            path: self.default_path("step"),
            scope: self.default_export_scope(),
        });
    }

    pub(crate) fn export_sketch_dialog(&mut self, svg: bool) {
        let sketch = self
            .selected_sketch()
            .map(|s| s.id)
            .or_else(|| self.unused_sketch());
        self.dialog = Some(Dialog::ExportDxf {
            path: self.default_path(if svg { "svg" } else { "dxf" }),
            sketch,
            svg,
        });
    }

    pub(crate) fn export_assembly_dialog(&mut self) {
        self.dialog = Some(Dialog::ExportAssembly {
            path: self.default_path("step"),
        });
    }

    pub(crate) fn create_sketch(&mut self) {
        if let Ok(plane) = self.create_sketch_target() {
            self.begin_sketch(SketchTarget::New { plane });
        }
    }

    pub(crate) fn open_extrude(&mut self) {
        let sketch = self
            .selected_sketch()
            .map(|s| s.id)
            .or_else(|| self.unused_sketch());
        self.dialog = Some(Dialog::Extrude(crate::extrude::ExtrudeForm::new(
            self, sketch,
        )));
    }

    pub(crate) fn open_cut(&mut self) {
        let Ok(general) = self.cut_mode() else { return };
        let sketch = self
            .selected_sketch()
            .map(|s| s.id)
            .or_else(|| self.unused_sketch());
        if general {
            let mut form = crate::extrude::ExtrudeForm::new(self, sketch);
            form.op = ExtrudeOp::Cut;
            form.through_all = true;
            self.dialog = Some(Dialog::Extrude(form));
        } else {
            self.dialog = Some(Dialog::Cut {
                sketch,
                blind: false,
                depth: "5".into(),
            });
        }
    }

    pub(crate) fn open_revolve(&mut self) {
        self.dialog = Some(Dialog::Revolve {
            sketch: self.unused_sketch(),
            angle: "360".into(),
            axis: Default::default(),
            reverse: false,
            op: ExtrudeOp::NewBody,
            target: None,
        });
    }

    pub(crate) fn open_thread(&mut self) {
        // A selected planar face places the thread (centre: the face's
        // frame origin); otherwise world XY at the origin.
        let face = self.need_sketchable_face().ok();
        self.dialog = Some(Dialog::Thread {
            diameter: "20".into(),
            pitch: "2".into(),
            length: "12".into(),
            nut: false,
            nut_width: "30".into(),
            left_hand: false,
            plane: face.map_or(SketchPlane::WorldXY, SketchPlane::Face),
            cx: "0".into(),
            cy: "0".into(),
        });
    }

    pub(crate) fn open_loft_sweep(&mut self, sweep: bool) {
        self.dialog = Some(Dialog::LoftSweep {
            a: None,
            b: None,
            sweep,
            via: None,
        });
    }

    pub(crate) fn open_hole(&mut self) {
        let Ok(r) = self.need_sketchable_face() else {
            return;
        };
        let Some(info) = self.selected_face().cloned() else {
            return;
        };
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

    pub(crate) fn open_pattern(&mut self, mirror: bool) {
        let Ok(feature) = self.pattern_source() else {
            return;
        };
        let face = self.selected_face().and_then(|i| i.reference);
        self.dialog = Some(Dialog::Pattern {
            feature,
            kind: if mirror {
                PatternKind::Mirror
            } else {
                PatternKind::Linear
            },
            count: "3".into(),
            spacing: "10".into(),
            along_y: false,
            reverse: false,
            count2: "1".into(),
            spacing2: "10".into(),
            angle: "60".into(),
            center_x: "0".into(),
            center_y: "0".into(),
            mirror: face.map_or(SketchPlane::WorldYZ, SketchPlane::Face),
            mirror_face: face,
            mirror_offset: "0".into(),
        });
    }

    pub(crate) fn open_chamfer(&mut self, fillet: bool) {
        if let Ok(a) = self.need_sketchable_face() {
            self.dialog = Some(Dialog::Chamfer {
                a,
                b: None,
                distance: if fillet { "2" } else { "1" }.into(),
                fillet,
            });
        }
    }

    pub(crate) fn open_loop(&mut self, chamfer: bool) {
        if let Ok(face) = self.need_sketchable_face() {
            self.dialog = Some(Dialog::FilletLoop {
                face,
                radius: if chamfer { "1" } else { "2" }.into(),
                chamfer,
                holes: false,
            });
        }
    }

    pub(crate) fn open_shell(&mut self) {
        // A picked face is the face to leave open (it also selects its
        // feature); otherwise a selected New Body extrusion's scoped shell.
        if let Ok(face) = self.need_sketchable_face() {
            self.dialog = Some(Dialog::ShellFace {
                face,
                thickness: self.shell_thickness.clone(),
            });
        } else if let Some(f) = self.selected_feature()
            && f.kind == "extrude"
            && matches!(f.operation, Some(rcad_document::Operation::NewBody { .. }))
        {
            self.dialog = Some(Dialog::Shell {
                extrude: f.id,
                open_end: true,
                thickness: "2".into(),
            });
        }
    }

    pub(crate) fn open_align(&mut self) {
        if let Ok(from) = self.need_sketchable_face() {
            self.dialog = Some(Dialog::Align { from, to: None });
        }
    }

    pub(crate) fn open_direct_edit(&mut self) {
        let group = self.selected_face.unwrap_or_default();
        if let Some(d) = crate::direct_edit_ui::default_direct_dialog(self, group) {
            self.dialog = Some(d);
        }
    }

    pub(crate) fn open_rigid_group(&mut self) {
        let occ: Vec<_> = self
            .summary
            .assembly
            .occurrences
            .iter()
            .map(|o| (o.id, o.name.clone(), false))
            .collect();
        self.dialog = Some(Dialog::RigidGroup { occurrences: occ });
    }

    pub(crate) fn open_drive_joints(&mut self) {
        let joints = self
            .summary
            .assembly
            .joints
            .iter()
            .filter(|j| j.kind.dof() > 0)
            .map(|j| {
                (
                    j.id,
                    j.name.clone(),
                    j.values()
                        .iter()
                        .map(|v| crate::params_ui::fmt_value(*v))
                        .collect(),
                )
            })
            .collect();
        self.dialog = Some(Dialog::DriveJoints { joints });
    }

    pub(crate) fn open_rectangle(&mut self, xz: bool) {
        self.dialog = Some(if xz {
            Dialog::RectangleXz {
                width: "20".into(),
                height: "10".into(),
                cx: "30".into(),
                cz: "5".into(),
            }
        } else {
            Dialog::Rectangle {
                width: "80".into(),
                height: "50".into(),
                cx: "0".into(),
                cy: "0".into(),
            }
        });
    }

    pub(crate) fn open_circle_on_face(&mut self) {
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
    }

    pub(crate) fn start_place_hole(&mut self) {
        if let Some(e) = self
            .summary
            .features
            .iter()
            .rev()
            .find(|f| f.kind == "extrude")
        {
            let extrude = e.id;
            self.start_tool(crate::place::Tool::Hole { extrude });
        }
    }

    pub(crate) fn start_measure(&mut self) {
        if let Some(info) = self.selected_face().cloned() {
            self.start_exact_measure(&info);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.select(None);
        self.select_face(None);
        self.select_body(None);
    }

    /// Delete key in model mode.
    pub(crate) fn delete_key(&mut self) {
        if self.selected_face.is_some() {
            // A picked face selects its owner too; deleting the whole feature
            // from a face pick would be surprising.
            self.diag(
                false,
                "info",
                "faces cannot be deleted; select the feature in the browser to delete it".into(),
            );
        } else {
            self.delete_selected_feature();
        }
    }

    /// Feature the open command dialog edits (Edit Feature).
    pub fn editing_feature(&self) -> Option<rcad_document::FeatureId> {
        self.shell.editing.filter(|_| self.dialog.is_some())
    }

    /// Last frame's status chips along the top of the view and the view
    /// cube's area.
    pub fn view_chrome_rects(&self) -> (Vec<eframe::egui::Rect>, Option<eframe::egui::Rect>) {
        (self.shell.top_chips.clone(), self.shell.view_cube)
    }

    /// Active key layer.
    pub fn keymap(&self) -> super::KeymapLayer {
        self.shell.prefs.keymap
    }
    pub fn set_keymap(&mut self, layer: super::KeymapLayer) {
        self.shell.prefs.keymap = layer;
    }

    /// Load preferences from `path` (and save changes there from now on).
    pub fn load_prefs(&mut self, path: std::path::PathBuf) {
        let (p, why) = super::prefs::Prefs::load(&path);
        self.shell.prefs = p;
        self.shell.prefs_path = Some(path);
        if let Some(why) = why {
            self.diag(false, "info", why);
        }
    }

    /// Save preferences (atomic); a failure is reported, never fatal.
    pub(crate) fn save_prefs(&mut self) {
        let Some(path) = self.shell.prefs_path.clone() else {
            return;
        };
        if let Err(e) = self.shell.prefs.save(&path) {
            self.diag(false, "info", format!("preferences not saved: {e}"));
        }
    }

    /// Remember the open document's path in Open Recent (saved prefs).
    pub(crate) fn track_recent(&mut self) {
        if let Some(p) = self.summary.path.clone()
            && self.shell.prefs.recent.first() != Some(&p)
        {
            self.shell.prefs.push_recent(p);
            self.save_prefs();
        }
    }

    /// `cmd`, refused as stale if the model moved on since the face it
    /// was built from was picked (`Command::AtRevision`).
    pub(crate) fn at_pick_revision(&self, cmd: Command) -> Command {
        match self.shell.pick_revision {
            Some(revision) => Command::AtRevision {
                revision,
                command: Box::new(cmd),
            },
            None => cmd,
        }
    }

    pub(crate) fn focus(&mut self, f: Focus) {
        self.shell.focus = Some(f);
    }

    // ----- dispatch -----

    /// Run `id` if enabled; otherwise report why (key presses).
    pub fn run_command(&mut self, id: CmdId) {
        let d = registry::def(id);
        match (d.enabled)(self) {
            Ok(()) => (d.run)(self),
            // "Extrude: …", not "Extrude…: …".
            Err(why) => self.notify(format!("{}: {why}", d.label.trim_end_matches('…'))),
        }
    }

    /// Whether `id` can run now (Err: the reason).
    pub fn command_enabled(&self, id: CmdId) -> Result<(), String> {
        (registry::def(id).enabled)(self)
    }

    /// Hover text: the tooltip and the active layer's key.
    pub(crate) fn command_tooltip(&self, d: &CommandDef) -> String {
        let keys = d.key_text(self.shell.prefs.keymap);
        let tip = match d.id {
            CmdId::Undo => self
                .summary
                .can_undo
                .as_ref()
                .map_or(d.tooltip.to_string(), |l| format!("Undo: {l}")),
            CmdId::Redo => self
                .summary
                .can_redo
                .as_ref()
                .map_or(d.tooltip.to_string(), |l| format!("Redo: {l}")),
            _ => d.tooltip.to_string(),
        };
        if keys.is_empty() {
            tip
        } else {
            format!("{tip} ({keys})")
        }
    }

    /// A text button for `id` (label = accessible name = probe key),
    /// disabled with the reason on hover. Runs the command when clicked.
    /// In menus the active layer's key is shown at the right of the item
    /// (the accessible name stays the label).
    pub(crate) fn cmd_button(&mut self, ui: &mut egui::Ui, id: CmdId) -> bool {
        self.cmd_button_keyed(ui, id, true)
    }

    /// [`Self::cmd_button`] without the key text (buttons on a bar).
    pub(crate) fn cmd_bar_button(&mut self, ui: &mut egui::Ui, id: CmdId) -> bool {
        self.cmd_button_keyed(ui, id, false)
    }

    fn cmd_button_keyed(&mut self, ui: &mut egui::Ui, id: CmdId, show_key: bool) -> bool {
        let d = registry::def(id);
        let state = (d.enabled)(self);
        let keys = d.key_text(self.shell.prefs.keymap);
        let mut button = egui::Button::new(d.label);
        if show_key && !keys.is_empty() {
            button = button.shortcut_text(keys.as_str());
        }
        let r = ui.add_enabled(state.is_ok(), button);
        if show_key && !keys.is_empty() {
            // The shortcut atom would otherwise join the accessible name.
            r.ctx.accesskit_node_builder(r.id, |n| {
                n.set_label(d.label.to_string());
                n.set_keyboard_shortcut(keys.clone());
            });
        }
        let r = r.probe(d.label);
        let r = match &state {
            Ok(()) => r.on_hover_text(self.command_tooltip(d)),
            Err(why) if keys.is_empty() => r.on_disabled_hover_text(why),
            Err(why) => r.on_disabled_hover_text(format!("{why} ({keys})")),
        };
        let clicked = r.clicked();
        if clicked {
            (d.run)(self);
        }
        clicked
    }

    /// Model-mode shortcuts of the active layer (most specific first).
    pub(crate) fn command_shortcuts(&mut self, ctx: &egui::Context) {
        let layer = self.shell.prefs.keymap;
        let mut binds: Vec<(super::keymap::Chord, CmdId)> = registry::COMMANDS
            .iter()
            .flat_map(|d| d.keys(layer).iter().map(move |c| (*c, d.id)))
            .collect();
        binds.sort_by_key(|(c, _)| std::cmp::Reverse(c.specificity()));
        for (c, id) in binds {
            if ctx.input_mut(|i| i.consume_shortcut(&c.shortcut())) {
                self.run_command(id);
                return;
            }
        }
    }
}
