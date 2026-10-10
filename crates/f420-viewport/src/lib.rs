//! Fission420 3D viewport: an egui widget backed by wgpu.
//!
//! GUI-only crate; it knows nothing about documents or the kernel. Callers
//! hand it a [`RenderMesh`] (derived tessellation) with per-triangle group
//! ids, and read back hover picks as group ids.
//!
//! Conventions: Z-up, right-handed, model units are millimetres.
//!
//! Controls: LMB or MMB drag = orbit, RMB or Shift+LMB drag = pan,
//! scroll = zoom. While a placement tool owns the primary button
//! ([`Viewport::set_primary_drag_navigates`]`(false)`), LMB does not move the
//! camera; MMB orbit, RMB pan and scroll still do.
//!
//! Previews are drawn with [`paint_overlay`] after [`Viewport::ui`], using
//! [`Viewport::camera`] and the rect returned in the response.

mod camera;
mod gpu;
mod math;
mod mesh;
mod overlay;

pub use camera::{Camera, Ray};
pub use mesh::{Aabb, MeshError, RenderMesh, box_mesh, ray_triangle};
pub use overlay::{
    Overlay, OverlayLabel, OverlayPolyline, overlay_shapes, paint_overlay, ray_plane,
};

use eframe::egui;
use gpu::{Gpu, LineVertex, MeshVertex, Uniforms};
use math::Vec3;

const BASE_COLOR: [f32; 4] = [0.42, 0.52, 0.66, 1.0];
const HIGHLIGHT_COLOR: [f32; 4] = [1.0, 0.50, 0.10, 1.0];
/// Gamma-encoded clear colour.
const CLEAR_COLOR: [f64; 3] = [0.15, 0.16, 0.18];
const DEFAULT_ASPECT: f32 = 1.5;

pub struct Viewport {
    gpu: Gpu,
    pub camera: Camera,
    mesh: RenderMesh,
    mesh_bounds: Option<Aabb>,
    /// Bounds of everything drawn (mesh + grid + axes), for clip planes.
    scene_bounds: Aabb,
    highlight: Option<u32>,
    hovered: Option<u32>,
    last_aspect: f32,
    /// Size (points) of the last drawn viewport rect; zero before the first frame.
    last_size: [f32; 2],
    /// Whether an LMB drag orbits/pans (false while a placement tool is active).
    primary_navigates: bool,
}

impl Viewport {
    /// Returns `None` if eframe was not started with the wgpu renderer.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let rs = cc.wgpu_render_state.as_ref()?;
        let mut vp = Self {
            gpu: Gpu::new(rs),
            camera: Camera::default(),
            mesh: RenderMesh::default(),
            mesh_bounds: None,
            scene_bounds: Aabb {
                min: [-1.0; 3],
                max: [1.0; 3],
            },
            highlight: None,
            hovered: None,
            last_aspect: DEFAULT_ASPECT,
            last_size: [0.0; 2],
            primary_navigates: true,
        };
        vp.rebuild_grid();
        Some(vp)
    }

    /// Upload a mesh. On error the previous mesh is kept.
    pub fn set_mesh(&mut self, mesh: &RenderMesh) -> Result<(), MeshError> {
        mesh.validate()?;
        self.gpu.set_mesh(&expand_mesh(mesh));
        self.mesh = mesh.clone();
        self.mesh_bounds = mesh.bounds();
        self.hovered = None;
        self.rebuild_grid();
        Ok(())
    }

    pub fn mesh_bounds(&self) -> Option<Aabb> {
        self.mesh_bounds
    }

    pub fn set_highlight(&mut self, group: Option<u32>) {
        self.highlight = group;
    }

    /// Fit the camera to the mesh bounding box (or a default 100 mm region).
    pub fn fit(&mut self) {
        let (center, radius) = match self.mesh_bounds {
            Some(b) => (b.center(), b.radius()),
            None => ([0.0; 3], 50.0),
        };
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

        self.hovered = response.hover_pos().and_then(|p| {
            let local = p - rect.min;
            let ray = self
                .camera
                .ray(local.x, local.y, rect.width(), rect.height())?;
            self.mesh.pick(&ray).map(|(_, _, g)| g)
        });

        let ppp = ui.ctx().pixels_per_point();
        let px = |v: f32| (v * ppp).round().max(1.0) as u32;
        let size = [px(rect.width()), px(rect.height())];
        let uniforms = self.uniforms(self.last_aspect);
        if let Some(tex) = self.gpu.render(size, &uniforms, CLEAR_COLOR) {
            ui.painter().image(
                tex,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
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
                self.camera.zoom(scroll);
            }
        }
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
        Uniforms {
            view_proj: self.camera.view_proj(
                aspect,
                self.scene_bounds.center(),
                self.scene_bounds.radius(),
            ),
            eye: [eye[0], eye[1], eye[2], 1.0],
            light_dir: [light[0], light[1], light[2], 0.0],
            base_color: BASE_COLOR,
            highlight_color: HIGHLIGHT_COLOR,
            highlight: [hl.unwrap_or(0), u32::from(hl.is_some()), 0, 0],
        }
    }

    fn rebuild_grid(&mut self) {
        let (lines, bounds) = grid_lines(self.mesh_bounds);
        self.gpu.set_lines(&lines);
        self.scene_bounds = match self.mesh_bounds {
            Some(m) => Aabb::from_points([m.min, m.max, bounds.min, bounds.max]).unwrap_or(bounds),
            None => bounds,
        };
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

/// XY grid at Z=0 plus RGB axes, sized to the mesh. Returns lines and bounds.
fn grid_lines(mesh_bounds: Option<Aabb>) -> (Vec<LineVertex>, Aabb) {
    let extent = match mesh_bounds {
        Some(b) => {
            let r: f32 = [b.min[0], b.min[1], b.max[0], b.max[1]]
                .iter()
                .fold(0.0, |m, v| m.max(v.abs()));
            (r * 1.5).max(b.radius())
        }
        None => 100.0,
    }
    .max(1.0e-3);
    // Decade spacing giving ~4-40 cells per half-extent.
    let spacing = 10f32.powf((extent / 4.0).log10().floor());
    let n = (extent / spacing).ceil().clamp(1.0, 200.0) as i32;
    let half = n as f32 * spacing;

    let mut v = Vec::with_capacity((4 * (2 * n + 1) + 6) as usize);
    let minor = [0.55, 0.57, 0.60, 0.25];
    let major = [0.65, 0.67, 0.70, 0.45];
    for i in -n..=n {
        if i == 0 {
            continue; // axes drawn below
        }
        let c = if i % 10 == 0 { major } else { minor };
        let s = i as f32 * spacing;
        line(&mut v, [s, -half, 0.0], [s, half, 0.0], c);
        line(&mut v, [-half, s, 0.0], [half, s, 0.0], c);
    }
    // Negative half-axes dim, positive half-axes saturated. Z axis shows up.
    let grey = [0.55, 0.57, 0.60, 0.35];
    line(&mut v, [-half, 0.0, 0.0], [0.0, 0.0, 0.0], grey);
    line(&mut v, [0.0, -half, 0.0], [0.0, 0.0, 0.0], grey);
    line(&mut v, [0.0; 3], [half, 0.0, 0.0], [0.90, 0.25, 0.25, 1.0]);
    line(&mut v, [0.0; 3], [0.0, half, 0.0], [0.30, 0.85, 0.30, 1.0]);
    line(
        &mut v,
        [0.0; 3],
        [0.0, 0.0, half * 0.5],
        [0.30, 0.50, 0.95, 1.0],
    );

    let bounds = Aabb {
        min: [-half, -half, 0.0],
        max: [half, half, half * 0.5],
    };
    (v, bounds)
}

fn line(v: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    v.push(LineVertex { pos: a, color });
    v.push(LineVertex { pos: b, color });
}

#[cfg(test)]
mod tests;
