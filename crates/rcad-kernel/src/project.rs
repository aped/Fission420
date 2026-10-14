//! Exact boundary curves of a planar face for sketch projection
//! (project/include model edges).
//!
//! The face in `frame`'s plane (exactly one face of the body may lie there)
//! is walked loop by loop; every edge is returned as a line, a circular arc
//! or a full circle in `frame`'s 2-D coordinates. A curved edge is accepted as
//! an arc only when samples along it lie on one circle within
//! [`tol::KERNEL_LINEAR`](crate::tol::KERNEL_LINEAR) (the rational quadratic
//! arcs the kernel builds are exact circles); anything else (an intersection
//! curve, a spline) is refused by name. No polygonization.
use crate::ops::coplanar_faces;
use crate::{Body, Frame, KernelError, Result, guarded, tol};
use truck_modeling::*;

/// A boundary curve in a frame's 2-D coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve2 {
    Line {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// Arc from `a` to `b` about `c`, counter-clockwise when `ccw`.
    Arc {
        c: [f64; 2],
        a: [f64; 2],
        b: [f64; 2],
        ccw: bool,
    },
    Circle {
        c: [f64; 2],
        r: f64,
    },
    /// Full ellipse: centre, end point of one axis, the other semi-axis
    /// (the image of a circle under an oblique projection; never produced
    /// from a face's own plane).
    Ellipse {
        c: [f64; 2],
        major: [f64; 2],
        minor: f64,
    },
}

/// Samples along a curved edge used to recognise a circle.
const SAMPLES: usize = 16;

fn circle3(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> Option<([f64; 2], f64)> {
    let d = 2.0 * (p[0] * (q[1] - r[1]) + q[0] * (r[1] - p[1]) + r[0] * (p[1] - q[1]));
    // Collinear within a relative 1e-9: d is twice the
    // signed triangle area; compare it with the product of the side lengths.
    let span = (q[0] - p[0]).hypot(q[1] - p[1]) * (r[0] - p[0]).hypot(r[1] - p[1]);
    if d.abs() <= 1e-9 * span {
        return None;
    }
    let s = |v: [f64; 2]| v[0] * v[0] + v[1] * v[1];
    let ux = (s(p) * (q[1] - r[1]) + s(q) * (r[1] - p[1]) + s(r) * (p[1] - q[1])) / d;
    let uy = (s(p) * (r[0] - q[0]) + s(q) * (p[0] - r[0]) + s(r) * (q[0] - p[0])) / d;
    let c = [ux, uy];
    Some((c, (p[0] - ux).hypot(p[1] - uy)))
}

/// Boundary loops (outer first, by area) of the single planar face of `body`
/// lying in `frame`'s plane, as exact curves in `frame` coordinates.
pub fn face_boundary_curves(body: &Body, frame: &Frame) -> Result<Vec<Vec<Curve2>>> {
    frame.validate()?;
    let faces = coplanar_faces(body, frame);
    let face = match faces.as_slice() {
        [f] => f.clone(),
        [] => {
            return Err(KernelError::InvalidInput(
                "projection: no face of the body lies in that plane".into(),
            ));
        }
        _ => {
            return Err(KernelError::InvalidInput(format!(
                "projection: {} faces lie in that plane; the face to project is ambiguous",
                faces.len()
            )));
        }
    };
    let mut loops = face_loops(&face, frame)?;
    loops.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(loops.into_iter().map(|(_, l)| l).collect())
}

/// Section of `body` by `frame`'s plane: the boundary loops of every face the
/// plane cuts through the body, as exact lines, arcs and circles in `frame`
/// coordinates (loops sorted by enclosed area, largest first). Built by
/// cutting away the material in front of the plane (the exact Truck boolean,
/// intersection edges rebuilt as verified exact curves) and reading the cap
/// faces left in the plane, so plane ∩ planar faces gives lines, plane ∩
/// cylinders / cones about axes normal to the plane circles or arcs, plane ∩
/// a cylinder parallel to it lines. Any other section curve (an oblique
/// cylinder's ellipse, a free-form surface) is refused by name. Refused too:
/// a plane holding a face of the body (project that face) and a plane that
/// does not cross the body.
pub fn section_curves(body: &Body, frame: &Frame) -> Result<Vec<Vec<Curve2>>> {
    frame.validate()?;
    if !coplanar_faces(body, frame).is_empty() {
        return Err(KernelError::InvalidInput(
            "section: the plane contains a face of the body (project that face instead)".into(),
        ));
    }
    let b = crate::measure::raw_bounds(&body.solid)?;
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    let (x, y, z) = (
        Vector3::new(frame.x[0], frame.x[1], frame.x[2]),
        Vector3::new(frame.y[0], frame.y[1], frame.y[2]),
        Vector3::new(frame.z[0], frame.z[1], frame.z[2]),
    );
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for cx in [b.min[0], b.max[0]] {
        for cy in [b.min[1], b.max[1]] {
            for cz in [b.min[2], b.max[2]] {
                let d = Point3::new(cx, cy, cz) - o;
                for (i, v) in [d.dot(x), d.dot(y), d.dot(z)].into_iter().enumerate() {
                    lo[i] = lo[i].min(v);
                    hi[i] = hi[i].max(v);
                }
            }
        }
    }
    if !(lo[2] < -tol::MIN_LENGTH && hi[2] > tol::MIN_LENGTH) {
        return Err(KernelError::InvalidInput(
            "section: the plane does not cross the body".into(),
        ));
    }
    let m = 1.0 + 0.05 * (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max);
    let cutter = crate::Profile::Polygon(vec![
        [lo[0] - m, lo[1] - m],
        [hi[0] + m, lo[1] - m],
        [hi[0] + m, hi[1] + m],
        [lo[0] - m, hi[1] + m],
    ]);
    let kept = crate::cut_region_pieces(body, &cutter, &[], frame, hi[2] + m)
        .map_err(|e| KernelError::InvalidInput(format!("section: cutting the body failed: {e}")))?;
    let mut loops = Vec::new();
    for f in coplanar_faces(&kept, frame) {
        loops.extend(face_loops(&f, frame)?);
    }
    if loops.is_empty() {
        return Err(KernelError::InvalidInput(
            "section: the plane does not cross the body".into(),
        ));
    }
    loops.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(loops.into_iter().map(|(_, l)| l).collect())
}

/// Boundary loops of one planar face as exact curves in `frame` coordinates,
/// each with its enclosed area (unsorted).
fn face_loops(face: &Face, frame: &Frame) -> Result<Vec<(f64, Vec<Curve2>)>> {
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    let x = Vector3::new(frame.x[0], frame.x[1], frame.x[2]);
    let y = Vector3::new(frame.y[0], frame.y[1], frame.y[2]);
    let loc = |p: Point3| [(p - o).dot(x), (p - o).dot(y)];
    let mut loops: Vec<(f64, Vec<Curve2>)> = Vec::new();
    for w in face.boundaries() {
        let mut out = Vec::new();
        let mut poly = Vec::new();
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let pts: Vec<[f64; 2]> = guarded("projection sampling", || {
                (0..=SAMPLES)
                    .map(|i| loc(c.subs(t0 + (t1 - t0) * i as f64 / SAMPLES as f64)))
                    .collect::<Vec<_>>()
            })?;
            poly.extend_from_slice(&pts[..SAMPLES]);
            let (a, b) = (pts[0], pts[SAMPLES]);
            if matches!(c, Curve::Line(_)) {
                out.push(Curve2::Line { a, b });
                continue;
            }
            let closed = (a[0] - b[0]).hypot(a[1] - b[1]) <= tol::KERNEL_LINEAR;
            let (p, q, r) = if closed {
                (pts[0], pts[SAMPLES / 3], pts[2 * SAMPLES / 3])
            } else {
                (a, pts[SAMPLES / 2], b)
            };
            let Some((cc, rad)) = circle3(p, q, r) else {
                // Collinear samples of a curved representation: a line.
                if pts.iter().all(|s| {
                    let (u, v) = ([b[0] - a[0], b[1] - a[1]], [s[0] - a[0], s[1] - a[1]]);
                    (u[0] * v[1] - u[1] * v[0]).abs() / u[0].hypot(u[1]).max(1e-300)
                        <= tol::KERNEL_LINEAR
                }) {
                    out.push(Curve2::Line { a, b });
                    continue;
                }
                return Err(KernelError::InvalidInput(
                    "projection: an edge of the face is neither a line nor a circular arc".into(),
                ));
            };
            if pts
                .iter()
                .any(|s| ((s[0] - cc[0]).hypot(s[1] - cc[1]) - rad).abs() > tol::KERNEL_LINEAR)
            {
                return Err(KernelError::InvalidInput(
                    "projection: an edge of the face is neither a line nor a circular arc \
                     (only lines and arcs are projected exactly)"
                        .into(),
                ));
            }
            if closed {
                out.push(Curve2::Circle { c: cc, r: rad });
            } else {
                // Direction of travel through the middle sample.
                let m = pts[SAMPLES / 2];
                let cross = (a[0] - cc[0]) * (m[1] - cc[1]) - (a[1] - cc[1]) * (m[0] - cc[0]);
                out.push(Curve2::Arc {
                    c: cc,
                    a,
                    b,
                    ccw: cross > 0.0,
                });
            }
        }
        let n = poly.len();
        let area: f64 = (0..n)
            .map(|i| {
                let (p, q) = (poly[i], poly[(i + 1) % n]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f64>()
            / 2.0;
        loops.push((area.abs(), out));
    }
    Ok(loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Profile, cut_prism, extrude};

    #[test]
    fn top_face_of_a_holed_block() {
        let b = extrude(
            &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
            &Frame::XY,
            10.0,
        )
        .unwrap();
        let b = cut_prism(
            &b,
            &Profile::Circle {
                center: [20.0, 15.0],
                radius: 4.0,
            },
            &Frame::XY.offset(-1.0),
            12.0,
        )
        .unwrap();
        let loops = face_boundary_curves(&b, &Frame::XY.offset(10.0)).unwrap();
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].len(), 4);
        assert!(loops[0].iter().all(|c| matches!(c, Curve2::Line { .. })));
        let circ: Vec<_> = loops[1]
            .iter()
            .map(|c| match c {
                Curve2::Circle { c, r } => (*c, *r),
                Curve2::Arc { c, a, .. } => (*c, (a[0] - c[0]).hypot(a[1] - c[1])),
                l => panic!("{l:?}"),
            })
            .collect();
        for (c, r) in circ {
            assert!(
                (c[0] - 20.0).abs() < 1e-9 && (c[1] - 15.0).abs() < 1e-9 && (r - 4.0).abs() < 1e-9
            );
        }
        // No face in an empty plane.
        assert!(face_boundary_curves(&b, &Frame::XY.offset(5.0)).is_err());
    }
}
