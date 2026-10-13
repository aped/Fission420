//! Product-limit containment check for circular cuts/pockets in the base
//! extrusion's profile, and sketch-to-kernel profile conversion.
//!
//! Clearance policy: a hole of radius `r` centred at `c` is accepted only when
//! every boundary piece of the host is farther than `r + tol::MIN_LENGTH` from
//! `c` (and, for a line/arc loop, `c` is inside the loop). Polygon hosts keep the
//! run-1 rule (signed distance to each edge's supporting line, which equals the
//! true distance for the convex outlines the product supports and is
//! conservative otherwise).

use crate::{DocError, Result};
use rcad_kernel as k;
use rcad_sketch::{LoopSegment, Profile as SketchProfile};
use std::f64::consts::{PI, TAU};

pub(crate) fn to_kernel(p: &SketchProfile) -> k::Profile {
    match p {
        SketchProfile::Polygon(v) => k::Profile::Polygon(v.clone()),
        SketchProfile::Circle { center, radius } => k::Profile::Circle {
            center: *center,
            radius: *radius,
        },
        SketchProfile::Loop(segs) => k::Profile::Loop(
            segs.iter()
                .map(|s| match *s {
                    LoopSegment::Line { start } => k::LoopSegment::Line { start },
                    LoopSegment::Arc { start, center, ccw } => {
                        k::LoopSegment::Arc { start, center, ccw }
                    }
                })
                .collect(),
        ),
    }
}

fn too_close(radius: f64, clearance: f64) -> DocError {
    DocError::ProductLimit(format!(
        "hole (diameter {:.6} mm) must lie strictly inside the face profile; \
         clearance to an edge is {:.6} mm (initial product limit)",
        2.0 * radius,
        clearance
    ))
}

/// Product limit (not a general CAD rule): a circular cut must lie strictly inside the
/// profile of the extrusion whose face it is sketched on, in that face's frame.
/// For a centred rectangle this means diameter < min(width, height).
pub(crate) fn check_hole_inside(hole: &SketchProfile, host: &SketchProfile) -> Result<()> {
    let SketchProfile::Circle { center, radius } = hole else {
        return Err(DocError::ProductLimit(
            "only a circular cut sketched on a polygonal or line/arc extrusion face is supported"
                .into(),
        ));
    };
    match host {
        SketchProfile::Polygon(poly) => {
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
                let len = ex.hypot(ey);
                // Signed distance of the centre to the edge's supporting line (inside > 0 for CCW).
                let d = (ex * (center[1] - a[1]) - ey * (center[0] - a[0])) / len;
                if d <= *radius + k::tol::MIN_LENGTH {
                    return Err(too_close(*radius, d - radius));
                }
            }
            Ok(())
        }
        SketchProfile::Loop(segs) => {
            let n = segs.len();
            let mut nearest = f64::INFINITY;
            for i in 0..n {
                let (p, q) = (segs[i].start(), segs[(i + 1) % n].start());
                let d = match segs[i] {
                    LoopSegment::Line { .. } => dist_to_segment(*center, p, q),
                    LoopSegment::Arc { center: c, ccw, .. } => dist_to_arc(*center, p, q, c, ccw),
                };
                nearest = nearest.min(d);
            }
            if !point_in_loop(*center, segs) {
                return Err(DocError::ProductLimit(format!(
                    "hole centre ({:.6}, {:.6}) mm lies outside the face profile \
                     (initial product limit)",
                    center[0], center[1]
                )));
            }
            if nearest <= *radius + k::tol::MIN_LENGTH {
                return Err(too_close(*radius, nearest - radius));
            }
            Ok(())
        }
        SketchProfile::Circle { .. } => Err(DocError::ProductLimit(
            "only a circular cut sketched on a polygonal or line/arc extrusion face is supported"
                .into(),
        )),
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Euclidean distance from `x` to the closed segment `a`-`b`.
pub(crate) fn dist_to_segment(x: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let e = sub(b, a);
    let l2 = dot(e, e);
    let t = if l2 > 0.0 {
        (dot(sub(x, a), e) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    norm(sub(x, [a[0] + t * e[0], a[1] + t * e[1]]))
}

/// Counter-clockwise sweep of the arc `p -> q` about `c` (traversed CCW when `ccw`)
/// and its CCW start angle.
fn arc_span(p: [f64; 2], q: [f64; 2], c: [f64; 2], ccw: bool) -> (f64, f64) {
    let (from, to) = if ccw { (p, q) } else { (q, p) };
    let a0 = (from[1] - c[1]).atan2(from[0] - c[0]);
    let a1 = (to[1] - c[1]).atan2(to[0] - c[0]);
    (a0, (a1 - a0).rem_euclid(TAU))
}

/// Euclidean distance from `x` to the arc `p -> q` about `c`: `| |x − c| − r |`
/// when the closest circle point (direction of `x`) lies within the arc's span,
/// else the distance to the nearer endpoint.
pub(crate) fn dist_to_arc(x: [f64; 2], p: [f64; 2], q: [f64; 2], c: [f64; 2], ccw: bool) -> f64 {
    let r = norm(sub(p, c));
    let (a0, sweep) = arc_span(p, q, c, ccw);
    let v = sub(x, c);
    let ends = norm(sub(x, p)).min(norm(sub(x, q)));
    if norm(v) == 0.0 {
        return r; // every circle point is equally close
    }
    let phi = (v[1].atan2(v[0]) - a0).rem_euclid(TAU);
    if phi <= sweep {
        (norm(v) - r).abs().min(ends)
    } else {
        ends
    }
}

/// Winding-number containment for a point not on the boundary. Each line adds
/// the angle its chord subtends at `x`; each arc adds its chord's angle plus
/// ±2π when `x` lies in the region between the arc and its chord (inside the
/// circle and on the arc's side of the chord), the sign being the arc's sense.
pub(crate) fn point_in_loop(x: [f64; 2], segs: &[LoopSegment]) -> bool {
    let n = segs.len();
    let mut w = 0.0;
    for i in 0..n {
        let (p, q) = (segs[i].start(), segs[(i + 1) % n].start());
        let (u, v) = (sub(p, x), sub(q, x));
        w += cross(u, v).atan2(dot(u, v));
        if let LoopSegment::Arc { center: c, ccw, .. } = segs[i] {
            let r = norm(sub(p, c));
            let (a0, sweep) = arc_span(p, q, c, ccw);
            let mid_angle = a0 + 0.5 * sweep;
            let m = [c[0] + r * mid_angle.cos(), c[1] + r * mid_angle.sin()];
            let chord = sub(q, p);
            let same_side = cross(chord, sub(m, p)) * cross(chord, sub(x, p)) > 0.0;
            if norm(sub(x, c)) < r && same_side {
                w += if ccw { TAU } else { -TAU };
            }
        }
    }
    w.abs() > PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_1_SQRT_2;

    /// Rounded rectangle 80 x 50, corner radius 8, CCW from (-32,-25).
    fn rounded() -> SketchProfile {
        let (x, y, r) = (40.0, 25.0, 8.0);
        let arc = |s: [f64; 2], c: [f64; 2]| LoopSegment::Arc {
            start: s,
            center: c,
            ccw: true,
        };
        SketchProfile::Loop(vec![
            LoopSegment::Line {
                start: [-x + r, -y],
            },
            arc([x - r, -y], [x - r, -y + r]),
            LoopSegment::Line { start: [x, -y + r] },
            arc([x, y - r], [x - r, y - r]),
            LoopSegment::Line { start: [x - r, y] },
            arc([-x + r, y], [-x + r, y - r]),
            LoopSegment::Line { start: [-x, y - r] },
            arc([-x, -y + r], [-x + r, -y + r]),
        ])
    }

    fn hole(c: [f64; 2], d: f64) -> SketchProfile {
        SketchProfile::Circle {
            center: c,
            radius: d / 2.0,
        }
    }

    #[test]
    fn distances_to_arc_and_segment() {
        // Quarter arc (8,0) -> (0,8) CCW about the origin.
        let (p, q, c) = ([8.0, 0.0], [0.0, 8.0], [0.0, 0.0]);
        assert!((dist_to_arc([3.0, 4.0], p, q, c, true) - 3.0).abs() < 1e-12);
        assert!((dist_to_arc([20.0, 20.0], p, q, c, true) - (800f64.sqrt() - 8.0)).abs() < 1e-12);
        // Direction outside the span: nearest endpoint (8,0) from (10,-2).
        assert!((dist_to_arc([10.0, -2.0], p, q, c, true) - 8f64.sqrt()).abs() < 1e-12);
        // Same point set traversed clockwise.
        assert!((dist_to_arc([3.0, 4.0], q, p, c, false) - 3.0).abs() < 1e-12);
        assert!((dist_to_segment([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]) - 3.0).abs() < 1e-12);
        assert!((dist_to_segment([13.0, 4.0], [0.0, 0.0], [10.0, 0.0]) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn point_in_rounded_loop_and_concave_arc() {
        let SketchProfile::Loop(segs) = rounded() else {
            unreachable!()
        };
        assert!(point_in_loop([0.0, 0.0], &segs));
        assert!(
            point_in_loop([37.0, 22.0], &segs),
            "inside the TR corner arc"
        );
        // The rectangle corner itself is outside the rounded loop.
        assert!(!point_in_loop([39.5, 24.5], &segs));
        assert!(!point_in_loop([50.0, 0.0], &segs));
        // Square with the top edge replaced by an arc dipping to y = 15 (circle
        // about (10, 27.5), r 12.5), traversed (20,20) -> (0,20) clockwise.
        let concave = vec![
            LoopSegment::Line { start: [0.0, 0.0] },
            LoopSegment::Line { start: [20.0, 0.0] },
            LoopSegment::Arc {
                start: [20.0, 20.0],
                center: [10.0, 27.5],
                ccw: false,
            },
            LoopSegment::Line { start: [0.0, 20.0] },
        ];
        assert!(point_in_loop([10.0, 14.0], &concave));
        assert!(!point_in_loop([10.0, 16.0], &concave), "in the bite");
        // At x = 1 the arc is at y = 27.5 - sqrt(12.5² - 9²) = 18.83: (1,18) is
        // below it (inside), (1,19) above it (in the bite).
        assert!(point_in_loop([1.0, 18.0], &concave));
        assert!(!point_in_loop([1.0, 19.0], &concave));
    }

    #[test]
    fn rounded_host_accepts_clear_holes_and_rejects_corner_breakouts() {
        let host = rounded();
        // End-cap centre, Ø10 and the largest clear sizes.
        check_hole_inside(&hole([0.0, 0.0], 10.0), &host).unwrap();
        check_hole_inside(&hole([30.0, 0.0], 10.0), &host).unwrap();
        // Ø49.99 at the centre: 0.005 mm clearance to the 25 mm half-height.
        check_hole_inside(&hole([0.0, 0.0], 49.99), &host).unwrap();
        assert!(check_hole_inside(&hole([0.0, 0.0], 50.0), &host).is_err());
        // Near the TR corner: centre (34, 19), Ø4 -> radius 2. Clearance to the
        // straight sides is 40-34-2 = 4 and 25-19-2 = 4, i.e. it fits inside the
        // bounding rectangle, but the corner arc (centre (32,17), r 8) is at
        // 8 - |(2,2)| = 8 - 2.83 = 5.17 from the centre: clearance 3.17 > 0. Ok.
        check_hole_inside(&hole([34.0, 19.0], 4.0), &host).unwrap();
        // Centre (37, 22), Ø4: inside the rectangle with 1 mm side clearance,
        // but 8 - |(5,5)| = 0.93 mm from the arc < radius 2 -> breaks out.
        let e = check_hole_inside(&hole([37.0, 22.0], 4.0), &host).unwrap_err();
        assert!(matches!(e, DocError::ProductLimit(_)), "{e}");
        // Centre beyond the arc (outside the loop, inside the rectangle).
        let c = 32.0 + 8.0 * FRAC_1_SQRT_2 + 0.3;
        let c2 = 17.0 + 8.0 * FRAC_1_SQRT_2 + 0.3;
        assert!(check_hole_inside(&hole([c, c2], 0.2), &host).is_err());
        // Polygon behaviour unchanged.
        let rect = SketchProfile::Polygon(vec![
            [-40.0, -25.0],
            [40.0, -25.0],
            [40.0, 25.0],
            [-40.0, 25.0],
        ]);
        check_hole_inside(&hole([37.0, 22.0], 4.0), &rect).unwrap();
        assert!(check_hole_inside(&hole([0.0, 0.0], 50.0), &rect).is_err());
    }

    #[test]
    fn loop_converts_to_kernel_segments() {
        let k::Profile::Loop(v) = to_kernel(&rounded()) else {
            panic!("expected a kernel Loop")
        };
        assert_eq!(v.len(), 8);
        assert!(
            matches!(v[1], k::LoopSegment::Arc { center, ccw: true, .. } if center == [32.0, -17.0])
        );
    }
}
