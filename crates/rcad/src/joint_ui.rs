//! Joint creation dialog (docs/ASSEMBLIES.md): kind (rigid, revolute,
//! slider, cylindrical, pin-slot, planar, ball), parent and child occurrences
//! (siblings), the axis source (explicit, a cylinder face or the edge line of
//! two planar faces picked in the viewport, exact from the B-rep), the second
//! direction of pin-slot / planar / ball joints, initial values with limits,
//! and degree-of-freedom feedback before committing. The engine validates
//! (loops, over-constraint, grounded child, singular frames, limits) and
//! refuses atomically; the reply is the only source of truth.
use eframe::egui::{self, RichText};
use rcad_commands::Command;
use rcad_commands::assembly::{AssemblySummary, AxisFrom};
use rcad_commands::measure_faces::FacePick;
use rcad_document::assembly::{DofValue, JointKind, OccurrenceId};

use crate::app::RcadApp;
use crate::assembly_ui::parse;
use crate::probe::Probe as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisSource {
    Explicit,
    Cylinder,
    Edge,
}

/// Dialog buffers.
#[derive(Debug, Clone)]
pub struct JointDialog {
    pub kind: JointKind,
    pub parent: Option<OccurrenceId>,
    pub child: Option<OccurrenceId>,
    pub source: AxisSource,
    pub origin: [String; 3],
    pub axis: [String; 3],
    pub dir2: [String; 3],
    pub face_a: Option<FacePick>,
    pub face_b: Option<FacePick>,
    /// Value, min, max per DOF (up to 3).
    pub values: [[String; 3]; 3],
    pub name: String,
    /// The joint command was sent; the reply decides whether to close.
    pub pending: bool,
    /// Snap the child's cylinder-face axis onto the joint axis first.
    pub child_face: Option<FacePick>,
    pub flip: bool,
}

impl JointDialog {
    pub fn new(parent: Option<OccurrenceId>) -> Self {
        JointDialog {
            kind: JointKind::Revolute,
            parent,
            child: None,
            source: AxisSource::Explicit,
            origin: ["0".into(), "0".into(), "0".into()],
            axis: ["0".into(), "0".into(), "1".into()],
            dir2: Default::default(),
            face_a: None,
            face_b: None,
            values: std::array::from_fn(|_| ["0".into(), String::new(), String::new()]),
            name: String::new(),
            pending: false,
            child_face: None,
            flip: false,
        }
    }

    /// The command this dialog would send, or why it cannot yet.
    pub fn command(&self) -> Result<Command, String> {
        let parent = self.parent.ok_or("choose the parent occurrence")?;
        let child = self.child.ok_or("choose the child occurrence")?;
        if parent == child {
            return Err("parent and child must differ".into());
        }
        let v3 = |b: &[String; 3], what: &str| -> Result<[f64; 3], String> {
            let v = b.each_ref().map(|t| parse(t));
            match v {
                [Some(x), Some(y), Some(z)] => Ok([x, y, z]),
                _ => Err(format!("{what}: enter three numbers")),
            }
        };
        let (origin, axis, axis_from) = match self.source {
            AxisSource::Explicit => (
                v3(&self.origin, "origin")?,
                Some(v3(&self.axis, "axis")?),
                None,
            ),
            AxisSource::Cylinder => (
                [0.0; 3],
                None,
                Some(AxisFrom::Cylinder {
                    face: self
                        .face_a
                        .ok_or("pick a cylinder face of the parent and press Use selected face")?,
                }),
            ),
            AxisSource::Edge => (
                [0.0; 3],
                None,
                Some(AxisFrom::Edge {
                    a: self.face_a.ok_or("pick the first planar face")?,
                    b: self.face_b.ok_or("pick the second planar face")?,
                }),
            ),
        };
        let dir2 = if self.dir2.iter().all(|t| t.trim().is_empty()) {
            None
        } else {
            Some(v3(&self.dir2, "second direction")?)
        };
        let n = self.kind.dof() as usize;
        let opt = |t: &String, what: &str| -> Result<Option<f64>, String> {
            if t.trim().is_empty() {
                Ok(None)
            } else {
                parse(t)
                    .map(Some)
                    .ok_or_else(|| format!("{what}: not a number"))
            }
        };
        let mut vals = Vec::new();
        for i in 0..n {
            let [v, lo, hi] = &self.values[i];
            vals.push(DofValue {
                value: parse(v).ok_or("values must be numbers")?,
                min: opt(lo, "min")?,
                max: opt(hi, "max")?,
            });
        }
        let first = vals.first().copied().unwrap_or_default();
        Ok(Command::AddJoint {
            name: (!self.name.trim().is_empty()).then(|| self.name.trim().to_string()),
            kind: self.kind,
            parent,
            child,
            origin,
            axis,
            axis_from,
            value: first.value,
            min: first.min,
            max: first.max,
            extra: vals.into_iter().skip(1).collect(),
            dir2,
            child_axis_from: self.child_face.map(|face| AxisFrom::Cylinder { face }),
            flip: self.flip,
        })
    }
}

fn kind_label(k: JointKind) -> &'static str {
    match k {
        JointKind::Rigid => "Rigid",
        JointKind::Revolute => "Revolute",
        JointKind::Slider => "Slider",
        JointKind::Cylindrical => "Cylindrical",
        JointKind::PinSlot => "Pin-slot",
        JointKind::Planar => "Planar",
        JointKind::Ball => "Ball",
    }
}

/// The dialog (when open).
pub(crate) fn joint_dialog(
    app: &mut RcadApp,
    ui: &mut egui::Ui,
    asm: &AssemblySummary,
    send: &mut Vec<Command>,
) {
    let picked = app.selected_face.and_then(|g| {
        let db = app.scene.body_of(g)?;
        (!db.mesh_body).then_some(FacePick {
            body: db.id,
            face: g - db.groups.start,
        })
    });
    let Some(d) = app.assembly_ui.joint_dialog.as_mut() else {
        return;
    };
    let mut close = false;
    ui.group(|ui| {
        ui.label(RichText::new("New joint").strong());
        let name_of = |o: Option<OccurrenceId>| {
            o.and_then(|o| asm.occurrences.iter().find(|x| x.id == o))
                .map_or("—".to_string(), |x| x.name.clone())
        };
        ui.horizontal(|ui| {
            let r = egui::ComboBox::from_label("Joint type")
                .selected_text(kind_label(d.kind))
                .show_ui(ui, |ui| {
                    for k in JointKind::ALL {
                        let l = format!("Joint type option {}", kind_label(k));
                        ui.selectable_value(&mut d.kind, k, kind_label(k)).probe(&l);
                    }
                });
            r.response.probe("Joint type");
        });
        // Siblings only: the child list follows the parent's owner.
        let owner = d
            .parent
            .and_then(|p| asm.occurrences.iter().find(|x| x.id == p))
            .map(|x| x.owner);
        for (label, slot, is_child) in [
            ("Joint parent", &mut d.parent, false),
            ("Joint child", &mut d.child, true),
        ] {
            ui.horizontal(|ui| {
                let r = egui::ComboBox::from_label(label)
                    .selected_text(name_of(*slot))
                    .show_ui(ui, |ui| {
                        for o in asm
                            .occurrences
                            .iter()
                            .filter(|o| !is_child || owner.is_none_or(|w| o.owner == w))
                        {
                            let l = format!("{label} option {}", o.name);
                            ui.selectable_value(slot, Some(o.id), &o.name).probe(&l);
                        }
                    });
                r.response.probe(label);
            });
        }
        ui.horizontal(|ui| {
            ui.label("Axis");
            ui.radio_value(&mut d.source, AxisSource::Explicit, "Explicit");
            ui.radio_value(&mut d.source, AxisSource::Cylinder, "Cylinder face");
            ui.radio_value(&mut d.source, AxisSource::Edge, "Edge of two faces");
        });
        match d.source {
            AxisSource::Explicit => {
                ui.small("In the parent's definition coordinates (mm):");
                ui.horizontal(|ui| {
                    for (i, l) in ["Origin X", "Origin Y", "Origin Z"].iter().enumerate() {
                        crate::panels::num_field(ui, l, &mut d.origin[i]);
                    }
                });
                ui.horizontal(|ui| {
                    for (i, l) in ["Axis X", "Axis Y", "Axis Z"].iter().enumerate() {
                        crate::panels::num_field(ui, l, &mut d.axis[i]);
                    }
                });
            }
            AxisSource::Cylinder | AxisSource::Edge => {
                let two = d.source == AxisSource::Edge;
                ui.small(match picked {
                    Some(p) => format!("Selected face: body {} face {}", p.body, p.face),
                    None => "Select a face of the parent in the 3D view".into(),
                });
                ui.horizontal(|ui| {
                    let a = d
                        .face_a
                        .map_or("—".into(), |p| format!("{}/{}", p.body, p.face));
                    ui.small(format!("Face{}: {a}", if two { " 1" } else { "" }));
                    if let Some(p) = picked
                        && ui
                            .small_button(if two {
                                "Use as face 1"
                            } else {
                                "Use selected face"
                            })
                            .probe("Use selected face")
                            .clicked()
                    {
                        d.face_a = Some(p);
                    }
                });
                if two {
                    ui.horizontal(|ui| {
                        let b = d
                            .face_b
                            .map_or("—".into(), |p| format!("{}/{}", p.body, p.face));
                        ui.small(format!("Face 2: {b}"));
                        if let Some(p) = picked
                            && ui
                                .small_button("Use as face 2")
                                .probe("Use as face 2")
                                .clicked()
                        {
                            d.face_b = Some(p);
                        }
                    });
                }
            }
        }
        if matches!(
            d.kind,
            JointKind::PinSlot | JointKind::Planar | JointKind::Ball
        ) {
            ui.small(match d.kind {
                JointKind::PinSlot => "Slot direction (⊥ axis; blank = default):",
                _ => "In-plane u direction (⊥ axis; blank = default):",
            });
            ui.horizontal(|ui| {
                for (i, l) in ["Dir X", "Dir Y", "Dir Z"].iter().enumerate() {
                    crate::panels::num_field(ui, l, &mut d.dir2[i]);
                }
            });
        }
        // Optional snap of the child's own axis (a hole or boss of the child).
        ui.horizontal(|ui| {
            let t = d
                .child_face
                .map_or("—".into(), |p| format!("{}/{}", p.body, p.face));
            ui.small(format!("Child axis to snap: {t}"));
            if let Some(p) = picked
                && ui
                    .small_button("Use selected face as child axis")
                    .on_hover_text("A cylinder face of the child: it is moved so this axis lies on the joint axis before the joint is made")
                    .probe("Use selected face as child axis")
                    .clicked()
            {
                d.child_face = Some(p);
            }
            if d.child_face.is_some() {
                ui.checkbox(&mut d.flip, "Flip").on_hover_text("Reverse the child axis direction");
                if ui.small_button("No snap").probe("No snap").clicked() {
                    d.child_face = None;
                }
            }
        });
        let names = d.kind.value_names();
        for (i, n) in names.iter().enumerate() {
            let unit = d.kind.units()[i].trim();
            ui.horizontal(|ui| {
                ui.small(format!("{n} ({unit})"));
                let [v, lo, hi] = &mut d.values[i];
                let sfx = if i == 0 {
                    String::new()
                } else {
                    format!(" {}", i + 1)
                };
                crate::panels::num_field(ui, &format!("Initial value{sfx}"), v);
                crate::panels::num_field(ui, &format!("Lower limit{sfx}"), lo);
                crate::panels::num_field(ui, &format!("Upper limit{sfx}"), hi);
            });
        }
        // DOF feedback.
        let child_dof = d
            .child
            .and_then(|c| asm.occurrences.iter().find(|x| x.id == c))
            .map(|x| u32::from(x.dof));
        let n = u32::from(d.kind.dof());
        ui.small(match child_dof {
            Some(cd) => format!(
                "Leaves {n} DOF between parent and child ({}); assembly total DOF {} -> {}",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                },
                asm.total_dof,
                asm.total_dof.saturating_sub(cd) + n
            ),
            None => format!("Leaves {n} DOF between parent and child"),
        });
        crate::panels::short_text_field(ui, "Joint name", &mut d.name);
        let cmd = d.command();
        ui.horizontal(|ui| {
            match &cmd {
                Ok(c) => {
                    let r = ui
                        .add_enabled(!d.pending, egui::Button::new("Create joint"))
                        .probe("Create joint");
                    // Keep the open form's commit button in view in the
                    // browser column (the column is shorter under the ribbon).
                    if !ui.clip_rect().contains_rect(r.rect) {
                        r.scroll_to_me(Some(egui::Align::Max));
                    }
                    if r.clicked() {
                        send.push(c.clone());
                        d.pending = true;
                    }
                }
                Err(e) => {
                    ui.add_enabled(false, egui::Button::new("Create joint"));
                    ui.small(e.as_str());
                }
            }
            if ui.button("Cancel").probe("Cancel joint").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.assembly_ui.joint_dialog = None;
    }
}
