//! Run 7 sheet-metal and surface inspector UI (docs/SHEET_METAL.md,
//! docs/SURFACES.md). Everything goes through the typed commands of
//! `rcad_commands::sheet_surface`; refusals are shown by the usual command
//! error path.
//!
//! * Sketch inspector: "Sheet base face" (closed profile → plate + rule).
//! * Sheet base / flange / flat features: rule editor, Folded / Flat state
//!   toggle, "Add edge flange" (edge picked from the base profile's lines),
//!   flat-pattern DXF export.
//! * Surface features (import / stitch / fill / thicken): body status
//!   (open shell or solid), analysis, stitch / fill with a displayed healing
//!   tolerance, thicken.
use crate::app::RcadApp;
use crate::panels::num_field;
use crate::probe::Probe;
use eframe::egui::{self, Color32, RichText};
use rcad_commands::sheet_surface::{
    AnalyzeSurfaceArgs, SheetBaseArgs, SheetFlangeArgs, SheetFlatArgs, SheetRuleArgs,
    SurfaceFromSketchArgs, SurfaceRepairArgs, ThickenArgs,
};
use rcad_commands::{Command, DocSummary, FeatureSummary, Response};
use rcad_document::sheet::ReliefShape;
use rcad_document::{BodyId, FeatureId};

/// Form state (one per app).
#[derive(Debug, Clone)]
pub struct SheetSurfaceForm {
    /// Base: thickness, radius, K, relief width, relief depth.
    pub rule: [String; 5],
    pub relief: ReliefShape,
    /// Rule editor: the base it was loaded from (reload on change).
    /// (base, document revision) the editor was loaded at: reloaded after
    /// any change (undo, parameter edit) so Apply never restores stale values.
    pub rule_for: Option<(FeatureId, u64)>,
    /// Rule editor: values as loaded (only changed fields are sent) and the
    /// relief shape of the base (separate from the new-base form).
    pub loaded: [Option<f64>; 5],
    pub edit_relief: ReliefShape,
    pub loaded_relief: ReliefShape,
    /// Flange: edge entity, angle (deg), length, start offset, end offset.
    pub edge: Option<u32>,
    pub flange: [String; 4],
    pub down: bool,
    /// Surfaces: gap search distance, healing tolerance, thickness.
    pub search: String,
    pub tolerance: String,
    pub thickness: String,
    pub reverse: bool,
    /// Surface extrusion distance and direction (sketch inspector).
    pub surface_distance: String,
    pub surface_reverse: bool,
    /// (doc_id, revision, body, report) of the last analysis reply.
    pub analysis: Option<(u64, u64, BodyId, rcad_kernel::ShellAnalysis)>,
}

impl Default for SheetSurfaceForm {
    fn default() -> Self {
        SheetSurfaceForm {
            rule: ["1", "2", "0.44", "1", "1"].map(String::from),
            relief: ReliefShape::Rectangular,
            rule_for: None,
            loaded: [None; 5],
            edit_relief: ReliefShape::Rectangular,
            loaded_relief: ReliefShape::Rectangular,
            edge: None,
            flange: ["90", "20", "0", "0"].map(String::from),
            down: false,
            search: "0.1".into(),
            tolerance: "0.001".into(),
            thickness: "2".into(),
            reverse: false,
            surface_distance: "10".into(),
            surface_reverse: false,
            analysis: None,
        }
    }
}

impl SheetSurfaceForm {
    /// Engine hook in `handle_reply`: keep the analysis report.
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary, cmd: &Command) {
        if let (Some(a), Command::AnalyzeSurface(args)) = (&response.surface_analysis, cmd) {
            self.analysis = Some((summary.doc_id, summary.revision, args.body, a.clone()));
        }
    }
}

/// One decimal, without "-0.0".
fn f1(x: f64) -> String {
    let s = format!("{x:.1}");
    if s == "-0.0" { "0.0".into() } else { s }
}

fn parse(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// STEP import dialog option (exchange.rs hook).
pub(crate) fn import_surfaces_toggle(ui: &mut egui::Ui, on: &mut bool) {
    ui.checkbox(on, "Import surface models (open shells) as a surface body")
        .probe("Import surfaces");
    if *on {
        ui.small("Every SHELL_BASED_SURFACE_MODEL shell becomes part of one surface body: open shells stay open (no volume) until stitched / filled / thickened.");
    }
}

fn relief_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    r: &mut ReliefShape,
) {
    egui::ComboBox::new(id, "Relief shape")
        .selected_text(match r {
            ReliefShape::Rectangular => "Rectangular relief",
            ReliefShape::Obround => "Obround relief",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(r, ReliefShape::Rectangular, "Rectangular relief");
            ui.selectable_value(r, ReliefShape::Obround, "Obround relief");
        });
}

impl RcadApp {
    /// Sketch inspector: sheet base face from this closed profile.
    pub(crate) fn sheet_base_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        if f.profile_error.is_some() {
            return;
        }
        let mut send = None;
        let header = egui::CollapsingHeader::new("Sheet metal base face")
            .id_salt(("sheet_base_ui", f.id.0))
            .show(ui, |ui| {
                let form = &mut self.sheet_surface;
                for (i, l) in [
                    "Thickness (mm)",
                    "Inside bend radius (mm)",
                    "K-factor",
                    "Relief width (mm)",
                    "Relief depth (mm)",
                ]
                .iter()
                .enumerate()
                {
                    num_field(ui, l, &mut form.rule[i]);
                }
                relief_combo(ui, ("relief_new", f.id.0), &mut form.relief);
                let v: Vec<Option<f64>> = form.rule.iter().map(|t| parse(t)).collect();
                if ui
                    .add_enabled(
                        v.iter().all(Option::is_some),
                        egui::Button::new("Create sheet base"),
                    )
                    .probe("Create sheet base")
                    .clicked()
                {
                    send = Some(Command::SheetBase(SheetBaseArgs {
                        name: None,
                        sketch: f.id,
                        thickness: v[0].unwrap_or(1.0),
                        radius: v[1].unwrap_or(1.0),
                        k_factor: v[2].unwrap_or(0.44),
                        relief: form.relief,
                        relief_width: v[3],
                        relief_depth: v[4],
                        flip: false,
                        body_name: None,
                    }));
                }
                ui.small("Closed profile = a flat plate; the rule (thickness, inside radius, K = neutral axis at K·t, reliefs) is shared by every flange of the body.");
            });
        header.header_response.probe("Sheet metal base face");
        if let Some(c) = send {
            self.request(c);
        }
    }

    /// Sketch inspector: surface bodies from this sketch (planar patch of a
    /// closed profile; ruled extrusion of a profile or open line chain).
    pub(crate) fn surface_sketch_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        let mut send = None;
        let closed = f.profile_error.is_none();
        let header = egui::CollapsingHeader::new("Surface from sketch")
            .id_salt(("surface_sketch_ui", f.id.0))
            .show(ui, |ui| {
                let form = &mut self.sheet_surface;
                if ui
                    .add_enabled(closed, egui::Button::new("Planar patch"))
                    .probe("Planar patch")
                    .on_hover_text("The closed profile (holes allowed) as a one-face surface body")
                    .on_disabled_hover_text("Needs a closed profile")
                    .clicked()
                {
                    send = Some(Command::SurfacePatch(SurfaceFromSketchArgs {
                        name: None,
                        sketch: f.id,
                        distance: 0.0,
                        reverse: false,
                        body_name: None,
                    }));
                }
                ui.horizontal(|ui| {
                    num_field(ui, "Surface distance (mm)", &mut form.surface_distance);
                    let d = parse(&form.surface_distance);
                    if ui
                        .add_enabled(d.is_some(), egui::Button::new("Extrude surface"))
                        .probe("Extrude surface")
                        .on_hover_text("Ruled surface: the profile (or open chain of lines) swept along the sketch normal, open (no caps)")
                        .clicked()
                    {
                        send = Some(Command::SurfaceExtrude(SurfaceFromSketchArgs {
                            name: None,
                            sketch: f.id,
                            distance: d.unwrap_or(10.0),
                            reverse: form.surface_reverse,
                            body_name: None,
                        }));
                    }
                });
                // Review run 7 #44: the side of the sketch plane (the
                // normal does not change; docs/SURFACES.md).
                ui.checkbox(&mut form.surface_reverse, "Reverse (below the sketch plane)")
                    .probe("Surface reverse")
                    .on_hover_text("Sweep against the sketch normal; the surface's facing side stays the same");
            });
        header.header_response.probe("Surface from sketch");
        if let Some(c) = send {
            self.request(c);
        }
    }

    /// Inspector of a sheet base / flange / flat feature.
    pub(crate) fn sheet_feature_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        let Some(base) = self.sheet_base_of(f) else {
            return;
        };
        let Some(bf) = self.summary.features.iter().find(|x| x.id == base).cloned() else {
            return;
        };
        let mut send: Vec<Command> = Vec::new();
        // Folded / flat state.
        let folded = bf.creates.first().copied();
        let flat = self
            .summary
            .features
            .iter()
            .find(|x| x.kind == "sheet_flat" && x.depends_on.contains(&base))
            .and_then(|x| x.creates.first().copied());
        let flat_shown = flat
            .and_then(|b| self.summary.bodies.iter().find(|x| x.id == b))
            .is_some_and(|b| b.visible);
        ui.horizontal(|ui| {
            ui.label("Display:");
            if ui
                .selectable_label(!flat_shown, "Folded")
                .probe("Sheet folded")
                .clicked()
                && flat_shown
            {
                send.push(Command::SheetFlatState(SheetFlatArgs { base, flat: false }));
            }
            if ui
                .selectable_label(flat_shown, "Flat pattern")
                .probe("Sheet flat")
                .on_hover_text(
                    "The exact flat solid (bend allowance θ·(R + K·t)); the folded body is hidden",
                )
                .clicked()
                && !flat_shown
            {
                send.push(Command::SheetFlatState(SheetFlatArgs { base, flat: true }));
            }
        });
        if folded.is_none() {
            ui.colored_label(Color32::from_rgb(255, 160, 120), "Sheet body not created");
        }
        // Rule editor (loaded from the base parameters).
        let rev = self.summary.revision;
        let form = &mut self.sheet_surface;
        if form.rule_for != Some((base, rev)) {
            form.rule_for = Some((base, rev));
            // Review S8 / run 7 #49: the relief shape of this base, from
            // its rule in the summary (not from note text).
            form.loaded_relief = bf.sheet_relief.unwrap_or_default();
            form.edit_relief = form.loaded_relief;
            for (i, n) in [
                "thickness",
                "radius",
                "k_factor",
                "relief_width",
                "relief_depth",
            ]
            .iter()
            .enumerate()
            {
                let v = bf.parameters.iter().find(|(k, _)| k == n).map(|p| p.1);
                form.loaded[i] = v;
                if let Some(v) = v {
                    form.rule[i] = crate::params_ui::fmt_value(v);
                }
            }
        }
        egui::CollapsingHeader::new("Sheet rule")
            .id_salt(("sheet_rule", base.0))
            .default_open(f.id == base)
            .show(ui, |ui| {
                for (i, l) in [
                    "Thickness (mm)",
                    "Inside bend radius (mm)",
                    "K-factor",
                    "Relief width (mm)",
                    "Relief depth (mm)",
                ]
                .iter()
                .enumerate()
                {
                    num_field(ui, l, &mut form.rule[i]);
                }
                relief_combo(ui, ("relief_edit", base.0), &mut form.edit_relief);
                let v: Vec<Option<f64>> = form.rule.iter().map(|t| parse(t)).collect();
                if ui
                    .add_enabled(
                        v.iter().all(Option::is_some),
                        egui::Button::new("Apply rule"),
                    )
                    .probe("Apply sheet rule")
                    .on_hover_text("One undo step; every flange and the flat pattern regenerate")
                    .clicked()
                {
                    // Only what the user changed (review S8).
                    let ch = |i: usize| {
                        v[i].filter(|x| {
                            form.loaded[i].is_none_or(|l| {
                                crate::params_ui::fmt_value(l) != crate::params_ui::fmt_value(*x)
                            })
                        })
                    };
                    send.push(Command::EditSheetRule(SheetRuleArgs {
                        base,
                        thickness: ch(0),
                        radius: ch(1),
                        k_factor: ch(2),
                        relief: (form.edit_relief != form.loaded_relief)
                            .then_some(form.edit_relief),
                        relief_width: ch(3),
                        relief_depth: ch(4),
                    }));
                }
            });
        // Add an edge flange: pick a line of the base profile.
        let base_sketch = self
            .summary
            .features
            .iter()
            .find(|x| bf.depends_on.contains(&x.id) && x.kind == "sketch")
            .and_then(|x| x.sketch.clone());
        egui::CollapsingHeader::new("Add edge flange")
            .id_salt(("sheet_flange_add", base.0))
            .show(ui, |ui| {
                let form = &mut self.sheet_surface;
                let Some(sk) = &base_sketch else {
                    ui.weak("Base sketch not evaluated");
                    return;
                };
                let pt = |p| {
                    sk.points
                        .iter()
                        .find(|q| q.id == p)
                        .map(|q| (q.x, q.y))
                        .unwrap_or_default()
                };
                let lines: Vec<(u32, String)> = sk
                    .entities
                    .iter()
                    .filter(|e| !sk.construction.contains(&e.id()))
                    .filter_map(|e| match e {
                        rcad_sketch::Entity::Line { id, a, b } => {
                            let (a, b) = (pt(*a), pt(*b));
                            Some((
                                id.0,
                                format!(
                                    "Line {} ({}, {}) → ({}, {})",
                                    id.0,
                                    f1(a.0),
                                    f1(a.1),
                                    f1(b.0),
                                    f1(b.1)
                                ),
                            ))
                        }
                        _ => None,
                    })
                    .collect();
                let cur = form
                    .edge
                    .and_then(|e| lines.iter().find(|l| l.0 == e))
                    .map_or("Pick an edge".to_string(), |l| l.1.clone());
                egui::ComboBox::from_label("Flange edge")
                    .selected_text(cur)
                    .show_ui(ui, |ui| {
                        for (id, label) in &lines {
                            ui.selectable_value(&mut form.edge, Some(*id), label);
                        }
                    });
                for (i, l) in [
                    "Bend angle (°)",
                    "Length beyond bend (mm)",
                    "Start offset (mm)",
                    "End offset (mm)",
                ]
                .iter()
                .enumerate()
                {
                    num_field(ui, l, &mut form.flange[i]);
                }
                ui.horizontal(|ui| {
                    ui.radio_value(&mut form.down, false, "Bend up (+normal)");
                    ui.radio_value(&mut form.down, true, "Bend down");
                });
                let v: Vec<Option<f64>> = form.flange.iter().map(|t| parse(t)).collect();
                if ui
                    .add_enabled(
                        form.edge.is_some() && v.iter().all(Option::is_some),
                        egui::Button::new("Add flange"),
                    )
                    .probe("Add flange")
                    .clicked()
                    && let Some(edge) = form.edge
                {
                    send.push(Command::SheetFlange(SheetFlangeArgs {
                        name: None,
                        base,
                        edge,
                        angle_deg: v[0].unwrap_or(90.0),
                        length: v[1].unwrap_or(10.0),
                        down: form.down,
                        start_offset: v[2].unwrap_or(0.0),
                        end_offset: v[3].unwrap_or(0.0),
                    }));
                }
                ui.small("Length = straight leg from the bend's tangent line to the flange end. Bend outside the base outline; offsets > 0 get the rule's relief.");
            });
        for c in send {
            self.request(c);
        }
        self.flat_pattern_ui(ui, f);
    }

    fn sheet_base_of(&self, f: &FeatureSummary) -> Option<FeatureId> {
        match f.kind.as_str() {
            "sheet_base" => Some(f.id),
            "sheet_flange" | "sheet_flat" => f.depends_on.first().copied(),
            _ => None,
        }
    }

    /// Inspector of a surface feature: the body it made, analysis and repair.
    pub(crate) fn surface_feature_ui(&mut self, ui: &mut egui::Ui, f: &FeatureSummary) {
        let Some(body) = f.creates.first().copied() else {
            return;
        };
        let Some(b) = self.summary.bodies.iter().find(|x| x.id == body).cloned() else {
            ui.weak("Body not evaluated");
            return;
        };
        let closed = b.measurements.as_ref().map(|m| m.all_shells_closed);
        match closed {
            Some(true) => {
                ui.label(RichText::new("Closed solid").color(Color32::from_rgb(120, 200, 120)));
            }
            Some(false) => {
                ui.label(
                    RichText::new("Open shell (surface body): no volume")
                        .color(Color32::from_rgb(255, 200, 120)),
                );
            }
            None => {
                ui.weak("Not measured yet");
            }
        }
        for n in &f.notes {
            ui.small(n);
        }
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        let mut send: Vec<Command> = Vec::new();
        let form = &mut self.sheet_surface;
        ui.horizontal(|ui| {
            num_field(ui, "Gap search (mm)", &mut form.search);
            let search = parse(&form.search).filter(|v| (0.0..=10.0).contains(v));
            if ui
                .add_enabled(search.is_some(), egui::Button::new("Analyse"))
                .probe("Analyse surface")
                .on_disabled_hover_text("Gap search must be a number in [0, 10] mm")
                .on_hover_text("Free edges, gaps between them, boundary loops and orientation conflicts, before any repair")
                .clicked()
                && let Some(search) = search
            {
                send.push(Command::AnalyzeSurface(AnalyzeSurfaceArgs { body, search }));
            }
        });
        if parse(&form.search).is_none_or(|v| !(0.0..=10.0).contains(&v)) {
            ui.colored_label(
                Color32::from_rgb(255, 160, 120),
                "Gap search must be a number in [0, 10] mm",
            );
        }
        if let Some((d, r, ab, a)) = &form.analysis
            && *d == doc
            && *r == rev
            && *ab == body
        {
            ui.small(format!(
                "{} shell(s), {} face(s); {} free edge(s); {} gap(s) ≤ {} mm{}; {} boundary loop(s){}; {} orientation conflict(s); {} non-manifold edge(s)",
                a.shells,
                a.faces,
                a.free_edges.len(),
                a.gaps_total,
                a.search,
                a.gaps
                    .last()
                    .map(|g| format!(" (largest {:.6} mm)", g.size))
                    .unwrap_or_default(),
                a.boundary_loops.len(),
                a.boundary_loops
                    .iter()
                    .map(|l| format!(" planar to {:.2e} mm", l.planarity))
                    .collect::<Vec<_>>()
                    .join(","),
                a.orientation_conflicts.len(),
                a.non_manifold_edges
            ));
        }
        if closed != Some(true) {
            ui.horizontal(|ui| {
                num_field(ui, "Healing tolerance (mm)", &mut form.tolerance);
            });
            let tol = parse(&form.tolerance);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(tol.is_some(), egui::Button::new("Stitch"))
                    .probe("Stitch surface")
                    .on_hover_text("Merge vertices/edges within the healing tolerance into a new body (the input is kept, hidden)")
                    .clicked()
                {
                    send.push(Command::StitchSurface(SurfaceRepairArgs {
                        body,
                        tolerance: tol.unwrap_or(1e-6),
                        body_name: None,
                        hide_source: true,
                    }));
                }
                if ui
                    .add_enabled(tol.is_some(), egui::Button::new("Fill planar holes"))
                    .probe("Fill surface")
                    .on_hover_text("Close every planar boundary loop with an exact planar face (new body)")
                    .clicked()
                {
                    send.push(Command::FillSurface(SurfaceRepairArgs {
                        body,
                        tolerance: tol.unwrap_or(1e-6),
                        body_name: None,
                        hide_source: true,
                    }));
                }
            });
            ui.horizontal(|ui| {
                num_field(ui, "Thickness (mm)", &mut form.thickness);
                ui.checkbox(&mut form.reverse, "Against normal")
                    .probe("Against normal");
                if ui
                    .add_enabled(
                        parse(&form.thickness).is_some(),
                        egui::Button::new("Thicken"),
                    )
                    .probe("Thicken surface")
                    .on_hover_text("One planar or cylindrical face → exact solid (new body)")
                    .clicked()
                {
                    send.push(Command::ThickenSurface(ThickenArgs {
                        body,
                        distance: parse(&form.thickness).unwrap_or(1.0),
                        reverse: form.reverse,
                        body_name: None,
                        hide_source: true,
                    }));
                }
            });
        }
        for c in send {
            self.request(c);
        }
    }
}
