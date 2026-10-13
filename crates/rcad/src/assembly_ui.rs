//! Browser "Components" section (run 6 pull-forward of Run-7 §1-2,
//! docs/ASSEMBLIES.md): create a component from the selected body, add linked
//! occurrences, move them numerically, ground them, joints with their values and
//! limits, the BOM and the interference list. Every change is a document command;
//! the engine's reply is the only source of truth.
use eframe::egui::{self, Color32, RichText};
use rcad_commands::assembly::{BomRow, Interference, InterferenceStatus};
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::assembly::{JointId, OccurrenceId};
use rcad_document::{BodyId, Evaluation};
use rcad_kernel::TriMesh;

use crate::app::RcadApp;
use crate::probe::Probe as _;

/// The display tessellation of body `id` once per occurrence placing it, or once
/// in place when no occurrence does.
pub fn placed_meshes(ev: &Evaluation, id: BodyId, tm: TriMesh) -> Vec<TriMesh> {
    let poses: Vec<rcad_kernel::Rigid> = ev
        .occurrences
        .iter()
        .filter(|o| o.bodies.contains(&id))
        .map(|o| o.transform)
        .collect();
    if poses.is_empty() {
        return vec![tm];
    }
    poses
        .iter()
        .map(|t| {
            let mut m = tm.clone();
            if !t.is_identity() {
                for p in &mut m.positions {
                    *p = t.point(*p);
                }
                for n in &mut m.normals {
                    *n = t.vector(*n);
                }
            }
            m
        })
        .collect()
}

/// Edit buffers and the last BOM / interference replies.
#[derive(Debug, Clone, Default)]
pub struct AssemblyUi {
    pub component_name: String,
    pub part_number: String,
    /// Occurrence being moved numerically: ΔX, ΔY, ΔZ (mm), rotation about Z (°).
    pub edit: Option<(OccurrenceId, [String; 4])>,
    /// Joint value / max-limit buffers.
    pub joint_edit: Option<(JointId, String, String, String)>,
    pub bom: Option<(u64, u64, Vec<BomRow>)>,
    pub interference: Option<(u64, u64, Vec<Interference>)>,
}

impl AssemblyUi {
    /// Engine hook in `handle_reply`.
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        let Some(a) = &response.assembly else {
            return;
        };
        if let Some(b) = &a.bom {
            self.bom = Some((summary.doc_id, summary.revision, b.clone()));
        }
        if let Some(i) = &a.interference {
            self.interference = Some((summary.doc_id, summary.revision, i.clone()));
        }
    }
}

fn fmt(x: f64) -> String {
    crate::params_ui::fmt_value(x)
}

fn parse(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

impl RcadApp {
    /// "Components" group of the browser.
    pub(crate) fn components_browser(&mut self, ui: &mut egui::Ui) {
        let asm = self.summary.assembly.clone();
        let mut send: Vec<Command> = Vec::new();
        ui.separator();
        ui.label(RichText::new(format!("Components ({})", asm.components.len())).strong())
            .on_hover_text("Linked occurrences of body groups (docs/ASSEMBLIES.md). Editing a component's features changes every occurrence; a body Copy is independent.");
        // Create from the selected body.
        if let Some(b) = self.selected_body
            && let Some(bs) = self.summary.bodies.iter().find(|x| x.id == b)
        {
            let owned = asm.components.iter().find(|c| c.bodies.contains(&b));
            match owned {
                Some(c) => {
                    ui.small(format!(
                        "{} is the definition of component {}",
                        bs.name, c.name
                    ));
                }
                None => {
                    let f = &mut self.assembly_ui;
                    if f.component_name.is_empty() {
                        f.component_name = bs.name.clone();
                    }
                    crate::panels::text_field(ui, "Component name", &mut f.component_name);
                    crate::panels::text_field(ui, "Part number", &mut f.part_number);
                    if ui
                        .button("Create component from selected body")
                        .probe("Create component from selected body")
                        .clicked()
                    {
                        send.push(Command::CreateComponent {
                            name: f.component_name.clone(),
                            bodies: vec![b],
                            part_number: (!f.part_number.trim().is_empty())
                                .then(|| f.part_number.clone()),
                        });
                        f.component_name.clear();
                        f.part_number.clear();
                    }
                }
            }
        } else if asm.components.is_empty() {
            ui.weak("Select a body to make it a component.");
        }
        for c in &asm.components {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&c.name).strong());
                if !c.part_number.is_empty() {
                    ui.small(format!("P/N {}", c.part_number));
                }
                ui.small(format!("× {}", c.occurrences));
                let l = format!("Add occurrence of {}", c.name);
                if ui
                    .small_button("Add occurrence")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    send.push(Command::AddOccurrence {
                        component: c.id,
                        name: None,
                        translate: [0.0; 3],
                        axis: None,
                        angle_deg: 0.0,
                    });
                }
            });
            for o in asm.occurrences.iter().filter(|o| o.component == c.id) {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(&o.name);
                    let state = match (o.grounded, o.driven_by) {
                        (true, _) => "grounded".to_string(),
                        (false, Some(j)) => format!("by {j}, DOF {}", o.dof),
                        (false, None) => format!("free, DOF {}", o.dof),
                    };
                    ui.small(state);
                    let g = if o.grounded { "Unground" } else { "Ground" };
                    let l = format!("{g} {}", o.name);
                    if o.driven_by.is_none() && ui.small_button(g).named(&l).probe(&l).clicked() {
                        send.push(Command::SetGrounded {
                            occurrence: o.id,
                            grounded: !o.grounded,
                        });
                    }
                    let l = format!("Move {}", o.name);
                    if !o.grounded
                        && o.driven_by.is_none()
                        && ui.small_button("Move…").named(&l).probe(&l).clicked()
                    {
                        let z = o.axis[2] >= 0.0;
                        let ang = if z { o.angle_deg } else { -o.angle_deg };
                        self.assembly_ui.edit = Some((
                            o.id,
                            [
                                fmt(o.translate[0]),
                                fmt(o.translate[1]),
                                fmt(o.translate[2]),
                                fmt(ang),
                            ],
                        ));
                    }
                    let l = format!("Delete {}", o.name);
                    if ui.small_button("Delete").named(&l).probe(&l).clicked() {
                        send.push(Command::DeleteOccurrence { occurrence: o.id });
                    }
                });
                if let Some((id, buf)) = &mut self.assembly_ui.edit
                    && *id == o.id
                {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        for (i, l) in ["X (mm)", "Y (mm)", "Z (mm)", "Rotation Z (°)"]
                            .iter()
                            .enumerate()
                        {
                            crate::panels::num_field(ui, l, &mut buf[i]);
                        }
                    });
                    let vals: Vec<Option<f64>> = buf.iter().map(|t| parse(t)).collect();
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        if let [Some(x), Some(y), Some(z), Some(a)] = vals[..] {
                            if ui
                                .button("Apply placement")
                                .probe("Apply placement")
                                .clicked()
                            {
                                send.push(Command::SetOccurrenceTransform {
                                    occurrence: o.id,
                                    translate: [x, y, z],
                                    axis: Some([0.0, 0.0, 1.0]),
                                    angle_deg: a,
                                });
                            }
                        } else {
                            ui.add_enabled(false, egui::Button::new("Apply placement"));
                        }
                    });
                }
            }
        }
        // Joints.
        if !asm.joints.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Joints ({}) · total DOF {}",
                    asm.joints.len(),
                    asm.total_dof
                ))
                .strong(),
            );
        }
        for j in &asm.joints {
            let unit = match j.kind {
                rcad_document::assembly::JointKind::Revolute => "°",
                rcad_document::assembly::JointKind::Slider => " mm",
                rcad_document::assembly::JointKind::Rigid => "",
            };
            let occ = |id: OccurrenceId| {
                asm.occurrences
                    .iter()
                    .find(|o| o.id == id)
                    .map_or(id.to_string(), |o| o.name.clone())
            };
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} {:?}: {} → {} = {}{unit} [{} … {}]",
                    j.name,
                    j.kind,
                    occ(j.parent),
                    occ(j.child),
                    fmt(j.value),
                    j.min.map_or("−∞".into(), fmt),
                    j.max.map_or("∞".into(), fmt),
                ));
                let l = format!("Edit {}", j.name);
                if j.kind != rcad_document::assembly::JointKind::Rigid
                    && ui.small_button("Edit…").named(&l).probe(&l).clicked()
                {
                    self.assembly_ui.joint_edit = Some((
                        j.id,
                        fmt(j.value),
                        j.min.map_or(String::new(), fmt),
                        j.max.map_or(String::new(), fmt),
                    ));
                }
            });
            if let Some((id, val, lo, hi)) = &mut self.assembly_ui.joint_edit
                && *id == j.id
            {
                ui.horizontal(|ui| {
                    crate::panels::num_field(ui, "Joint value", val);
                    if let Some(v) = parse(val)
                        && ui.button("Set value").probe("Set value").clicked()
                    {
                        send.push(Command::SetJointValue {
                            joint: j.id,
                            value: v,
                        });
                    }
                });
                ui.horizontal(|ui| {
                    crate::panels::num_field(ui, "Min", lo);
                    crate::panels::num_field(ui, "Max", hi);
                    let (a, b) = (parse(lo), parse(hi));
                    if (lo.trim().is_empty() || a.is_some())
                        && (hi.trim().is_empty() || b.is_some())
                        && ui.button("Set limits").probe("Set limits").clicked()
                    {
                        send.push(Command::SetJointLimits {
                            joint: j.id,
                            min: a,
                            max: b,
                        });
                    }
                });
            }
        }
        if !asm.components.is_empty() {
            ui.horizontal(|ui| {
                if ui.button("Bill of materials").probe("Bill of materials").clicked() {
                    send.push(Command::Bom);
                }
                if ui
                    .button("Check interference")
                    .on_hover_text("Static overlap check between occurrences at their current placement (not collision avoidance)")
                    .probe("Check interference")
                    .clicked()
                {
                    send.push(Command::Interference { occurrences: None });
                }
            });
        }
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        if let Some((d, r, rows)) = &self.assembly_ui.bom
            && (*d, *r) == (doc, rev)
        {
            ui.label(RichText::new("BOM (flat; quantity = occurrences)").strong());
            for r in rows {
                ui.small(format!(
                    "{}. {} ×{}{}{}",
                    r.item,
                    r.name,
                    r.quantity,
                    if r.part_number.is_empty() {
                        String::new()
                    } else {
                        format!(" · P/N {}", r.part_number)
                    },
                    if r.component.is_none() {
                        " · loose body"
                    } else {
                        ""
                    }
                ));
            }
        }
        if let Some((d, r, list)) = &self.assembly_ui.interference
            && (*d, *r) == (doc, rev)
        {
            if list.is_empty() {
                ui.small("No interference found between occurrences.");
            }
            for i in list {
                let t = match &i.status {
                    InterferenceStatus::Overlap { volume, exact, .. } => format!(
                        "{} ∩ {}: {} mm³{}",
                        i.a_name,
                        i.b_name,
                        fmt(*volume),
                        if *exact { "" } else { " (estimate)" }
                    ),
                    InterferenceStatus::OverlapVolumeUnknown { reason } => {
                        format!(
                            "{} ∩ {}: overlap, volume unknown ({reason})",
                            i.a_name, i.b_name
                        )
                    }
                    _ => continue,
                };
                ui.colored_label(Color32::from_rgb(255, 150, 120), t);
            }
        }
        for c in send {
            self.request(c);
        }
    }
}
