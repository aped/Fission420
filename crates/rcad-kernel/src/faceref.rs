//! Telling coplanar faces apart by the footprint of the tool that generated
//! them. A semantic face reference first resolves to a plane and an
//! outward normal; when several faces of the body lie in that plane (pattern
//! occurrences with a common floor, walls continuing each other), the face the
//! reference means is the one its own tool's footprint overlaps. This is
//! provenance, not proximity: no nearest-centroid choice is made, and a
//! footprint overlapping two faces (a split cap) stays ambiguous.
//!
//! The overlap is a 2-D polygon intersection (`i_overlay`). Straight edges are
//! exact; curved boundaries are polygonized finely ([`FINE_SAMPLES`] points
//! per arc or curved edge), and an overlap must exceed the polygonization's
//! possible sliver: twice the sagitta of that sampling times the perimeter
//! (with the 64-point region sampling the slack reached tens of
//! mm² on large round footprints and could hide a real second face).
use crate::{Body, Frame, Profile, Result, guarded, tol};

/// Points per arc / curved edge in the overlap test.
pub const FINE_SAMPLES: usize = 2048;

/// Boundary of `profile`, arcs and circles sampled with [`FINE_SAMPLES`].
fn fine_profile_points(profile: &Profile) -> Vec<[f64; 2]> {
    match profile {
        Profile::Polygon(v) => v.clone(),
        Profile::Circle { center, radius } => (0..FINE_SAMPLES)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / FINE_SAMPLES as f64;
                [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
            })
            .collect(),
        Profile::Loop(segs) => {
            let n = segs.len();
            let mut out = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let (p, q) = (s.start(), segs[(i + 1) % n].start());
                match s {
                    crate::LoopSegment::Line { .. } => out.push(p),
                    crate::LoopSegment::Arc { center, ccw, .. } => {
                        let r = (p[0] - center[0]).hypot(p[1] - center[1]);
                        let sweep = crate::arc_signed_sweep(p, q, *center, *ccw);
                        let a0 = (p[1] - center[1]).atan2(p[0] - center[0]);
                        for k in 0..FINE_SAMPLES {
                            let t = a0 + sweep * k as f64 / FINE_SAMPLES as f64;
                            out.push([center[0] + r * t.cos(), center[1] + r * t.sin()]);
                        }
                    }
                    crate::LoopSegment::Cubic { c1, c2, w, .. } => {
                        out.extend(crate::cubic::samples([p, *c1, *c2, q], *w, FINE_SAMPLES));
                    }
                }
            }
            out
        }
    }
}

/// Boundary loops of `face` in `frame` coordinates, curved edges sampled
/// with [`FINE_SAMPLES`] (lines by their start points).
fn fine_face_loops(face: &Face, frame: &Frame) -> Option<Vec<Vec<[f64; 2]>>> {
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    let x = Vector3::new(frame.x[0], frame.x[1], frame.x[2]);
    let y = Vector3::new(frame.y[0], frame.y[1], frame.y[2]);
    let mut loops = Vec::new();
    for w in face.boundaries() {
        let mut pts = Vec::new();
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let n = if matches!(c, Curve::Line(_)) {
                1
            } else {
                FINE_SAMPLES
            };
            for i in 0..n {
                let t = t0 + (t1 - t0) * i as f64 / n as f64;
                let p = match &c {
                    Curve::IntersectionCurve(ic) => ic.search_triple(t)?.0,
                    other => other.subs(t),
                };
                pts.push([(p - o).dot(x), (p - o).dot(y)]);
            }
        }
        loops.push(pts);
    }
    Some(loops)
}
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use truck_modeling::*;

/// Boundary of `profile` as one closed polygon (its 2-D coordinates; arcs and
/// circles sampled with [`FINE_SAMPLES`]), and whether it has curved edges.
pub fn profile_outline(profile: &Profile) -> (Vec<[f64; 2]>, bool) {
    let curved = match profile {
        Profile::Polygon(_) => false,
        Profile::Circle { .. } => true,
        Profile::Loop(segs) => segs
            .iter()
            .any(|s| !matches!(s, crate::LoopSegment::Line { .. })),
    };
    (fine_profile_points(profile), curved)
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
            let Some(loops) = fine_face_loops(f, frame) else {
                continue;
            };
            let face_curved = f
                .boundaries()
                .iter()
                .flat_map(|w| w.edge_iter())
                .any(|e| !matches!(e.oriented_curve(), Curve::Line(_)));
            // Possible sliver between two polygonizations of one curve: each
            // curved edge/arc spans at most a full turn of a circle no larger
            // than the bigger of the two bounding boxes.
            let slack = if curved || face_curved {
                let (flo, fhi) = loops.iter().flatten().fold(
                    ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
                    |(l, h), p| {
                        (
                            [l[0].min(p[0]), l[1].min(p[1])],
                            [h[0].max(p[0]), h[1].max(p[1])],
                        )
                    },
                );
                let r = radius
                    .max(0.5 * (fhi[0] - flo[0]).hypot(fhi[1] - flo[1]))
                    .max(1.0);
                let sag = r * (1.0 - (std::f64::consts::TAU / FINE_SAMPLES as f64 / 2.0).cos());
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
