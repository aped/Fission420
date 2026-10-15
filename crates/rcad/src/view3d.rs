//! The central 3D view: input routing (sketch editor, placement tools,
//! selection), face analysis for selection, overlays, and inline dimension
//! editing.
//!
//! Face selection maps a picked display-mesh face (a tessellation group, which
//! is never persisted) to its body (group ranges of the [`DisplayScene`]) and to
//! semantic provenance:
//! * planar faces matching a referenceable face reported by the document (same
//!   body, coplanar, same outward normal: extrusion caps and line side faces,
//!   cut floors, imported planar faces) → that `FaceRef` (the faces that can
//!   host a sketch). The document only reports faces that resolve uniquely on
//!   the final geometry, so a planar face without a match (split or merged by a
//!   later operation) gets a stated reason instead of a guessed reference;
//! * cylindrical faces whose vertices all lie on a circular cut's circle (axis
//!   and radius from the cut's sketch) → that cut;
//! * every other face → the feature that created its body, labelled as such.
//!
//! The analysis is display-side only; commands always carry semantic references
//! that the regenerator re-validates against the exact B-rep.

use eframe::egui::{self, Color32, Pos2, Response, Stroke};
use rcad_commands::{BodySummary, Command, FeatureSummary};
use rcad_document::{BodyId, FaceRef, FeatureId};
use rcad_viewport::{Camera, GridPlane, Ray, ViewPreset};

use crate::engine::DisplayScene;

use crate::app::{DocDimEdit, RcadApp, SketchTarget};
use crate::place::{Phase, PlaneFrame};
use crate::probe::Probe as _;
use crate::sketch_edit::{Item, SketchTool, StateKind, View};
use crate::sketch_view::{self, PALETTE, Projector, SketchStyle};

/// Press-to-release movement (points) below which a press is a click.
const CLICK_SLOP_PX: f32 = 6.0;
/// Maximum interval (s) between two clicks of a double click.
const DOUBLE_CLICK_S: f64 = 0.3;

/// Number-row view shortcuts (shown in Help).
pub const VIEW_KEYS: &[(egui::Key, ViewPreset)] = &[
    (egui::Key::Num7, ViewPreset::Top),
    (egui::Key::Num1, ViewPreset::Front),
    (egui::Key::Num3, ViewPreset::Right),
    (egui::Key::Num4, ViewPreset::Left),
    (egui::Key::Num6, ViewPreset::Back),
    (egui::Key::Num0, ViewPreset::Iso),
];

/// Display-side analysis of one tessellation face group.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    pub group: u32,
    pub planar: bool,
    /// Body the face belongs to.
    pub body: Option<BodyId>,
    /// The body is a triangle mesh (STL import): no exact faces.
    pub mesh_body: bool,
    /// Semantic reference when the face is a supported sketch host.
    pub reference: Option<FaceRef>,
    /// Feature this face reliably comes from (see module docs).
    pub owner: Option<FeatureId>,
    pub description: String,
    /// Planar B-rep faces: the exact plane (from the document summary, f64),
    /// used for measurements; None for curved/mesh faces.
    pub plane: Option<rcad_kernel::FacePlane>,
}

impl FaceInfo {
    /// Why this face cannot host a sketch, or None if it can.
    pub fn sketch_blocker(&self) -> Option<&'static str> {
        if self.reference.is_some() {
            None
        } else if self.mesh_body {
            Some("Mesh (STL) bodies have no exact faces; sketch on a B-rep face or a world plane")
        } else if self.planar {
            Some(
                "This planar face has no unique semantic reference (it was split, merged or is not tracked); sketch on another face or a world plane",
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

/// Whether `p` lies on one of the triangles `tris` (consecutive vertex
/// triples), within `tol` of the triangle's plane and edges.
fn on_triangles(p: [f64; 3], tris: &[[f64; 3]], tol: f64) -> bool {
    tris.as_chunks::<3>().0.iter().any(|t| {
        let (a, b, c) = (t[0], t[1], t[2]);
        let n = cross(sub(b, a), sub(c, a));
        let l = norm(n);
        if l <= 0.0 {
            return false;
        }
        let n = [n[0] / l, n[1] / l, n[2] / l];
        if dot(sub(p, a), n).abs() > tol {
            return false;
        }
        // Signed distances to the three edges (inside: all >= -tol).
        [(a, b), (b, c), (c, a)].iter().all(|&(u, v)| {
            let e = sub(v, u);
            let el = norm(e);
            el > 0.0 && dot(cross(e, sub(p, u)), n) / el >= -tol
        })
    })
}

/// Classify the faces of the display scene against the document's semantic faces.
pub fn analyse_faces(
    scene: &DisplayScene,
    features: &[FeatureSummary],
    bodies: &[BodySummary],
) -> Vec<FaceInfo> {
    use std::collections::BTreeMap;
    let mesh = &scene.mesh;
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
    let is_cut = |f: &FeatureSummary| {
        f.kind == "cut_through_all"
            || f.kind == "pocket"
            || matches!(f.operation, Some(rcad_document::Operation::Cut { .. }))
    };
    let holes: Vec<(FeatureId, [f64; 3], [f64; 3], f64)> = features
        .iter()
        .filter(|f| is_cut(f))
        .filter_map(|c| {
            let s = features
                .iter()
                .find(|s| s.kind == "sketch" && c.depends_on.contains(&s.id))?;
            let frame = s.frame?;
            match s.sketch.as_ref()?.profile().ok()? {
                rcad_sketch::Profile::Circle { center, radius } => {
                    Some((c.id, frame.to_world(center), frame.z, radius))
                }
                _ => None,
            }
        })
        .collect();
    let name = |id: FeatureId| {
        features
            .iter()
            .find(|f| f.id == id)
            .map_or_else(|| id.to_string(), |f| f.name.clone())
    };
    groups
        .into_iter()
        .map(|(group, (normals, pts))| {
            let db = scene.body_of(group);
            let body = db.map(|b| b.id);
            let mesh_body = db.is_some_and(|b| b.mesh_body);
            let bs = body.and_then(|id| bodies.iter().find(|b| b.id == id));
            let n0 = normals.first().copied().unwrap_or([0.0, 0.0, 1.0]);
            let p0 = pts.first().copied().unwrap_or([0.0; 3]);
            let planar = !mesh_body
                && normals.iter().all(|n| dot(*n, n0) > 1.0 - 1e-4)
                && pts.iter().all(|p| dot(sub(*p, p0), n0).abs() <= plane_tol);
            // Same body, coplanar, same outward normal. When several faces
            // share the plane, the candidate's sample point (inside its own
            // face) must lie on this group's triangles; never a guess.
            let reference = planar
                .then(|| {
                    let on_plane: Vec<_> = features
                        .iter()
                        .flat_map(|f| f.faces.iter())
                        .filter(|fs| {
                            Some(fs.body) == body
                                && dot(n0, fs.frame.z) > 1.0 - 1e-4
                                && dot(sub(p0, fs.frame.origin), fs.frame.z).abs() <= plane_tol
                        })
                        .collect();
                    let mut hit = on_plane
                        .iter()
                        .filter(|fs| fs.sample.is_none_or(|s| on_triangles(s, &pts, plane_tol)));
                    match (hit.next(), hit.next()) {
                        (Some(fs), None) => Some(fs.reference),
                        _ => None,
                    }
                })
                .flatten();
            // Legacy blind pockets: the floor has no semantic reference.
            let floor = (planar && reference.is_none())
                .then(|| {
                    features.iter().filter(|f| f.kind == "pocket").find(|pk| {
                        let depth = pk
                            .parameters
                            .iter()
                            .find(|(n, _)| n == "depth")
                            .map_or(0.0, |(_, v)| *v);
                        holes.iter().any(|(id, c, axis, r)| {
                            *id == pk.id
                                && dot(n0, *axis).abs() > 1.0 - 1e-4
                                && (dot(sub(p0, *c), *axis) + depth).abs() <= plane_tol
                                && pts
                                    .iter()
                                    .all(|p| norm(cross(sub(*p, *c), *axis)) <= r + cyl_tol)
                        })
                    })
                })
                .flatten()
                .map(|pk| pk.id);
            let hole = (!planar && !mesh_body)
                .then(|| {
                    holes.iter().find(|(_, c, axis, r)| {
                        pts.iter().all(|p| {
                            let v = sub(*p, *c);
                            (norm(cross(v, *axis)) - r).abs() <= cyl_tol
                        })
                    })
                })
                .flatten();
            let body_name = bs.map_or_else(|| "body".to_string(), |b| b.name.clone());
            let (owner, description) = if let Some(r) = reference {
                let role = crate::face_role_label(&r.role);
                (
                    Some(r.feature),
                    format!(
                        "{body_name}: {} {role} (semantic {} {:?})",
                        name(r.feature),
                        r.feature,
                        r.role
                    ),
                )
            } else if let Some(id) = floor {
                (Some(id), format!("{body_name}: floor of {}", name(id)))
            } else if let Some((id, ..)) = hole {
                (
                    Some(*id),
                    format!("{body_name}: hole wall of {}", name(*id)),
                )
            } else if mesh_body {
                (
                    bs.map(|b| b.created_by),
                    format!("{body_name}: mesh body (STL triangles)"),
                )
            } else {
                (
                    bs.map(|b| b.created_by),
                    format!(
                        "{body_name}: {} face{}",
                        if planar { "planar" } else { "curved" },
                        bs.map_or(String::new(), |b| format!(
                            " (body from {})",
                            name(b.created_by)
                        ))
                    ),
                )
            };
            // The exact plane of this display face (same body, same outward
            // normal, coplanar within the display tolerance).
            let plane = planar
                .then(|| {
                    bs.and_then(|b| {
                        b.planes.iter().copied().find(|p| {
                            dot(n0, p.normal) > 1.0 - 1e-4
                                && dot(sub(p0, p.point), p.normal).abs() <= plane_tol
                        })
                    })
                })
                .flatten();
            FaceInfo {
                group,
                planar,
                body,
                mesh_body,
                reference,
                owner,
                description,
                plane,
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

impl RcadApp {
    fn ray_at(&self, rect: egui::Rect, p: Pos2) -> Option<Ray> {
        let (cam, [w, h]) = self.view_camera();
        let l = p - rect.min;
        cam.ray(l.x, l.y, w, h)
    }

    /// Screen rect of a dimension label of the selected finished sketch, or of
    /// the open sketch session (tests and scripted demos).
    pub fn dimension_label(&self, name: &str) -> Option<egui::Rect> {
        if let Some(s) = &self.sketch {
            let i = s
                .editor
                .sketch
                .constraints
                .iter()
                .position(|c| c.dimension().is_some_and(|d| d.name == name))?;
            return s.editor.screen_target(Item::Constraint(i));
        }
        let f = self
            .summary
            .features
            .iter()
            .find(|f| Some(f.id) == self.selected)?;
        let i = f
            .sketch
            .as_ref()?
            .constraints
            .iter()
            .position(|c| c.dimension().is_some_and(|d| d.name == name))?;
        self.doc_dim_targets
            .iter()
            .find(|(k, _)| *k == i)
            .map(|(_, r)| *r)
    }

    /// Display-mesh face under a screen position and the picked point
    /// (world, from the display mesh: only a pick hint, never geometry).
    pub fn face_hit(&self, p: Pos2) -> Option<(u32, [f64; 3])> {
        let rect = self.view_rect?;
        let ray = self.ray_at(rect, p)?;
        let (t, _, g) = self.scene.mesh.pick_section(&ray, self.section_plane())?;
        let q = |i: usize| f64::from(ray.origin[i] + ray.dir[i] * t);
        Some((g, [q(0), q(1), q(2)]))
    }

    /// Edge pick mode click: project the edge of the clicked planar face
    /// nearest to the click into sketch `sk` (the command refuses with the
    /// reason when the face cannot be referenced or the pick is ambiguous).
    fn project_edge_click(&mut self, sk: FeatureId, p: Pos2) {
        // A click on empty space or a face without a semantic reference
        // (curved, mesh) ends the pick mode without a change, with the reason.
        let Some((g, at)) = self.face_hit(p) else {
            self.diag(
                true,
                "Validation",
                "Project edge: no face under the cursor; choose Project edge again and click a \
                 planar face next to the edge"
                    .into(),
            );
            return;
        };
        let Some(face) = self
            .faces
            .iter()
            .find(|f| f.group == g)
            .and_then(|f| f.reference)
        else {
            let why = self
                .faces
                .iter()
                .find(|f| f.group == g)
                .and_then(|f| f.sketch_blocker())
                .unwrap_or("this face cannot be referenced");
            self.diag(
                true,
                "Validation",
                format!(
                    "Project edge: {why}. Click a planar face that borders the edge (its edges are \
                     projected; curved faces have no edge reference)"
                ),
            );
            return;
        };
        self.request(Command::Project {
            sketch: sk,
            source: rcad_document::ProjectSource::Edge(face),
            construction: false,
            at: Some(at),
        });
    }

    /// Display-mesh face under a screen position.
    pub fn face_at(&self, p: Pos2) -> Option<u32> {
        let rect = self.view_rect?;
        let ray = self.ray_at(rect, p)?;
        self.scene
            .mesh
            .pick_section(&ray, self.section_plane())
            .map(|(_, _, g)| g)
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
        let section = self.section_plane();
        let resp = match self.viewport.as_mut() {
            Some(vp) => {
                vp.set_section(section);
                vp.set_primary_drag_navigates(
                    !tool_active && self.shell.nav_mode == crate::shell::viewcube::NavMode::Orbit,
                );
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
        if !tool_active {
            self.nav_drag(&resp);
        }
        let painter = ui.painter_at(rect);
        if sketching {
            self.sketch_input(ui, &resp);
            self.paint_sketch_session(&painter, rect);
            self.sketch_dim_input(ui);
            self.sketch_hud(ui);
            self.sketch_offset_input(ui);
            self.sketch_fillet_input(ui);
            if let Some(s) = self.sketch.as_mut() {
                crate::text_ui::text_window(ui.ctx(), &mut s.editor);
                crate::pattern_ui::pattern_window(ui.ctx(), &mut s.editor);
            }
        } else {
            self.model_input(ui, &resp);
            self.paint_document_sketches(&painter, rect);
            self.doc_dim_input(ui);
            self.paint_placement(&painter, rect);
            self.paint_extrude_preview(&painter, rect);
            self.paint_reattach_preview(&painter, rect);
            self.paint_mesh_edit_preview(&painter);
            if (!self.summary.open || self.summary.features.is_empty())
                && self.placement.is_none()
                && self.dialog.is_none()
            {
                self.empty_state(ui, rect);
            }
        }
        self.paint_view_chrome(ui, rect);
        self.view_cube(ui, rect);
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
        if let Some(sk) = self.edge_pick_for.take() {
            self.project_edge_click(sk, p);
            return;
        }
        // Extrude dialog open: a click inside a sketch region picks it.
        if self.extrude_region_click(p) {
            return;
        }
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
                buffer: rcad_sketch::fmt_mm(d.value),
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
                Some(rcad_sketch::SolveStatus::FullyConstrained) => StateKind::Full,
                _ => StateKind::Under,
            };
            let editing = self
                .doc_dim_edit
                .as_ref()
                .filter(|d| d.feature == f.id)
                .map(|d| d.constraint);
            let projected: Vec<rcad_sketch::EntityId> =
                f.projection_entities.iter().flatten().copied().collect();
            let style = SketchStyle {
                selection: &[],
                hover: None,
                editing,
                state,
                conflicts: &[],
                glyphs: false,
                alpha: if selected { 1.0 } else { 0.55 },
                projected: &projected,
            };
            let mut shown = sk.clone();
            if !selected {
                // Unselected sketches: curves only.
                shown.constraints.retain(|c| c.dimension().is_none());
            }
            let targets = sketch_view::paint_sketch(painter, &pr, &shown, &style);
            if selected {
                for (it, r) in &targets {
                    if let Item::Constraint(i) = it
                        && let Some(d) = sk.constraints.get(*i).and_then(|c| c.dimension())
                    {
                        crate::probe::note(format!("dim:{}", d.name), *r);
                    }
                }
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
        rcad_viewport::paint_overlay(painter, &cam, rect, &self.placement_overlay());
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

    fn placement_overlay(&self) -> rcad_viewport::Overlay {
        use rcad_viewport::{Overlay, OverlayLabel, OverlayPolyline};
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
        let (apply, cancel) = inline_number_editor(
            ui.ctx(),
            "doc_dim_edit",
            at,
            &format!("Edit {} (mm)", ed.name),
            &mut buf,
            ed.focus,
        );
        if cancel {
            self.doc_dim_edit = None;
            return;
        }
        if apply {
            match buf.trim().trim_end_matches("mm").trim().parse::<f64>() {
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
        let shift = ui.input(|i| i.modifiers.shift);
        // Snap step: a power of ten (1, 10, 0.1 mm, ...) at least
        // SNAP_CELL_PX / 2 apart on screen, so a feature smaller than one
        // drawn grid cell (a 4 mm slot when zoomed out) can still be drawn,
        // and round values (25, 40) stay exactly reachable: a 2 mm or 5 mm
        // step would round 25 to 26 or 40 to 40 but 22 to 20.
        let grid = {
            let ppm = f64::from(rcad_viewport::pixels_per_mm(&cam, rect.height()));
            let need = f64::from(crate::sketch_edit::SNAP_CELL_PX) / 2.0 / ppm.max(1e-9);
            let step = 10f64.powf(need.log10().ceil());
            (step.is_finite() && step > 0.0).then_some(step)
        };
        let view_at = |p: [f64; 2]| View {
            px_per_mm: pr.px_per_mm(p),
            grid,
            snapping: !shift,
        };
        let ed = &mut s.editor;
        if ed.dim_edit.is_some() {
            // The inline editor owns input; forget any press it interrupted.
            s.press = None;
            s.press_dragging = false;
            return;
        }
        if s.press.is_some() && s.press_epoch != ed.epoch() {
            // Tool change / Escape / undo since the press: void it.
            s.press = None;
            s.press_dragging = false;
        }
        // Clicks and drags are derived from the raw primary-button events in
        // order, each at its own position. egui's response-level click uses the
        // pointer's latest position and reports one click per frame, which
        // misplaces or drops clicks when a slow frame batches a release with the
        // next movement (observed with real input).
        let layer = ui.layer_id();
        let ctx = ui.ctx().clone();
        let (events, now) = ui.input(|i| (i.events.clone(), i.time));
        let pass = ctx.cumulative_pass_nr();
        let in_view = |p: Pos2| rect.contains(p) && ctx.layer_id_at(p) == Some(layer);
        let trace = std::env::var_os("RCAD_TRACE_ENGINE").is_some();
        let mut saw_pointer = false;
        for ev in events {
            if ed.dim_edit.is_some() {
                // A click in this batch opened the inline editor: stop here.
                s.press = None;
                s.press_dragging = false;
                break;
            }
            match ev {
                egui::Event::PointerMoved(m) => {
                    saw_pointer = true;
                    if s.press.is_none() && !in_view(m) {
                        ed.cursor = None;
                        ed.hover = None;
                    }
                    if let Some(o) = s.press
                        && !s.press_dragging
                        && o.distance(m) > CLICK_SLOP_PX
                    {
                        // Drag starts at the press position.
                        s.press_dragging = true;
                        if let Some(p) = pr.unproject(o) {
                            if ed.tool == SketchTool::Select {
                                ed.press(p, Some(o), &view_at(p));
                            } else {
                                ed.pointer_move(p, Some(o), &view_at(p));
                                ed.click(p, Some(o), &view_at(p), shift);
                            }
                        }
                    }
                    if (s.press.is_some() || in_view(m))
                        && let Some(p) = pr.unproject(m)
                    {
                        ed.pointer_move(p, Some(m), &view_at(p));
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => {
                    saw_pointer = true;
                    if !in_view(pos) {
                        // A press elsewhere (panels, dialogs) is not ours.
                        s.press = None;
                        s.press_dragging = false;
                        continue;
                    }
                    s.press = Some(pos);
                    s.press_epoch = ed.epoch();
                    s.press_dragging = false;
                    if let Some(p) = pr.unproject(pos) {
                        ed.pointer_move(p, Some(pos), &view_at(p));
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                } => {
                    let Some(origin) = s.press.take() else {
                        continue;
                    };
                    saw_pointer = true;
                    let dragged = std::mem::take(&mut s.press_dragging);
                    if !dragged && !in_view(pos) {
                        continue; // a click released outside the view
                    }
                    let Some(p) = pr.unproject(pos) else { continue };
                    let v = View {
                        snapping: !modifiers.shift,
                        ..view_at(p)
                    };
                    if trace {
                        eprintln!(
                            "{:.3} ui: sketch release at {pos:?} (pressed {origin:?}) dragged={dragged} tool={:?}",
                            crate::engine::trace_clock(),
                            ed.tool
                        );
                    }
                    ed.pointer_move(p, Some(pos), &v);
                    if dragged {
                        if ed.tool == SketchTool::Select {
                            ed.release();
                        } else {
                            ed.click(p, Some(pos), &v, modifiers.shift);
                        }
                        s.last_click = None;
                    } else {
                        ed.click(p, Some(pos), &v, modifiers.shift);
                        // Frame times are the only clock egui gives; require the two
                        // clicks to come from different passes so one slow batch
                        // never fakes a double click.
                        let double = s.last_click.is_some_and(|(lp, lt, lpass)| {
                            lpass != pass
                                && now - lt < DOUBLE_CLICK_S
                                && lp.distance(pos) <= CLICK_SLOP_PX
                        });
                        if double {
                            ed.double_click(p, Some(pos), &v);
                            s.last_click = None;
                        } else {
                            s.last_click = Some((pos, now, pass));
                        }
                    }
                }
                _ => {}
            }
        }
        // Keep the hover preview current between pointer events.
        if !saw_pointer {
            if let Some(h) = resp.hover_pos()
                && let Some(p) = pr.unproject(h)
            {
                if !ed.dragging() {
                    ed.pointer_move(p, Some(h), &view_at(p));
                }
            } else if !ed.dragging() && s.press.is_none() {
                ed.cursor = None;
                ed.hover = None;
            }
        }
        if resp.clicked_by(egui::PointerButton::Secondary) {
            ed.finish_chain();
        }
        self.sketch_project_poll();
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
        let projected: Vec<rcad_sketch::EntityId> = ed.projected.keys().copied().collect();
        let style = SketchStyle {
            selection: &ed.selection,
            hover: ed.hover,
            editing: ed.dim_edit.as_ref().map(|d| d.constraint),
            state,
            conflicts: &conflicts,
            glyphs: true,
            alpha: 1.0,
            projected: &projected,
        };
        let targets = sketch_view::paint_sketch(painter, &pr, &ed.sketch, &style);
        for (it, r) in &targets {
            if let Item::Constraint(i) = it
                && let Some(d) = ed.sketch.constraints.get(*i).and_then(|c| c.dimension())
            {
                crate::probe::note(format!("dim:{}", d.name), *r);
            }
        }
        sketch_view::paint_preview(painter, &pr, ed);
        s.editor.set_screen_targets(targets);
    }

    /// Inline radius entry of the Fillet tool (next to the picked corner).
    fn sketch_fillet_input(&mut self, ui: &egui::Ui) {
        let rect = self.view_rect;
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(ed) = s.editor.fillet_edit.clone() else {
            return;
        };
        let at = s
            .editor
            .screen_target(Item::Point(ed.point))
            .map(|r| r.right_bottom() + egui::vec2(6.0, 4.0))
            .or_else(|| rect.map(|r| r.center_top() + egui::vec2(-80.0, 40.0)));
        let Some(at) = at else { return };
        let mut buf = ed.buffer.clone();
        let (apply, cancel) = inline_number_editor(
            ui.ctx(),
            "sketch_fillet_edit",
            at,
            "Fillet radius (mm)",
            &mut buf,
            false,
        );
        if let Some(f) = &mut s.editor.fillet_edit {
            f.buffer = buf;
        }
        if cancel {
            s.editor.fillet_edit = None;
        } else if apply {
            s.editor.apply_fillet();
        }
    }

    /// Inline distance entry of the Offset tool (next to the picked curve).
    fn sketch_offset_input(&mut self, ui: &egui::Ui) {
        let rect = self.view_rect;
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(ed) = s.editor.offset_edit.clone() else {
            return;
        };
        let at = s
            .editor
            .screen_target(Item::Entity(ed.entity))
            .map(|r| r.right_bottom() + egui::vec2(6.0, 4.0))
            .or_else(|| rect.map(|r| r.center_top() + egui::vec2(-80.0, 40.0)));
        let Some(at) = at else { return };
        let mut buf = ed.buffer.clone();
        let (apply, cancel) = inline_number_editor(
            ui.ctx(),
            "sketch_offset_edit",
            at,
            "Offset distance (mm, + out / − in)",
            &mut buf,
            false,
        );
        if let Some(o) = &mut s.editor.offset_edit {
            o.buffer = buf;
        }
        if cancel {
            s.editor.offset_edit = None;
        } else if apply {
            s.editor.apply_offset();
        }
    }

    /// Inline numeric editor for a dimension inside the sketch editor.
    fn sketch_dim_input(&mut self, ui: &egui::Ui) {
        let rect = self.view_rect;
        let last_open = self.sketch_dim_opened;
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(ed) = s.editor.dim_edit.clone() else {
            self.sketch_dim_opened = None;
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
        let first = last_open != Some(ed.constraint);
        let (apply, cancel) = inline_number_editor(
            ui.ctx(),
            "sketch_dim_edit",
            at,
            &format!("Edit {name} (mm)"),
            &mut buf,
            first,
        );
        if let Some(d) = &mut s.editor.dim_edit {
            d.buffer = buf;
        }
        if cancel {
            s.editor.cancel_dim_edit();
        } else if apply && !s.editor.apply_dim_edit() {
            // Keep the editor open with everything selected for retyping.
            self.sketch_dim_opened = None;
            return;
        }
        self.sketch_dim_opened = Some(ed.constraint);
    }

    // ----- chrome: banner, hints, pending indicator -----

    fn paint_view_chrome(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        // The status lines along the top end before the view cube (they wrap
        // inside the narrower area instead of running under it).
        let cube = crate::shell::viewcube::view_cube_rect(rect);
        let top = egui::Rect::from_min_max(rect.min, egui::pos2(cube.min.x - 8.0, rect.max.y));
        let painter = ui.painter_at(top);
        let mut chips: Vec<egui::Rect> = vec![];
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
            chips.push(r);
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
            chips.push(r);
            y = r.max.y + 6.0;
            if let Some(fb) = &s.editor.feedback {
                let col = if fb.error {
                    PALETTE.conflict
                } else {
                    Color32::from_rgb(200, 230, 200)
                };
                chips.push(chip(
                    &painter,
                    egui::pos2(rect.min.x + 10.0, y),
                    fb.text.clone(),
                    col,
                ));
            }
        } else if let Some(b) = self.banner.clone() {
            let text = format!("✖ [{}] {}", b.kind, b.text);
            let galley = painter.layout(
                text,
                egui::FontId::proportional(14.0),
                PALETTE.conflict,
                // Room for the Dismiss button before the view cube.
                (top.width() - 100.0).max(200.0),
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
            chips.push(r);
            let btn = egui::Rect::from_min_size(
                egui::pos2(r.max.x + 6.0, r.min.y),
                egui::vec2(70.0, 22.0),
            );
            if ui.put(btn, egui::Button::new("Dismiss")).clicked() {
                self.banner = None;
            }
            if b.kind == "BrokenReference"
                && let Some(s) = self.broken_sketch
            {
                let btn2 = btn
                    .translate(egui::vec2(0.0, 26.0))
                    .with_max_x(btn.min.x + 130.0);
                if ui
                    .put(btn2, egui::Button::new("Re-attach sketch…"))
                    .probe("Re-attach sketch…")
                    .clicked()
                {
                    self.start_reattach(s);
                }
            }
        }
        if self.engine.busy() {
            chips.push(chip(
                &painter,
                egui::pos2((top.max.x - 330.0).max(rect.min.x + 10.0), rect.min.y + 8.0),
                "Regenerating… showing the last valid model".into(),
                Color32::from_rgb(255, 210, 120),
            ));
        }
        self.shell.top_chips = chips;
        self.shell.view_cube = Some(cube);
        if self.sketch.is_none()
            && let Some(info) = self.selected_face()
        {
            let text = match info.sketch_blocker() {
                None => format!("Face: {} — Create Sketch is available", info.description),
                Some(why) => format!("Face: {} — {why}", info.description),
            };
            // Right of the axis triad (bottom-left, ~90 points).
            chip(
                &painter,
                egui::pos2(rect.min.x + 110.0, rect.max.y - 34.0),
                text,
                Color32::from_gray(225),
            );
        }
        self.paint_notice(ui, rect);
    }

    /// Why the last key press or menu action did nothing, above the face
    /// line at the bottom of the view; it goes with ×, the next command,
    /// or after a few seconds.
    fn paint_notice(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs() >= crate::app::NOTICE_SECS)
        {
            self.notice = None;
        }
        let Some((text, _)) = self.notice.clone() else {
            return;
        };
        let painter = ui.painter_at(rect);
        let color = Color32::from_rgb(255, 210, 120);
        // Wrapped to the view (room for ×); the bottom edge stays above the
        // selected-face line however many lines it takes.
        let galley = painter.layout(
            format!("ⓘ {text}"),
            egui::FontId::proportional(13.5),
            color,
            (rect.width() - 150.0).max(160.0),
        );
        let size = galley.size() + egui::vec2(10.0, 6.0);
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 110.0, rect.max.y - 40.0 - size.y),
            size,
        );
        painter.rect_filled(r, 4.0, Color32::from_black_alpha(200));
        painter.galley(r.min + egui::vec2(5.0, 3.0), galley, color);
        let btn = egui::Rect::from_min_size(
            egui::pos2(r.max.x + 4.0, r.min.y),
            egui::vec2(22.0, r.height()),
        );
        if ui
            .put(btn, egui::Button::new("×"))
            .on_hover_text("Dismiss")
            .named("Dismiss notice")
            .probe("Dismiss notice")
            .clicked()
        {
            self.notice = None;
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
                    .probe("New Sketch on XY")
                    .on_hover_text("Create a sketch on the world XY plane")
                    .clicked()
                {
                    if self.summary.open {
                        self.begin_sketch(SketchTarget::New {
                            plane: rcad_document::SketchPlane::WorldXY,
                        });
                    } else {
                        self.guarded(crate::app::Guarded::NewAndSketch);
                    }
                }
                if ui
                    .button("Open Document…")
                    .probe("Open Document…")
                    .clicked()
                {
                    self.open_dialog(false);
                }
            });
            ui.weak("Help: F1 · Navigate: MMB orbit, RMB pan, wheel zoom");
        });
    }
}

/// Inline numeric editor popup at `at`. Keeps keyboard focus until Enter,
/// Escape or a button; `select_all` selects the current text (on open, so
/// typing replaces the value). Returns (apply, cancel).
fn inline_number_editor(
    ctx: &egui::Context,
    key: &str,
    at: Pos2,
    label: &str,
    buf: &mut String,
    select_all: bool,
) -> (bool, bool) {
    let (mut apply, mut cancel) = (false, false);
    let id = egui::Id::new((key, "field"));
    egui::Area::new(egui::Id::new(key))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let l = ui.label(label);
                    let mut out = egui::TextEdit::singleline(buf)
                        .id(id)
                        .desired_width(80.0)
                        .show(ui);
                    if select_all {
                        let n = buf.chars().count();
                        out.state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                        out.state.store(ui.ctx(), id);
                    }
                    let r = Response::clone(&out.response)
                        .labelled_by(l.id)
                        .probe(label);
                    let (enter, esc) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter),
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    // Take focus when opened only: the user may click into other
                    // fields; Escape elsewhere still cancels via the shortcuts.
                    if select_all {
                        r.request_focus();
                    }
                    apply |= enter && (r.lost_focus() || r.has_focus());
                    cancel |= esc && (r.lost_focus() || r.has_focus());
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
    (apply, cancel)
}

/// A rounded label with a dark background at `at` (top-left); returns its rect.
pub fn chip(painter: &egui::Painter, at: Pos2, text: String, color: Color32) -> egui::Rect {
    let wrap = (painter.clip_rect().max.x - at.x - 20.0).max(120.0);
    let galley = painter.layout(text, egui::FontId::proportional(13.5), color, wrap);
    let r = egui::Rect::from_min_size(at, galley.size() + egui::vec2(10.0, 6.0));
    painter.rect_filled(r, 4.0, Color32::from_black_alpha(185));
    painter.galley(r.min + egui::vec2(5.0, 3.0), galley, color);
    r
}
