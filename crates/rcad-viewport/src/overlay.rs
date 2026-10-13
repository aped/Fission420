//! Screen-space overlay for previews: world polylines and labels projected
//! with the same [`Camera`] as the 3D pass and painted with egui on top of the
//! rendered image (always on top; no depth test).
//!
//! Also hosts the f64 ray/plane helpers used for interactive placement. They
//! are display-side helpers: the caller turns the hit point into a typed
//! command parameter; nothing here mutates the document.

use eframe::egui;

use crate::camera::{Camera, Ray};
use crate::math;

/// A world-space polyline to draw over the 3D view.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPolyline {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    pub color: egui::Color32,
    /// Stroke width in points.
    pub width: f32,
    pub dashed: bool,
}

/// Text anchored at a world point (drawn just right/below the projection).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayLabel {
    pub at: [f32; 3],
    pub text: String,
    pub color: egui::Color32,
}

/// Everything drawn over the 3D view this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub polylines: Vec<OverlayPolyline>,
    pub labels: Vec<OverlayLabel>,
}

impl Overlay {
    pub fn is_empty(&self) -> bool {
        self.polylines.is_empty() && self.labels.is_empty()
    }
}

/// Dash/gap lengths in points.
const DASH: f32 = 7.0;
const GAP: f32 = 4.0;

/// Intersect a ray with the plane through `origin` with normal `normal` (f64).
/// Returns the hit point, or `None` if the ray is parallel to the plane
/// (|cos| below 1e-9) or the plane is behind the ray origin.
pub fn ray_plane(ray: &Ray, origin: [f64; 3], normal: [f64; 3]) -> Option<[f64; 3]> {
    let o = ray.origin.map(f64::from);
    let d = ray.dir.map(f64::from);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let n_len = dot(normal, normal).sqrt();
    if !(n_len.is_finite() && n_len > 0.0) {
        return None;
    }
    let denom = dot(d, normal) / n_len;
    // Pixel-level grazing angles are rejected rather than producing huge hits.
    if denom.abs() < 1.0e-9 {
        return None;
    }
    let rel = [origin[0] - o[0], origin[1] - o[1], origin[2] - o[2]];
    let t = dot(rel, normal) / n_len / denom;
    if !(t.is_finite() && t >= 0.0) {
        return None;
    }
    Some([o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]])
}

/// Project polylines (labels are painted by [`paint_overlay`]) into `rect` (screen points) using `camera`.
/// Segments with an endpoint behind the eye are dropped.
pub fn overlay_shapes(camera: &Camera, rect: egui::Rect, overlay: &Overlay) -> Vec<egui::Shape> {
    let (w, h) = (rect.width(), rect.height());
    let to_screen = |p: [f32; 3]| {
        camera
            .project(p, w, h)
            .map(|[x, y]| rect.min + egui::vec2(x, y))
    };
    let mut shapes = vec![];
    for pl in &overlay.polylines {
        let stroke = egui::Stroke::new(pl.width, pl.color);
        let mut pts = pl.points.clone();
        if pl.closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        // Split into runs of visible points.
        let mut run: Vec<egui::Pos2> = vec![];
        let flush = |run: &mut Vec<egui::Pos2>, shapes: &mut Vec<egui::Shape>| {
            if run.len() >= 2 {
                if pl.dashed {
                    shapes.extend(egui::Shape::dashed_line(run, stroke, DASH, GAP));
                } else {
                    shapes.push(egui::Shape::line(std::mem::take(run), stroke));
                }
            }
            run.clear();
        };
        for p in pts {
            match to_screen(p) {
                Some(s) => run.push(s),
                None => flush(&mut run, &mut shapes),
            }
        }
        flush(&mut run, &mut shapes);
    }
    shapes
}

/// Paint `overlay` into `painter` (clipped to `rect`).
pub fn paint_overlay(
    painter: &egui::Painter,
    camera: &Camera,
    rect: egui::Rect,
    overlay: &Overlay,
) {
    let painter = painter.with_clip_rect(rect);
    painter.extend(overlay_shapes(camera, rect, overlay));
    let (w, h) = (rect.width(), rect.height());
    for l in &overlay.labels {
        if let Some([x, y]) = camera.project(l.at, w, h) {
            let pos = rect.min + egui::vec2(x + 10.0, y + 6.0);
            let galley =
                painter.layout_no_wrap(l.text.clone(), egui::FontId::proportional(14.0), l.color);
            let bg = egui::Rect::from_min_size(pos, galley.size()).expand(3.0);
            painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha(170));
            painter.galley(pos, galley, l.color);
        }
    }
}

impl Camera {
    /// World point → pixel relative to the viewport's top-left (same units as
    /// `width, height`). Inverse of [`Camera::ray`]. `None` if the point is
    /// at or behind the eye.
    pub fn project(&self, p: [f32; 3], width: f32, height: f32) -> Option<[f32; 2]> {
        if !(width > 0.0 && height > 0.0) {
            return None;
        }
        let (r, u, f) = self.basis();
        let rel = math::sub(p, self.eye());
        let z = math::dot(rel, f);
        let min_depth = 1.0e-6 * self.distance.max(1.0e-6);
        if z.is_nan() || z <= min_depth {
            return None;
        }
        let t = (0.5 * self.fov_y).tan();
        let aspect = width / height;
        let ndc_x = math::dot(rel, r) / (z * t * aspect);
        let ndc_y = math::dot(rel, u) / (z * t);
        let px = (ndc_x + 1.0) * 0.5 * width;
        let py = (1.0 - ndc_y) * 0.5 * height;
        (px.is_finite() && py.is_finite()).then_some([px, py])
    }
}
