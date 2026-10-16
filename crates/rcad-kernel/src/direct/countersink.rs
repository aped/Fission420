//! Exact countersink of an existing round hole.
//!
//! The countersink is built directly into the topology, like the direct edits
//! of the parent module: exact faces and edges, no boolean intersection curves
//! to rebuild and no tolerance retries. (Unpatched Truck 0.6 could not cut a
//! revolved cone at all; with the truck-shapeops fork it can, see
//! `validate::tests::probe_countersink_cone`, but the direct construction stays
//! the simpler exact path.) Precondition: a cylindrical hole of radius `r` whose entry loop is a
//! whole inner loop of a planar entry face perpendicular to the hole axis, the
//! hole wall reaching deeper than the countersink. Construction with
//! `h = (R - r) / tan(angle / 2)` and the entry face's outward normal `n`:
//!
//! * every entry-loop vertex `v` moves to `v - h n` (it stays on the cylinder),
//!   and the entry-loop arcs are translated by `-h n` (they stay the
//!   cylinder's top edges, exact circles of radius `r`);
//! * new vertices `c + (v - c) R / r` on the entry plane and new arcs, the
//!   entry-loop arcs scaled radially by `R / r` (affine, exact), replace the
//!   entry face's inner loop;
//! * one conical face per entry-loop arc: the homotopy of its lower and upper
//!   arcs (both affine images of one rational arc, so identical weights and
//!   knots: every `v`-line is the straight generator between corresponding
//!   points, i.e. the exact cone), bounded by two straight generators;
//! * the hole wall's straight edges along the axis are shortened by `h`.
//!
//! Checked like every direct edit (edges on surfaces, one closed shell), plus
//! point probes on both sides of the cone and the counterpart of the
//! clearance rule on the entry face (`R` + `MIN_LENGTH` clear of every other
//! edge of it, exact distances).
use super::*;

/// Countersink the round hole of radius `hole_diameter / 2` whose axis passes
/// through `axis_point` along `axis` (either sense): diameter `diameter` at the
/// entry face, included angle `angle` (radians, in (0, pi)).
pub fn countersink(
    body: &Body,
    axis_point: [f64; 3],
    axis: [f64; 3],
    hole_diameter: f64,
    diameter: f64,
    angle: f64,
) -> Result<Body> {
    let finite = [hole_diameter, diameter, angle]
        .iter()
        .chain(&axis_point)
        .chain(&axis)
        .all(|v| v.is_finite());
    if !finite {
        return Err(invalid("countersink: non-finite input"));
    }
    if !(angle > 1e-3 && angle < std::f64::consts::PI - 1e-3) {
        return Err(invalid(format!(
            "countersink angle must be between 0 and 180 degrees, got {:.6} degrees",
            angle.to_degrees()
        )));
    }
    let (r, big) = (hole_diameter / 2.0, diameter / 2.0);
    if big < r + tol::MIN_LENGTH {
        return Err(invalid(format!(
            "countersink diameter {diameter} mm must exceed the hole diameter {hole_diameter} mm"
        )));
    }
    let a = Vector3::new(axis[0], axis[1], axis[2]);
    if a.magnitude() < 1e-12 {
        return Err(invalid("countersink: zero axis"));
    }
    let a = a.normalize();
    let c0 = Point3::new(axis_point[0], axis_point[1], axis_point[2]);
    let mut sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    // Hole wall: coaxial concave cylinder faces of radius r.
    let mut cyl = BTreeSet::new();
    for (i, f) in sh.faces.iter().enumerate() {
        if let Some((c, ax, rr, hole)) = cylinder_of(&sh, f, tol)? {
            let d = c - c0;
            if hole
                && ax.cross(a).magnitude() <= 1e-9
                && (d - a * d.dot(a)).magnitude() <= tol
                && (rr - r).abs() <= tol
            {
                cyl.insert(i);
            }
        }
    }
    if cyl.is_empty() {
        return Err(invalid(format!(
            "countersink: no Ø{hole_diameter} mm hole wall on this axis"
        )));
    }
    let cyl_edges: BTreeSet<usize> = cyl.iter().flat_map(|g| face_edges(&sh.faces[*g])).collect();
    // Entry face: a plane perpendicular to the axis with a whole inner loop
    // made of hole-wall edges; the one on the entry side (outward normal
    // pointing out of the hole's material at the given axis point's plane).
    let mut entry = None;
    for (i, f) in sh.faces.iter().enumerate() {
        let Some((o, n)) = plane_of(f) else { continue };
        if n.cross(a).magnitude() > 1e-9 || (o - c0).dot(n).abs() > tol {
            continue;
        }
        for (w, wire) in f.boundaries.iter().enumerate() {
            if !wire.is_empty() && wire.iter().all(|e| cyl_edges.contains(&e.index)) {
                if f.boundaries.len() < 2 {
                    return Err(invalid(
                        "countersink: the entry face is bounded only by the hole",
                    ));
                }
                if entry.is_some() {
                    return Err(invalid("countersink: the hole has two entry loops here"));
                }
                entry = Some((i, w, n));
            }
        }
    }
    let Some((ef, wire_idx, n)) = entry else {
        return Err(invalid(
            "countersink: the hole does not open into a planar face perpendicular to its axis \
             at the given point",
        ));
    };
    let h = (big - r) / (angle / 2.0).tan();
    let entry_origin = plane_of(&sh.faces[ef]).map_or(c0, |x| x.0);
    // The axis point on the entry plane.
    let centre = c0 - n * (c0 - entry_origin).dot(n);
    // Clearance on the entry face.
    {
        let (o, _) = plane_of(&sh.faces[ef]).ok_or_else(|| invalid("entry face"))?;
        let fr = frame_on(o, n);
        let c2 = to2(&fr, centre);
        for (w, wire) in sh.faces[ef].boundaries.iter().enumerate() {
            if w == wire_idx {
                continue;
            }
            for ei in wire {
                let Some(e2) = edge2(&sh, ei.index, ei.orientation, &fr, tol)? else {
                    return Err(invalid(
                        "countersink: the entry face has a free-form edge; clearance cannot be \
                         checked exactly",
                    ));
                };
                let d = dist2(c2, &e2);
                if d <= big + tol::MIN_LENGTH {
                    return Err(invalid(format!(
                        "countersink Ø{diameter} mm does not fit on the entry face: another edge \
                         is {d:.6} mm from the hole axis"
                    )));
                }
            }
        }
    }
    let lp: Vec<(usize, bool)> = sh.faces[ef].boundaries[wire_idx]
        .iter()
        .map(|e| (e.index, e.orientation))
        .collect();
    let lset: BTreeSet<usize> = lp.iter().map(|x| x.0).collect();
    let lverts = edge_vertices(&sh, &lset);
    // Only the entry face and the hole wall may touch the entry loop.
    let vf = vertex_faces(&sh);
    for v in &lverts {
        if vf[*v].iter().any(|g| *g != ef && !cyl.contains(g)) {
            return Err(invalid(
                "countersink: other faces meet the hole's entry loop (a feature already there)",
            ));
        }
    }
    // Wall edges along the axis from the loop: shortened by h.
    let mut seams = BTreeMap::new();
    for &e in &cyl_edges {
        if lset.contains(&e) {
            continue;
        }
        let (p, q) = sh.edges[e].vertices;
        let (on_p, on_q) = (lverts.contains(&p), lverts.contains(&q));
        if !(on_p || on_q) {
            continue;
        }
        if on_p && on_q {
            return Err(invalid(
                "countersink: a hole-wall edge joins two entry-loop vertices",
            ));
        }
        let (fixed, moving) = if on_p { (q, p) } else { (p, q) };
        let (f0, m0) = (sh.vertices[fixed], sh.vertices[moving]);
        let along = (m0 - f0).dot(n);
        let pts = curve_samples(&sh.edges[e].curve, 16)?;
        if pts
            .iter()
            .any(|x| ((*x - f0) - n * (*x - f0).dot(n)).magnitude() > tol)
            || ((m0 - f0) - n * along).magnitude() > tol
        {
            return Err(invalid(
                "countersink: the hole wall has an edge that is not straight along the axis",
            ));
        }
        if along - h < tol::MIN_LENGTH {
            return Err(invalid(format!(
                "countersink depth {h:.6} mm reaches the end of the hole wall ({along:.6} mm \
                 deep): choose a smaller countersink diameter or a larger angle"
            )));
        }
        seams.insert(e, ());
    }
    let down = Matrix4::from_translation(-n * h);
    let scale_up = radial_scaling(centre, a, big / r);
    // New upper vertices; move the loop vertices down.
    let mut hi = BTreeMap::new();
    for &v in &lverts {
        let p = sh.vertices[v];
        hi.insert(v, sh.vertices.len());
        sh.vertices.push(scale_up.transform_point(p));
        sh.vertices[v] = p - n * h;
    }
    for &e in &seams.keys().copied().collect::<Vec<_>>() {
        let (p, q) = sh.edges[e].vertices;
        sh.edges[e].curve = Curve::Line(Line(sh.vertices[p], sh.vertices[q]));
    }
    // Upper arcs, generators, cone faces.
    let nurbs = |c: &Curve| -> Result<NurbsCurve<Vector4>> {
        match c {
            Curve::NurbsCurve(c) => Ok(c.clone()),
            Curve::BSplineCurve(c) => Ok(NurbsCurve::from(c.clone())),
            _ => Err(invalid(
                "countersink: the hole's entry edge is not a (rational) B-spline arc",
            )),
        }
    };
    let mut gen_edge = BTreeMap::new();
    for &v in &lverts {
        gen_edge.insert(v, sh.edges.len());
        sh.edges.push(truck_topology::compress::CompressedEdge {
            vertices: (v, hi[&v]),
            curve: Curve::Line(Line(sh.vertices[v], sh.vertices[hi[&v]])),
        });
    }
    // Wires in compressed faces are stored as if the face were not inverted;
    // the effective (outward) boundary is reversed when `orientation` is false.
    let entry_flip = !sh.faces[ef].orientation;
    let mut touched: BTreeSet<usize> = seams.keys().copied().collect();
    touched.extend(gen_edge.values().copied());
    for (k, &(e, o)) in lp.iter().enumerate() {
        let original = sh.edges[e].curve.clone();
        let mut lower = original.clone();
        lower.transform_by(down);
        let mut upper = original;
        upper.transform_by(scale_up);
        let (va, vb) = sh.edges[e].vertices;
        sh.edges[e].curve = lower.clone();
        let eu = sh.edges.len();
        sh.edges.push(truck_topology::compress::CompressedEdge {
            vertices: (hi[&va], hi[&vb]),
            curve: upper.clone(),
        });
        sh.faces[ef].boundaries[wire_idx][k].index = eu;
        touched.extend([e, eu]);
        // Effective wire: the loop edge as the entry face effectively used it
        // (from s to t), the generator up at t, the upper arc back, the
        // generator down at s.
        let o = o != entry_flip;
        let (s, t) = if o { (va, vb) } else { (vb, va) };
        use truck_topology::compress::CompressedEdgeIndex as Ix;
        let wire = vec![
            Ix {
                index: e,
                orientation: o,
            },
            Ix {
                index: gen_edge[&t],
                orientation: true,
            },
            Ix {
                index: eu,
                orientation: !o,
            },
            Ix {
                index: gen_edge[&s],
                orientation: false,
            },
        ];
        let surf = BSplineSurface::homotopy(
            nurbs(&lower)?.into_non_rationalized(),
            nurbs(&upper)?.into_non_rationalized(),
        );
        let surface = Surface::NurbsSurface(NurbsSurface::new(surf));
        // Outward = into the countersink void: toward the axis.
        let (t0, t1) = lower.range_tuple();
        let tm = 0.5 * (t0 + t1);
        let pm = surface.subs(tm, 0.5);
        let nm = surface.normal(tm, 0.5);
        let to_axis = {
            let d = centre - pm;
            d - a * d.dot(a)
        };
        let orientation = nm.dot(to_axis) > 0.0;
        let wire = if orientation {
            wire
        } else {
            wire.into_iter()
                .rev()
                .map(|x| Ix {
                    index: x.index,
                    orientation: !x.orientation,
                })
                .collect()
        };
        sh.faces.push(CompressedFace {
            boundaries: vec![wire],
            orientation,
            surface,
        });
    }
    check_edges_on_faces(&sh, &touched, tol)?;
    let out = rebuild(sh)?;
    check_solid(&out)?;
    // Probes at mid-depth: inside the void (between the cylinder and the
    // cone) and just inside the material beyond the cone.
    let rho = 0.5 * (big + r);
    let u = {
        let x = crate::step_in::least_aligned_axis(a);
        x - a * x.dot(a)
    }
    .normalize();
    let mid = centre - n * (0.5 * h);
    let void = mid + u * (0.5 * (r + rho));
    let solid = mid + u * (rho + 0.5 * (rho - r));
    let m = crate::point_membership(&out, &[p3(void), p3(solid)])?;
    if m[0] != Some(false) || m[1] != Some(true) {
        return Err(KernelError::InvalidResult(format!(
            "countersink failed the probe check (void {:?}, material {:?})",
            m[0], m[1]
        )));
    }
    Ok(out)
}
