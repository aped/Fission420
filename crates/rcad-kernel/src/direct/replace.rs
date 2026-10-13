//! Replace a planar face with another plane (run 6, SESSION_6 §C d): the
//! "offset + tilt" direct edit for faces whose neighbours are all planes.
//!
//! Precondition (refused by name otherwise): the face has one boundary loop,
//! it is convex, every edge of it is straight, every face touching it is a
//! plane, and every vertex of it has exactly one other edge (a *side edge*,
//! straight). Construction: each vertex moves along its side edge's line to
//! the new plane (`u + t (v − u)`, `t > 0`, the side edge keeps at least
//! `MIN_LENGTH`); the face's edges become the exact segments between the
//! moved vertices (they lie on the new plane and on the unchanged neighbour
//! planes); topology and face order are unchanged.
//!
//! Checks: every touched edge on both its faces' surfaces; every touched face
//! stays a simple loop with the same orientation; one closed shell; and point
//! probes against the expected solid: inside the column over the face (the
//! intersection of its neighbours' material half-spaces) material lies below
//! the new plane where it lay below the old one or above the old plane; the
//! rest of the body is unchanged (old body's winding number).
use super::*;

/// `body` with planar face `face` replaced by the plane through `origin` with
/// outward unit normal `normal` (within 89° of the old outward normal).
pub fn replace_face(
    body: &Body,
    face: usize,
    origin: [f64; 3],
    normal: [f64; 3],
) -> Result<DirectEdit> {
    if origin.iter().chain(&normal).any(|v| !v.is_finite()) {
        return Err(invalid("replace face: non-finite plane"));
    }
    let n1 = Vector3::new(normal[0], normal[1], normal[2]);
    if n1.magnitude() < 1e-12 {
        return Err(invalid("replace face: zero normal"));
    }
    let n1 = n1.normalize();
    let o1 = Point3::new(origin[0], origin[1], origin[2]);
    let mut sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    let f = &face_at(&sh, face)?.clone();
    let (o0, n0) = plane_of(f).ok_or_else(|| {
        invalid(format!(
            "face {face} is not planar; only planar faces can be replaced"
        ))
    })?;
    if n1.dot(n0) < 89f64.to_radians().cos() {
        return Err(invalid(
            "replace face: the new plane must face the same way as the face (within 89 degrees)",
        ));
    }
    if f.boundaries.len() != 1 {
        return Err(invalid(format!(
            "face {face} has {} boundary loops; replacing it by another plane is supported for \
             single-loop faces only",
            f.boundaries.len()
        )));
    }
    let fedges: BTreeSet<usize> = face_edges(f).collect();
    for &e in &fedges {
        if !matches!(sh.edges[e].curve, Curve::Line(_)) {
            let pts = curve_samples(&sh.edges[e].curve, 16)?;
            let (a, b) = (pts[0], pts[pts.len() - 1]);
            let d = (b - a).normalize();
            if pts
                .iter()
                .any(|p| ((*p - a) - d * (*p - a).dot(d)).magnitude() > tol)
            {
                return Err(invalid(format!(
                    "face {face} has a curved edge; replacing it by another plane needs straight \
                     edges and planar neighbours"
                )));
            }
        }
    }
    let fverts = edge_vertices(&sh, &fedges);
    let vf = vertex_faces(&sh);
    let mut neighbours = BTreeSet::new();
    for v in &fverts {
        neighbours.extend(vf[*v].iter().copied().filter(|g| *g != face));
    }
    for g in &neighbours {
        if plane_of(&sh.faces[*g]).is_none() {
            return Err(invalid(format!(
                "face {g} next to face {face} is not planar; replacing a face by another plane \
                 is supported with planar neighbours only"
            )));
        }
    }
    // Convexity of the face (in its own frame, effective orientation).
    let fr0 = frame_on(o0, n0);
    let wire = &f.boundaries[0];
    let eff_rev = !f.orientation;
    let loop_pts = |sh: &CS, fr: &Frame| -> Vec<[f64; 2]> {
        let mut pts: Vec<[f64; 2]> = wire
            .iter()
            .map(|ei| {
                let (a, b) = sh.edges[ei.index].vertices;
                to2(fr, sh.vertices[if ei.orientation { a } else { b }])
            })
            .collect();
        if eff_rev {
            pts.reverse();
        }
        pts
    };
    let p0 = loop_pts(&sh, &fr0);
    if !convex_ccw(&p0) {
        return Err(invalid(format!(
            "face {face} is not convex; replacing it by another plane is supported for convex \
             faces only"
        )));
    }
    // Side edges: exactly one per vertex, straight.
    let mut side: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut side_of = BTreeMap::new();
    for (e, ce) in sh.edges.iter().enumerate() {
        if fedges.contains(&e) {
            continue;
        }
        let (a, b) = ce.vertices;
        let (on_a, on_b) = (fverts.contains(&a), fverts.contains(&b));
        if !(on_a || on_b) {
            continue;
        }
        if on_a && on_b {
            return Err(invalid(format!(
                "edge {e} joins two vertices of face {face}; the topology would change"
            )));
        }
        let (fixed, moving) = if on_a { (b, a) } else { (a, b) };
        if side_of.insert(moving, e).is_some() {
            return Err(invalid(format!(
                "a vertex of face {face} has more than one side edge (more than three faces meet \
                 there); not supported"
            )));
        }
        let pts = curve_samples(&ce.curve, 16)?;
        let (u, v) = (sh.vertices[fixed], sh.vertices[moving]);
        let d = (v - u).normalize();
        if pts
            .iter()
            .any(|p| ((*p - u) - d * (*p - u).dot(d)).magnitude() > tol)
        {
            return Err(invalid(format!("side edge {e} is not straight")));
        }
        side.insert(e, (fixed, moving));
    }
    if side_of.len() != fverts.len() {
        return Err(invalid(format!(
            "every vertex of face {face} needs exactly one side edge"
        )));
    }
    let mut moved = BTreeMap::new();
    for (&e, &(fixed, moving)) in &side {
        let (u, v) = (sh.vertices[fixed], sh.vertices[moving]);
        let den = (v - u).dot(n1);
        if den.abs() < 1e-12 {
            return Err(invalid(format!(
                "side edge {e} is parallel to the new plane; the face would be unbounded"
            )));
        }
        let t = (o1 - u).dot(n1) / den;
        let w = u + (v - u) * t;
        if t.is_nan() || t <= 0.0 || (w - u).magnitude() < tol::MIN_LENGTH {
            return Err(invalid(format!(
                "the new plane collapses or inverts side edge {e} (a neighbouring face would \
                 degenerate or the topology change)"
            )));
        }
        moved.insert(moving, w);
    }
    let old_shell = sh.clone();
    for (v, w) in &moved {
        sh.vertices[*v] = *w;
    }
    for &e in &fedges {
        let (a, b) = sh.edges[e].vertices;
        sh.edges[e].curve = Curve::Line(Line(sh.vertices[a], sh.vertices[b]));
    }
    for &e in side.keys() {
        let (p, q) = sh.edges[e].vertices;
        sh.edges[e].curve = Curve::Line(Line(sh.vertices[p], sh.vertices[q]));
    }
    // The new plane, with the surface normal on the same side as before.
    let s_old = match &sh.faces[face].surface {
        Surface::Plane(pl) => pl.normal(),
        _ => return Err(invalid("face is not planar")),
    };
    let sn = if n1.dot(s_old) > 0.0 { n1 } else { -n1 };
    let fr = frame_on(o1, sn);
    let (ox, oy) = (
        Vector3::new(fr.x[0], fr.x[1], fr.x[2]),
        Vector3::new(fr.y[0], fr.y[1], fr.y[2]),
    );
    sh.faces[face].surface = Surface::Plane(Plane::new(o1, o1 + ox, o1 + oy));
    // Touched faces stay simple, convex for the face itself, same orientation.
    let fr1 = frame_on(o1, n1);
    if !convex_ccw(&loop_pts(&sh, &fr1)) {
        return Err(invalid(format!(
            "face {face} would not stay a convex loop on the new plane"
        )));
    }
    for &g in &neighbours {
        let (og, ng) = plane_of(&sh.faces[g]).ok_or_else(|| invalid("neighbour"))?;
        let frg = frame_on(og, ng);
        for (w, wire_g) in sh.faces[g].boundaries.iter().enumerate() {
            let poly = |s: &CS| -> Result<Vec<[f64; 2]>> {
                let mut out = Vec::new();
                for ei in wire_g {
                    let c = &s.edges[ei.index].curve;
                    let k = if matches!(c, Curve::Line(_)) { 1 } else { 8 };
                    let mut pts = curve_samples(c, k)?;
                    if !ei.orientation {
                        pts.reverse();
                    }
                    pts.pop();
                    out.extend(pts.into_iter().map(|p| to2(&frg, p)));
                }
                Ok(out)
            };
            let (before, after) = (poly(&old_shell)?, poly(&sh)?);
            let area = |c: &[[f64; 2]]| {
                let n = c.len();
                (0..n)
                    .map(|i| c[i][0] * c[(i + 1) % n][1] - c[(i + 1) % n][0] * c[i][1])
                    .sum::<f64>()
                    / 2.0
            };
            if area(&before).signum() != area(&after).signum()
                || area(&after).abs() < tol::MIN_LENGTH * tol::MIN_LENGTH
                || !simple(&after)
            {
                return Err(invalid(format!(
                    "loop {w} of neighbouring face {g} would degenerate or cross itself"
                )));
            }
        }
    }
    let touched: BTreeSet<usize> = fedges.iter().chain(side.keys()).copied().collect();
    check_edges_on_faces(&sh, &touched, tol)?;
    let out = rebuild(sh)?;
    check_solid(&out)?;
    probe_check(
        body,
        &out,
        &old_shell,
        face,
        &neighbours,
        (o0, n0),
        (o1, n1),
    )?;
    Ok(DirectEdit {
        body: out,
        removed_faces: vec![],
    })
}

/// Counter-clockwise strictly convex polygon.
fn convex_ccw(p: &[[f64; 2]]) -> bool {
    let n = p.len();
    n >= 3
        && (0..n).all(|i| {
            let (a, b, c) = (p[i], p[(i + 1) % n], p[(i + 2) % n]);
            (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) > 0.0
        })
}

/// No two non-adjacent segments of the closed polygon cross or touch.
fn simple(p: &[[f64; 2]]) -> bool {
    let n = p.len();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (a, b, c, d) = (p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]);
            let (d1, d2) = (cross(a, b, c), cross(a, b, d));
            let (d3, d4) = (cross(c, d, a), cross(c, d, b));
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return false;
            }
        }
    }
    true
}

/// Probes on both sides of every touched face of the result against the
/// expected solid (module docs), membership from tessellation winding numbers
/// of the old and new bodies.
fn probe_check(
    old: &Body,
    new: &Body,
    old_shell: &CS,
    face: usize,
    neighbours: &BTreeSet<usize>,
    (o0, n0): (Point3, Vector3),
    (o1, n1): (Point3, Vector3),
) -> Result<()> {
    let halfspaces: Vec<(Point3, Vector3)> = neighbours
        .iter()
        .filter_map(|g| plane_of(&old_shell.faces[*g]))
        .collect();
    let m = crate::measure(new)?;
    let diag = ((0..3)
        .map(|i| (m.bounds.max[i] - m.bounds.min[i]).powi(2))
        .sum::<f64>())
    .sqrt();
    let h = (1e-4 * diag).max(1e-3);
    let in_column = |q: Point3| halfspaces.iter().all(|(o, n)| (q - *o).dot(*n) < 0.0);
    let sh_new = shell_of(new)?;
    let mut probes = Vec::new();
    for g in std::iter::once(face).chain(neighbours.iter().copied()) {
        let f = &sh_new.faces[g];
        let Some((_, n)) = plane_of(f) else { continue };
        // Interior points: the vertex average of the outer loop nudged toward
        // each vertex (convex enough for these faces), and that average.
        let verts: Vec<Point3> = f.boundaries[0]
            .iter()
            .map(|ei| {
                let (a, b) = sh_new.edges[ei.index].vertices;
                sh_new.vertices[if ei.orientation { a } else { b }]
            })
            .collect();
        let c = verts.iter().fold(Point3::origin(), |s, v| s + v.to_vec()) / verts.len() as f64;
        let mut pts = vec![c];
        pts.extend(verts.iter().map(|v| c + (*v - c) * 0.6));
        for p in pts {
            probes.push((p - n * h, true));
            probes.push((p + n * h, false));
        }
    }
    let pts: Vec<[f64; 3]> = probes.iter().map(|(p, _)| p3(*p)).collect();
    let old_m = crate::point_membership(old, &pts)?;
    let new_m = crate::point_membership(new, &pts)?;
    let mut decided = 0;
    for (k, (p, side_in)) in probes.iter().enumerate() {
        let below_new = (*p - o1).dot(n1) < 0.0;
        let above_old = (*p - o0).dot(n0) > 0.0;
        let expected = if in_column(*p) {
            old_m[k].map(|o| below_new && (o || above_old))
        } else {
            old_m[k]
        };
        match (expected, new_m[k]) {
            (Some(e), Some(r)) if e != r || r != *side_in => {
                // A probe on a face's material side must be material in both.
                return Err(KernelError::InvalidResult(format!(
                    "replace face failed the probe check at {:?}: expected {e}, result {r}",
                    p3(*p)
                )));
            }
            (Some(_), Some(_)) => decided += 1,
            _ => {}
        }
    }
    if decided == 0 {
        return Err(KernelError::InvalidResult(
            "replace face: no probe could be classified; cannot verify".into(),
        ));
    }
    Ok(())
}

/// The plane of `frame` (origin, x, y, z = outward normal) tilted by `angle`
/// (radians) about its x axis through its origin (+angle turns +y toward the
/// normal), then moved `distance` along the tilted normal: the new frame.
pub fn tilt_offset_frame(frame: &Frame, distance: f64, angle: f64) -> Frame {
    let (s, c) = angle.sin_cos();
    let y = std::array::from_fn(|i| frame.y[i] * c + frame.z[i] * s);
    let z: [f64; 3] = std::array::from_fn(|i| frame.z[i] * c - frame.y[i] * s);
    Frame {
        origin: std::array::from_fn(|i| frame.origin[i] + z[i] * distance),
        x: frame.x,
        y,
        z,
    }
}
