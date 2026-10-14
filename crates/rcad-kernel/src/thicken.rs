//! Thicken of a single cylindrical surface patch (ADR 0012): the exact
//! solid between the cylinder of radius r and the coaxial cylinder of radius
//! r ± d, over the patch's axial and angular extent, built as the extrusion of
//! an annular sector (or annulus) along the axis. Supported subset: the patch
//! is bounded by circles/arcs perpendicular to the axis and (for a partial
//! patch) two straight rulings. Thickening toward the axis by d ≥ r collapses
//! the offset surface and is refused.
use crate::{Body, DirectFace, Frame, KernelError, LoopSegment, Profile, Result, tol};
use std::f64::consts::TAU;
use truck_modeling::*;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

fn refuse(m: String) -> KernelError {
    KernelError::InvalidInput(format!("thicken: {m}"))
}

pub(crate) fn thicken_curved(body: &Body, distance: f64, reverse: bool) -> Result<Body> {
    let DirectFace::Cylinder {
        origin,
        axis,
        radius,
        hole,
    } = crate::classify_face(body, 0)?
    else {
        return Err(refuse(
            "only planar and cylindrical faces are supported (this face is neither)".into(),
        ));
    };
    let face = body
        .solid
        .face_iter()
        .next()
        .ok_or_else(|| refuse("no face".into()))?;
    let z = unit(axis);
    // Outward normal points away from the axis unless `hole`.
    let away = !hole ^ reverse;
    let (r_in, r_out) = if away {
        (radius, radius + distance)
    } else {
        if distance >= radius - tol::MIN_LENGTH {
            return Err(refuse(format!(
                "thickness {distance} mm toward the axis reaches the axis of the {radius} mm radius cylinder: the offset surface collapses (largest possible {:.6} mm)",
                radius - tol::MIN_LENGTH
            )));
        }
        (radius - distance, radius)
    };
    // Boundary samples: axial and angular extent.
    let x = {
        let a = crate::step_in::least_aligned_axis(Vector3::new(z[0], z[1], z[2]));
        [a.x, a.y, a.z]
    };
    let y = cross(z, x);
    let mut hs = Vec::new();
    let mut lines = Vec::new();
    let mut arc_mids = Vec::new();
    for w in face.boundaries() {
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let pts: Vec<[f64; 3]> = (0..=8)
                .map(|k| c.subs(t0 + (t1 - t0) * k as f64 / 8.0).into())
                .collect();
            let h: Vec<f64> = pts.iter().map(|p| dot(sub(*p, origin), z)).collect();
            hs.extend(h.iter().copied());
            let ang = |p: [f64; 3]| {
                let d = sub(p, origin);
                dot(d, y).atan2(dot(d, x))
            };
            if (h[0] - h[8]).abs() > tol::MIN_LENGTH {
                // A ruling: must be straight along the axis.
                let a0 = ang(pts[0]);
                if pts.iter().any(|p| {
                    let d = (ang(*p) - a0).rem_euclid(TAU);
                    d.min(TAU - d) * radius > tol::MIN_LENGTH
                }) {
                    return Err(refuse(
                        "the patch has a boundary edge that is neither a ruling nor a circle (unsupported trimmed cylinder)".into(),
                    ));
                }
                lines.push(a0);
            } else {
                if h.iter().any(|v| (v - h[0]).abs() > tol::MIN_LENGTH) {
                    return Err(refuse("unsupported boundary curve on the cylinder".into()));
                }
                arc_mids.push((h[0], ang(pts[4]), ang(pts[0]), ang(pts[8])));
            }
        }
    }
    let h0 = hs.iter().copied().fold(f64::INFINITY, f64::min);
    let h1 = hs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if (h1 - h0).is_nan() || h1 - h0 <= tol::MIN_LENGTH {
        return Err(refuse("the cylinder patch has no length".into()));
    }
    for (h, ..) in &arc_mids {
        if (h - h0).abs() > tol::MIN_LENGTH && (h - h1).abs() > tol::MIN_LENGTH {
            return Err(refuse(
                "the patch ends are not perpendicular to the axis (unsupported trim)".into(),
            ));
        }
    }
    let base = [
        origin[0] + z[0] * h0,
        origin[1] + z[1] * h0,
        origin[2] + z[2] * h0,
    ];
    let frame = Frame {
        origin: base,
        x,
        y,
        z,
    };
    let solid = match lines.as_slice() {
        [] => crate::extrude_region(
            &Profile::Circle {
                center: [0.0, 0.0],
                radius: r_out,
            },
            &[Profile::Circle {
                center: [0.0, 0.0],
                radius: r_in,
            }],
            &frame,
            h1 - h0,
        )?,
        [la, lb] => {
            // Span from a0 counter-clockwise to a1 containing an arc midpoint.
            let mid = arc_mids
                .first()
                .map(|m| m.1)
                .ok_or_else(|| refuse("partial patch without arc edges".into()))?;
            let ccw_contains =
                |a: f64, b: f64, m: f64| (m - a).rem_euclid(TAU) < (b - a).rem_euclid(TAU);
            let (a0, a1) = if ccw_contains(*la, *lb, mid) {
                (*la, *lb)
            } else {
                (*lb, *la)
            };
            let p = |r: f64, a: f64| [r * a.cos(), r * a.sin()];
            crate::extrude_region(
                &Profile::Loop(vec![
                    LoopSegment::Arc {
                        start: p(r_out, a0),
                        center: [0.0, 0.0],
                        ccw: true,
                    },
                    LoopSegment::Line {
                        start: p(r_out, a1),
                    },
                    LoopSegment::Arc {
                        start: p(r_in, a1),
                        center: [0.0, 0.0],
                        ccw: false,
                    },
                    LoopSegment::Line { start: p(r_in, a0) },
                ]),
                &[],
                &frame,
                h1 - h0,
            )?
        }
        other => {
            return Err(refuse(format!(
                "the cylinder patch has {} straight boundary rulings (supported: 0 or 2)",
                other.len()
            )));
        }
    };
    Ok(solid)
}
