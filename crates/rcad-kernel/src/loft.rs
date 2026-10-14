//! Exact loft between two planar closed profiles in parallel planes, as a new
//! body (correspondence rule in ADR 0008).
//!
//! Supported (anything else is refused by name):
//! * two straight-edged loops (polygons, or line-only loops) with the same
//!   vertex count whose corresponding edges are parallel: every side face is
//!   an exact planar quad (a frustum of a pyramid, a tapered tray blank);
//! * two coaxial circles: an exact cone frustum (a cylinder for equal radii),
//!   the line between the rims revolved about the axis, capped by disks.
//!
//! Correspondence: both loops counter-clockwise about the loft direction (from
//! `a`'s plane to `b`'s); `b` rotated so that its vertex 0 is the one closest,
//! in the shared in-plane frame, to `a`'s vertex 0 (a tie is refused); vertex
//! i of `a` joins vertex i of `b`.
use crate::build::check_single_closed;
use crate::{Body, Frame, KernelError, LoopSegment, Profile, Result, guarded, measure, tol};
use truck_modeling::*;

fn refuse<T>(m: impl Into<String>) -> Result<T> {
    Err(KernelError::InvalidInput(format!("loft: {}", m.into())))
}

/// Loft from profile `a` (in `fa`'s 2-D coordinates) to profile `b` (in
/// `fb`'s 2-D coordinates). The planes must be parallel and distinct; the
/// body spans the space between them.
pub fn loft(a: &Profile, fa: &Frame, b: &Profile, fb: &Frame) -> Result<Body> {
    fa.validate()?;
    fb.validate()?;
    a.validate()?;
    b.validate()?;
    let z = Vector3::new(fa.z[0], fa.z[1], fa.z[2]);
    let zb = Vector3::new(fb.z[0], fb.z[1], fb.z[2]);
    if z.cross(zb).magnitude() > 1e-9 {
        return refuse("the profile planes are not parallel (only parallel planes are supported)");
    }
    let oa = Vector3::new(fa.origin[0], fa.origin[1], fa.origin[2]);
    let ob = Vector3::new(fb.origin[0], fb.origin[1], fb.origin[2]);
    let h0 = (ob - oa).dot(z);
    if h0.abs() < tol::MIN_LENGTH {
        return refuse("the two profiles lie in the same plane");
    }
    // Shared frame F: a's plane, normal towards b (right-handed).
    let mut f = *fa;
    if h0 < 0.0 {
        f.z = [-fa.z[0], -fa.z[1], -fa.z[2]];
        f.y = [-fa.y[0], -fa.y[1], -fa.y[2]];
    }
    let h = h0.abs();
    let (fx, fy) = (
        Vector3::new(f.x[0], f.x[1], f.x[2]),
        Vector3::new(f.y[0], f.y[1], f.y[2]),
    );
    // A point of `frame`'s 2-D coordinates in F's 2-D coordinates.
    let to_f = |frame: &Frame, p: [f64; 2]| {
        let w = frame.to_world(p);
        let d = Vector3::new(w[0], w[1], w[2]) - oa;
        [d.dot(fx), d.dot(fy)]
    };
    let world = |p: [f64; 2], w: f64| {
        let q = f.to_world(p);
        Point3::new(q[0] + w * f.z[0], q[1] + w * f.z[1], q[2] + w * f.z[2])
    };
    match (straight(a), straight(b), a, b) {
        (Some(pa), Some(pb), _, _) => {
            let pa: Vec<[f64; 2]> = pa.iter().map(|&p| to_f(fa, p)).collect();
            let pb: Vec<[f64; 2]> = pb.iter().map(|&p| to_f(fb, p)).collect();
            polygon_loft(pa, pb, h, &world)
        }
        (
            _,
            _,
            Profile::Circle {
                center: ca,
                radius: ra,
            },
            Profile::Circle {
                center: cb,
                radius: rb,
            },
        ) => {
            let (ca, cb) = (to_f(fa, *ca), to_f(fb, *cb));
            if (ca[0] - cb[0]).hypot(ca[1] - cb[1]) > tol::KERNEL_LINEAR {
                return refuse("the circles are not coaxial (an oblique cone is not supported)");
            }
            circle_loft(ca, *ra, *rb, h, &world, &f)
        }
        _ => refuse(
            "both profiles must be straight-edged loops or both circles (arcs in a loop, or a \
             circle with a polygon, are not supported)",
        ),
    }
}

/// The vertices of a straight-edged profile (polygon or line-only loop).
fn straight(p: &Profile) -> Option<Vec<[f64; 2]>> {
    match p {
        Profile::Polygon(v) => Some(v.clone()),
        Profile::Loop(segs) => segs
            .iter()
            .map(|s| match s {
                LoopSegment::Line { start } => Some(*start),
                LoopSegment::Arc { .. } => None,
            })
            .collect(),
        Profile::Circle { .. } => None,
    }
}

fn signed_area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1])
        .sum::<f64>()
        / 2.0
}

/// Whether the closed polygon is simple (no two non-adjacent edges meet).
fn simple(p: &[[f64; 2]]) -> bool {
    let n = p.len();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let (a, b, c, d) = (p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]);
            let (d1, d2) = (cross(a, b, c), cross(a, b, d));
            let (d3, d4) = (cross(c, d, a), cross(c, d, b));
            if d1 * d2 <= 0.0 && d3 * d4 <= 0.0 {
                return false;
            }
        }
    }
    true
}

fn polygon_loft(
    mut pa: Vec<[f64; 2]>,
    mut pb: Vec<[f64; 2]>,
    h: f64,
    world: &dyn Fn([f64; 2], f64) -> Point3,
) -> Result<Body> {
    let n = pa.len();
    if pb.len() != n {
        return refuse(format!(
            "the profiles have different vertex counts ({n} and {})",
            pb.len()
        ));
    }
    if !simple(&pa) || !simple(&pb) {
        return refuse("a profile is self-intersecting");
    }
    // Counter-clockwise about the loft direction.
    if signed_area(&pa) < 0.0 {
        pa.reverse();
    }
    if signed_area(&pb) < 0.0 {
        pb.reverse();
    }
    // b's vertex 0: the one closest to a's vertex 0; a tie is ambiguous.
    let d: Vec<f64> = pb
        .iter()
        .map(|q| (q[0] - pa[0][0]).hypot(q[1] - pa[0][1]))
        .collect();
    let k = (0..n).min_by(|&i, &j| d[i].total_cmp(&d[j])).unwrap_or(0);
    let scale = pa
        .iter()
        .chain(&pb)
        .map(|p| p[0].abs().max(p[1].abs()))
        .fold(1.0, f64::max);
    if (0..n).any(|i| i != k && (d[i] - d[k]).abs() <= 1e-9 * scale) {
        return refuse(
            "the vertex correspondence is ambiguous: two vertices of the second profile are \
             equally close to the first profile's vertex 0",
        );
    }
    pb.rotate_left(k);
    // Corresponding edges must be parallel and alike in direction: then each
    // side is a planar quad. (Non-parallel edges would need a bilinear side.)
    for i in 0..n {
        let (a0, a1, b0, b1) = (pa[i], pa[(i + 1) % n], pb[i], pb[(i + 1) % n]);
        let (ea, eb) = (
            [a1[0] - a0[0], a1[1] - a0[1]],
            [b1[0] - b0[0], b1[1] - b0[1]],
        );
        let (la, lb) = (ea[0].hypot(ea[1]), eb[0].hypot(eb[1]));
        let cr = (ea[0] * eb[1] - ea[1] * eb[0]) / (la * lb);
        let dt = (ea[0] * eb[0] + ea[1] * eb[1]) / (la * lb);
        if cr.abs() > 1e-9 || dt <= 0.0 {
            return refuse(format!(
                "corresponding edges {i} are not parallel: the side face would be a twisted \
                 (non-planar) surface, which is not supported"
            ));
        }
    }
    // Every intermediate section must stay simple (no twisted correspondence).
    let section = |t: f64| -> Vec<[f64; 2]> {
        (0..n)
            .map(|i| {
                [
                    (1.0 - t) * pa[i][0] + t * pb[i][0],
                    (1.0 - t) * pa[i][1] + t * pb[i][1],
                ]
            })
            .collect()
    };
    for t in [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875] {
        let s = section(t);
        if !simple(&s) || signed_area(&s) <= 0.0 {
            return refuse("the side faces would cross each other (twisted correspondence)");
        }
    }
    let solid = guarded("loft", || -> Result<Solid> {
        let va: Vec<Vertex> = pa.iter().map(|p| builder::vertex(world(*p, 0.0))).collect();
        let vb: Vec<Vertex> = pb.iter().map(|p| builder::vertex(world(*p, h))).collect();
        let ea: Vec<Edge> = (0..n)
            .map(|i| builder::line(&va[i], &va[(i + 1) % n]))
            .collect();
        let eb: Vec<Edge> = (0..n)
            .map(|i| builder::line(&vb[i], &vb[(i + 1) % n]))
            .collect();
        let ev: Vec<Edge> = (0..n).map(|i| builder::line(&va[i], &vb[i])).collect();
        let pt = |v: &Vertex| v.point();
        let mut faces = Vec::with_capacity(n + 2);
        // Bottom (outward -normal): a's loop reversed.
        let bottom: Wire = ea.iter().rev().map(|e| e.inverse()).collect();
        let (o, ox, oy) = (
            world([0.0, 0.0], 0.0),
            world([1.0, 0.0], 0.0),
            world([0.0, 1.0], 0.0),
        );
        faces.push(face(bottom, Plane::new(o, oy, ox))?);
        // Top (outward +normal): b's loop.
        let top: Wire = eb.iter().cloned().collect();
        let (o, ox, oy) = (
            world([0.0, 0.0], h),
            world([1.0, 0.0], h),
            world([0.0, 1.0], h),
        );
        faces.push(face(top, Plane::new(o, ox, oy))?);
        // Sides: a_i -> a_i+1 -> b_i+1 -> b_i (outward for a CCW loop).
        for i in 0..n {
            let j = (i + 1) % n;
            let w: Wire = vec![
                ea[i].clone(),
                ev[j].clone(),
                eb[i].inverse(),
                ev[i].inverse(),
            ]
            .into();
            faces.push(face(w, Plane::new(pt(&va[i]), pt(&va[j]), pt(&vb[i])))?);
        }
        let shell: Shell = faces.into();
        Solid::try_new(vec![shell])
            .map_err(|e| KernelError::InvalidResult(format!("loft: solid: {e:?}")))
    })??;
    let body = Body { solid };
    check_single_closed(&body, "loft")?;
    // Prismatoid: the section area is quadratic in the height, so Simpson's
    // rule is exact: V = h/6 (A1 + 4 Am + A2).
    let want = h / 6.0 * (signed_area(&pa) + 4.0 * signed_area(&section(0.5)) + signed_area(&pb));
    check_volume(&body, want, "prismatoid")?;
    Ok(body)
}

/// Loft through several straight-edged profiles on parallel planes,
/// in order: each consecutive pair obeys the
/// two-profile rules (same vertex count, corresponding edges parallel, vertex
/// 0 nearest to the previous section's vertex 0) and the sections must
/// advance monotonically along the normal. Built as one exact shell: planar
/// side quads per segment sharing the intermediate rings, caps at both ends.
/// Volume checked against the sum of the segments' prismatoid volumes.
pub fn loft_sections(sections: &[(Profile, Frame)]) -> Result<Body> {
    if sections.len() == 2 {
        return loft(
            &sections[0].0,
            &sections[0].1,
            &sections[1].0,
            &sections[1].1,
        );
    }
    if sections.len() < 2 {
        return refuse("a loft needs at least two profiles");
    }
    let (p0, f0) = (&sections[0].0, &sections[0].1);
    f0.validate()?;
    let z0 = Vector3::new(f0.z[0], f0.z[1], f0.z[2]);
    let o0 = Vector3::new(f0.origin[0], f0.origin[1], f0.origin[2]);
    let hlast = {
        let fl = &sections[sections.len() - 1].1;
        (Vector3::new(fl.origin[0], fl.origin[1], fl.origin[2]) - o0).dot(z0)
    };
    let mut f = *f0;
    if hlast < 0.0 {
        f.z = [-f0.z[0], -f0.z[1], -f0.z[2]];
        f.y = [-f0.y[0], -f0.y[1], -f0.y[2]];
    }
    let fz = Vector3::new(f.z[0], f.z[1], f.z[2]);
    let (fx, fy) = (
        Vector3::new(f.x[0], f.x[1], f.x[2]),
        Vector3::new(f.y[0], f.y[1], f.y[2]),
    );
    let world = |p: [f64; 2], w: f64| {
        let q = f.to_world(p);
        Point3::new(q[0] + w * f.z[0], q[1] + w * f.z[1], q[2] + w * f.z[2])
    };
    let _ = p0;
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut hs: Vec<f64> = Vec::new();
    for (k, (p, fr)) in sections.iter().enumerate() {
        fr.validate()?;
        p.validate()?;
        let zk = Vector3::new(fr.z[0], fr.z[1], fr.z[2]);
        if zk.cross(fz).magnitude() > 1e-9 {
            return refuse("the profile planes are not parallel");
        }
        let ok = Vector3::new(fr.origin[0], fr.origin[1], fr.origin[2]);
        let h = (ok - o0).dot(fz);
        if k > 0 && h <= hs[k - 1] + tol::MIN_LENGTH {
            return refuse("the sections must advance along the normal in order (no repeats)");
        }
        let Some(pts) = straight(p) else {
            return refuse("sections with arcs or circles are not supported for 3+ profiles");
        };
        let mut pts: Vec<[f64; 2]> = pts
            .iter()
            .map(|&q| {
                let w = fr.to_world(q);
                let d = Vector3::new(w[0], w[1], w[2]) - o0;
                [d.dot(fx), d.dot(fy)]
            })
            .collect();
        if !simple(&pts) {
            return refuse("a profile is self-intersecting");
        }
        if signed_area(&pts) < 0.0 {
            pts.reverse();
        }
        if let Some(prev) = rings.last() {
            let n = prev.len();
            if pts.len() != n {
                return refuse(format!(
                    "section {} has {} vertices, the previous one {n}",
                    k + 1,
                    pts.len()
                ));
            }
            let d: Vec<f64> = pts
                .iter()
                .map(|q| (q[0] - prev[0][0]).hypot(q[1] - prev[0][1]))
                .collect();
            let m = (0..n).min_by(|&i, &j| d[i].total_cmp(&d[j])).unwrap_or(0);
            if (0..n).any(|i| i != m && (d[i] - d[m]).abs() <= 1e-9 * d[m].max(1.0)) {
                return refuse("the vertex correspondence is ambiguous between two sections");
            }
            pts.rotate_left(m);
            for i in 0..n {
                let (a0, a1, b0, b1) = (prev[i], prev[(i + 1) % n], pts[i], pts[(i + 1) % n]);
                let (ea, eb) = (
                    [a1[0] - a0[0], a1[1] - a0[1]],
                    [b1[0] - b0[0], b1[1] - b0[1]],
                );
                let (la, lb) = (ea[0].hypot(ea[1]), eb[0].hypot(eb[1]));
                let cr = (ea[0] * eb[1] - ea[1] * eb[0]) / (la * lb);
                let dt = (ea[0] * eb[0] + ea[1] * eb[1]) / (la * lb);
                if cr.abs() > 1e-9 || dt <= 0.0 {
                    return refuse(format!(
                        "between sections {} and {}, corresponding edges {i} are not parallel \
                         (a twisted side is not supported)",
                        k,
                        k + 1
                    ));
                }
            }
        }
        rings.push(pts);
        hs.push(h);
    }
    let n = rings[0].len();
    let m = rings.len();
    let solid = guarded("loft", || -> Result<Solid> {
        let v: Vec<Vec<Vertex>> = (0..m)
            .map(|k| {
                rings[k]
                    .iter()
                    .map(|p| builder::vertex(world(*p, hs[k])))
                    .collect()
            })
            .collect();
        let ring_e: Vec<Vec<Edge>> = (0..m)
            .map(|k| {
                (0..n)
                    .map(|i| builder::line(&v[k][i], &v[k][(i + 1) % n]))
                    .collect()
            })
            .collect();
        let up: Vec<Vec<Edge>> = (0..m - 1)
            .map(|k| {
                (0..n)
                    .map(|i| builder::line(&v[k][i], &v[k + 1][i]))
                    .collect()
            })
            .collect();
        let mut faces = Vec::new();
        let bottom: Wire = ring_e[0].iter().rev().map(|e| e.inverse()).collect();
        faces.push(face(
            bottom,
            Plane::new(
                world([0.0, 0.0], hs[0]),
                world([0.0, 1.0], hs[0]),
                world([1.0, 0.0], hs[0]),
            ),
        )?);
        let top: Wire = ring_e[m - 1].iter().cloned().collect();
        let ht = hs[m - 1];
        faces.push(face(
            top,
            Plane::new(
                world([0.0, 0.0], ht),
                world([1.0, 0.0], ht),
                world([0.0, 1.0], ht),
            ),
        )?);
        for k in 0..m - 1 {
            for i in 0..n {
                let j = (i + 1) % n;
                let w: Wire = vec![
                    ring_e[k][i].clone(),
                    up[k][j].clone(),
                    ring_e[k + 1][i].inverse(),
                    up[k][i].inverse(),
                ]
                .into();
                faces.push(face(
                    w,
                    Plane::new(v[k][i].point(), v[k][j].point(), v[k + 1][i].point()),
                )?);
            }
        }
        let shell: Shell = faces.into();
        Solid::try_new(vec![shell])
            .map_err(|e| KernelError::InvalidResult(format!("loft: solid: {e:?}")))
    })??;
    let body = Body { solid };
    check_single_closed(&body, "loft")?;
    let mid = |a: &[[f64; 2]], b: &[[f64; 2]]| -> Vec<[f64; 2]> {
        a.iter()
            .zip(b)
            .map(|(p, q)| [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0])
            .collect()
    };
    let want: f64 = (0..m - 1)
        .map(|k| {
            (hs[k + 1] - hs[k]) / 6.0
                * (signed_area(&rings[k])
                    + 4.0 * signed_area(&mid(&rings[k], &rings[k + 1]))
                    + signed_area(&rings[k + 1]))
        })
        .sum();
    check_volume(&body, want, "multi-section prismatoid")?;
    Ok(body)
}

fn face(w: Wire, plane: Plane) -> Result<Face> {
    Face::try_new(vec![w], Surface::Plane(plane))
        .map_err(|e| KernelError::InvalidResult(format!("loft: face: {e:?}")))
}

fn circle_loft(
    c: [f64; 2],
    ra: f64,
    rb: f64,
    h: f64,
    world: &dyn Fn([f64; 2], f64) -> Point3,
    f: &Frame,
) -> Result<Body> {
    let axis = Vector3::new(f.z[0], f.z[1], f.z[2]);
    let origin = world(c, 0.0);
    let solid = guarded("loft", || -> Result<Solid> {
        // The rim-to-rim line revolved about the axis: the exact cone (or
        // cylinder) side; its two boundary circles capped by planar disks.
        let v0 = builder::vertex(world([c[0] + ra, c[1]], 0.0));
        let v1 = builder::vertex(world([c[0] + rb, c[1]], h));
        let line = builder::line(&v0, &v1);
        let mut shell: Shell = builder::rsweep(&line, origin, axis, Rad(7.0));
        for w in shell.extract_boundaries() {
            let cap = builder::try_attach_plane(&[w.inverse()])
                .map_err(|e| KernelError::InvalidResult(format!("loft: cap: {e:?}")))?;
            shell.push(cap);
        }
        Solid::try_new(vec![shell])
            .map_err(|e| KernelError::InvalidResult(format!("loft: solid: {e:?}")))
    })??;
    let mut body = Body { solid };
    check_single_closed(&body, "loft")?;
    // Make the shell outward (positive volume), as revolve does.
    if let crate::VolumeResult::Value { volume, .. } = measure(&body)?.volume
        && volume < 0.0
    {
        body.solid.not();
    }
    let want = std::f64::consts::PI * h / 3.0 * (ra * ra + ra * rb + rb * rb);
    check_volume(&body, want, "frustum")?;
    Ok(body)
}

/// The body's volume must equal the analytic one (a construction check).
fn check_volume(body: &Body, want: f64, what: &str) -> Result<()> {
    match measure(body)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => {
            let tol = (1e-9 * want.abs()).max(1e-6) + 3.0 * error_estimate;
            if (volume - want).abs() > tol {
                return Err(KernelError::InvalidResult(format!(
                    "loft: volume {volume:.6} mm^3 differs from the {what} formula {want:.6}"
                )));
            }
            Ok(())
        }
        crate::VolumeResult::Unavailable { reason } => Err(KernelError::InvalidResult(format!(
            "loft: cannot verify the volume ({reason})"
        ))),
    }
}

#[cfg(test)]
mod tests;
