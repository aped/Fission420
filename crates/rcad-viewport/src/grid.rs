//! Adaptive reference grid on the world XY plane or a sketch plane.
//!
//! Spacing is chosen from {1, 2, 5} × 10^k mm so one minor cell is at least
//! [`MIN_CELL_PX`] screen points at the camera target depth. Every 10th line
//! is major (every 5th for 2 × 10^k spacing, so majors stay on decades). The
//! grid is centred on the target's projection onto the plane (snapped to a
//! major line) rather than the plane origin, so it keeps covering the view
//! after panning; it spans about 1.5× the visible area.
//! Lines are rebuilt only when the resulting [`GridSpec`] changes.

use crate::camera::Camera;
use crate::gpu::LineVertex;
use crate::math::{self, Vec3};
use crate::mesh::Aabb;

/// Minimum minor cell size in screen points.
pub const MIN_CELL_PX: f32 = 14.0;
/// At most this many lines per direction on each side of the centre, i.e.
/// `2 * (2 * MAX_HALF_LINES + 1) <= 400` grid lines in total.
const MAX_HALF_LINES: i32 = 99;
/// Fallback spacing for invalid input.
const FALLBACK_SPACING: f32 = 10.0;

/// A plane to draw the grid on: `origin` plus in-plane axes `x` and `y`
/// (world coordinates, mm). `x` and `y` need not be unit length; they are
/// orthonormalized (y is made perpendicular to x, keeping its side) by
/// [`GridPlane::orthonormalized`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPlane {
    pub origin: [f32; 3],
    pub x: [f32; 3],
    pub y: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridPlaneError {
    /// Non-finite values, a zero axis, or parallel axes.
    Degenerate,
}

impl std::fmt::Display for GridPlaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("grid plane has a non-finite, zero-length or parallel axis")
    }
}

impl std::error::Error for GridPlaneError {}

impl GridPlane {
    /// World XY plane at Z = 0.
    pub const XY: GridPlane = GridPlane {
        origin: [0.0; 3],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    };

    /// Unit, mutually perpendicular axes (Gram-Schmidt on `y`).
    pub fn orthonormalized(&self) -> Result<GridPlane, GridPlaneError> {
        if !self.origin.iter().all(|c| c.is_finite()) {
            return Err(GridPlaneError::Degenerate);
        }
        let x = math::normalize(self.x).ok_or(GridPlaneError::Degenerate)?;
        let y = math::sub(self.y, math::scale(x, math::dot(self.y, x)));
        // Reject nearly parallel axes (sin of the angle below 1e-4).
        let ylen = math::length(self.y);
        if math::length(y).partial_cmp(&(1.0e-4 * ylen)) != Some(std::cmp::Ordering::Greater) {
            return Err(GridPlaneError::Degenerate);
        }
        let y = math::normalize(y).ok_or(GridPlaneError::Degenerate)?;
        Ok(GridPlane {
            origin: self.origin,
            x,
            y,
        })
    }

    /// Unit normal `x × y` (assumes orthonormalized axes).
    pub fn normal(&self) -> [f32; 3] {
        math::normalize(math::cross(self.x, self.y)).unwrap_or([0.0, 0.0, 1.0])
    }

    fn at(&self, u: f32, v: f32) -> Vec3 {
        math::add(
            self.origin,
            math::add(math::scale(self.x, u), math::scale(self.y, v)),
        )
    }
}

/// Smallest spacing in {1, 2, 5} × 10^k (mm) whose on-screen size
/// `spacing * px_per_mm` is at least `min_px`. Invalid input returns 10 mm.
pub fn adaptive_grid_spacing(px_per_mm: f32, min_px: f32) -> f32 {
    if !(px_per_mm.is_finite() && px_per_mm > 0.0 && min_px.is_finite() && min_px > 0.0) {
        return FALLBACK_SPACING;
    }
    let need = f64::from(min_px) / f64::from(px_per_mm);
    let decade = 10f64.powf(need.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        let s = m * decade;
        // Small relative slack so exact 1-2-5 values are not skipped by
        // rounding in log10/powf.
        if s >= need * (1.0 - 1.0e-9) {
            let s = s as f32;
            return if s.is_finite() && s > 0.0 {
                s
            } else {
                FALLBACK_SPACING
            };
        }
    }
    (10.0 * decade) as f32
}

/// Next larger step in the 1-2-5 series.
fn next_spacing(s: f32) -> f32 {
    match mantissa(s) {
        1 => s * 2.0,
        2 => s * 2.5,
        _ => s * 2.0,
    }
}

/// 1, 2 or 5 for a 1-2-5 series value.
fn mantissa(s: f32) -> u8 {
    let m = s / 10f32.powf(s.log10().floor());
    if m < 1.5 {
        1
    } else if m < 3.5 {
        2
    } else {
        5
    }
}

/// Lines per major line: majors at 10 mm multiples for 1/2 mantissas
/// (every 10th or 5th line) and at 50 mm multiples for 5 (every 10th).
pub fn major_every(spacing: f32) -> i64 {
    if mantissa(spacing) == 2 { 5 } else { 10 }
}

/// Everything that determines the grid geometry. Lines are rebuilt only when
/// this changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    /// `None` = world XY with the world axes; otherwise an orthonormal plane.
    pub plane: Option<GridPlane>,
    pub spacing: f32,
    /// Centre in plane coordinates, in units of whole major cells.
    pub center: [i64; 2],
    /// Lines on each side of the centre (per direction).
    pub half_lines: i32,
    /// Colours for a light backdrop (darker lines) instead of the dark one.
    pub light: bool,
}

impl GridSpec {
    /// Choose spacing/centre/extent for `camera` and a viewport of
    /// `height` × `height * aspect` screen points. `plane` must already be
    /// orthonormalized.
    pub fn new(plane: Option<GridPlane>, camera: &Camera, height: f32, aspect: f32) -> Self {
        let p = plane.unwrap_or(GridPlane::XY);
        let ppm = crate::camera::pixels_per_mm(camera, height);
        let mut spacing = adaptive_grid_spacing(ppm, MIN_CELL_PX);
        let half_h = camera.distance * (0.5 * camera.fov_y).tan();
        let half_extent = half_h * aspect.clamp(1.0, 10.0) * 1.5;
        let mut half_lines;
        loop {
            let me = major_every(spacing) as f32;
            // Extra major cell absorbs the snapping of the centre.
            let need = (half_extent / spacing).ceil() + me;
            // Quantize so the spec does not change on every zoom tick.
            let need = (need / 10.0).ceil() * 10.0;
            half_lines = need.min(MAX_HALF_LINES as f32) as i32;
            if need <= MAX_HALF_LINES as f32 || !spacing.is_finite() {
                break;
            }
            spacing = next_spacing(spacing);
        }
        let rel = math::sub(camera.target, p.origin);
        let major = spacing * major_every(spacing) as f32;
        let snap = |c: f32| {
            let k = (c / major).round();
            if k.is_finite() { k as i64 } else { 0 }
        };
        Self {
            plane,
            spacing,
            center: [snap(math::dot(rel, p.x)), snap(math::dot(rel, p.y))],
            half_lines: half_lines.max(1),
            light: false,
        }
    }

    /// Grid line vertices and their bounds.
    pub fn build(&self) -> (Vec<LineVertex>, Aabb) {
        let world = self.plane.is_none();
        let p = self.plane.unwrap_or(GridPlane::XY);
        let s = self.spacing;
        let me = major_every(s);
        let n = i64::from(self.half_lines);
        // Centre index in minor cells.
        let cu = self.center[0] * me;
        let cv = self.center[1] * me;
        let (umin, umax) = ((cu - n) as f32 * s, (cu + n) as f32 * s);
        let (vmin, vmax) = ((cv - n) as f32 * s, (cv + n) as f32 * s);

        let (minor, major) = match (world, self.light) {
            (true, false) => ([0.55, 0.57, 0.60, 0.22], [0.65, 0.67, 0.70, 0.42]),
            (false, false) => ([0.60, 0.64, 0.70, 0.30], [0.70, 0.74, 0.80, 0.55]),
            (true, true) => ([0.30, 0.32, 0.36, 0.16], [0.22, 0.24, 0.28, 0.32]),
            (false, true) => ([0.20, 0.30, 0.45, 0.22], [0.12, 0.22, 0.38, 0.42]),
        };
        let mut v = Vec::with_capacity((4 * (2 * n + 1) + 10) as usize);
        for i in -n..=n {
            for (axis_index, center) in [(0, cu), (1, cv)] {
                let gi = center + i;
                if gi == 0 {
                    continue; // the plane axis is drawn below
                }
                let c = if gi % me == 0 { major } else { minor };
                let t = gi as f32 * s;
                let (a, b) = if axis_index == 0 {
                    (p.at(t, vmin), p.at(t, vmax))
                } else {
                    (p.at(umin, t), p.at(umax, t))
                };
                line(&mut v, a, b, c);
            }
        }

        let (red, green, blue, grey) = if self.light {
            (
                [0.82, 0.12, 0.12, 1.0],
                [0.08, 0.55, 0.12, 1.0],
                [0.12, 0.32, 0.85, 1.0],
                [0.30, 0.32, 0.36, 0.40],
            )
        } else {
            (
                [0.90, 0.25, 0.25, 1.0],
                [0.30, 0.85, 0.30, 1.0],
                [0.30, 0.50, 0.95, 1.0],
                [0.55, 0.57, 0.60, 0.35],
            )
        };
        let u_axis_visible = vmin <= 0.0 && vmax >= 0.0;
        let v_axis_visible = umin <= 0.0 && umax >= 0.0;
        let half = n as f32 * s;
        let mut extra = vec![];
        if world {
            // Negative half-axes dim, positive half-axes saturated, Z up.
            if u_axis_visible {
                seg_clipped(&mut v, &p, [umin, 0.0], [umax.min(0.0), 0.0], grey);
                seg_clipped(&mut v, &p, [umin.max(0.0), 0.0], [umax, 0.0], red);
            }
            if v_axis_visible {
                seg_clipped(&mut v, &p, [0.0, vmin], [0.0, vmax.min(0.0)], grey);
                seg_clipped(&mut v, &p, [0.0, vmin.max(0.0)], [0.0, vmax], green);
            }
            if u_axis_visible && v_axis_visible {
                let tip = [0.0, 0.0, half * 0.3];
                line(&mut v, [0.0; 3], tip, blue);
                extra.push(tip);
            }
        } else {
            if u_axis_visible {
                line(&mut v, p.at(umin, 0.0), p.at(umax, 0.0), red);
            }
            if v_axis_visible {
                line(&mut v, p.at(0.0, vmin), p.at(0.0, vmax), green);
            }
        }

        let corners = [
            p.at(umin, vmin),
            p.at(umax, vmin),
            p.at(umin, vmax),
            p.at(umax, vmax),
        ];
        let bounds = Aabb::from_points(corners.into_iter().chain(extra)).unwrap_or(Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        });
        (v, bounds)
    }
}

fn seg_clipped(v: &mut Vec<LineVertex>, p: &GridPlane, a: [f32; 2], b: [f32; 2], c: [f32; 4]) {
    if a[0] < b[0] || a[1] < b[1] {
        line(v, p.at(a[0], a[1]), p.at(b[0], b[1]), c);
    }
}

pub(crate) fn line(v: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    v.push(LineVertex { pos: a, color });
    v.push(LineVertex { pos: b, color });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_spacing_picks_smallest_125_step() {
        // 1 px/mm, need 14 px -> 20 mm.
        assert_eq!(adaptive_grid_spacing(1.0, 14.0), 20.0);
        // 14 px/mm -> exactly 1 mm qualifies.
        assert_eq!(adaptive_grid_spacing(14.0, 14.0), 1.0);
        // 10 px/mm, need 14 px -> 1.4 mm -> 2 mm.
        assert_eq!(adaptive_grid_spacing(10.0, 14.0), 2.0);
        // 4 px/mm -> 3.5 mm -> 5 mm.
        assert_eq!(adaptive_grid_spacing(4.0, 14.0), 5.0);
        // 2 px/mm -> 7 mm -> 10 mm.
        assert_eq!(adaptive_grid_spacing(2.0, 14.0), 10.0);
        // Sub-mm.
        let s = adaptive_grid_spacing(100.0, 14.0);
        assert!((s - 0.2).abs() < 1e-6, "{s}");
        // Invalid input.
        assert_eq!(adaptive_grid_spacing(0.0, 14.0), 10.0);
        assert_eq!(adaptive_grid_spacing(f32::NAN, 14.0), 10.0);
        assert_eq!(adaptive_grid_spacing(1.0, -1.0), 10.0);
    }

    #[test]
    fn adaptive_spacing_cells_are_at_least_min_px_and_minimal() {
        let mut ppm = 0.013f32;
        while ppm < 5000.0 {
            let s = adaptive_grid_spacing(ppm, 14.0);
            assert!(s * ppm >= 14.0 * (1.0 - 1e-5), "ppm {ppm} s {s}");
            // The previous 1-2-5 step would be too small.
            let prev = match mantissa(s) {
                1 => s / 2.0,
                2 => s / 2.0,
                _ => s / 2.5,
            };
            assert!(prev * ppm < 14.0 * (1.0 + 1e-5), "ppm {ppm} s {s}");
            ppm *= 1.37;
        }
    }

    #[test]
    fn major_lines_and_series() {
        assert_eq!(major_every(1.0), 10);
        assert_eq!(major_every(2.0), 5);
        assert_eq!(major_every(0.5), 10);
        assert_eq!(major_every(200.0), 5);
        assert_eq!(next_spacing(1.0), 2.0);
        assert_eq!(next_spacing(2.0), 5.0);
        assert_eq!(next_spacing(5.0), 10.0);
    }

    /// The light backdrop draws the same lines, each darker than on the dark
    /// backdrop (lines must contrast with a light background).
    #[test]
    fn light_backdrop_grid_has_same_lines_in_darker_colours() {
        let dark = GridSpec::new(None, &Camera::default(), 800.0, 1.5);
        let light = GridSpec {
            light: true,
            ..dark
        };
        let ((dv, db), (lv, lb)) = (dark.build(), light.build());
        assert_eq!(db, lb);
        assert_eq!(dv.len(), lv.len());
        let luma = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        for (d, l) in dv.iter().zip(&lv) {
            assert_eq!(d.pos, l.pos);
            assert!(
                luma(l.color) < luma(d.color),
                "{:?} vs {:?}",
                l.color,
                d.color
            );
        }
    }

    #[test]
    fn grid_spec_tracks_zoom_and_caps_line_count() {
        let mut c = Camera::default();
        let near = GridSpec::new(None, &c, 800.0, 1.5);
        c.distance *= 10.0;
        let far = GridSpec::new(None, &c, 800.0, 1.5);
        assert!(far.spacing > near.spacing * 5.0);
        for spec in [near, far] {
            assert!(spec.half_lines <= MAX_HALF_LINES);
            let (v, _) = spec.build();
            // Two vertices per line; grid + up to 5 axis segments.
            assert!(v.len() / 2 <= 400 + 5, "{} lines", v.len() / 2);
        }
        // Huge, wide viewport still caps the line count by coarsening.
        let s = GridSpec::new(None, &Camera::default(), 4000.0, 4.0);
        assert!(s.half_lines <= MAX_HALF_LINES);
        assert!(s.spacing * crate::camera::pixels_per_mm(&Camera::default(), 4000.0) >= 14.0);
    }

    #[test]
    fn grid_covers_visible_area_around_target() {
        let c = Camera {
            target: [1234.0, -567.0, 0.0],
            ..Camera::default()
        };
        let spec = GridSpec::new(None, &c, 700.0, 1.6);
        let (_, b) = spec.build();
        let half_h = c.distance * (0.5 * c.fov_y).tan();
        for k in 0..2 {
            assert!(b.min[k] <= c.target[k] - half_h * 1.6);
            assert!(b.max[k] >= c.target[k] + half_h * 1.6);
        }
        assert_eq!(b.min[2], 0.0);
    }

    #[test]
    fn sketch_plane_grid_lies_in_plane_with_axes() {
        let plane = GridPlane {
            origin: [5.0, 0.0, 30.0],
            x: [0.0, 2.0, 0.0], // unnormalized
            y: [0.0, 1.0, 1.0], // not perpendicular
        }
        .orthonormalized()
        .expect("valid");
        assert!((math::length(plane.y) - 1.0).abs() < 1e-6);
        assert!(math::dot(plane.x, plane.y).abs() < 1e-6);
        let c = Camera {
            target: [5.0, 3.0, 30.0],
            ..Camera::default()
        };
        let spec = GridSpec::new(Some(plane), &c, 600.0, 1.0);
        let (v, _) = spec.build();
        let n = plane.normal();
        for lv in &v {
            let d = math::dot(math::sub(lv.pos, plane.origin), n);
            assert!(d.abs() < 1e-2, "off plane by {d}");
        }
        // Red x axis and green y axis present.
        assert!(v.iter().any(|lv| lv.color == [0.90, 0.25, 0.25, 1.0]));
        assert!(v.iter().any(|lv| lv.color == [0.30, 0.85, 0.30, 1.0]));
    }

    #[test]
    fn degenerate_planes_are_rejected() {
        let bad = [
            GridPlane {
                x: [0.0; 3],
                ..GridPlane::XY
            },
            GridPlane {
                y: [2.0, 0.0, 0.0],
                ..GridPlane::XY
            },
            GridPlane {
                origin: [f32::NAN, 0.0, 0.0],
                ..GridPlane::XY
            },
        ];
        for p in bad {
            assert_eq!(p.orthonormalized(), Err(GridPlaneError::Degenerate));
        }
    }
}
