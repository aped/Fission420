//! Z-up orbit camera.

use crate::math::{self, Mat4, Vec3};

const WORLD_UP: Vec3 = [0.0, 0.0, 1.0];
/// Keep pitch strictly inside (-90°, 90°) so the view basis never degenerates.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 1.0e-3;
const MIN_DISTANCE: f32 = 1.0e-3;
const MAX_DISTANCE: f32 = 1.0e7;

/// Orbit camera around `target`. Z is up; `yaw` rotates about +Z measured from
/// +X, `pitch` is elevation above the XY plane. Units are model units (mm).
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

/// A world-space ray with a unit direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0, 0.0],
            distance: 300.0,
            // Classic isometric-ish view from +X/-Y/+Z.
            yaw: -std::f32::consts::FRAC_PI_4,
            pitch: 0.6,
            fov_y: 35.0_f32.to_radians(),
        }
    }
}

impl Camera {
    /// Unit vector from target to eye.
    pub fn back_dir(&self) -> Vec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        [cp * cy, cp * sy, sp]
    }

    pub fn eye(&self) -> Vec3 {
        math::add(self.target, math::scale(self.back_dir(), self.distance))
    }

    /// Orthonormal (right, up, forward) basis.
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let forward = math::scale(self.back_dir(), -1.0);
        // Pitch is clamped, so forward is never parallel to WORLD_UP; still
        // fall back to +X rather than panic if the state was set externally.
        let right = math::normalize(math::cross(forward, WORLD_UP)).unwrap_or([1.0, 0.0, 0.0]);
        let up = math::cross(right, forward);
        (right, up, forward)
    }

    pub fn view(&self) -> Mat4 {
        let (r, u, f) = self.basis();
        math::view_from_basis(self.eye(), r, u, f)
    }

    /// Near/far planes enclosing a scene bounding sphere (`center`, `radius`).
    /// Near is pushed as far out as possible to keep depth precision for
    /// mm-scale parts; it never goes below `far * 1e-5`.
    pub fn clip_planes(&self, center: Vec3, radius: f32) -> (f32, f32) {
        let r = if radius.is_finite() {
            radius.max(1.0e-3)
        } else {
            1.0
        };
        let d = math::length(math::sub(self.eye(), center));
        let far = (d + r) * 1.01;
        let near = ((d - r) * 0.9).max(far * 1.0e-5).max(1.0e-5);
        (near, far.max(near * 2.0))
    }

    pub fn view_proj(&self, aspect: f32, center: Vec3, radius: f32) -> Mat4 {
        let (near, far) = self.clip_planes(center, radius);
        let proj = math::perspective_rh_zo(self.fov_y, aspect.max(1.0e-6), near, far);
        math::mul(&proj, &self.view())
    }

    /// Ray through a pixel. `px, py` are relative to the viewport's top-left,
    /// in the same units as `width, height`.
    pub fn ray(&self, px: f32, py: f32, width: f32, height: f32) -> Option<Ray> {
        if !(width > 0.0 && height > 0.0) {
            return None;
        }
        let ndc_x = 2.0 * px / width - 1.0;
        let ndc_y = 1.0 - 2.0 * py / height;
        let t = (0.5 * self.fov_y).tan();
        let aspect = width / height;
        let (r, u, f) = self.basis();
        let dir = math::add(
            f,
            math::add(
                math::scale(r, ndc_x * t * aspect),
                math::scale(u, ndc_y * t),
            ),
        );
        Some(Ray {
            origin: self.eye(),
            dir: math::normalize(dir)?,
        })
    }

    /// Rotate by pixel deltas.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const RAD_PER_PX: f32 = 0.008;
        self.yaw -= dx * RAD_PER_PX;
        self.pitch = (self.pitch + dy * RAD_PER_PX).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
    }

    /// Pan so the point under the cursor (at target depth) follows the cursor.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height_px: f32) {
        if viewport_height_px <= 0.0 {
            return;
        }
        let units_per_px = 2.0 * self.distance * (0.5 * self.fov_y).tan() / viewport_height_px;
        let (r, u, _) = self.basis();
        let delta = math::add(
            math::scale(r, -dx * units_per_px),
            math::scale(u, dy * units_per_px),
        );
        self.target = math::add(self.target, delta);
    }

    /// Zoom by a scroll amount (positive = closer).
    pub fn zoom(&mut self, scroll: f32) {
        let factor = (-scroll * 0.0015).exp();
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Frame a bounding sphere so it fits in the view for the given aspect.
    pub fn fit_sphere(&mut self, center: Vec3, radius: f32, aspect: f32) {
        let radius = if radius.is_finite() && radius > 0.0 {
            radius
        } else {
            50.0
        };
        let half_v = 0.5 * self.fov_y;
        let half_h = (half_v.tan() * aspect.max(1.0e-3)).atan();
        let half = half_v.min(half_h);
        self.target = center;
        self.distance = (radius / half.sin() * 1.08).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}
