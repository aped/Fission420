//! Exact sweep of a closed planar profile along a planar path of straight
//! lines and tangent-continuous circular arcs.
//!
//! * The path starts at the profile frame's origin with tangent `frame.z`
//!   (the profile's normal); every segment must leave tangentially.
//! * Frame rule: the profile is carried rigidly: translated along a line,
//!   rotated about the arc's axis along an arc. For a planar path this is the
//!   rotation-minimizing frame (the in-plane axes turn exactly with the
//!   tangent, no twist about it).
//! * Each segment is a Truck sweep of the previous segment's end cap (`tsweep`
//!   for a line, `rsweep` about the arc axis for an arc), so consecutive
//!   segments share that cap's edges exactly and every face is exact (planes,
//!   cylinders and tori from revolved lines and arcs).
//!
//! Refused by name: a path not starting along the frame normal, non-tangent
//! joints, a non-planar path, arcs of a full turn or more, an arc whose radius
//! does not clear the profile on its inner side, a path turning a full turn or
//! coming back within the profile's reach (self-intersection).
use crate::build::{check_single_closed, region_face};
use crate::{
    Body, Frame, KernelError, LoopSegment, Profile, Result, arc_signed_sweep, guarded, measure, tol,
};
use serde::{Deserialize, Serialize};
use truck_modeling::*;

/// One segment of a sweep path, continuing from the end of the previous one
/// (the first starts at the profile frame's origin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PathSegment {
    /// Straight segment to `end` (world mm); its direction must be the current
    /// tangent.
    Line { end: [f64; 3] },
    /// Circular arc from the current point about `center` to `end` (world
    /// mm), leaving along the current tangent (so `start - center` must be
    /// perpendicular to it); the sweep angle is that of the rotation about
    /// the axis `(start - center) × tangent`, in (0, 2π).
    Arc { center: [f64; 3], end: [f64; 3] },
}

fn v3(a: [f64; 3]) -> Vector3 {
    Vector3::new(a[0], a[1], a[2])
}

fn refuse<T>(m: impl Into<String>) -> Result<T> {
    Err(KernelError::InvalidInput(format!("sweep: {}", m.into())))
}

/// A checked segment: translation, or rotation about (center, axis) by angle.
enum Step {
    Line(Vector3),
    Arc {
        center: Vector3,
        axis: Vector3,
        angle: f64,
        radius: f64,
        /// In-profile-plane unit direction from the axis to the path point.
        out2: [f64; 2],
    },
}

/// Sweep `profile` (in `frame`'s 2-D coordinates) along `path`. See the
/// module docs for the path and frame rules.
pub fn sweep(profile: &Profile, frame: &Frame, path: &[PathSegment]) -> Result<Body> {
    frame.validate()?;
    profile.validate()?;
    if path.is_empty() {
        return refuse("the path is empty");
    }
    let (area, centroid) = area_centroid(profile);
    let reach = reach(profile);
    // Walk the path: current point, tangent, and the profile's in-plane axes.
    let (mut p, mut t) = (v3(frame.origin), v3(frame.z));
    let (mut ax, mut ay) = (v3(frame.x), v3(frame.y));
    let mut plane_n: Option<Vector3> = None;
    let mut steps = Vec::with_capacity(path.len());
    let mut turned = 0.0;
    let mut centerline: Vec<(f64, Vector3)> = vec![(0.0, p)];
    let mut s_len = 0.0;
    let joint = |i: usize| {
        if i == 0 {
            "the path must start along the profile frame's normal".to_string()
        } else {
            format!(
                "segment {i} does not continue tangentially from segment {}",
                i - 1
            )
        }
    };
    for (i, seg) in path.iter().enumerate() {
        match seg {
            PathSegment::Line { end } => {
                let d = v3(*end) - p;
                let len = d.magnitude();
                if !len.is_finite() || len < tol::MIN_LENGTH {
                    return refuse(format!(
                        "line segment {i} is shorter than {} mm",
                        tol::MIN_LENGTH
                    ));
                }
                if (d / len - t).magnitude() > 1e-9 {
                    return refuse(joint(i));
                }
                let k = ((len / (0.5 * reach)).ceil() as usize).clamp(1, 10_000);
                for j in 1..=k {
                    centerline.push((
                        s_len + len * j as f64 / k as f64,
                        p + d * (j as f64 / k as f64),
                    ));
                }
                s_len += len;
                steps.push(Step::Line(d));
                p += d;
            }
            PathSegment::Arc { center, end } => {
                let c = v3(*center);
                let r0 = p - c;
                let radius = r0.magnitude();
                if !radius.is_finite() || radius < tol::MIN_LENGTH {
                    return refuse(format!("arc {i} has a degenerate radius"));
                }
                if r0.dot(t).abs() > 1e-9 * radius {
                    return refuse(joint(i));
                }
                let axis = r0.cross(t).normalize();
                let r1 = v3(*end) - c;
                if (r1.magnitude() - radius).abs() > tol::KERNEL_LINEAR {
                    return refuse(format!("arc {i}: its end is not on its circle"));
                }
                if r1.dot(axis).abs() > tol::KERNEL_LINEAR {
                    return refuse(format!(
                        "arc {i}: its end leaves the plane of the arc (non-planar path)"
                    ));
                }
                let mut angle = r0.cross(r1).dot(axis).atan2(r0.dot(r1));
                if angle <= 1e-12 {
                    angle += std::f64::consts::TAU;
                }
                if angle >= std::f64::consts::TAU - 1e-9 {
                    return refuse(format!("arc {i} is a full turn or more"));
                }
                match plane_n {
                    None => plane_n = Some(axis),
                    Some(n) => {
                        if n.cross(axis).magnitude() > 1e-9
                            || (c - v3(frame.origin)).dot(n).abs() > tol::KERNEL_LINEAR
                        {
                            return refuse(format!(
                                "arc {i} is not in the plane of the path (non-planar path)"
                            ));
                        }
                    }
                }
                // The profile must stay on the outer side of the axis.
                let u = r0 / radius;
                let out2 = [u.dot(ax), u.dot(ay)];
                let inward = max_along(profile, [-out2[0], -out2[1]]);
                if inward >= radius - tol::MIN_LENGTH {
                    return refuse(format!(
                        "arc {i}: radius {radius:.6} mm does not clear the profile ({inward:.6} mm \
                         towards the arc's centre): the sweep would intersect itself"
                    ));
                }
                let k = ((radius * angle / (0.5 * reach)).ceil() as usize).clamp(2, 10_000);
                for j in 1..=k {
                    let a = angle * j as f64 / k as f64;
                    centerline.push((s_len + radius * a, c + rotate(r0, axis, a)));
                }
                s_len += radius * angle;
                turned += angle;
                steps.push(Step::Arc {
                    center: c,
                    axis,
                    angle,
                    radius,
                    out2,
                });
                p = c + r1;
                t = rotate(t, axis, angle);
                ax = rotate(ax, axis, angle);
                ay = rotate(ay, axis, angle);
            }
        }
    }
    if turned >= std::f64::consts::TAU - 1e-9 {
        return refuse("the path turns a full turn or more (it would meet itself)");
    }
    // Lines in a path with arcs must stay in the path's plane (they do when
    // tangent, since tangents stay in the plane); a later part of the path
    // coming back within twice the profile's reach would intersect the sweep.
    for (i, &(si, pi)) in centerline.iter().enumerate() {
        for &(sj, pj) in &centerline[i + 1..] {
            if sj - si > (std::f64::consts::PI + 2.0) * reach && (pi - pj).magnitude() < 2.0 * reach
            {
                return refuse(
                    "the path comes back within the profile's reach (the sweep would intersect \
                     itself)",
                );
            }
        }
    }
    let solid = guarded("sweep", || -> Result<Solid> {
        let mut cur = region_face(profile, &[], frame, 0.0)?;
        let mut faces: Vec<Face> = Vec::new();
        let n = steps.len();
        for (k, step) in steps.iter().enumerate() {
            let s: Solid = match step {
                Step::Line(d) => builder::tsweep(&cur, *d),
                Step::Arc {
                    center,
                    axis,
                    angle,
                    ..
                } => builder::rsweep(
                    &cur,
                    Point3::new(center.x, center.y, center.z),
                    *axis,
                    Rad(*angle),
                ),
            };
            let shell = s.into_boundaries().pop().ok_or_else(|| {
                KernelError::Internal("sweep: a segment produced no shell".into())
            })?;
            let m = shell.len();
            // shell[0] = the swept face inverted (the start cap), shell[m-1] =
            // the end cap; the next segment sweeps that very face.
            for (j, f) in shell.into_iter().enumerate() {
                if j == 0 && k > 0 {
                    continue;
                }
                if j == m - 1 {
                    cur = f.clone();
                    if k + 1 < n {
                        continue;
                    }
                }
                faces.push(f);
            }
        }
        Solid::try_new(vec![faces.into()])
            .map_err(|e| KernelError::InvalidResult(format!("sweep: solid: {e:?}")))
    })??;
    let body = Body { solid };
    check_single_closed(&body, "sweep")?;
    // Pappus: each line sweeps area × length; each arc area × angle × the
    // centroid's distance from the axis.
    let want: f64 = steps
        .iter()
        .map(|s| match s {
            Step::Line(d) => area * d.magnitude(),
            Step::Arc {
                angle,
                radius,
                out2,
                ..
            } => area * angle * (radius + centroid[0] * out2[0] + centroid[1] * out2[1]),
        })
        .sum();
    match measure(&body)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => {
            let tolerance = (1e-9 * want).max(1e-6) + 3.0 * error_estimate;
            if (volume - want).abs() > tolerance {
                return Err(KernelError::InvalidResult(format!(
                    "sweep: volume {volume:.6} mm^3 differs from Pappus {want:.6} mm^3"
                )));
            }
        }
        crate::VolumeResult::Unavailable { reason } => {
            return Err(KernelError::InvalidResult(format!(
                "sweep: cannot verify the volume ({reason})"
            )));
        }
    }
    Ok(body)
}

/// `v` rotated by `a` about the unit `axis` (Rodrigues).
fn rotate(v: Vector3, axis: Vector3, a: f64) -> Vector3 {
    v * a.cos() + axis.cross(v) * a.sin() + axis * (axis.dot(v) * (1.0 - a.cos()))
}

/// Largest extent of the profile along the 2-D unit direction `d`.
fn max_along(p: &Profile, d: [f64; 2]) -> f64 {
    let dot = |q: [f64; 2]| q[0] * d[0] + q[1] * d[1];
    match p {
        Profile::Polygon(v) => v.iter().map(|q| dot(*q)).fold(f64::NEG_INFINITY, f64::max),
        Profile::Circle { center, radius } => dot(*center) + radius,
        Profile::Loop(segs) => {
            let n = segs.len();
            let mut m = f64::NEG_INFINITY;
            for (i, s) in segs.iter().enumerate() {
                let a = s.start();
                m = m.max(dot(a));
                if let LoopSegment::Arc { center, ccw, .. } = s {
                    let b = segs[(i + 1) % n].start();
                    let r = (a[0] - center[0]).hypot(a[1] - center[1]);
                    let sweep = arc_signed_sweep(a, b, *center, *ccw);
                    let a0 = (a[1] - center[1]).atan2(a[0] - center[0]);
                    let target = d[1].atan2(d[0]);
                    // Is the direction d within the arc's angular span?
                    let rel = if sweep > 0.0 {
                        (target - a0).rem_euclid(std::f64::consts::TAU)
                    } else {
                        (a0 - target).rem_euclid(std::f64::consts::TAU)
                    };
                    if rel <= sweep.abs() {
                        m = m.max(dot(*center) + r);
                    }
                }
            }
            m
        }
    }
}

/// Largest distance of the profile from the frame origin.
fn reach(p: &Profile) -> f64 {
    (0..64)
        .map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 64.0;
            max_along(p, [a.cos(), a.sin()])
        })
        .fold(0.0, f64::max)
        / (std::f64::consts::PI / 64.0).cos()
}

/// Exact area and centroid of a profile (arcs contribute circular segments).
fn area_centroid(p: &Profile) -> (f64, [f64; 2]) {
    let poly = |v: &[[f64; 2]]| -> (f64, [f64; 2]) {
        let n = v.len();
        let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (p, q) = (v[i], v[(i + 1) % n]);
            let c = p[0] * q[1] - q[0] * p[1];
            a += c;
            cx += (p[0] + q[0]) * c;
            cy += (p[1] + q[1]) * c;
        }
        a /= 2.0;
        (a, [cx / (6.0 * a), cy / (6.0 * a)])
    };
    match p {
        Profile::Polygon(v) => poly(v),
        Profile::Circle { center, radius } => (std::f64::consts::PI * radius * radius, *center),
        Profile::Loop(segs) => {
            let n = segs.len();
            let pts: Vec<[f64; 2]> = segs.iter().map(|s| s.start()).collect();
            let (a0, c0) = poly(&pts);
            let (mut a, mut mx, mut my) = (a0, a0 * c0[0], a0 * c0[1]);
            for (i, s) in segs.iter().enumerate() {
                if let LoopSegment::Arc { center, ccw, .. } = s {
                    let (st, en) = (s.start(), segs[(i + 1) % n].start());
                    let r = (st[0] - center[0]).hypot(st[1] - center[1]);
                    let th = arc_signed_sweep(st, en, *center, *ccw);
                    let seg_a = 0.5 * r * r * (th - th.sin());
                    let ta = th.abs();
                    let dist = 4.0 * r * (ta / 2.0).sin().powi(3) / (3.0 * (ta - ta.sin()));
                    let mid = (st[1] - center[1]).atan2(st[0] - center[0]) + th / 2.0;
                    let c = [center[0] + dist * mid.cos(), center[1] + dist * mid.sin()];
                    a += seg_a;
                    mx += seg_a * c[0];
                    my += seg_a * c[1];
                }
            }
            (a, [mx / a, my / a])
        }
    }
}

#[cfg(test)]
mod tests;
