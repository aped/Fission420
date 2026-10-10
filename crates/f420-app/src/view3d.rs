//! The central 3D view: input routing (sketch editor, placement tools,
//! selection), face analysis for selection, overlays, and inline dimension
//! editing.
//!
//! Face selection maps a picked display-mesh face (a tessellation group, which
//! is never persisted) to semantic provenance:
//! * planar faces lying on an extrusion cap frame reported by the document →
//!   that `FaceRef` (the only faces that can host a sketch);
//! * cylindrical faces whose vertices all lie on a through-all cut's circle
//!   (axis and radius from the cut's sketch) → that cut;
//! * every other face → the base body feature (extrude or revolve). This is
//!   reliable under the current product limits (one base body; the only other
//!   solid features are circular through-all cuts), and is labelled as such.
//!
//! The analysis is display-side only; commands always carry semantic references
//! that the regenerator re-validates against the exact B-rep.

use eframe::egui::{self, Color32, Pos2, Stroke};
use f420_commands::{Command, FeatureSummary};
use f420_document::{FaceRef, FaceRole, FeatureId};
use f420_viewport::{Camera, GridPlane, Ray, RenderMesh, ViewPreset};

use crate::app::{DocDimEdit, F420App, SketchTarget};
use crate::place::{Phase, PlaneFrame};
use crate::sketch_edit::{Item, SketchTool, StateKind, View};
use crate::sketch_view::{self, PALETTE, Projector, SketchStyle};

/// Number-row view shortcuts (shown in Help).
pub const VIEW_KEYS: &[(egui::Key, ViewPreset)] = &[
    (egui::Key::Num7, ViewPreset::Top),
    (egui::Key::Num1, ViewPreset::Front),
    (egui::Key::Num3, ViewPreset::Right),
    (egui::Key::Num0, ViewPreset::Iso),
];

/// Display-side analysis of one tessellation face group.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    pub group: u32,
    pub planar: bool,
    /// Semantic reference when the face is a supported sketch host.
    pub reference: Option<FaceRef>,
    /// Feature this face reliably comes from (see module docs).
    pub owner: Option<FeatureId>,
    pub description: String,
}

impl FaceInfo {
    /// Why this face cannot host a sketch, or None if it can.
    pub fn sketch_blocker(&self) -> Option<&'static str> {
        if self.reference.is_some() {
            None
        } else if self.planar {
            Some(
                "Only extrusion end/start caps carry a semantic reference; side faces cannot host a sketch yet",
            )
        } else {
            Some("Curved faces cannot host a sketch")
        }
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Classify the faces of a display mesh against the document's semantic faces.
pub fn analyse_faces(mesh: &RenderMesh, features: &[FeatureSummary]) -> Vec<FaceInfo> {
    use std::collections::BTreeMap;
    // group -> (unit triangle normals, vertices)
    type Samples = (Vec<[f64; 3]>, Vec<[f64; 3]>);
    let mut groups: BTreeMap<u32, Samples> = BTreeMap::new();
    let pos = |i: u32| mesh.positions.get(i as usize).map(|p| p.map(f64::from));
    for (t, idx) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let Some(&g) = mesh.tri_group.get(t) else {
            continue;
        };
        let (Some(a), Some(b), Some(c)) = (pos(idx[0]), pos(idx[1]), pos(idx[2])) else {
            continue;
        };
        let n = cross(sub(b, a), sub(c, a));
        let l = norm(n);
        let e = groups.entry(g).or_default();
        if l > 0.0 {
            e.0.push([n[0] / l, n[1] / l, n[2] / l]);
        }
        e.1.extend([a, b, c]);
    }
    // Display tessellation is f32; planar faces are exact planes up to f32 error.
    let extent = mesh
        .bounds()
        .map_or(1.0, |b| f64::from(b.radius()).max(1.0));
    let plane_tol = 1e-3_f64.max(1e-5 * extent);
    // Cylinder walls are chordal approximations: vertices lie on the exact
    // surface, so only f32 error matters, but allow the display chord too.
    let cyl_tol = crate::engine::DISPLAY_CHORD + plane_tol;
    let base = features
        .iter()
        .find(|f| f.kind == "extrude" || f.kind == "revolve");
    let holes: Vec<(FeatureId, [f64; 3], [f64; 3], f64)> = features
        .iter()
        .filter(|f| f.kind == "cut_through_all")
        .filter_map(|c| {
            let s = features
                .iter()
                .find(|s| s.kind == "sketch" && c.depends_on.contains(&s.id))?;
            let frame = s.frame?;
            match s.sketch.as_ref()?.profile().ok()? {
                f420_sketch::Profile::Circle { center, radius } => {
                    Some((c.id, frame.to_world(center), frame.z, radius))
                }
                _ => None,
            }
        })
        .collect();
    groups
        .into_iter()
        .map(|(group, (normals, pts))| {
            let n0 = normals.first().copied().unwrap_or([0.0, 0.0, 1.0]);
            let p0 = pts.first().copied().unwrap_or([0.0; 3]);
            let planar = normals.iter().all(|n| dot(*n, n0) > 1.0 - 1e-4)
                && pts.iter().all(|p| dot(sub(*p, p0), n0).abs() <= plane_tol);
            let reference = planar
                .then(|| {
                    features.iter().flat_map(|f| f.faces.iter()).find(|fs| {
                        dot(n0, fs.frame.z) > 1.0 - 1e-4
                            && dot(sub(p0, fs.frame.origin), fs.frame.z).abs() <= plane_tol
                    })
                })
                .flatten()
                .map(|fs| fs.reference);
            let hole = (!planar)
                .then(|| {
                    holes.iter().find(|(_, c, axis, r)| {
                        pts.iter().all(|p| {
                            let v = sub(*p, *c);
                            (norm(cross(v, *axis)) - r).abs() <= cyl_tol
                        })
                    })
                })
                .flatten();
            let name = |id: FeatureId| {
                features
                    .iter()
                    .find(|f| f.id == id)
                    .map_or_else(|| id.to_string(), |f| f.name.clone())
            };
            let (owner, description) = if let Some(r) = reference {
                let role = match r.role {
                    FaceRole::ExtrudeEndCap => "end cap",
                    FaceRole::ExtrudeStartCap => "start cap",
                };
                (
                    Some(r.feature),
                    format!(
                        "{} {role} (semantic {} {:?})",
                        name(r.feature),
                        r.feature,
                        r.role
                    ),
                )
            } else if let Some((id, ..)) = hole {
                (Some(*id), format!("Hole wall of {}", name(*id)))
            } else if let Some(b) = base {
                (
                    Some(b.id),
                    format!(
                        "{} face of {}",
                        if planar { "Planar side" } else { "Curved" },
                        b.name
                    ),
                )
            } else {
                (None, "Body face".into())
            };
            FaceInfo {
                group,
                planar,
                reference,
                owner,
                description,
            }
        })
        .collect()
}

/// Orient `cam` to look straight at a sketch plane and frame its contents.
pub fn look_at_plane(
    cam: &mut Camera,
    frame: &PlaneFrame,
    bounds: Option<([f64; 2], [f64; 2])>,
    aspect: f32,
) {
    let f = |v: [f64; 3]| v.map(|c| c as f32);
    cam.look_along_normal(f(frame.z), f(frame.x));
    let (center, radius) = match bounds {
        Some((lo, hi)) => {
            let c = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
            let r = 0.5 * (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
            (c, (r * 1.25).max(20.0))
        }
        None => ([0.0, 0.0], 60.0),
    };
    cam.fit_sphere(f(frame.to_world(center)), radius as f32, aspect);
}

impl F420App {
    fn ray_at(&self, rect: egui::Rect, p: Pos2) -> Option<Ray> {
        let (cam, [w, h]) = self.view_camera();
        let l = p - rect.min;
        cam.ray(l.x, l.y, w, h)
    }

    /// Display-mesh face under a screen position.
    pub fn face_at(&self, p: Pos2) -> Option<u32> {
        let rect = self.view_rect?;
        let ray = self.ray_at(rect, p)?;
        self.display_mesh.pick(&ray).map(|(_, _, g)| g)
    }

    pub(crate) fn view_ui(&mut self, ui: &mut egui::Ui) {
        let sketching = self.sketch.is_some();
        let tool_active = self.placement.is_some() || sketching;
        let grid = self.sketch.as_ref().map(|s| {
            let f = |v: [f64; 3]| v.map(|c| c as f32);
            GridPlane {
                origin: f(s.frame.origin),
                x: f(s.frame.x),
                y: f(s.frame.y),
            }
        });
        let resp = match self.viewport.as_mut() {
            Some(vp) => {
                vp.set_primary_drag_navigates(!tool_active);
                vp.set_dimmed(sketching);
                if vp.grid_plane() != grid {
                    let _ = vp.set_grid_plane(grid);
                }
                let r = vp.ui(ui);
                let h = if sketching { None } else { vp.hovered_group() };
                vp.set_highlight(h);
                r
            }
            None => {
                let (rect, r) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                ui.painter().text(
                    rect.center() + egui::vec2(0.0, 80.0),
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "3D view unavailable (no wgpu renderer). Display mesh: {} triangles",
                        self.mesh_triangles
                    ),
                    egui::FontId::proportional(13.0),
                    ui.visuals().weak_text_color(),
                );
                r
            }
        };
        let rect = resp.rect;
        self.view_rect = Some(rect);
        let painter = ui.painter_at(rect);
        if sketching {
            self.sketch_input(ui, &resp);
            self.paint_sketch_session(&painter, rect);
            self.sketch_dim_input(ui);
        } else {
            self.model_input(ui, &resp);
            self.paint_document_sketches(&painter, rect);
            self.doc_dim_input(ui);
            self.paint_placement(&painter, rect);
            if (!self.summary.open || self.summary.features.is_empty())
                && self.placement.is_none()
                && self.dialog.is_none()
            {
                self.empty_state(ui, rect);
            }
        }
        self.paint_view_chrome(ui, rect);
    }

    // ----- normal mode -----

    fn model_input(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let prim = egui::PointerButton::Primary;
        if self.placement.is_some() {
            let (shift, origin, latest) = ui.input(|i| {
                (
                    i.modifiers.shift,
                    i.pointer.press_origin(),
                    i.pointer.latest_pos(),
                )
            });
            let mut events = vec![];
            if resp.drag_started_by(prim)
                && let Some(o) = origin
            {
                events.push((Phase::Press, o));
            }
            if (resp.dragged_by(prim) || resp.drag_stopped_by(prim))
                && let Some(p) = latest
            {
                events.push((Phase::Move, p));
            }
            if resp.drag_stopped_by(prim)
                && let Some(p) = latest
            {
                events.push((Phase::Release, p));
            }
            for (phase, pos) in events {
                let l = pos - resp.rect.min;
                self.place_pointer(phase, [l.x, l.y], !shift);
            }
            return;
        }
        if resp.clicked_by(prim)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.view_click(p);
        }
    }

    /// A primary click in the 3D view outside sketch mode: dimension label of
    /// the selected sketch → inline edit; otherwise select the face under the
    /// cursor (or clear the selection on empty space).
    pub fn view_click(&mut self, p: Pos2) {
        if let Some((i, _)) = self
            .doc_dim_targets
            .iter()
            .find(|(_, r)| r.contains(p))
            .copied()
            && let Some(f) = self.selected
            && let Some(fs) = self.summary.features.iter().find(|x| x.id == f)
            && let Some(d) = fs
                .sketch
                .as_ref()
                .and_then(|s| s.constraints.get(i))
                .and_then(|c| c.dimension())
        {
            self.doc_dim_edit = Some(DocDimEdit {
                feature: f,
                name: d.name.clone(),
                buffer: f420_sketch::fmt_mm(d.value),
                constraint: i,
                focus: true,
            });
            return;
        }
        let g = self.face_at(p);
        self.select_face(g);
    }

    /// Sketches drawn in the model view: unconsumed sketches thin; the selected
    /// sketch with its (editable) dimensions; a consumed selected sketch as a
    /// labelled preview of its profile.
    fn paint_document_sketches(&mut self, painter: &egui::Painter, rect: egui::Rect) {
        self.doc_dim_targets.clear();
        let (cam, _) = self.view_camera();
        let sel = self.selected;
        let feats = self.summary.features.clone();
        for f in feats.iter().filter(|f| f.kind == "sketch") {
            let (Some(sk), Some(fr)) = (&f.sketch, f.frame) else {
                continue;
            };
            let selected = sel == Some(f.id);
            if !selected && self.consumed(f.id) {
                continue;
            }
            let pr = Projector {
                cam: cam.clone(),
                rect,
                frame: PlaneFrame::from_kernel(&fr),
            };
            let state = match f.solve_status {
                Some(f420_sketch::SolveStatus::FullyConstrained) => StateKind::Full,
                _ => StateKind::Under,
            };
            let editing = self
                .doc_dim_edit
                .as_ref()
                .filter(|d| d.feature == f.id)
                .map(|d| d.constraint);
            let style = SketchStyle {
                selection: &[],
                hover: None,
                editing,
                state,
                conflicts: &[],
                glyphs: false,
                alpha: if selected { 1.0 } else { 0.55 },
            };
            let mut shown = sk.clone();
            if !selected {
                // Unselected sketches: curves only.
                shown.constraints.retain(|c| c.dimension().is_none());
            }
            let targets = sketch_view::paint_sketch(painter, &pr, &shown, &style);
            if selected {
                self.doc_dim_targets = targets
                    .into_iter()
                    .filter_map(|(it, r)| match it {
                        Item::Constraint(i) => Some((i, r)),
                        _ => None,
                    })
                    .collect();
                if let Some(c) = self.consumer_of(f.id)
                    && let Some((lo, _)) = sketch_view::sketch_bounds(sk)
                    && let Some(at) = pr.screen(lo)
                {
                    let text = format!("{} — sketch preview (profile of {})", f.name, c.name);
                    painter.text(
                        at + egui::vec2(0.0, 16.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::proportional(12.5),
                        PALETTE.dim,
                    );
                }
            }
        }
    }

    fn paint_placement(&self, painter: &egui::Painter, rect: egui::Rect) {
        let (cam, _) = self.view_camera();
        f420_viewport::paint_overlay(painter, &cam, rect, &self.placement_overlay());
        if let Some(hint) = self.placement_hint() {
            chip(
                painter,
                rect.min + egui::vec2(10.0, 10.0),
                hint,
                PALETTE.preview,
            );
        }
    }

    fn placement_hint(&self) -> Option<String> {
        let p = self.placement.as_ref()?;
        Some(match p.tool {
            crate::place::Tool::Rectangle => {
                "Place rectangle: drag corner to corner on world XY · Shift = no snap · Esc cancels"
                    .into()
            }
            crate::place::Tool::Hole { extrude } => format!(
                "Place hole on {}.EndCap: press at centre, drag to set size · Shift = no snap · Esc cancels",
                self.feature_name(extrude)
            ),
        })
    }

    fn placement_overlay(&self) -> f420_viewport::Overlay {
        use f420_viewport::{Overlay, OverlayLabel, OverlayPolyline};
        let mut ov = Overlay::default();
        let (p, live) = match (&self.placement, &self.held_preview) {
            (Some(p), _) => (p, true),
            (None, Some(p)) => (p, false),
            _ => return ov,
        };
        let Some(pv) = p.preview() else {
            return ov;
        };
        let color = if pv.plausible {
            PALETTE.preview
        } else {
            PALETTE.conflict
        };
        let f = |q: [f64; 3]| q.map(|v| v as f32);
        ov.polylines.push(OverlayPolyline {
            points: pv.outline.iter().map(|q| f(*q)).collect(),
            closed: true,
            color,
            width: 2.0,
            dashed: live,
        });
        for g in &pv.guides {
            ov.polylines.push(OverlayPolyline {
                points: vec![f(g[0]), f(g[1])],
                closed: false,
                color,
                width: 1.0,
                dashed: false,
            });
        }
        let mut text = pv.label;
        if !pv.plausible {
            text += "  (outside base: will be rejected)";
        }
        ov.labels.push(OverlayLabel {
            at: f(pv.label_at),
            text,
            color,
        });
        ov
    }

    /// Inline numeric editor for a dimension of the selected finished sketch.
    fn doc_dim_input(&mut self, ui: &egui::Ui) {
        let Some(ed) = self.doc_dim_edit.clone() else {
            return;
        };
        let anchor = self
            .doc_dim_targets
            .iter()
            .find(|(i, _)| *i == ed.constraint)
            .map(|(_, r)| r.left_bottom() + egui::vec2(0.0, 4.0))
            .or_else(|| {
                self.view_rect
                    .map(|r| r.center_top() + egui::vec2(-80.0, 40.0))
            });
        let Some(at) = anchor else { return };
        let mut buf = ed.buffer.clone();
        let (mut apply, mut cancel) = (false, false);
        egui::Area::new(egui::Id::new("doc_dim_edit"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let l = ui.label(format!("{} (mm)", ed.name));
                        let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(80.0));
                        let r = r.labelled_by(l.id);
                        let (enter, esc) = ui.input(|i| {
                            (
                                i.key_pressed(egui::Key::Enter),
                                i.key_pressed(egui::Key::Escape),
                            )
                        });
                        if !r.has_focus() && !enter && !esc {
                            r.request_focus();
                        }
                        apply |= enter && (r.lost_focus() || r.has_focus());
                        cancel |= esc;
                        if ui.button("Set dimension").clicked() {
                            apply = true;
                        }
                        if ui.button("Cancel dimension").clicked() {
                            cancel = true;
                        }
                    });
                    ui.weak("Enter applies · Esc cancels");
                });
            });
        if cancel {
            self.doc_dim_edit = None;
            return;
        }
        if apply {
            match buf.trim().parse::<f64>() {
                Ok(v) if v.is_finite() && v > 0.0 => {
                    self.doc_dim_edit = None;
                    self.request(Command::SetParameter {
                        feature: ed.feature,
                        name: ed.name.clone(),
                        value: v,
                    });
                }
                _ => {
                    self.diag(
                        true,
                        "Input",
                        format!(
                            "{}: '{}' is not a valid dimension; enter a positive number of millimetres",
                            ed.name,
                            buf.trim()
                        ),
                    );
                    if let Some(e) = &mut self.doc_dim_edit {
                        e.buffer = buf;
                        e.focus = true;
                    }
                }
            }
            return;
        }
        if let Some(e) = &mut self.doc_dim_edit {
            e.buffer = buf;
            e.focus = false;
        }
    }

    // ----- sketch mode -----

    fn sketch_input(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let (cam, _) = self.view_camera();
        let rect = resp.rect;
        let grid_mm = self.viewport.as_ref().map(|v| v.grid_spacing());
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        if s.committing {
            return;
        }
        let pr = Projector {
            cam: cam.clone(),
            rect,
            frame: s.frame,
        };
        let (shift, origin, latest) = ui.input(|i| {
            (
                i.modifiers.shift,
                i.pointer.press_origin(),
                i.pointer.latest_pos(),
            )
        });
        let grid = grid_mm.map(f64::from).or_else(|| {
            let ppm = f420_viewport::pixels_per_mm(&cam, rect.height());
            Some(f64::from(f420_viewport::adaptive_grid_spacing(
                ppm,
                f420_viewport::MIN_CELL_PX,
            )))
        });
        let view_at = |p: [f64; 2]| View {
            px_per_mm: pr.px_per_mm(p),
            grid,
            snapping: !shift,
        };
        let ed = &mut s.editor;
        if ed.dim_edit.is_some() {
            return;
        }
        let prim = egui::PointerButton::Primary;
        if let Some(h) = resp.hover_pos().or(latest.filter(|_| ed.dragging()))
            && let Some(p) = pr.unproject(h)
        {
            ed.pointer_move(p, Some(h), &view_at(p));
        } else if !ed.dragging() {
            ed.cursor = None;
            ed.hover = None;
        }
        if resp.drag_started_by(prim)
            && let Some(o) = origin
            && let Some(p) = pr.unproject(o)
        {
            if ed.tool == SketchTool::Select {
                ed.press(p, Some(o), &view_at(p));
            } else {
                ed.pointer_move(p, Some(o), &view_at(p));
                ed.click(p, Some(o), &view_at(p), shift);
            }
        }
        if resp.drag_stopped_by(prim) {
            if ed.tool == SketchTool::Select {
                ed.release();
            } else if let Some(l) = latest
                && let Some(p) = pr.unproject(l)
            {
                ed.click(p, Some(l), &view_at(p), shift);
            }
        }
        if resp.clicked_by(prim)
            && let Some(c) = resp.interact_pointer_pos()
            && let Some(p) = pr.unproject(c)
        {
            ed.click(p, Some(c), &view_at(p), shift);
        }
        if resp.double_clicked()
            && let Some(c) = resp.interact_pointer_pos()
            && let Some(p) = pr.unproject(c)
        {
            ed.double_click(p, Some(c), &view_at(p));
        }
        if resp.clicked_by(egui::PointerButton::Secondary) {
            ed.finish_chain();
        }
    }

    fn paint_sketch_session(&mut self, painter: &egui::Painter, rect: egui::Rect) {
        let (cam, _) = self.view_camera();
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let pr = Projector {
            cam,
            rect,
            frame: s.frame,
        };
        let ext = sketch_view::sketch_bounds(&s.editor.sketch)
            .map_or(50.0, |(lo, hi)| {
                lo.iter()
                    .chain(hi.iter())
                    .fold(0.0_f64, |m, v| m.max(v.abs()))
                    * 1.3
            })
            .max(30.0);
        sketch_view::paint_plane(painter, &pr, ext);
        let ed = &s.editor;
        let (state, _) = ed.status();
        let conflicts = if state == StateKind::Conflict {
            ed.report.conflicting.clone()
        } else {
            vec![]
        };
        let style = SketchStyle {
            selection: &ed.selection,
            hover: ed.hover,
            editing: ed.dim_edit.as_ref().map(|d| d.constraint),
            state,
            conflicts: &conflicts,
            glyphs: true,
            alpha: 1.0,
        };
        let targets = sketch_view::paint_sketch(painter, &pr, &ed.sketch, &style);
        sketch_view::paint_preview(painter, &pr, ed);
        s.editor.set_screen_targets(targets);
    }

    /// Inline numeric editor for a dimension inside the sketch editor.
    fn sketch_dim_input(&mut self, ui: &egui::Ui) {
        let rect = self.view_rect;
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(ed) = s.editor.dim_edit.clone() else {
            return;
        };
        let name = s
            .editor
            .sketch
            .constraints
            .get(ed.constraint)
            .and_then(|c| c.dimension())
            .map_or(String::new(), |d| d.name.clone());
        let at = s
            .editor
            .screen_target(Item::Constraint(ed.constraint))
            .map(|r| r.left_bottom() + egui::vec2(0.0, 4.0))
            .or_else(|| rect.map(|r| r.center_top() + egui::vec2(-80.0, 40.0)));
        let Some(at) = at else { return };
        let mut buf = ed.buffer.clone();
        let (mut apply, mut cancel) = (false, false);
        let id = egui::Id::new(("sketch_dim_edit", ed.constraint));
        egui::Area::new(egui::Id::new("sketch_dim_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let l = ui.label(format!("{name} (mm)"));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .id(id)
                                .desired_width(80.0),
                        );
                        let r = r.labelled_by(l.id);
                        let (enter, esc) = ui.input(|i| {
                            (
                                i.key_pressed(egui::Key::Enter),
                                i.key_pressed(egui::Key::Escape),
                            )
                        });
                        // The inline editor keeps focus until Enter/Escape/a button.
                        if !r.has_focus() && !enter && !esc {
                            r.request_focus();
                        }
                        apply |= enter && (r.lost_focus() || r.has_focus());
                        cancel |= esc;
                        if ui.button("Set dimension").clicked() {
                            apply = true;
                        }
                        if ui.button("Cancel dimension").clicked() {
                            cancel = true;
                        }
                    });
                    ui.weak("Enter applies · Esc cancels");
                });
            });
        if let Some(d) = &mut s.editor.dim_edit {
            d.buffer = buf;
        }
        if cancel {
            s.editor.cancel_dim_edit();
        } else if apply {
            s.editor.apply_dim_edit();
        }
    }

    // ----- chrome: banner, hints, pending indicator -----

    fn paint_view_chrome(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let painter = ui.painter_at(rect);
        let mut y = rect.min.y + 8.0;
        if let Some(s) = &self.sketch {
            let (kind, text) = s.editor.status();
            let col = match kind {
                StateKind::Full => Color32::from_rgb(120, 220, 120),
                StateKind::Under => Color32::from_rgb(120, 190, 255),
                StateKind::Conflict => PALETTE.conflict,
            };
            let r = chip(
                &painter,
                egui::pos2(rect.min.x + 10.0, y),
                format!("{} — {}", s.title, text),
                col,
            );
            y = r.max.y + 6.0;
            let hint = if s.committing {
                "Committing sketch…".to_string()
            } else {
                format!(
                    "{} ({}): {}",
                    s.editor.tool.label(),
                    s.editor.tool.shortcut(),
                    s.editor.tool.hint()
                )
            };
            let r = chip(
                &painter,
                egui::pos2(rect.min.x + 10.0, y),
                hint,
                Color32::from_gray(220),
            );
            y = r.max.y + 6.0;
            if let Some(fb) = &s.editor.feedback {
                let col = if fb.error {
                    PALETTE.conflict
                } else {
                    Color32::from_rgb(200, 230, 200)
                };
                chip(
                    &painter,
                    egui::pos2(rect.min.x + 10.0, y),
                    fb.text.clone(),
                    col,
                );
            }
        } else if let Some(b) = self.banner.clone() {
            let text = format!("✖ [{}] {}", b.kind, b.text);
            let galley = painter.layout(
                text,
                egui::FontId::proportional(14.0),
                PALETTE.conflict,
                (rect.width() - 140.0).max(200.0),
            );
            let r = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 10.0, y),
                galley.size() + egui::vec2(12.0, 10.0),
            );
            painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(40, 10, 10, 235));
            painter.rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, PALETTE.conflict),
                egui::StrokeKind::Inside,
            );
            painter.galley(r.min + egui::vec2(6.0, 5.0), galley, PALETTE.conflict);
            let btn = egui::Rect::from_min_size(
                egui::pos2(r.max.x + 6.0, r.min.y),
                egui::vec2(70.0, 22.0),
            );
            if ui.put(btn, egui::Button::new("Dismiss")).clicked() {
                self.banner = None;
            }
        }
        if self.engine.busy() {
            chip(
                &painter,
                egui::pos2(rect.max.x - 330.0, rect.min.y + 8.0),
                "⟳ Regenerating… showing the last valid model".into(),
                Color32::from_rgb(255, 210, 120),
            );
        }
        if self.sketch.is_none()
            && let Some(info) = self.selected_face()
        {
            let text = match info.sketch_blocker() {
                None => format!("Face: {} — Create Sketch is available", info.description),
                Some(why) => format!("Face: {} — {why}", info.description),
            };
            chip(
                &painter,
                egui::pos2(rect.min.x + 10.0, rect.max.y - 34.0),
                text,
                Color32::from_gray(225),
            );
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let size = egui::vec2(360.0, 150.0);
        let card = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 40.0), size);
        ui.painter()
            .rect_filled(card, 8.0, Color32::from_black_alpha(190));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card.shrink(16.0)));
        child.vertical_centered(|ui| {
            ui.heading(if self.summary.open {
                "Empty document"
            } else {
                "No document open"
            });
            ui.label("Start by sketching a profile on the XY plane, then extrude it.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button("New Sketch on XY")
                    .on_hover_text("Create a sketch on the world XY plane")
                    .clicked()
                {
                    if self.summary.open {
                        self.begin_sketch(SketchTarget::New {
                            plane: f420_document::SketchPlane::WorldXY,
                        });
                    } else {
                        self.guarded(crate::app::Guarded::NewAndSketch);
                    }
                }
                if ui.button("Open Document…").clicked() {
                    self.dialog = Some(crate::app::Dialog::Open {
                        path: self.default_path("f420.json"),
                    });
                }
            });
            ui.weak("Help: F1 · Navigate: MMB orbit, RMB pan, wheel zoom");
        });
    }
}

/// A rounded label with a dark background at `at` (top-left); returns its rect.
pub fn chip(painter: &egui::Painter, at: Pos2, text: String, color: Color32) -> egui::Rect {
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.5), color);
    let r = egui::Rect::from_min_size(at, galley.size() + egui::vec2(10.0, 6.0));
    painter.rect_filled(r, 4.0, Color32::from_black_alpha(185));
    painter.galley(r.min + egui::vec2(5.0, 3.0), galley, color);
    r
}
