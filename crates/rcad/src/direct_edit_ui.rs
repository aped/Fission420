//! Direct editing of imported STEP bodies (run 6, ADR 0010): inspector actions
//! on a selected face of an imported body ("Offset face…", "Resize hole…",
//! "Remove hole") and their dialog. Everything goes through
//! `Command::DirectEdit`; the kernel decides what is allowed and the dialog
//! shows its named refusal.
use crate::app::{Dialog, RcadApp};
use crate::probe::Probe;
use eframe::egui::{self, RichText};
use rcad_commands::{Command, ImportedFace};
use rcad_document::{DirectOp, FaceRef, FeatureId};
use rcad_kernel::DirectFace;

/// What the dialog does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectKind {
    Offset,
    Resize,
    Remove,
    /// Countersink the hole of this Hole feature (`face` unused).
    Countersink(FeatureId),
}

/// Form state of a direct edit dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectForm {
    pub face: FaceRef,
    pub kind: DirectKind,
    /// Offset distance or new diameter (mm).
    pub value: String,
    /// What was picked (shown in the dialog).
    pub description: String,
    /// Countersink: included angle (degrees).
    pub angle: String,
}

impl DirectForm {
    pub fn title(&self) -> &'static str {
        match self.kind {
            DirectKind::Offset => "Offset face",
            DirectKind::Resize => "Resize hole / boss",
            DirectKind::Remove => "Remove hole / boss",
            DirectKind::Countersink(_) => "Countersink",
        }
    }

    /// Features the dialog refers to.
    pub fn refs(&self) -> Vec<FeatureId> {
        match self.kind {
            DirectKind::Countersink(h) => vec![h],
            _ => vec![self.face.feature],
        }
    }

    pub fn command(&self) -> Result<Command, String> {
        let op = match self.kind {
            DirectKind::Countersink(hole) => {
                return Ok(Command::Countersink {
                    name: None,
                    hole,
                    diameter: crate::app::parse_num("countersink diameter", &self.value)?,
                    angle_deg: crate::app::parse_num("countersink angle", &self.angle)?,
                });
            }
            DirectKind::Offset => DirectOp::OffsetFace {
                distance: crate::app::parse_num("distance", &self.value)?,
            },
            DirectKind::Resize => DirectOp::ResizeCylinder {
                diameter: crate::app::parse_num("diameter", &self.value)?,
            },
            DirectKind::Remove => DirectOp::RemoveCylinder,
        };
        Ok(Command::DirectEdit {
            name: None,
            face: self.face,
            op,
        })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label(&self.description);
        match self.kind {
            DirectKind::Offset => {
                crate::panels::num_field(ui, "Distance (mm)", &mut self.value);
                ui.small("Moves the face along its outward normal (positive adds material). Every face next to it must be perpendicular to it (walls, or holes/bosses along its normal); refused when a neighbouring face would degenerate or the topology change. Editable and bindable afterwards as the parameter distance.");
            }
            DirectKind::Resize => {
                crate::panels::num_field(ui, "New diameter (mm)", &mut self.value);
                ui.small("Through hole, flat-floored blind hole or flat-topped boss ending on planes perpendicular to its axis. Growing is checked against every other edge of those planes. Bindable afterwards as the parameter diameter.");
            }
            DirectKind::Countersink(_) => {
                crate::panels::num_field(ui, "Countersink diameter (mm)", &mut self.value);
                crate::panels::num_field(ui, "Countersink angle (deg)", &mut self.angle);
                ui.small("Exact cone at the hole's entry face (planar, perpendicular to the hole; no counterbore). Refused when it leaves the face or reaches the end of the hole. Parameters diameter and angle_deg stay editable.");
            }
            DirectKind::Remove => {
                ui.small("Deletes the hole or boss and closes the openings it leaves on the planar faces (exact). References to its faces become broken.");
            }
        }
    }
}

/// The imported face shown as display group `group`, if its body is an
/// imported body changed only by direct edits.
fn imported_face(app: &RcadApp, group: u32) -> Option<ImportedFace> {
    let db = app.scene.body_of(group)?;
    let pos = (group - db.groups.start) as usize;
    let body = app.summary.bodies.iter().find(|b| b.id == db.id)?;
    body.imported_faces.get(pos).cloned()
}

impl RcadApp {
    /// Inspector actions for a selected face of an imported body.
    pub(crate) fn direct_edit_actions(&mut self, ui: &mut egui::Ui, group: u32) {
        let Some(f) = imported_face(self, group) else {
            return;
        };
        ui.label(RichText::new("Direct edit (imported body)").strong());
        let open = |kind, value: String, description: String| {
            Dialog::DirectEdit(DirectForm {
                face: f.face,
                kind,
                value,
                description,
                angle: String::new(),
            })
        };
        match f.kind {
            DirectFace::Plane { .. } => {
                if                     ui.button("Offset face…")
                        .on_hover_text("Move this planar face along its normal (exact; neighbours must be perpendicular walls)")
                    .probe("Offset face…")
                .clicked()
                {
                    self.dialog = Some(open(
                        DirectKind::Offset,
                        "1".into(),
                        format!("Planar {}", crate::face_role_label(&f.face.role)),
                    ));
                }
            }
            DirectFace::Cylinder { radius, hole, .. } => {
                let what = if hole { "hole" } else { "boss" };
                let d = crate::params_ui::fmt_value(2.0 * radius);
                ui.label(format!("Cylindrical {what} wall, Ø{d} mm"));
                if                     ui.button("Resize hole…")
                        .on_hover_text("Change the diameter of this hole or boss (exact)")
                    .probe("Resize hole…")
                .clicked()
                {
                    self.dialog = Some(open(
                        DirectKind::Resize,
                        d.clone(),
                        format!("{what} Ø{d} mm ({})", crate::face_role_label(&f.face.role)),
                    ));
                }
                if                     ui.button("Remove hole")
                        .on_hover_text("Delete this hole or boss and close the openings (exact)")
                    .probe("Remove hole")
                .clicked()
                {
                    self.dialog = Some(open(
                        DirectKind::Remove,
                        String::new(),
                        format!("{what} Ø{d} mm ({})", crate::face_role_label(&f.face.role)),
                    ));
                }
            }
            DirectFace::Other => {
                ui.label(
                    RichText::new("This face is neither planar nor a circular cylinder: no direct edits")
                        .weak()
                        .small(),
                );
            }
        }
    }
}

impl RcadApp {
    /// Feature inspector action for a selected Hole feature.
    pub(crate) fn countersink_action(&mut self, ui: &mut egui::Ui, hole: FeatureId) {
        let d = self
            .summary
            .features
            .iter()
            .find(|f| f.id == hole)
            .and_then(|f| f.parameters.iter().find(|(n, _)| n == "diameter"))
            .map_or(6.0, |(_, v)| *v);
        if ui
            .button("Countersink…")
            .on_hover_text("Add an exact countersink (cone) at this hole's entry")
            .probe("Countersink…")
            .clicked()
        {
            self.dialog = Some(Dialog::DirectEdit(DirectForm {
                face: FaceRef {
                    feature: hole,
                    role: rcad_document::FaceRole::ExtrudeEndCap,
                },
                kind: DirectKind::Countersink(hole),
                value: crate::params_ui::fmt_value(2.0 * d),
                description: format!("Hole Ø{} mm", crate::params_ui::fmt_value(d)),
                angle: "90".into(),
            }));
        }
    }
}
