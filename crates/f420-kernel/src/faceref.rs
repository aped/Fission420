//! Telling coplanar faces apart by the footprint of the tool that generated
//! them (run 5). A semantic face reference first resolves to a plane and an
//! outward normal; when several faces of the body lie in that plane (pattern
//! occurrences with a common floor, walls continuing each other), the face the
//! reference means is the one its own tool's footprint overlaps. This is
//! provenance, not proximity: no nearest-centroid choice is made, and a
//! footprint overlapping two faces (a split cap) stays ambiguous.
//!
//! The overlap is a 2-D polygon intersection (`i_overlay`). Straight edges are
//! exact; curved boundaries are polygonized, so for them an overlap must exceed
//! the polygonization's possible sliver (perimeter × twice the sagitta).
use crate::ops::{REGION_SAMPLES, face_loops_2d, profile_points};
use crate::{Body, Frame, Profile, Result, guarded, tol};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use truck_modeling::*;

/// Boundary of `profile` as one closed polygon (its 2-D coordinates; arcs and
/// circles sampled), and whether it has curved edges.
pub fn profile_outline(profile: &Profile) -> (Vec<[f64; 2]>, bool) {
    let curved = match profile {
        Profile::Polygon(_) => false,
        Profile::Circle { .. } => true,
        Profile::Loop(segs) => segs
            .iter()
            .any(|s| matches!(s, crate::LoopSegment::Arc { .. })),
    };
    (profile_points(profile), curved)
}

fn signed_area(c: &[[f64; 2]]) -> f64 {
    let n = c.len();
    (0..n)
        .map(|i| {
            let (p, q) = (c[i], c[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

/// A point well inside a region given as contours (even-odd): the midpoint of
/// the widest inside span over a few scanlines.
fn interior_point(contours: &[Vec<[f64; 2]>]) -> Option<[f64; 2]> {
    let pts = contours.iter().flatten();
    let (lo, hi) = pts.fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| {
        (l.min(p[1]), h.max(p[1]))
    });
    if hi <= lo || hi.is_nan() || lo.is_nan() {
        return None;
    }
    let mut best: Option<(f64, [f64; 2])> = None;
    for k in 1..16 {
        let y = lo + (hi - lo) * (k as f64 - 0.0137) / 16.0;
        let mut xs: Vec<f64> = contours
            .iter()
            .flat_map(|c| {
                let n = c.len();
                (0..n).filter_map(move |i| {
                    let (a, b) = (c[i], c[(i + 1) % n]);
                    ((a[1] > y) != (b[1] > y))
                        .then(|| a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]))
                })
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let w = pair[1] - pair[0];
            if best.is_none_or(|(bw, _)| w > bw) {
                best = Some((w, [0.5 * (pair[0] + pair[1]), y]));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Number of distinct planar faces of `body` in `frame`'s plane with outward
/// normal `frame.z` whose region overlaps the footprint polygon `outline`
/// (world points on or near the plane, projected) with positive area, and a
/// world point inside the overlap of the last such face (the face's own point
/// when the count is 1). `curved`: the outline samples curves.
pub fn planar_faces_overlapping(
    body: &Body,
    frame: &Frame,
    outline: &[[f64; 3]],
    curved: bool,
) -> Result<(usize, Option<[f64; 3]>)> {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    let (x, y) = (
        Vector3::new(frame.x[0], frame.x[1], frame.x[2]),
        Vector3::new(frame.y[0], frame.y[1], frame.y[2]),
    );
    let foot: Vec<[f64; 2]> = outline
        .iter()
        .map(|p| {
            let d = Point3::new(p[0], p[1], p[2]) - o;
            [d.dot(x), d.dot(y)]
        })
        .collect();
    let foot_area = signed_area(&foot).abs();
    if foot.len() < 3 || foot_area <= 0.0 {
        return Ok((0, None));
    }
    let n = foot.len();
    let perimeter: f64 = (0..n)
        .map(|i| {
            let (a, b) = (foot[i], foot[(i + 1) % n]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum();
    // Largest curve radius bound: half the footprint's bounding diagonal.
    let (lo, hi) = foot
        .iter()
        .fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(l, h), p| {
            (
                [l[0].min(p[0]), l[1].min(p[1])],
                [h[0].max(p[0]), h[1].max(p[1])],
            )
        });
    let radius = 0.5 * (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    guarded("face footprint", || {
        let mut hits = 0;
        let mut inside = None;
        for f in body.solid.face_iter() {
            let Surface::Plane(pl) = f.surface() else {
                continue;
            };
            let mut nrm = pl.normal();
            if !f.orientation() {
                nrm = -nrm;
            }
            if (nrm - z).magnitude() >= tol::FACE_MATCH
                || (pl.origin() - o).dot(z).abs() >= tol::FACE_MATCH
            {
                continue;
            }
            let Some(loops) = face_loops_2d(f, frame) else {
                continue;
            };
            let face_curved = f
                .boundaries()
                .iter()
                .flat_map(|w| w.edge_iter())
                .any(|e| !matches!(e.oriented_curve(), Curve::Line(_)));
            // Possible sliver between two polygonizations of one curve.
            let slack = if curved || face_curved {
                let sag =
                    radius.max(1.0) * (1.0 - (std::f64::consts::PI / REGION_SAMPLES as f64).cos());
                2.0 * sag * perimeter
            } else {
                1.0e-9 * foot_area.max(1.0)
            };
            let shapes =
                vec![foot.clone()].overlay(&loops, OverlayRule::Intersect, FillRule::EvenOdd);
            let area: f64 = shapes
                .iter()
                .flat_map(|s| s.iter())
                .map(|c| signed_area(c))
                .sum::<f64>()
                .abs();
            if area > slack {
                hits += 1;
                let biggest = shapes.iter().max_by(|a, b| {
                    let ar = |s: &Vec<Vec<[f64; 2]>>| {
                        s.iter().map(|c| signed_area(c)).sum::<f64>().abs()
                    };
                    ar(a).total_cmp(&ar(b))
                });
                inside = biggest.and_then(|s| interior_point(s)).map(|p| {
                    let w = o + x * p[0] + y * p[1];
                    [w.x, w.y, w.z]
                });
            }
        }
        (hits, inside)
    })
}
