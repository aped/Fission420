//! STEP AP203/214 export through truck-stepio, with boolean intersection edges replaced
//! by exact NURBS curves first.
//!
//! Why: truck-stepio 0.3 writes `INTERSECTION_CURVE` with the second surface at the
//! first surface's entity index (duplicate ids; see ADR 0001), and the curve's 3D
//! geometry would only be Truck's polyline leader. For a plane z = c cut by a surface
//! extruded along Z, the intersection is exactly the extrusion's generating curve
//! moved to z = c, which we rebuild as a rational B-spline. Other intersection types
//! are refused rather than approximated.
use crate::{Body, KernelError, Result, guarded};
use truck_modeling::*;
use truck_stepio::out;

/// Geometric tolerance for accepting the rebuilt curve (mm).
const REBUILD_TOL: f64 = 1.0e-7;

fn unsupported(msg: &str) -> KernelError {
    KernelError::InvalidInput(format!("STEP export unsupported: {msg}"))
}

/// Generating curve of a NURBS surface extruded along Z, translated to z = `z`.
fn generating_curve_at(s: &NurbsSurface<Vector4>, z: f64) -> Result<NurbsCurve<Vector4>> {
    // One parameter direction must be pure Z translation: try both.
    for mut c in [s.column_curve(0), s.row_curve(0)] {
        c.transform_control_points(|v| {
            let w = v.w;
            v.z = z * w;
        });
        // Accept if the curve varies in XY (not the Z-direction isoline).
        let (t0, t1) = c.range_tuple();
        let (a, b) = (c.subs(t0), c.subs((t0 + t1) / 2.0));
        if (a.x - b.x).hypot(a.y - b.y) > 1e-9 {
            return Ok(c);
        }
    }
    Err(unsupported("could not identify generating curve"))
}

fn is_z_extrusion(s: &NurbsSurface<Vector4>) -> bool {
    let ((u0, u1), (v0, v1)) = s.range_tuple();
    (0..=8).all(|i| {
        (0..=8).all(|j| {
            let n = s.normal(
                u0 + (u1 - u0) * i as f64 / 8.0,
                v0 + (v1 - v0) * j as f64 / 8.0,
            );
            n.z.abs() < 1e-12
        })
    })
}

/// Exact replacement for one intersection edge from `p0` to `p1`.
fn exact_edge(ic: &IntersectionCurve<Leader, Surface>, p0: Point3, p1: Point3) -> Result<Curve> {
    let (plane, other) = match (ic.surface0(), ic.surface1()) {
        (Surface::Plane(p), o) | (o, Surface::Plane(p)) => (p, o),
        _ => return Err(unsupported("intersection without a planar face")),
    };
    let n = plane.normal();
    if n.x.abs() + n.y.abs() > 1e-12 {
        return Err(unsupported("planar face not perpendicular to Z"));
    }
    let Surface::NurbsSurface(ns) = other else {
        return Err(unsupported("intersection with a non-NURBS surface"));
    };
    if !is_z_extrusion(ns) {
        return Err(unsupported("curved face is not a Z extrusion"));
    }
    let full = generating_curve_at(ns, plane.origin().z)?;
    let t0 = full
        .search_nearest_parameter(p0, None, 100)
        .ok_or_else(|| unsupported("edge start not on generating curve"))?;
    let t1 = full
        .search_nearest_parameter(p1, None, 100)
        .ok_or_else(|| unsupported("edge end not on generating curve"))?;
    let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
    let mut c = full;
    let mut sub = c.cut(lo);
    let _tail = sub.cut(hi);
    if t0 > t1 {
        sub.invert();
    }
    let (s0, s1) = sub.range_tuple();
    if sub.subs(s0).distance(p0) > REBUILD_TOL || sub.subs(s1).distance(p1) > REBUILD_TOL {
        return Err(unsupported("rebuilt edge endpoints do not match vertices"));
    }
    // Interior samples must lie on both surfaces (i.e. on the exact intersection) ...
    for k in 1..8 {
        let q = sub.subs(s0 + (s1 - s0) * k as f64 / 8.0);
        let on_plane = (q.z - plane.origin().z).abs() < REBUILD_TOL;
        let on_surface = ns
            .search_nearest_parameter(q, None, 100)
            .is_some_and(|(u, v)| ns.subs(u, v).distance(q) < REBUILD_TOL);
        if !(on_plane && on_surface) {
            return Err(unsupported("rebuilt edge does not lie on both surfaces"));
        }
    }
    // ... and follow the same branch as Truck's edge (leader midpoint lies on it).
    let (l0, l1) = ic.leader().range_tuple();
    let mid = ic
        .search_triple((l0 + l1) / 2.0)
        .ok_or_else(|| unsupported("cannot evaluate original edge"))?
        .0;
    let on_sub = sub
        .search_nearest_parameter(mid, None, 100)
        .is_some_and(|t| t >= s0 - 1e-9 && t <= s1 + 1e-9 && sub.subs(t).distance(mid) < 1e-5);
    if !on_sub {
        return Err(unsupported(
            "rebuilt edge is not the same arc as the original",
        ));
    }
    Ok(Curve::NurbsCurve(sub))
}

/// Export `body` as a STEP file string. Fails (never approximates) on unsupported edges.
pub fn export_step(body: &Body, organization_system: &str) -> Result<String> {
    guarded("export_step", || -> Result<String> {
        let mut cs = body.solid.compress();
        for shell in &mut cs.boundaries {
            let verts = shell.vertices.clone();
            for e in &mut shell.edges {
                if let Curve::IntersectionCurve(ic) = &e.curve {
                    let (a, b) = (verts[e.vertices.0], verts[e.vertices.1]);
                    e.curve = exact_edge(ic, a, b)?;
                }
            }
        }
        let text = out::CompleteStepDisplay::new(
            out::StepModel::from(&cs),
            out::StepHeaderDescriptor {
                organization_system: organization_system.into(),
                ..Default::default()
            },
        )
        .to_string();
        check_unique_ids(&text)?;
        Ok(text)
    })?
}

/// Every `#N = ...` entity id must be unique (guards the writer bug described above).
fn check_unique_ids(text: &str) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        if let Some((id, _)) = line.split_once(" = ")
            && id.starts_with('#')
            && !seen.insert(id)
        {
            return Err(KernelError::Internal(format!(
                "STEP writer produced duplicate entity {id}"
            )));
        }
    }
    Ok(())
}
