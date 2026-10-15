//! R-CAD 3D viewport: an egui widget backed by wgpu.
//!
//! GUI-only crate; it knows nothing about documents or the kernel. Callers
//! hand it a [`RenderMesh`] (derived tessellation) with per-triangle group
//! ids, and read back hover picks as group ids.
//!
//! Conventions: Z-up, right-handed, model units are millimetres. Screen
//! quantities ("px") are egui points unless stated otherwise.
//!
//! Controls: see [`NAV_HELP`]. While a placement tool owns the primary button
//! ([`Viewport::set_primary_drag_navigates`]`(false)`), LMB does not move the
//! camera; MMB orbit, RMB pan and scroll still do.
//!
//! Drawn by the viewport itself: shaded body (hover and selection colours,
//! optional dimming), feature edges, an adaptive grid (world XY or a sketch
//! plane), and an orientation triad. Previews are drawn with
//! [`paint_overlay`] after [`Viewport::ui`], using [`Viewport::camera`] and
//! the rect returned in the response.

mod camera;
mod edges;
mod gpu;
mod grid;
mod math;
mod mesh;
mod overlay;
mod pick;
mod triad;

pub use camera::{Camera, Ray, ViewPreset, pixels_per_mm};
pub use edges::{edge_quantum, feature_edges};
pub use grid::{GridPlane, GridPlaneError, MIN_CELL_PX, adaptive_grid_spacing};
pub use mesh::{Aabb, MeshError, RenderMesh, SectionPlane, box_mesh, ray_triangle};
pub use overlay::{
    Overlay, OverlayLabel, OverlayPolyline, overlay_shapes, paint_overlay, ray_plane,
};
pub use pick::{Pick, PickEdge, PickScene, PickVertex, PixelTolerance};
pub use triad::{axis_triad_origin, axis_triad_shapes};

/// Tint palette index of highlighted assembly occurrences.
pub const TINT_HIGHLIGHT: u8 = 8;
/// Tint palette index of bodies outside the active component.
pub const TINT_DIMMED: u8 = 9;

use eframe::egui;
use gpu::{FLAG_SELECTED, Gpu, LineVertex, MeshVertex, TINT_SHIFT, Uniforms};
use grid::GridSpec;
use math::Vec3;

/// Mouse bindings, for a Help window.
pub const NAV_HELP: &str = "\
Left drag / middle drag: orbit
Right drag / Shift + left drag: pan
Scroll: zoom toward the pointer
While a sketch or placement tool is active, left drag belongs to the tool;
middle drag, right drag and scroll still navigate.";

const BASE_COLOR: [f32; 4] = [0.42, 0.52, 0.66, 1.0];
const HIGHLIGHT_COLOR: [f32; 4] = [1.0, 0.50, 0.10, 1.0];
const SELECTION_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 1.0];
const EDGE_COLOR: [f32; 4] = [0.03, 0.035, 0.05, 0.9];
const EDGE_COLOR_DIMMED: [f32; 4] = [0.03, 0.035, 0.05, 0.25];
/// How far a dimmed body is blended toward the background.
const DIM_AMOUNT: f32 = 0.55;
/// Gamma-encoded clear colour.
const CLEAR_COLOR: [f64; 3] = [0.15, 0.16, 0.18];
const DEFAULT_ASPECT: f32 = 1.5;
/// Assumed viewport height (points) before the first frame.
const DEFAULT_HEIGHT: f32 = 600.0;
/// Near plane never closer than this fraction of far just to include grid
/// lines near the eye (keeps depth precision on the body).
const GRID_NEAR_RATIO: f32 = 1.0e-3;

pub struct Viewport {
    gpu: Gpu,
    pub camera: Camera,
    mesh: RenderMesh,
    mesh_bounds: Option<Aabb>,
    /// Group id per expanded (de-indexed) mesh vertex, for selection flags.
    vertex_groups: Vec<u32>,
    /// Feature edges of the current mesh (world segments).
    edges: Vec<[Vec3; 2]>,
    highlight: Option<u32>,
    /// Sorted, deduplicated selected groups.
    selection: Vec<u32>,
    /// Tint palette index per group range (see [`Viewport::set_group_tints`]).
    tints: Vec<(std::ops::Range<u32>, u8)>,
    dimmed: bool,
    hovered: Option<u32>,
    /// Requested sketch grid plane (orthonormalized); `None` = world XY.
    grid_plane: Option<GridPlane>,
    /// Spec of the grid currently uploaded.
    grid_spec: Option<GridSpec>,
    grid_bounds: Option<Aabb>,
    last_aspect: f32,
    /// Size (points) of the last drawn viewport rect; zero before the first frame.
    last_size: [f32; 2],
    /// Whether an LMB drag orbits/pans (false while a placement tool is active).
    primary_navigates: bool,
    /// Display-only section plane (None = off).
    section: Option<SectionPlane>,
}

impl Viewport {
    /// Returns `None` if eframe was not started with the wgpu renderer.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let rs = cc.wgpu_render_state.as_ref()?;
        Some(Self::with_render_state(rs))
    }

    fn with_render_state(rs: &eframe::egui_wgpu::RenderState) -> Self {
        let mut vp = Self {
            gpu: Gpu::new(rs),
            camera: Camera::default(),
            mesh: RenderMesh::default(),
            mesh_bounds: None,
            vertex_groups: vec![],
            edges: vec![],
            highlight: None,
            selection: vec![],
            tints: vec![],
            dimmed: false,
            hovered: None,
            grid_plane: None,
            grid_spec: None,
            grid_bounds: None,
            last_aspect: DEFAULT_ASPECT,
            last_size: [0.0; 2],
            primary_navigates: true,
            section: None,
        };
        vp.update_grid();
        vp
    }

    /// Upload a mesh. On error the previous mesh is kept. Feature edges are
    /// derived here; the current selection is re-applied to the new mesh.
    pub fn set_mesh(&mut self, mesh: &RenderMesh) -> Result<(), MeshError> {
        mesh.validate()?;
        let verts = expand_mesh(mesh);
        self.vertex_groups = verts.iter().map(|v| v.group).collect();
        let flags = self.vertex_flags();
        self.gpu.set_mesh(&verts, &flags);
        self.edges = feature_edges(mesh);
        self.upload_edges();
        self.mesh = mesh.clone();
        self.mesh_bounds = mesh.bounds();
        self.hovered = None;
        Ok(())
    }

    pub fn mesh_bounds(&self) -> Option<Aabb> {
        self.mesh_bounds
    }

    /// Hover highlight (orange). Wins over selection colour.
    pub fn set_highlight(&mut self, group: Option<u32>) {
        self.highlight = group;
    }

    /// Selected face groups, drawn in the selection colour. Only re-uploads
    /// GPU data when the set actually changes (order/duplicates ignored).
    pub fn set_selection(&mut self, groups: &[u32]) {
        let mut sel = groups.to_vec();
        sel.sort_unstable();
        sel.dedup();
        if sel == self.selection {
            return;
        }
        self.selection = sel;
        self.upload_flags();
    }

    /// Tint group ranges with a palette colour (`0` = the default body colour,
    /// `1..=6` distinct body tints, `7` = mesh-body colour, [`TINT_HIGHLIGHT`],
    /// [`TINT_DIMMED`]). Groups outside every
    /// range use the default. Used to tell several bodies apart; applies to the
    /// current and later meshes until changed.
    pub fn set_group_tints(&mut self, tints: &[(std::ops::Range<u32>, u8)]) {
        if self.tints.as_slice() == tints {
            return;
        }
        self.tints = tints.to_vec();
        self.upload_flags();
    }

    fn upload_flags(&mut self) {
        if !self.vertex_groups.is_empty() {
            let flags = self.vertex_flags();
            self.gpu.set_mesh_flags(&flags);
        }
    }

    /// Per-vertex flag words: selection bit and tint index (bits 8..11).
    fn vertex_flags(&self) -> Vec<u32> {
        let mut flags = selection_flags(&self.vertex_groups, &self.selection);
        if !self.tints.is_empty() {
            for (f, g) in flags.iter_mut().zip(&self.vertex_groups) {
                if let Some((_, t)) = self.tints.iter().find(|(r, _)| r.contains(g)) {
                    *f |= u32::from(*t & 0xF) << TINT_SHIFT;
                }
            }
        }
        flags
    }

    /// Currently selected groups (sorted, deduplicated).
    pub fn selection(&self) -> &[u32] {
        &self.selection
    }

    /// Fade and desaturate the body (e.g. while editing a sketch).
    pub fn set_dimmed(&mut self, dimmed: bool) {
        if dimmed != self.dimmed {
            self.dimmed = dimmed;
            self.upload_edges();
        }
    }

    pub fn dimmed(&self) -> bool {
        self.dimmed
    }

    /// Draw the grid on a sketch plane (`Some`) or the world XY plane
    /// (`None`). A degenerate plane is rejected and the grid is unchanged.
    pub fn set_grid_plane(&mut self, plane: Option<GridPlane>) -> Result<(), GridPlaneError> {
        self.grid_plane = match plane {
            Some(p) => Some(p.orthonormalized()?),
            None => None,
        };
        self.update_grid();
        Ok(())
    }

    pub fn grid_plane(&self) -> Option<GridPlane> {
        self.grid_plane
    }

    /// Minor grid spacing in mm currently shown (for snapping). Before the
    /// first frame this assumes a 600-point-high view.
    pub fn grid_spacing(&self) -> f32 {
        match &self.grid_spec {
            Some(s) => s.spacing,
            None => self.current_grid_spec().spacing,
        }
    }

    /// Screen points per mm at the camera target depth for the last drawn
    /// size (600-point height assumed before the first frame). Use for
    /// screen-space pick tolerances: `tol_mm = tol_px / pixels_per_mm()`.
    pub fn pixels_per_mm(&self) -> f32 {
        pixels_per_mm(&self.camera, self.view_height())
    }

    /// Orient the camera to a standard view (target and distance kept).
    pub fn set_view(&mut self, preset: ViewPreset) {
        self.camera.set_view(preset);
    }

    /// See [`Camera::look_along_normal`].
    pub fn look_along_normal(&mut self, normal: [f32; 3], x_axis: [f32; 3]) {
        self.camera.look_along_normal(normal, x_axis);
    }

    /// Fit the camera to the mesh bounding box (or a default 100 mm region).
    pub fn fit(&mut self) {
        let (center, radius) = match self.mesh_bounds {
            Some(b) => (b.center(), b.radius()),
            None => ([0.0; 3], 50.0),
        };
        self.fit_to(center, radius);
    }

    /// Frame a sphere, keeping the view orientation.
    pub fn fit_to(&mut self, center: [f32; 3], radius: f32) {
        self.camera.fit_sphere(center, radius, self.last_aspect);
    }

    /// Size in points of the last drawn view (zero before the first frame).
    pub fn last_size(&self) -> [f32; 2] {
        self.last_size
    }

    /// When `false`, primary-button drags no longer orbit or pan, so a tool
    /// can use them. Other navigation stays available.
    pub fn set_primary_drag_navigates(&mut self, on: bool) {
        self.primary_navigates = on;
    }

    /// Group id of the triangle under the pointer as of the last `ui` call.
    pub fn hovered_group(&self) -> Option<u32> {
        self.hovered
    }

    /// Fill the available space, handle input, and draw.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return response;
        }
        self.last_aspect = rect.width() / rect.height();
        self.last_size = [rect.width(), rect.height()];
        self.handle_input(ui, &response, rect);
        self.update_grid();

        self.hovered = response.hover_pos().and_then(|p| {
            let local = p - rect.min;
            let ray = self
                .camera
                .ray(local.x, local.y, rect.width(), rect.height())?;
            self.mesh
                .pick_section(&ray, self.section)
                .map(|(_, _, g)| g)
        });

        let ppp = ui.ctx().pixels_per_point();
        let px = |v: f32| (v * ppp).round().max(1.0) as u32;
        let size = [px(rect.width()), px(rect.height())];
        let uniforms = self.uniforms(self.last_aspect);
        let painter = ui.painter().with_clip_rect(rect);
        if let Some(tex) = self.gpu.render(size, &uniforms, CLEAR_COLOR) {
            painter.image(
                tex,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.extend(axis_triad_shapes(&self.camera, rect));
        response
    }

    fn handle_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let shift = ui.input(|i| i.modifiers.shift);
        let d = response.drag_delta();
        let primary = self.primary_navigates && response.dragged_by(egui::PointerButton::Primary);
        if (primary && !shift) || response.dragged_by(egui::PointerButton::Middle) {
            self.camera.orbit(d.x, d.y);
        } else if (primary && shift) || response.dragged_by(egui::PointerButton::Secondary) {
            self.camera.pan(d.x, d.y, rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll != 0.0 {
                match response.hover_pos() {
                    Some(p) => {
                        let l = p - rect.min;
                        self.camera
                            .zoom_at(scroll, l.x, l.y, rect.width(), rect.height());
                    }
                    None => self.camera.zoom(scroll),
                }
            }
        }
    }

    fn view_height(&self) -> f32 {
        if self.last_size[1] > 0.0 {
            self.last_size[1]
        } else {
            DEFAULT_HEIGHT
        }
    }

    fn current_grid_spec(&self) -> GridSpec {
        GridSpec::new(
            self.grid_plane,
            &self.camera,
            self.view_height(),
            self.last_aspect,
        )
    }

    /// Rebuild grid lines if spacing, plane, centre or extent changed.
    fn update_grid(&mut self) {
        let spec = self.current_grid_spec();
        if self.grid_spec.as_ref() == Some(&spec) {
            return;
        }
        let (lines, bounds) = spec.build();
        self.gpu.set_lines(&lines);
        self.grid_bounds = Some(bounds);
        self.grid_spec = Some(spec);
    }

    fn upload_edges(&mut self) {
        let color = if self.dimmed {
            EDGE_COLOR_DIMMED
        } else {
            EDGE_COLOR
        };
        let edges: Vec<[Vec3; 2]> = match self.section {
            Some(s) => self
                .edges
                .iter()
                .filter_map(|[a, b]| s.clip_segment(*a, *b))
                .collect(),
            None => self.edges.clone(),
        };
        self.gpu.set_edges(&edge_vertices(&edges, color));
    }

    /// Display-only section view: hide material beyond `section` (drawing,
    /// edges and picking). `None` turns it off. Never changes the model.
    pub fn set_section(&mut self, section: Option<SectionPlane>) {
        if section != self.section {
            self.section = section;
            self.upload_edges();
        }
    }

    pub fn section(&self) -> Option<SectionPlane> {
        self.section
    }

    /// Near/far: near is fitted to the body (depth precision where it
    /// matters) but may come in to `GRID_NEAR_RATIO * far` to show grid lines
    /// near the eye; far encloses body and grid.
    fn clip_range(&self) -> (f32, f32) {
        let scene = match (self.mesh_bounds, self.grid_bounds) {
            (Some(m), Some(g)) => Aabb::from_points([m.min, m.max, g.min, g.max]),
            (m, g) => m.or(g),
        };
        let (scene_near, scene_far) = match scene {
            Some(s) => self.camera.clip_planes(s.center(), s.radius()),
            None => self.camera.clip_planes(self.camera.target, 50.0),
        };
        let Some(m) = self.mesh_bounds else {
            return (scene_near, scene_far);
        };
        let (mesh_near, mesh_far) = self.camera.clip_planes(m.center(), m.radius());
        let far = mesh_far.max(scene_far);
        let near = mesh_near.min(scene_near.max(far * GRID_NEAR_RATIO));
        (near, far.max(near * 2.0))
    }

    fn uniforms(&self, aspect: f32) -> Uniforms {
        let (right, up, forward) = self.camera.basis();
        let back = math::scale(forward, -1.0);
        // Key light from over the viewer's upper-left shoulder.
        let light = math::normalize(math::add(
            back,
            math::add(math::scale(up, 0.6), math::scale(right, -0.4)),
        ))
        .unwrap_or(back);
        let eye = self.camera.eye();
        let hl = self.highlight;
        let (near, far) = self.clip_range();
        let bg = CLEAR_COLOR.map(|c| (c as f32).powf(2.2));
        Uniforms {
            view_proj: self.camera.view_proj_near_far(aspect, near, far),
            eye: [eye[0], eye[1], eye[2], 1.0],
            light_dir: [light[0], light[1], light[2], 0.0],
            base_color: BASE_COLOR,
            highlight_color: HIGHLIGHT_COLOR,
            selection_color: SELECTION_COLOR,
            fade: [
                bg[0],
                bg[1],
                bg[2],
                if self.dimmed { DIM_AMOUNT } else { 0.0 },
            ],
            section: self.section.map_or([0.0; 4], |s| {
                [s.normal[0], s.normal[1], s.normal[2], s.offset]
            }),
            highlight: [
                hl.unwrap_or(0),
                u32::from(hl.is_some()),
                u32::from(self.section.is_some()),
                0,
            ],
        }
    }
}

/// De-index into per-corner vertices so each triangle carries its group id.
fn expand_mesh(mesh: &RenderMesh) -> Vec<MeshVertex> {
    let mut out = Vec::with_capacity(mesh.indices.len());
    for (t, idx) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let group = mesh.tri_group.get(t).copied().unwrap_or(u32::MAX);
        let p = |k: usize| mesh.positions.get(idx[k] as usize).copied();
        let (Some(a), Some(b), Some(c)) = (p(0), p(1), p(2)) else {
            continue;
        };
        let flat = math::normalize(math::cross(math::sub(b, a), math::sub(c, a)))
            .unwrap_or([0.0, 0.0, 1.0]);
        for (k, pos) in [a, b, c].into_iter().enumerate() {
            let normal = mesh
                .normals
                .get(idx[k] as usize)
                .and_then(|n| math::normalize(*n))
                .unwrap_or(flat);
            out.push(MeshVertex { pos, normal, group });
        }
    }
    out
}

/// Per-vertex flag words for a sorted `selection`.
fn selection_flags(vertex_groups: &[u32], selection: &[u32]) -> Vec<u32> {
    vertex_groups
        .iter()
        .map(|g| {
            if selection.binary_search(g).is_ok() {
                FLAG_SELECTED
            } else {
                0
            }
        })
        .collect()
}

fn edge_vertices(edges: &[[Vec3; 2]], color: [f32; 4]) -> Vec<LineVertex> {
    let mut v = Vec::with_capacity(edges.len() * 2);
    for [a, b] in edges {
        grid::line(&mut v, *a, *b, color);
    }
    v
}

#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod tests;
