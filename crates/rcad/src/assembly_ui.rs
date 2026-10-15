//! Browser "Assembly" section
//! (docs/ASSEMBLIES.md): a navigable occurrence tree (sub-assemblies indented,
//! linked vs independent copies labelled, inactive parts dimmed), the selected
//! occurrence's actions (activate its component, hide, ground, suppress, move
//! with snapping, align face to face, rename, make independent, group into a
//! sub-assembly, insert into a sub-assembly, explode offset, delete), joints
//! with per-value editing, structured / flattened BOM and selectable
//! interference results highlighted in the viewport. Every change is a document
//! command; the engine's reply is the only source of truth. The joint-creation
//! dialog is in `joint_ui.rs`.
use eframe::egui::{self, Color32, RichText};
use rcad_commands::assembly::{
    AssemblySummary, BomRow, Interference, InterferenceStatus, StructuredBomRow,
};
use rcad_commands::measure_faces::FacePick;
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::assembly::{ComponentId, JointId, OccurrenceId};
use rcad_document::{BodyId, Evaluation};
use rcad_kernel::TriMesh;

use crate::app::RcadApp;
use crate::engine::DisplayScene;
use crate::probe::Probe as _;

/// The display tessellation of body `id` once per visible instance placing it
/// (with its exploded offset), or once in place when no instance does. Each
/// copy carries its instance path and whether it lies outside the active
/// component (drawn dimmed).
pub fn placed_meshes(
    ev: &Evaluation,
    id: BodyId,
    tm: TriMesh,
) -> Vec<(TriMesh, Option<String>, bool)> {
    let inst: Vec<_> = ev
        .occurrences
        .iter()
        .filter(|o| o.bodies.contains(&id))
        .collect();
    if inst.is_empty() {
        // A loose body is outside the active component when one is active.
        let dim = ev.occurrences.iter().any(|o| !o.in_active);
        return vec![(tm, None, dim)];
    }
    inst.iter()
        .filter(|o| o.visible)
        .map(|o| {
            let mut t = o.transform;
            for (k, e) in o.explode.iter().enumerate() {
                t.t[k] += e;
            }
            let mut m = tm.clone();
            if !t.is_identity() {
                for p in &mut m.positions {
                    *p = t.point(*p);
                }
                for n in &mut m.normals {
                    *n = t.vector(*n);
                }
            }
            (m, Some(o.path_string()), !o.in_active)
        })
        .collect()
}

/// Viewport tints: highlighted instances (selection / interference) first,
/// then dimmed bodies outside the active component, else the body tint.
pub fn tints(scene: &DisplayScene, highlight: &[String]) -> Vec<(std::ops::Range<u32>, u8)> {
    scene
        .bodies
        .iter()
        .map(|b| {
            let t = if b.instance.as_ref().is_some_and(|p| {
                highlight
                    .iter()
                    .any(|h| p == h || p.starts_with(&format!("{h}/")))
            }) {
                rcad_viewport::TINT_HIGHLIGHT
            } else if b.inactive {
                rcad_viewport::TINT_DIMMED
            } else {
                b.tint
            };
            (b.groups.clone(), t)
        })
        .collect()
}

/// Joint being edited: value buffers and (min, max) buffers per value.
pub type JointEdit = (JointId, Vec<String>, Vec<(String, String)>);

/// Edit buffers and the last BOM / interference replies.
#[derive(Debug, Clone, Default)]
pub struct AssemblyUi {
    pub component_name: String,
    pub part_number: String,
    /// Name of a new empty component.
    pub empty_name: String,
    /// Selected tree row (instance path "O3/O7").
    pub selected: Option<String>,
    /// Occurrence being moved numerically: X, Y, Z (mm), rotation about Z (°).
    pub edit: Option<(OccurrenceId, [String; 4])>,
    /// Snap increment (mm) of the nudge buttons and rotation step (°).
    pub snap: String,
    pub rotate_step: String,
    /// Rename buffer for the selected occurrence / its component.
    pub rename: Option<(OccurrenceId, String, String)>,
    /// Exploded offset buffer for the selected path.
    pub explode: Option<(String, [String; 3])>,
    /// Sub-assembly grouping: siblings checked, name.
    pub group: Option<(Vec<OccurrenceId>, String)>,
    /// Component chosen to insert into the selected component.
    pub insert: Option<ComponentId>,
    /// "Align from" face and its occurrence.
    pub align_from: Option<(FacePick, OccurrenceId, String)>,
    /// Joint value / limit buffers: values, (min, max) per value.
    pub joint_edit: Option<JointEdit>,
    /// Axis buffers of the joint being edited: origin, axis.
    pub joint_axis: [String; 6],
    /// Part number buffer (rename panel).
    pub part_number_edit: String,
    pub bom: Option<(u64, u64, Vec<BomRow>, Vec<StructuredBomRow>)>,
    /// Show the structured BOM (else flattened).
    pub structured: bool,
    pub interference: Option<(u64, u64, Vec<Interference>)>,
    /// Interference row whose occurrences are highlighted.
    pub interference_pick: Option<usize>,
    pub joint_dialog: Option<crate::joint_ui::JointDialog>,
    /// BOM CSV path; whether an assembly mass report was requested.
    pub bom_path: String,
    pub mass_requested: bool,
    /// Last joint sweep: (doc, revision, joint, value index, samples).
    pub sweep: Option<SweepResult>,
    /// Joint and value index of the sweep request in flight.
    pub sweep_joint: Option<(JointId, usize)>,
    /// Drive-slider value while the pointer is held on it
    /// (joint, value index, value); committed once on release.
    pub drive_pending: Option<(JointId, usize, f64)>,
    /// Index into `exchange::UNITS` used when an imported
    /// STEP assembly declares no length unit (None: ask by refusing).
    pub assembly_units: Option<usize>,
    /// Instance path of the last viewport face pick (a new pick selects its
    /// tree row).
    pub last_pick: Option<String>,
    /// External component insertion: path, linked.
    pub external_path: String,
    pub external_linked: bool,
    /// Last external status reply.
    pub external_status: Option<(u64, u64, Vec<rcad_commands::assembly::ExternalStatus>)>,
}

/// (doc, revision, joint, value index, samples) of a joint sweep.
pub type SweepResult = (
    u64,
    u64,
    JointId,
    usize,
    Vec<rcad_commands::assembly::SweepStep>,
);

impl AssemblyUi {
    /// Engine hook in `handle_reply`.
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        // The joint dialog closes only when its joint was
        // made; a refusal keeps every input.
        if response.command == "add_joint"
            && let Some(d) = &mut self.joint_dialog
            && d.pending
        {
            if response.ok {
                self.joint_dialog = None;
            } else {
                d.pending = false;
            }
        }
        // A refused sweep does not leave the previous
        // result on screen as if it were the answer.
        if response.command == "joint_sweep" && !response.ok {
            self.sweep = None;
            self.sweep_joint = None;
        }
        let Some(a) = &response.assembly else {
            return;
        };
        if let Some(b) = &a.bom {
            self.bom = Some((
                summary.doc_id,
                summary.revision,
                b.clone(),
                a.structured_bom.clone().unwrap_or_default(),
            ));
        }
        if let (Some(sw), Some((j, i))) = (&a.sweep, self.sweep_joint.take()) {
            self.sweep = Some((summary.doc_id, summary.revision, j, i, sw.clone()));
        }
        if let Some(x) = &a.external {
            self.external_status = Some((summary.doc_id, summary.revision, x.clone()));
        }
        if let Some(i) = &a.interference {
            self.interference = Some((summary.doc_id, summary.revision, i.clone()));
            self.interference_pick = None;
        }
    }

    /// Instance paths drawn highlighted: the selected tree row and the
    /// picked interference pair.
    pub fn highlight_paths(&self, summary: &DocSummary) -> Vec<String> {
        let mut out: Vec<String> = self.selected.iter().cloned().collect();
        if let (Some(i), Some((d, r, list))) = (self.interference_pick, &self.interference)
            && (*d, *r) == (summary.doc_id, summary.revision)
            && let Some(x) = list.get(i)
        {
            for p in [&x.a_path, &x.b_path] {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
        }
        out
    }
}

pub(crate) fn fmt(x: f64) -> String {
    crate::params_ui::fmt_value(x)
}

pub(crate) fn parse(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Leaf occurrence of a path "O3/O7".
fn leaf(path: &str) -> Option<OccurrenceId> {
    rcad_document::assembly::parse_path(path)
        .ok()
        .and_then(|p| p.last().copied())
}

/// What the tree row says about an occurrence's link state.
fn link_text(a: &AssemblySummary, c: ComponentId) -> String {
    let Some(comp) = a.components.iter().find(|x| x.id == c) else {
        return String::new();
    };
    let mut parts = vec![comp.name.clone()];
    if let Some(src) = comp.derived_from {
        let n = a
            .components
            .iter()
            .find(|x| x.id == src)
            .map_or(src.to_string(), |x| x.name.clone());
        parts.push(format!("independent copy of {n}"));
    } else if comp.instances > 1 {
        parts.push(format!("linked ×{}", comp.instances));
    }
    if !comp.children.is_empty() {
        parts.push("sub-assembly".into());
    }
    if let Some(x) = &comp.external {
        parts.push(format!(
            "external {} ({})",
            x.file_name,
            if x.path.is_some() {
                "linked"
            } else {
                "embedded"
            }
        ));
    }
    parts.join(" · ")
}

impl RcadApp {
    /// Instance paths currently highlighted (tree selection, picked
    /// interference pair).
    pub fn assembly_highlight(&self) -> Vec<String> {
        self.assembly_ui.highlight_paths(&self.summary)
    }

    /// Number of interference results of the current revision (None when
    /// no check was run for it).
    pub fn interference_count(&self) -> Option<usize> {
        let (d, r) = (self.summary.doc_id, self.summary.revision);
        self.assembly_ui
            .interference
            .as_ref()
            .filter(|x| (x.0, x.1) == (d, r))
            .map(|x| x.2.len())
    }

    /// The viewport tint table for the current scene and highlight.
    pub fn display_tints(&self) -> Vec<(std::ops::Range<u32>, u8)> {
        tints(&self.scene, &self.assembly_highlight())
    }

    /// Re-tint the viewport after a highlight change (no re-tessellation).
    pub(crate) fn apply_assembly_tints(&mut self) {
        let hl = self.assembly_ui.highlight_paths(&self.summary);
        let t = tints(&self.scene, &hl);
        if let Some(vp) = self.viewport.as_mut() {
            vp.set_group_tints(&t);
        }
    }

    /// The selected viewport face as a definition face pick and the instance
    /// path of the copy it was picked on.
    fn picked_face_instance(&self) -> Option<(FacePick, Option<String>)> {
        let g = self.selected_face?;
        let db = self.scene.body_of(g)?;
        if db.mesh_body {
            return None;
        }
        Some((
            FacePick {
                body: db.id,
                face: g - db.groups.start,
            },
            db.instance.clone(),
        ))
    }

    /// "Components" group of the browser.
    pub(crate) fn components_browser(&mut self, ui: &mut egui::Ui) {
        let asm = self.summary.assembly.clone();
        let mut send: Vec<Command> = Vec::new();
        // A face picked on a placed copy selects that instance's tree row.
        let pick = self.picked_face_instance().and_then(|(_, p)| p);
        if pick != self.assembly_ui.last_pick {
            self.assembly_ui.last_pick = pick.clone();
            if pick.is_some() {
                self.assembly_ui.selected = pick;
            }
        }
        let before_hl = self.assembly_ui.highlight_paths(&self.summary);
        ui.separator();
        ui.label(RichText::new(format!("Assembly ({} components)", asm.components.len())).strong())
            .on_hover_text("Components, linked occurrences and sub-assemblies (docs/ASSEMBLIES.md). Editing a component changes every linked occurrence; an independent copy does not follow.");
        // Active component.
        match asm.active {
            Some(c) => {
                let n = asm
                    .components
                    .iter()
                    .find(|x| x.id == c)
                    .map_or(c.to_string(), |x| x.name.clone());
                ui.horizontal(|ui| {
                    ui.colored_label(
                        Color32::from_rgb(120, 200, 255),
                        format!("Editing component {n} (others dimmed)"),
                    );
                    if ui
                        .small_button("Return to top level")
                        .probe("Return to top level")
                        .clicked()
                    {
                        send.push(Command::ActivateComponent { component: None });
                    }
                });
            }
            None if !asm.components.is_empty() => {
                ui.small("Editing the top level");
            }
            None => {}
        }
        self.create_component_ui(ui, &asm, &mut send);
        // Tree.
        if !asm.instances.is_empty() {
            ui.label(RichText::new("Occurrences").strong());
            self.tree_ui(ui, &asm);
        }
        // Components with no instance (e.g. all suppressed) stay reachable.
        for c in asm.components.iter().filter(|c| c.instances == 0) {
            ui.horizontal(|ui| {
                ui.weak(format!("{} (no active occurrence)", c.name));
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
                        owner: None,
                        translate: [0.0; 3],
                        axis: None,
                        angle_deg: 0.0,
                    });
                }
            });
        }
        // Selected occurrence actions.
        if let Some(path) = self.assembly_ui.selected.clone() {
            if asm.instances.iter().any(|i| i.path == path) {
                self.selected_actions(ui, &asm, &path, &mut send);
            } else {
                self.assembly_ui.selected = None;
            }
        }
        self.align_ui(ui, &asm, &mut send);
        self.joints_ui(ui, &asm, &mut send);
        crate::joint_ui::joint_dialog(self, ui, &asm, &mut send);
        self.bom_interference_ui(ui, &asm, &mut send);
        for c in send {
            self.request(c);
        }
        if self.assembly_ui.highlight_paths(&self.summary) != before_hl {
            self.apply_assembly_tints();
        }
    }

    fn create_component_ui(
        &mut self,
        ui: &mut egui::Ui,
        asm: &AssemblySummary,
        send: &mut Vec<Command>,
    ) {
        if let Some(b) = self.selected_body
            && let Some(bs) = self.summary.bodies.iter().find(|x| x.id == b)
        {
            match asm.components.iter().find(|c| c.bodies.contains(&b)) {
                Some(c) => {
                    ui.small(format!(
                        "{} is a definition body of component {}",
                        bs.name, c.name
                    ));
                }
                None => {
                    let f = &mut self.assembly_ui;
                    if f.component_name.is_empty() {
                        f.component_name = bs.name.clone();
                    }
                    crate::panels::short_text_field(ui, "Component name", &mut f.component_name);
                    crate::panels::short_text_field(ui, "Part number", &mut f.part_number);
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
                            occurrences: vec![],
                        });
                        f.component_name.clear();
                        f.part_number.clear();
                    }
                }
            }
        } else if asm.components.is_empty() {
            ui.weak("Select a body to make it a component, or create an empty one.");
        }
        ui.horizontal(|ui| {
            crate::panels::short_text_field(ui, "New component", &mut self.assembly_ui.empty_name);
            let n = self.assembly_ui.empty_name.trim().to_string();
            if ui
                .add_enabled(!n.is_empty(), egui::Button::new("Create empty"))
                .on_hover_text("A component without bodies and one occurrence; activate it, then model into it")
                .probe("Create empty component")
                .clicked()
            {
                send.push(Command::CreateComponent {
                    name: n,
                    bodies: vec![],
                    part_number: None,
                    occurrences: vec![],
                });
                self.assembly_ui.empty_name.clear();
            }
        });
        crate::panels::short_path_field(
            ui,
            "External document",
            &mut self.assembly_ui.external_path,
            crate::file_pick::PickMode::Open,
            crate::file_pick::EXTERNAL,
        );
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.assembly_ui.external_linked, "Linked");
            let p = self.assembly_ui.external_path.trim().to_string();
            if ui
                .add_enabled(!p.is_empty(), egui::Button::new("Insert"))
                .on_hover_text("Insert another .rcad.json as a component: an embedded copy of its solids with a pinned content hash. Linked copies update only when you press Update from file.")
                .named("Insert external component")
                .clicked()
            {
                send.push(Command::InsertExternalComponent {
                    path: p.clone().into(),
                    name: None,
                    linked: self.assembly_ui.external_linked,
                });
            }
        });
        {
            let p = self.assembly_ui.external_path.trim().to_string();
            let tip = "Import a STEP file with its product structure: components, \
                       sub-assemblies and linked occurrences with their placements";
            // The length unit for a file that declares
            // none (a declared unit always wins).
            let units = crate::exchange::UNITS;
            let shown = self
                .assembly_ui
                .assembly_units
                .and_then(|i| units.get(i))
                .map_or("declared only", |u| u.0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("STEP assembly units")
                    .selected_text(format!("Undeclared units: {shown}"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.assembly_ui.assembly_units,
                            None,
                            "declared only",
                        )
                        .on_hover_text("Refuse a file without a length unit");
                        for (i, (n, _)) in units.iter().enumerate() {
                            ui.selectable_value(&mut self.assembly_ui.assembly_units, Some(i), *n);
                        }
                    })
                    .response
                    .named("STEP assembly units")
                    .on_hover_text("Length unit used only when the STEP file declares none");
            });
            if ui
                .add_enabled(!p.is_empty(), egui::Button::new("Import STEP assembly"))
                .on_hover_text(tip)
                .named("Import STEP assembly")
                .clicked()
            {
                send.push(Command::ImportStepAssembly {
                    path: p.into(),
                    units_to_mm: self
                        .assembly_ui
                        .assembly_units
                        .and_then(|i| units.get(i))
                        .map(|u| u.1),
                });
            }
        }
        if asm.components.iter().any(|c| c.external.is_some()) {
            if ui
                .small_button("Check external files")
                .probe("Check external files")
                .clicked()
            {
                send.push(Command::CheckExternalComponents);
            }
            let (doc, rev) = (self.summary.doc_id, self.summary.revision);
            if let Some((d, r, list)) = &self.assembly_ui.external_status
                && (*d, *r) == (doc, rev)
            {
                for x in list {
                    let t = format!("{}: {} — {}", x.name, x.status, x.detail);
                    if x.status == "up_to_date" || x.status == "embedded" {
                        ui.small(t);
                    } else {
                        ui.colored_label(Color32::from_rgb(255, 190, 90), t);
                    }
                }
            }
        }
    }

    fn tree_ui(&mut self, ui: &mut egui::Ui, asm: &AssemblySummary) {
        for i in &asm.instances {
            let occ = asm.occurrences.iter().find(|o| o.id == i.occurrence);
            let Some(o) = occ else { continue };
            let sel = self.assembly_ui.selected.as_deref() == Some(i.path.as_str());
            ui.horizontal(|ui| {
                ui.add_space(4.0 + 14.0 * i.level as f32);
                let state = match (o.grounded, o.driven_by) {
                    (true, _) => "grounded".to_string(),
                    (false, Some(j)) => format!("by {j}, DOF {}", o.dof),
                    (false, None) => format!("free, DOF {}", o.dof),
                };
                let mut text = RichText::new(&i.path_name);
                if !i.in_active {
                    text = text.weak();
                }
                if !i.visible {
                    text = text.italics();
                }
                let key = format!("Occurrence {}", i.path_name);
                let r = ui
                    .selectable_label(sel, text)
                    .on_hover_text(format!("{} ({})", i.path, link_text(asm, i.component)))
                    .named(&key);
                if r.clicked() {
                    self.assembly_ui.selected = (!sel).then(|| i.path.clone());
                    self.assembly_ui.edit = None;
                    self.assembly_ui.rename = None;
                    self.assembly_ui.explode = None;
                    self.assembly_ui.group = None;
                }
                let mut tags = link_text(asm, i.component);
                tags.push_str(" · ");
                tags.push_str(&state);
                if !i.visible {
                    tags.push_str(" · hidden");
                }
                if asm.active == Some(i.component) {
                    tags.push_str(" · (active)");
                }
                ui.small(tags);
            });
        }
    }

    fn selected_actions(
        &mut self,
        ui: &mut egui::Ui,
        asm: &AssemblySummary,
        path: &str,
        send: &mut Vec<Command>,
    ) {
        let Some(inst) = asm.instances.iter().find(|i| i.path == path).cloned() else {
            return;
        };
        let Some(o) = asm
            .occurrences
            .iter()
            .find(|o| Some(o.id) == leaf(path))
            .cloned()
        else {
            return;
        };
        let Some(comp) = asm.components.iter().find(|c| c.id == o.component).cloned() else {
            return;
        };
        ui.group(|ui| {
            ui.label(RichText::new(format!("Selected: {}", inst.path_name)).strong());
            ui.small(format!(
                "{} · component {}{}",
                path,
                comp.name,
                if o.owner.is_some() {
                    " · edits apply to every instance of the parent sub-assembly"
                } else {
                    ""
                }
            ));
            ui.horizontal_wrapped(|ui| {
                let active = asm.active == Some(comp.id);
                let (t, l, c) = if active {
                    (
                        "Return to top level",
                        format!("Deactivate {}", comp.name),
                        None,
                    )
                } else {
                    ("Activate", format!("Activate {}", comp.name), Some(comp.id))
                };
                if ui
                    .small_button(t)
                    .on_hover_text("New bodies go into the active component; the rest is dimmed")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    send.push(Command::ActivateComponent { component: c });
                }
                let h = if o.hidden { "Show" } else { "Hide" };
                let l = format!("{h} {}", o.name);
                if ui.small_button(h).named(&l).probe(&l).clicked() {
                    send.push(Command::SetOccurrenceHidden {
                        occurrence: o.id,
                        hidden: !o.hidden,
                    });
                }
                let g = if o.grounded { "Unground" } else { "Ground" };
                let l = format!("{g} {}", o.name);
                if o.driven_by.is_none() && ui.small_button(g).named(&l).probe(&l).clicked() {
                    send.push(Command::SetGrounded {
                        occurrence: o.id,
                        grounded: !o.grounded,
                    });
                }
                let l = format!("Suppress {}", o.name);
                if ui
                    .small_button("Suppress")
                    .on_hover_text("Excluded from display, BOM, mass, interference and export (definition kept)")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    send.push(Command::SetOccurrenceSuppressed {
                        occurrence: o.id,
                        suppressed: true,
                    });
                    self.assembly_ui.selected = None;
                }
                let l = format!("Move {}", o.name);
                if !o.grounded
                    && o.driven_by.is_none()
                    && ui.small_button("Move…").named(&l).probe(&l).clicked()
                {
                    // Keep the stored rotation axis (an aligned or
                    // joint-released occurrence may turn about any axis).
                    let ang = o.angle_deg;
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
                let l = format!("Rename {}", o.name);
                if ui.small_button("Rename…").named(&l).probe(&l).clicked() {
                    self.assembly_ui.rename = Some((o.id, o.name.clone(), comp.name.clone()));
                    self.assembly_ui.part_number_edit = comp.part_number.clone();
                }
                let l = format!("Make {} independent", o.name);
                if comp.children.is_empty()
                    && !comp.bodies.is_empty()
                    && ui
                        .small_button("Make independent")
                        .on_hover_text("Copy the definition bodies into a new component; this occurrence stops following the shared definition")
                        .named(&l)
                        .probe(&l)
                        .clicked()
                {
                    send.push(Command::MakeIndependent {
                        occurrence: o.id,
                        name: None,
                    });
                }
                let l = format!("Update {} from file", comp.name);
                if comp.external.as_ref().is_some_and(|x| x.path.is_some())
                    && ui
                        .small_button("Update from file")
                        .on_hover_text("Re-read the linked source; refused (copy kept) when it is missing or its solids changed in number")
                        .named(&l)
                        .probe(&l)
                        .clicked()
                {
                    send.push(Command::UpdateExternalComponent { component: comp.id });
                }
                let l = format!("Add occurrence of {}", comp.name);
                if ui
                    .small_button("Add occurrence")
                    .on_hover_text("A further linked occurrence next to this one")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    send.push(Command::AddOccurrence {
                        component: comp.id,
                        name: None,
                        owner: o.owner,
                        translate: [0.0; 3],
                        axis: None,
                        angle_deg: 0.0,
                    });
                }
                let l = format!("Group {}", o.name);
                if ui
                    .small_button("Group…")
                    .on_hover_text("Make a sub-assembly from this and sibling occurrences")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    self.assembly_ui.group = Some((vec![o.id], format!("{}Asm", comp.name)));
                }
                let l = format!("Joint from {}", o.name);
                if ui
                    .small_button("Joint…")
                    .on_hover_text("New joint with this occurrence as the parent")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    self.assembly_ui.joint_dialog =
                        Some(crate::joint_ui::JointDialog::new(Some(o.id)));
                }
                let l = format!("Explode {}", inst.path_name);
                if ui.small_button("Explode…").named(&l).probe(&l).clicked() {
                    let e = asm.explode.get(path).copied().unwrap_or([0.0; 3]);
                    self.assembly_ui.explode = Some((path.to_string(), e.map(fmt)));
                }
                let l = format!("Delete {}", o.name);
                if ui.small_button("Delete").named(&l).probe(&l).clicked() {
                    send.push(Command::DeleteOccurrence { occurrence: o.id });
                    self.assembly_ui.selected = None;
                }
            });
            // Move with snapping.
            if let Some((id, buf)) = &mut self.assembly_ui.edit
                && *id == o.id
            {
                ui.horizontal(|ui| {
                    for (i, l) in ["X (mm)", "Y (mm)", "Z (mm)", "Rotation (°)"]
                        .iter()
                        .enumerate()
                    {
                        crate::panels::num_field(ui, l, &mut buf[i]);
                    }
                });
                ui.small(format!(
                    "rotation about the axis ({}, {}, {}) through the owner's origin",
                    fmt(o.axis[0]),
                    fmt(o.axis[1]),
                    fmt(o.axis[2])
                ));
                let vals: Vec<Option<f64>> = buf.iter().map(|t| parse(t)).collect();
                ui.horizontal(|ui| {
                    if let [Some(x), Some(y), Some(z), Some(a)] = vals[..] {
                        if ui
                            .button("Apply placement")
                            .probe("Apply placement")
                            .clicked()
                        {
                            send.push(Command::SetOccurrenceTransform {
                                occurrence: o.id,
                                translate: [x, y, z],
                                axis: Some(o.axis),
                                angle_deg: a,
                            });
                        }
                    } else {
                        ui.add_enabled(false, egui::Button::new("Apply placement"));
                    }
                });
                let f = &mut self.assembly_ui;
                if f.snap.is_empty() {
                    f.snap = "5".into();
                }
                if f.rotate_step.is_empty() {
                    f.rotate_step = "15".into();
                }
                ui.horizontal(|ui| {
                    crate::panels::num_field(ui, "Snap (mm)", &mut f.snap);
                    crate::panels::num_field(ui, "Step (deg)", &mut f.rotate_step);
                });
                let (step, rstep) = (parse(&f.snap), parse(&f.rotate_step));
                if let (Some(step), Some(rstep), [Some(x), Some(y), Some(z), Some(a)]) =
                    (step, rstep, &vals[..])
                    && step > 0.0
                {
                    // Nudges snap the coordinate to the increment grid.
                    let snapv = |v: f64, d: f64| ((v / step).round() + d) * step;
                    ui.horizontal_wrapped(|ui| {
                        let mut nudge = |ui: &mut egui::Ui, label: &str, t: [f64; 3], ang: f64| {
                            if ui.small_button(label).probe(label).clicked() {
                                send.push(Command::SetOccurrenceTransform {
                                    occurrence: o.id,
                                    translate: t,
                                    axis: Some(o.axis),
                                    angle_deg: ang,
                                });
                            }
                        };
                        nudge(ui, "−X", [snapv(*x, -1.0), *y, *z], *a);
                        nudge(ui, "+X", [snapv(*x, 1.0), *y, *z], *a);
                        nudge(ui, "−Y", [*x, snapv(*y, -1.0), *z], *a);
                        nudge(ui, "+Y", [*x, snapv(*y, 1.0), *z], *a);
                        nudge(ui, "−Z", [*x, *y, snapv(*z, -1.0)], *a);
                        nudge(ui, "+Z", [*x, *y, snapv(*z, 1.0)], *a);
                        let r = |d: f64| {
                            let v = ((a / rstep).round() + d) * rstep;
                            if v.abs() > 360.0 { v % 360.0 } else { v }
                        };
                        nudge(ui, "⟲ step", [*x, *y, *z], r(1.0));
                        nudge(ui, "⟳ step", [*x, *y, *z], r(-1.0));
                    });
                }
            }
            // Rename occurrence / component.
            if let Some((id, on, cn)) = &mut self.assembly_ui.rename
                && *id == o.id
            {
                ui.horizontal(|ui| {
                    crate::panels::short_text_field(ui, "Occurrence name", on);
                    if ui
                        .small_button("Rename occurrence")
                        .probe("Rename occurrence")
                        .clicked()
                    {
                        send.push(Command::RenameOccurrence {
                            occurrence: o.id,
                            name: on.clone(),
                        });
                    }
                });
                ui.horizontal(|ui| {
                    crate::panels::short_text_field(ui, "Component name ", cn);
                    if ui
                        .small_button("Rename component")
                        .probe("Rename component")
                        .clicked()
                    {
                        send.push(Command::RenameComponent {
                            component: comp.id,
                            name: cn.clone(),
                        });
                    }
                });
                let pn = &mut self.assembly_ui.part_number_edit;
                ui.horizontal(|ui| {
                    crate::panels::short_text_field(ui, "Part number ", pn);
                    if ui
                        .small_button("Set part number")
                        .probe("Set part number")
                        .clicked()
                    {
                        send.push(Command::SetPartNumber {
                            component: comp.id,
                            part_number: pn.clone(),
                        });
                    }
                });
                let l = format!("Delete component {}", comp.name);
                if ui
                    .small_button("Delete component")
                    .on_hover_text("Remove the component and all its occurrences (its bodies become loose bodies); refused while a joint or another component uses it")
                    .named(&l)
                    .probe(&l)
                    .clicked()
                {
                    send.push(Command::DeleteComponent { component: comp.id });
                    self.assembly_ui.selected = None;
                }
            }
            // Exploded offset.
            if let Some((p, buf)) = &mut self.assembly_ui.explode
                && p == path
            {
                ui.horizontal(|ui| {
                    for (i, l) in ["Explode X", "Explode Y", "Explode Z"].iter().enumerate() {
                        crate::panels::num_field(ui, l, &mut buf[i]);
                    }
                });
                if let [Some(x), Some(y), Some(z)] = buf.each_ref().map(|t| parse(t))
                    && ui
                        .small_button("Set explode offset")
                        .on_hover_text("Display only: geometry, joints, BOM and mass are unchanged")
                        .probe("Set explode offset")
                        .clicked()
                {
                    send.push(Command::SetExplodeOffset {
                        path: path.to_string(),
                        offset: [x, y, z],
                    });
                }
            }
            // Group into a sub-assembly.
            if let Some((list, name)) = &mut self.assembly_ui.group
                && list.first() == Some(&o.id)
            {
                ui.small("Group with siblings:");
                for s in asm
                    .occurrences
                    .iter()
                    .filter(|x| x.owner == o.owner && x.id != o.id && !x.suppressed)
                {
                    let mut on = list.contains(&s.id);
                    if ui
                        .checkbox(&mut on, format!("Include {}", s.name))
                        .changed()
                    {
                        if on {
                            list.push(s.id);
                        } else {
                            list.retain(|x| *x != s.id);
                        }
                    }
                }
                crate::panels::short_text_field(ui, "Sub-assembly name", name);
                if ui
                    .small_button("Create sub-assembly")
                    .probe("Create sub-assembly")
                    .clicked()
                {
                    send.push(Command::CreateComponent {
                        name: name.clone(),
                        bodies: vec![],
                        part_number: None,
                        occurrences: list.clone(),
                    });
                    self.assembly_ui.selected = None;
                }
            }
            // Insert another component into this one's definition.
            let candidates: Vec<(ComponentId, String)> = asm
                .components
                .iter()
                .filter(|c| c.id != comp.id)
                .map(|c| (c.id, c.name.clone()))
                .collect();
            if !candidates.is_empty() {
                ui.horizontal(|ui| {
                    let cur = self
                        .assembly_ui
                        .insert
                        .and_then(|c| candidates.iter().find(|x| x.0 == c))
                        .map_or("Component…".to_string(), |x| x.1.clone());
                    egui::ComboBox::from_id_salt("insert-component")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for (id, n) in &candidates {
                                ui.selectable_value(&mut self.assembly_ui.insert, Some(*id), n);
                            }
                        });
                    let l = format!("Insert into {}", comp.name);
                    if let Some(c) = self.assembly_ui.insert
                        && ui
                            .small_button("Insert into")
                            .on_hover_text("Place an occurrence of that component inside this component's definition (sub-assembly); cyclic containment is refused")
                            .named(&l)
                            .probe(&l)
                            .clicked()
                    {
                        send.push(Command::AddOccurrence {
                            component: c,
                            name: None,
                            owner: Some(comp.id),
                            translate: [0.0; 3],
                            axis: None,
                            angle_deg: 0.0,
                        });
                    }
                });
            }
        });
        // Suppressed siblings can be restored from here.
        for s in asm.occurrences.iter().filter(|x| x.suppressed) {
            ui.horizontal(|ui| {
                ui.weak(format!("{} (suppressed)", s.name));
                let l = format!("Restore {}", s.name);
                if ui.small_button("Restore").named(&l).probe(&l).clicked() {
                    send.push(Command::SetOccurrenceSuppressed {
                        occurrence: s.id,
                        suppressed: false,
                    });
                }
            });
        }
    }

    /// Face-to-face alignment from viewport face picks.
    fn align_ui(&mut self, ui: &mut egui::Ui, asm: &AssemblySummary, send: &mut Vec<Command>) {
        let Some((pick, Some(path))) = self.picked_face_instance() else {
            return;
        };
        let Some(o) = leaf(&path) else { return };
        let Some(inst) = asm.instances.iter().find(|i| i.path == path) else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.small(format!("Face {} on {}", pick.face, inst.path_name));
            match self.assembly_ui.align_from.clone() {
                Some((from, fo, fname)) if fo != o => {
                    if ui
                        .small_button(format!("Align {fname} to this face"))
                        .named("Align to this face")
                        .probe("Align to this face")
                        .clicked()
                    {
                        send.push(Command::AlignOccurrence {
                            occurrence: fo,
                            from,
                            target: o,
                            to: pick,
                            offset: 0.0,
                        });
                        self.assembly_ui.align_from = None;
                    }
                    if ui.small_button("Cancel align").clicked() {
                        self.assembly_ui.align_from = None;
                    }
                }
                _ => {
                    if ui
                        .small_button("Align from this face")
                        .on_hover_text("Then select a planar face of a sibling occurrence: this occurrence moves so the faces touch, facing each other")
                        .probe("Align from this face")
                        .clicked()
                    {
                        self.assembly_ui.align_from = Some((pick, o, inst.path_name.clone()));
                    }
                }
            }
        });
    }

    fn joints_ui(&mut self, ui: &mut egui::Ui, asm: &AssemblySummary, send: &mut Vec<Command>) {
        if asm.occurrences.len() >= 2 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Joints ({}) · total DOF {}",
                        asm.joints.len(),
                        asm.total_dof
                    ))
                    .strong(),
                );
                if ui.small_button("New joint…").probe("New joint").clicked() {
                    self.assembly_ui.joint_dialog = Some(crate::joint_ui::JointDialog::new(None));
                }
            });
        }
        let occ = |id: OccurrenceId| {
            asm.occurrences
                .iter()
                .find(|o| o.id == id)
                .map_or(id.to_string(), |o| o.name.clone())
        };
        for j in &asm.joints {
            let vals = std::iter::once(j.value)
                .chain(j.extra.iter().map(|d| d.value))
                .take(j.kind.dof() as usize)
                .collect::<Vec<_>>();
            let txt = vals
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let (lo, hi) = if i == 0 {
                        (j.min, j.max)
                    } else {
                        j.extra.get(i - 1).map_or((None, None), |d| (d.min, d.max))
                    };
                    format!(
                        "{}{} [{} … {}]",
                        fmt(*v),
                        j.kind.units()[i],
                        lo.map_or("−∞".into(), fmt),
                        hi.map_or("∞".into(), fmt)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} {:?}: {} -> {} {} · DOF {}",
                    j.name,
                    j.kind,
                    occ(j.parent),
                    occ(j.child),
                    txt,
                    j.kind.dof()
                ));
                let l = format!("Edit {}", j.name);
                if j.kind.dof() > 0 && ui.small_button("Edit…").named(&l).probe(&l).clicked() {
                    self.assembly_ui.joint_axis = [
                        fmt(j.origin[0]),
                        fmt(j.origin[1]),
                        fmt(j.origin[2]),
                        fmt(j.axis[0]),
                        fmt(j.axis[1]),
                        fmt(j.axis[2]),
                    ];
                    self.assembly_ui.joint_edit = Some((
                        j.id,
                        vals.iter().map(|v| fmt(*v)).collect(),
                        (0..vals.len())
                            .map(|i| {
                                let (lo, hi) = if i == 0 {
                                    (j.min, j.max)
                                } else {
                                    j.extra.get(i - 1).map_or((None, None), |d| (d.min, d.max))
                                };
                                (lo.map_or(String::new(), fmt), hi.map_or(String::new(), fmt))
                            })
                            .collect(),
                    ));
                }
                let l = format!("Delete {}", j.name);
                if ui.small_button("Delete").named(&l).probe(&l).clicked() {
                    send.push(Command::DeleteJoint { joint: j.id });
                }
            });
            if let Some((id, vals, lims)) = &mut self.assembly_ui.joint_edit
                && *id == j.id
            {
                // Axis (parent definition coordinates); the child keeps its pose.
                let ax = &mut self.assembly_ui.joint_axis;
                ui.horizontal(|ui| {
                    for (k, l) in ["Joint origin X", "Joint origin Y", "Joint origin Z"]
                        .iter()
                        .enumerate()
                    {
                        crate::panels::num_field(ui, l, &mut ax[k]);
                    }
                });
                ui.horizontal(|ui| {
                    for (k, l) in ["Joint axis X", "Joint axis Y", "Joint axis Z"]
                        .iter()
                        .enumerate()
                    {
                        crate::panels::num_field(ui, l, &mut ax[3 + k]);
                    }
                });
                {
                    let nums: Vec<Option<f64>> = ax.iter().map(|t| parse(t)).collect();
                    if let [Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)] = nums[..] {
                        if ui
                            .small_button("Set axis")
                            .on_hover_text("Redefine the joint axis; nothing moves now, later values move about the new axis")
                            .probe("Set axis")
                            .clicked()
                        {
                            send.push(Command::SetJointAxis {
                                joint: j.id,
                                origin: Some([a, b, c]),
                                axis: Some([d, e, f]),
                                axis_from: None,
                                dir2: j.dir2,
                            });
                        }
                    } else {
                        // Say why the button is unavailable.
                        let bad: Vec<&str> = [
                            "origin X", "origin Y", "origin Z", "axis X", "axis Y", "axis Z",
                        ]
                        .iter()
                        .zip(&nums)
                        .filter(|(_, n)| n.is_none())
                        .map(|(l, _)| *l)
                        .collect();
                        ui.horizontal(|ui| {
                            ui.add_enabled(false, egui::Button::new("Set axis").small());
                            ui.colored_label(
                                Color32::from_rgb(255, 190, 90),
                                format!("Not a number: {}", bad.join(", ")),
                            );
                        });
                    }
                }
                for i in 0..vals.len() {
                    let sfx = if i == 0 {
                        String::new()
                    } else {
                        format!(" {}", i + 1)
                    };
                    let name = j.kind.value_names().get(i).copied().unwrap_or("");
                    ui.horizontal(|ui| {
                        ui.small(format!("{name} ({})", j.kind.units()[i].trim()));
                        crate::panels::num_field(ui, &format!("Joint value{sfx}"), &mut vals[i]);
                        let l = format!("Set value{sfx}");
                        if let Some(v) = parse(&vals[i])
                            && ui.button(&l).probe(&l).clicked()
                        {
                            send.push(Command::SetJointValue {
                                joint: j.id,
                                value: v,
                                index: i,
                            });
                        }
                    });
                    // Drive within the stored limits (motion preview of the
                    // solved joint; one command when the drag ends).
                    let (smin, smax) = if i == 0 {
                        (j.min, j.max)
                    } else {
                        j.extra.get(i - 1).map_or((None, None), |d| (d.min, d.max))
                    };
                    let limited = matches!((smin, smax), (Some(a), Some(b)) if a < b);
                    if !limited {
                        // The controls stay visible with the
                        // reason they are unavailable.
                        let why = "Set both limits (min < max) to drive or sweep this value";
                        ui.horizontal(|ui| {
                            ui.add_enabled(false, egui::Button::new("Sweep check").small())
                                .on_disabled_hover_text(why);
                            ui.small(format!("Drive and sweep{sfx}: {}", why.to_lowercase()));
                        });
                    }
                    if let (Some(a), Some(b)) = (smin, smax)
                        && a < b
                    {
                        let l = format!("Sweep check{sfx} {}", j.name);
                        if ui
                            .small_button("Sweep check")
                            .on_hover_text("Static interference at 21 samples across the limits, for the parts that move with the joint (contacts between samples are not detected)")
                            .named(&l)
                            .probe(&l)
                            .clicked()
                        {
                            self.assembly_ui.sweep = None;
                            self.assembly_ui.sweep_joint = Some((j.id, i));
                            send.push(Command::JointSweep {
                                joint: j.id,
                                index: i,
                                from: a,
                                to: b,
                                steps: 21,
                                occurrences: None,
                            });
                        }
                        // The slider shows the solved (stored) value.
                        let stored = if i == 0 {
                            j.value
                        } else {
                            j.extra.get(i - 1).map_or(a, |d| d.value)
                        };
                        let mut x = stored.clamp(a, b);
                        let l = format!("Drive{sfx} {}", j.name);
                        let r = ui
                            .add(egui::Slider::new(&mut x, a..=b).text("drive"))
                            .on_hover_text("Drag to move the joint within its limits; the value is committed when you release")
                            .named(&l);
                        if r.changed() {
                            vals[i] = fmt(x);
                        }
                        // One commit per gesture. While the
                        // pointer is held on the slider the value is only
                        // remembered; it is committed once when the pointer is
                        // released. Keyboard steps commit at once.
                        let pending = &mut self.assembly_ui.drive_pending;
                        if r.is_pointer_button_down_on() {
                            if r.changed() {
                                *pending = Some((j.id, i, x));
                            }
                        } else {
                            let held = pending.filter(|p| (p.0, p.1) == (j.id, i)).map(|p| p.2);
                            if held.is_some() {
                                *pending = None;
                            }
                            if let Some(value) = r.changed().then_some(x).or(held) {
                                send.push(Command::SetJointValue {
                                    joint: j.id,
                                    value,
                                    index: i,
                                });
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        let (lo, hi) = &mut lims[i];
                        crate::panels::num_field(ui, &format!("Min{sfx}"), lo);
                        crate::panels::num_field(ui, &format!("Max{sfx}"), hi);
                        let (a, b) = (parse(lo), parse(hi));
                        let l = format!("Set limits{sfx}");
                        if (lo.trim().is_empty() || a.is_some())
                            && (hi.trim().is_empty() || b.is_some())
                            && ui.button(&l).probe(&l).clicked()
                        {
                            send.push(Command::SetJointLimits {
                                joint: j.id,
                                min: a,
                                max: b,
                                index: i,
                            });
                        }
                    });
                }
            }
        }
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        if let Some((d, r, jid, idx, steps)) = &self.assembly_ui.sweep
            && (*d, *r) == (doc, rev)
        {
            let hits: Vec<String> = steps
                .iter()
                .filter(|s| !s.interference.is_empty())
                .map(|s| fmt(s.value))
                .collect();
            // Name the swept value on multi-DOF joints.
            let name = asm
                .joints
                .iter()
                .find(|j| j.id == *jid)
                .map_or(jid.to_string(), |j| {
                    if j.kind.dof() > 1 {
                        let v = j.kind.value_names().get(*idx).copied().unwrap_or("value");
                        format!("{} {v} (value {})", j.name, idx + 1)
                    } else {
                        j.name.clone()
                    }
                });
            let t = if hits.is_empty() {
                format!("{name} sweep: no interference at {} samples", steps.len())
            } else {
                format!(
                    "{name} sweep: interference at {} of {} samples: {}",
                    hits.len(),
                    steps.len(),
                    hits.join(", ")
                )
            };
            ui.colored_label(
                if hits.is_empty() {
                    Color32::GRAY
                } else {
                    Color32::from_rgb(255, 150, 120)
                },
                t,
            );
        }
    }

    fn bom_interference_ui(
        &mut self,
        ui: &mut egui::Ui,
        asm: &AssemblySummary,
        send: &mut Vec<Command>,
    ) {
        if asm.components.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Bill of materials")
                .on_hover_text("Flattened: total quantity of each part across all nesting levels")
                .probe("Bill of materials")
                .clicked()
            {
                self.assembly_ui.structured = false;
                send.push(Command::Bom);
            }
            if ui
                .button("Structured BOM")
                .on_hover_text("Indented: quantity per parent assembly")
                .probe("Structured BOM")
                .clicked()
            {
                self.assembly_ui.structured = true;
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
            if ui
                .button("Assembly mass")
                .on_hover_text("Mass, volume and centre of mass of every occurrence instance and loose visible body (world frame)")
                .probe("Assembly mass")
                .clicked()
            {
                self.assembly_ui.mass_requested = true;
                send.push(Command::MassProperties {
                    bodies: None,
                    occurrences: None,
                });
            }
            let mut ex = asm.exploded;
            if ui
                .checkbox(&mut ex, "Exploded view")
                .on_hover_text("Display the per-occurrence exploded offsets (display only)")
                .probe("Exploded view")
                .changed()
            {
                send.push(Command::SetExploded { exploded: ex });
            }
        });
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        if self.assembly_ui.mass_requested
            && let Some((d, r, rep)) = &self.body_props.report
            && (*d, *r) == (doc, rev)
        {
            match &rep.total {
                Some(t) => {
                    ui.label(format!(
                        "Assembly: {} g, {} mm³, centre of mass ({}, {}, {}) mm{} · {} instance(s)",
                        fmt(t.mass),
                        fmt(t.volume),
                        fmt(t.center_of_mass[0]),
                        fmt(t.center_of_mass[1]),
                        fmt(t.center_of_mass[2]),
                        if t.exact { "" } else { " (estimate)" },
                        rep.occurrences.len()
                    ));
                }
                None => {
                    ui.colored_label(
                        Color32::from_rgb(255, 190, 90),
                        format!("No assembly total: {}", rep.total_missing.join("; ")),
                    );
                }
            }
        }
        ui.horizontal(|ui| {
            crate::panels::short_path_field(
                ui,
                "BOM CSV path",
                &mut self.assembly_ui.bom_path,
                crate::file_pick::PickMode::Save,
                crate::file_pick::CSV,
            );
        });
        {
            let p = self.assembly_ui.bom_path.trim().to_string();
            for (label, structured) in [
                ("Export BOM CSV", false),
                ("Export structured BOM CSV", true),
            ] {
                if ui
                    .add_enabled(!p.is_empty(), egui::Button::new(label))
                    .probe(label)
                    .clicked()
                {
                    send.push(Command::ExportBom {
                        path: p.clone().into(),
                        structured,
                    });
                }
            }
        }
        if let Some((d, r, flat, tree)) = &self.assembly_ui.bom
            && (*d, *r) == (doc, rev)
        {
            if self.assembly_ui.structured {
                ui.label(RichText::new("BOM (structured; quantity per parent)").strong());
                for r in tree {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0 * r.level as f32);
                        ui.small(format!(
                            "{} {} ×{}{}{}",
                            r.item,
                            r.name,
                            r.quantity,
                            if r.part_number.is_empty() {
                                String::new()
                            } else {
                                format!(" · P/N {}", r.part_number)
                            },
                            if r.assembly { " · sub-assembly" } else { "" }
                        ));
                    });
                }
            } else {
                ui.label(RichText::new("BOM (flattened; total quantity)").strong());
                for r in flat {
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
        }
        if let Some((d, r, list)) = self.assembly_ui.interference.clone()
            && (d, r) == (doc, rev)
        {
            if list.is_empty() {
                ui.small("No interference found between occurrences.");
            } else {
                ui.small("Click a result to highlight its occurrences.");
            }
            for (k, i) in list.iter().enumerate() {
                let (t, scope) = match &i.status {
                    InterferenceStatus::Overlap {
                        volume,
                        exact,
                        error_estimate,
                    } => (
                        format!(
                            "{} × {}: {} mm³{}",
                            i.a_name,
                            i.b_name,
                            fmt(*volume),
                            if *exact { "" } else { " (estimate)" }
                        ),
                        format!(
                            "Exact B-rep intersection solid (checked by point membership); volume {} ± {} mm³",
                            if *exact {
                                "exact"
                            } else {
                                "tessellation estimate"
                            },
                            fmt(*error_estimate)
                        ),
                    ),
                    InterferenceStatus::OverlapVolumeUnknown { reason } => (
                        format!("{} × {}: overlap, volume unknown", i.a_name, i.b_name),
                        format!("Sampled (10³ points in the common box): {reason}"),
                    ),
                    _ => continue,
                };
                let sel = self.assembly_ui.interference_pick == Some(k);
                if ui
                    .selectable_label(sel, RichText::new(&t).color(Color32::from_rgb(255, 150, 120)))
                    .on_hover_text(format!(
                        "{scope}. Touching contact (coincident faces) is not reported; no contact tolerance is applied."
                    ))
                    .named(&t)
                    .probe(&format!("Interference {}", k + 1))
                    .clicked()
                {
                    self.assembly_ui.interference_pick = (!sel).then_some(k);
                }
            }
        }
    }
}
