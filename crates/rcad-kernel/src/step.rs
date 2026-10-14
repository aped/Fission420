//! STEP AP203/214 export through truck-stepio, with boolean intersection edges replaced
//! by exact NURBS curves first.
//!
//! Why: truck-stepio 0.3 writes `INTERSECTION_CURVE` with the second surface at the
//! first surface's entity index (duplicate ids; see ADR 0001), and the curve's 3D
//! geometry would only be Truck's polyline leader. Booleans already replace supported
//! intersection edges with verified exact curves (`crate::exact`); any edge that is
//! still implicit here is retried with the same rebuild and refused, never
//! approximated, if that fails.
use crate::exact::exact_edge;
use crate::{Body, KernelError, Result, guarded};
use truck_modeling::*;
use truck_stepio::out;
use truck_topology::compress::CompressedSolid;

/// Export `body` as a STEP file string. Fails (never approximates) on unsupported edges.
pub fn export_step(body: &Body, organization_system: &str) -> Result<String> {
    guarded("export_step", || -> Result<String> {
        let cs = canonical_compressed(exact_compressed(body)?);
        let text = if body.is_open_shell() {
            surface_representation(&surface_text(&cs, organization_system)?)
        } else {
            out::CompleteStepDisplay::new(out::StepModel::from(&cs), header(organization_system))
                .to_string()
        };
        check_unique_ids(&text)?;
        Ok(round_reals(&fix_file_name(&text)))
    })?
}

/// Export several bodies into ONE STEP file, each as its own solid
/// (`MANIFOLD_SOLID_BREP`, or `BREP_WITH_VOIDS` for a body with inner shells).
///
/// Structure (fixed by truck-stepio 0.3's `StepModels` writer): a single `PRODUCT`
/// whose one `ADVANCED_BREP_SHAPE_REPRESENTATION` lists all solids, in input order.
/// The writer has no per-product or per-item naming, so each name is written as the
/// label of its solid entity (post-processing the writer's empty `''` label);
/// products are not split per body and the product name stays empty. Importers that
/// only read product names will not show them: observed with FreeCAD 1.1.4
/// (`Import.insert` gives three solids labelled `Shapes`, `Shapes001`, ...).
/// Non-ASCII characters are encoded as ISO 10303-21 `\X2\....\X0\` sequences.
///
/// Fails (never approximates) on unsupported edges, like [`export_step`];
/// an empty slice is `InvalidInput`.
pub fn export_step_bodies(bodies: &[(&str, &Body)], organization_system: &str) -> Result<String> {
    if bodies.is_empty() {
        return Err(KernelError::InvalidInput(
            "STEP export needs at least one body".into(),
        ));
    }
    guarded("export_step_bodies", || -> Result<String> {
        let open = bodies.iter().filter(|(_, b)| b.is_open_shell()).count();
        if open > 0 && open < bodies.len() {
            return Err(KernelError::InvalidInput(format!(
                "STEP export of solids and surface bodies in one file is not supported ({open} \
                 surface bodies among {} bodies); export the surface bodies separately",
                bodies.len()
            )));
        }
        let solids = bodies
            .iter()
            .enumerate()
            .map(|(i, (name, body))| {
                exact_compressed(body)
                    .map(canonical_compressed)
                    .map_err(|e| match e {
                        KernelError::InvalidInput(m) => {
                            KernelError::InvalidInput(format!("body {i} ('{name}'): {m}"))
                        }
                        other => other,
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let text = if open > 0 {
            // Surface bodies: one SHELL_BASED_SURFACE_MODEL (OPEN_SHELL) each.
            let shells: Vec<_> = solids.iter().map(one_shell).collect::<Result<_>>()?;
            let models: out::StepModels<_, _, _> = shells.iter().collect();
            surface_representation(
                &out::CompleteStepDisplay::new(models, header(organization_system)).to_string(),
            )
        } else {
            let models: out::StepModels<_, _, _> = solids.iter().collect();
            out::CompleteStepDisplay::new(models, header(organization_system)).to_string()
        };
        check_unique_ids(&text)?;
        let names: Vec<&str> = bodies.iter().map(|(n, _)| *n).collect();
        label_solids(&round_reals(&fix_file_name(&text)), &names)
    })?
}

/// Compressed copy of `body` with every `IntersectionCurve` edge replaced by its
/// verified exact curve; refuses (InvalidInput) when an edge cannot be rebuilt.
fn exact_compressed(body: &Body) -> Result<CompressedSolid<Point3, Curve, Surface>> {
    // An open surface body (one open shell) is written by the callers as a
    // SHELL_BASED_SURFACE_MODEL, never as a MANIFOLD_SOLID_BREP; its edges and
    // faces get the same exact-curve and generatrix treatment.
    let mut cs = body.solid.compress();
    for shell in &mut cs.boundaries {
        let verts = shell.vertices.clone();
        for e in &mut shell.edges {
            if let Curve::IntersectionCurve(ic) = &e.curve {
                let (a, b) = (verts[e.vertices.0], verts[e.vertices.1]);
                e.curve = exact_edge(ic, a, b).map_err(|err| {
                    KernelError::InvalidInput(format!("STEP export unsupported: {err}"))
                })?;
            }
        }
        // After the edges are exact: each face's boundary samples bound its generatrix.
        for face in &mut shell.faces {
            let samples: Vec<Point3> = face
                .boundaries
                .iter()
                .flatten()
                .flat_map(|ei| {
                    let c = &shell.edges[ei.index].curve;
                    let (t0, t1) = c.range_tuple();
                    (0..=GENERATRIX_SAMPLES)
                        .map(move |k| c.subs(t0 + (t1 - t0) * k as f64 / GENERATRIX_SAMPLES as f64))
                })
                .collect();
            bounded_revolution_generatrix(&mut face.surface, &samples);
        }
    }
    Ok(cs)
}

/// The single shell of a compressed surface body.
fn one_shell(
    cs: &CompressedSolid<Point3, Curve, Surface>,
) -> Result<truck_topology::compress::CompressedShell<Point3, Curve, Surface>> {
    match &cs.boundaries[..] {
        [sh] => Ok(sh.clone()),
        other => Err(KernelError::InvalidInput(format!(
            "a surface body for STEP export must have exactly one shell, found {}",
            other.len()
        ))),
    }
}

/// Surface models belong in a `MANIFOLD_SURFACE_SHAPE_REPRESENTATION`
/// (ISO 10303-42), not the `ADVANCED_BREP_SHAPE_REPRESENTATION` truck-stepio
/// writes for every model; strict readers drop surface models listed in the
/// latter.
fn surface_representation(text: &str) -> String {
    text.replace(
        "ADVANCED_BREP_SHAPE_REPRESENTATION(",
        "MANIFOLD_SURFACE_SHAPE_REPRESENTATION(",
    )
}

/// STEP text of one surface body (SHELL_BASED_SURFACE_MODEL with an OPEN_SHELL).
fn surface_text(
    cs: &CompressedSolid<Point3, Curve, Surface>,
    organization_system: &str,
) -> Result<String> {
    let sh = one_shell(cs)?;
    Ok(
        out::CompleteStepDisplay::new(out::StepModel::from(&sh), header(organization_system))
            .to_string(),
    )
}

/// Points sampled per boundary edge to bound a revolution's generatrix.
const GENERATRIX_SAMPLES: usize = 16;

/// truck-stepio writes a revolved straight edge as `SURFACE_OF_REVOLUTION` of a `LINE`, and a
/// STEP `LINE` is unbounded: for a generatrix that is not parallel to the axis (a revolved annulus
/// or cone) the line crosses the axis, the surface covers itself twice and readers rebuild the
/// face trims in the wrong half (OpenCascade: invalid faces at most seam angles). The generatrix
/// is written instead as the degree-1 B-spline over the part of the line the face uses: the
/// parameter range of its boundary samples (`samples`, world coordinates). The line's own two
/// points are not that range (an imported cylinder's line is a unit direction from one point).
fn bounded_revolution_generatrix(surface: &mut Surface, samples: &[Point3]) {
    let Surface::RevolutedCurve(p) = surface else {
        return;
    };
    let rev = p.entity();
    let Curve::Line(line) = rev.entity_curve() else {
        return;
    };
    let line = *line;
    // The transform is affine, so a point's line parameter is the same in world coordinates.
    let t = *p.transform();
    let (w0, w1) = (t.transform_point(line.0), t.transform_point(line.1));
    let d = w1 - w0;
    let len2 = d.magnitude2();
    if len2 < 1e-24 || samples.is_empty() {
        return;
    }
    // A boundary sample's generatrix parameter comes from its meridian
    // coordinates (axial z, radius rho): the face's points lie at other
    // rotation angles, so projecting them onto the line itself is only right
    // for a generatrix parallel to the axis.
    let wo = t.transform_point(rev.origin());
    let wa = t.transform_vector(rev.axis());
    if wa.magnitude2() < 1e-24 {
        return;
    }
    let wa = wa.normalize();
    let meridian = |q: Point3| {
        let r = q - wo;
        let z = r.dot(wa);
        [z, (r - wa * z).magnitude()]
    };
    let (m0, m1) = (meridian(w0), meridian(w1));
    let dm = [m1[0] - m0[0], m1[1] - m0[1]];
    let dm2 = dm[0] * dm[0] + dm[1] * dm[1];
    if dm2 < 1e-24 || (dm2 - len2).abs() > 1e-9 * len2 {
        // Not a line in a plane through the axis (a skew line sweeps a
        // hyperboloid): keep the projection onto the line.
        let (lo, hi) = samples
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                let s = (q - w0).dot(d) / len2;
                (lo.min(s), hi.max(s))
            });
        return bound_line(p, line, t, lo, hi);
    }
    let (lo, hi) = samples
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
            let m = meridian(*q);
            let s = ((m[0] - m0[0]) * dm[0] + (m[1] - m0[1]) * dm[1]) / dm2;
            (lo.min(s), hi.max(s))
        });
    bound_line(p, line, t, lo, hi);
}

/// Replace the revolution's line generatrix by its segment over [lo, hi].
fn bound_line(
    p: &mut Processor<RevolutedCurve<Curve>, Matrix4>,
    line: Line<Point3>,
    t: Matrix4,
    lo: f64,
    hi: f64,
) {
    let rev = p.entity();
    if !(lo.is_finite() && hi.is_finite()) || hi - lo < 1e-12 {
        return;
    }
    let pad = (hi - lo) * 1e-7;
    let at = |s: f64| line.0 + (line.1 - line.0) * s;
    let segment = BSplineCurve::new(KnotVec::bezier_knot(1), vec![at(lo - pad), at(hi + pad)]);
    let mut q = Processor::new(RevolutedCurve::by_revolution(
        Curve::BSplineCurve(segment),
        rev.origin(),
        rev.axis(),
    ));
    q.transform_by(t);
    if !p.orientation() {
        q.invert();
    }
    *p = q;
}

/// STEP header. The `FILE_NAME` time stamp is `SOURCE_DATE_EPOCH` (seconds, the
/// reproducible-builds convention) when set, otherwise the current time, both
/// as ISO 8601 UTC to the second: the only part of an export that may differ
/// between two exports of the same model.
fn header(organization_system: &str) -> out::StepHeaderDescriptor {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    out::StepHeaderDescriptor {
        organization_system: organization_system.into(),
        time_stamp: iso8601_utc(secs),
        ..Default::default()
    }
}

/// truck-stepio 0.3 writes the `FILE_NAME` author and organization as nested
/// lists (`((''))`); ISO 10303-21 declares both `LIST OF STRING`, so they are
/// `('')`. OCCT (FreeCAD 1.1.4) reports the nested form as "StepReaderData:
/// Unresolved Reference: Fails Count: 2". Only the header line is rewritten;
/// a header that already has flat lists is left unchanged.
fn fix_file_name(text: &str) -> String {
    let Some(start) = text.find("FILE_NAME(") else {
        return text.to_string();
    };
    let end = text[start..].find(";\n").map_or(text.len(), |e| start + e);
    let line = text[start..end].replace("(('')), (('')), 'truck'", "(''), (''), 'truck'");
    format!("{}{}{}", &text[..start], line, &text[end..])
}

/// `secs` since the Unix epoch as `YYYY-MM-DDTHH:MM:SS` (UTC; proleptic
/// Gregorian, Howard Hinnant's days-to-civil algorithm).
fn iso8601_utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Canonical form of a compressed solid for export: Truck orders vertices,
/// edges and faces by pointer-derived ids and picks a face's plane and
/// orientation per boolean, all of which change between regenerations of the
/// same model. Here vertices are sorted by position (1e-6 mm keys), edges run
/// from the lower to the higher vertex and are sorted by (vertices, midpoint),
/// faces are sorted by their edges, every loop starts at its smallest edge
/// (inner loops sorted), and planar faces get `orientation = true` with a
/// canonical placement of their plane (the same plane and outward side).
fn canonical_compressed(
    mut cs: CompressedSolid<Point3, Curve, Surface>,
) -> CompressedSolid<Point3, Curve, Surface> {
    use truck_topology::compress::CompressedEdgeIndex;
    let q = |p: Point3| [p.x, p.y, p.z].map(|c| (c * 1.0e6).round() as i64);
    for shell in &mut cs.boundaries {
        // Vertices.
        let mut vo: Vec<usize> = (0..shell.vertices.len()).collect();
        vo.sort_by(|&a, &b| {
            let (pa, pb) = (shell.vertices[a], shell.vertices[b]);
            q(pa).cmp(&q(pb)).then_with(|| {
                [pa.x, pa.y, pa.z]
                    .iter()
                    .zip([pb.x, pb.y, pb.z].iter())
                    .map(|(x, y)| x.total_cmp(y))
                    .find(|o| o.is_ne())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        let mut vnew = vec![0; vo.len()];
        for (n, &o) in vo.iter().enumerate() {
            vnew[o] = n;
        }
        shell.vertices = vo.iter().map(|&o| shell.vertices[o]).collect();
        // Edges: renumber vertices, orient low -> high, then sort.
        let mut flipped = vec![false; shell.edges.len()];
        for (i, e) in shell.edges.iter_mut().enumerate() {
            let (a, b) = (vnew[e.vertices.0], vnew[e.vertices.1]);
            if a > b {
                e.vertices = (b, a);
                e.curve = e.curve.inverse();
                flipped[i] = true;
            } else {
                e.vertices = (a, b);
            }
        }
        let mid = |c: &Curve| {
            let (t0, t1) = c.range_tuple();
            q(c.subs(0.5 * (t0 + t1)))
        };
        let mut eo: Vec<usize> = (0..shell.edges.len()).collect();
        let keys: Vec<_> = shell
            .edges
            .iter()
            .map(|e| (e.vertices, mid(&e.curve)))
            .collect();
        eo.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
        let mut enew = vec![0; eo.len()];
        for (n, &o) in eo.iter().enumerate() {
            enew[o] = n;
        }
        shell.edges = eo.iter().map(|&o| shell.edges[o].clone()).collect();
        let remap = |x: &CompressedEdgeIndex| CompressedEdgeIndex {
            index: enew[x.index],
            orientation: x.orientation != flipped[x.index],
        };
        // Faces.
        for f in &mut shell.faces {
            for w in &mut f.boundaries {
                *w = w.iter().map(remap).collect();
            }
            if let Surface::Plane(pl) = &f.surface {
                let mut n = pl.normal();
                if !f.orientation {
                    // Same face with the inverse surface: loops reversed.
                    n = -n;
                    for w in &mut f.boundaries {
                        w.reverse();
                        for x in w.iter_mut() {
                            x.orientation = !x.orientation;
                        }
                    }
                    f.orientation = true;
                }
                let o = Point3::origin() + n * n.dot(pl.origin().to_vec());
                let axes = [Vector3::unit_x(), Vector3::unit_y(), Vector3::unit_z()];
                let e = axes
                    .into_iter()
                    .min_by(|a, b| n.dot(*a).abs().total_cmp(&n.dot(*b).abs()))
                    .unwrap_or(Vector3::unit_x());
                let u = (e - n * n.dot(e)).normalize();
                let v = n.cross(u);
                f.surface = Surface::Plane(Plane::new(o, o + u, o + v));
            }
            let key = |x: &CompressedEdgeIndex| (x.index, x.orientation);
            for w in &mut f.boundaries {
                if let Some(r) = (0..w.len()).min_by_key(|&i| key(&w[i])) {
                    w.rotate_left(r);
                }
            }
            if f.boundaries.len() > 2 {
                f.boundaries[1..].sort_by_key(|w| w.first().map(key));
            }
        }
        let fkey = |f: &truck_topology::compress::CompressedFace<Surface>| {
            let mut v: Vec<usize> = f.boundaries.iter().flatten().map(|x| x.index).collect();
            v.sort_unstable();
            v
        };
        shell.faces.sort_by_key(|f| fkey(f));
    }
    cs
}

/// Round every REAL in the DATA section to 1e-9 (mm for coordinates) and
/// write it in one fixed form, so that float noise of recomputed geometry
/// (~1e-15) does not change the text. Strings and entity ids are untouched.
fn round_reals(text: &str) -> String {
    let Some(start) = text.find("\nDATA;") else {
        return text.to_string();
    };
    let (head, data) = text.split_at(start);
    let mut out = String::with_capacity(text.len());
    out.push_str(head);
    let b = data.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c as char);
            in_str = c != b'\'';
            i += 1;
            continue;
        }
        if c == b'\'' {
            in_str = true;
            out.push('\'');
            i += 1;
            continue;
        }
        let starts_number = (c.is_ascii_digit() || c == b'-' || c == b'+')
            && i > 0
            && matches!(b[i - 1], b'(' | b',' | b' ');
        if starts_number {
            let mut j = i + 1;
            while j < b.len()
                && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'.' | b'-' | b'+'))
            {
                j += 1;
            }
            let tok = &data[i..j];
            if tok.contains('.')
                && let Ok(x) = tok.parse::<f64>()
            {
                out.push_str(&step_real(x));
            } else {
                out.push_str(tok);
            }
            i = j;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// `x` rounded to 1e-9 as an ISO 10303-21 REAL: fixed notation, trailing zeros
/// trimmed but at least one decimal, never "-0.0".
fn step_real(x: f64) -> String {
    let r = (x * 1.0e9).round() / 1.0e9 + 0.0;
    let mut t = format!("{r:.9}");
    while t.ends_with('0') && !t.ends_with(".0") {
        t.pop();
    }
    t
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

/// Replace the empty label of the i-th solid entity line (in file order, which is
/// input order) with `names[i]`. The number of solid entities must equal `names.len()`.
fn label_solids(text: &str, names: &[&str]) -> Result<String> {
    const SOLIDS: [&str; 3] = [
        "MANIFOLD_SOLID_BREP('',",
        "BREP_WITH_VOIDS('',",
        "SHELL_BASED_SURFACE_MODEL('',",
    ];
    let mut out =
        String::with_capacity(text.len() + names.iter().map(|n| n.len() + 8).sum::<usize>());
    let mut next = 0usize;
    for line in text.split_inclusive('\n') {
        let solid = line.split_once(" = ").and_then(|(id, rest)| {
            if !id.starts_with('#') {
                return None;
            }
            SOLIDS
                .iter()
                .find(|p| rest.starts_with(*p))
                .map(|p| (id, &p[..p.len() - 4], &rest[p.len()..]))
        });
        match solid {
            Some((id, keyword, tail)) => {
                let name = names.get(next).ok_or_else(|| {
                    KernelError::Internal("STEP writer produced more solids than bodies".into())
                })?;
                out.push_str(&format!("{id} = {keyword}('{}',{tail}", step_string(name)));
                next += 1;
            }
            None => out.push_str(line),
        }
    }
    if next != names.len() {
        return Err(KernelError::Internal(format!(
            "STEP writer produced {next} solids for {} bodies",
            names.len()
        )));
    }
    Ok(out)
}

/// Encode `s` as the contents of an ISO 10303-21 string literal: printable ASCII kept
/// (`\` doubled), everything else as `\X2\hhhh...\X0\` UTF-16 code units. The
/// apostrophe is also written as `\X2\0027\X0\` instead of the equally valid `''`,
/// because truck-stepio's own reader (ruststep 0.4 tokenizer) rejects `''` in a string.
fn step_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut wide: Vec<u16> = Vec::new();
    let flush = |wide: &mut Vec<u16>, out: &mut String| {
        if !wide.is_empty() {
            out.push_str("\\X2\\");
            for u in wide.drain(..) {
                out.push_str(&format!("{u:04X}"));
            }
            out.push_str("\\X0\\");
        }
    };
    for c in s.chars() {
        if (' '..='~').contains(&c) && c != '\'' {
            flush(&mut wide, &mut out);
            if c == '\\' {
                out.push_str("\\\\");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            wide.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut wide, &mut out);
    out
}

/// One product of a hierarchical STEP export ([`export_step_assembly`]).
#[derive(Debug, Clone)]
pub struct StepProduct<'a> {
    /// Product name (component name).
    pub name: String,
    /// Named solids in the product's own coordinates (empty for a pure
    /// assembly).
    pub solids: Vec<(String, &'a Body)>,
    /// Usages of other products inside this one: (child product index,
    /// occurrence name, rigid placement of the child in this product's
    /// coordinates).
    pub children: Vec<(usize, String, crate::Rigid)>,
}

/// One `#N = ...;` statement of a DATA section.
struct Stmt {
    id: u64,
    body: String,
}

/// DATA statements of a STEP text (multi-line statements joined).
fn data_statements(text: &str) -> Result<Vec<Stmt>> {
    let start = text
        .find("\nDATA;\n")
        .ok_or_else(|| KernelError::Internal("STEP text without DATA section".into()))?
        + 7;
    let end = text
        .rfind("\nENDSEC;")
        .filter(|e| *e >= start)
        .ok_or_else(|| KernelError::Internal("STEP text without ENDSEC".into()))?;
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text[start..end].lines() {
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
        if line.trim_end().ends_with(';') {
            let (id, body) = cur
                .split_once(" = ")
                .ok_or_else(|| KernelError::Internal(format!("STEP statement: {cur}")))?;
            let id = id
                .trim()
                .strip_prefix('#')
                .and_then(|n| n.parse::<u64>().ok())
                .ok_or_else(|| KernelError::Internal(format!("STEP statement id: {cur}")))?;
            out.push(Stmt {
                id,
                body: body.to_string(),
            });
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        return Err(KernelError::Internal("unterminated STEP statement".into()));
    }
    Ok(out)
}

/// Rewrite every `#N` reference outside string literals through `map`.
fn renumber(body: &str, map: &dyn Fn(u64) -> u64) -> String {
    let b = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 8);
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if c == b'\'' {
            in_str = !in_str;
            out.push('\'');
            i += 1;
            continue;
        }
        if !in_str && c == b'#' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if let Ok(n) = body[i + 1..j].parse::<u64>() {
                out.push_str(&format!("#{}", map(n)));
                i = j;
                continue;
            }
        }
        // Copy one UTF-8 character.
        let ch = body[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Last `#N` reference in a statement body (truck writes the representation
/// context last).
fn last_ref(body: &str) -> Option<u64> {
    let k = body.rfind('#')?;
    body[k + 1..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Export an assembly with its product structure into one STEP AP214 file:
/// one `PRODUCT` per entry of `products` (each written once even when used
/// several times), parts carrying their solids in an
/// `ADVANCED_BREP_SHAPE_REPRESENTATION` (via a `SHAPE_REPRESENTATION_RELATIONSHIP`
/// from the product's `SHAPE_REPRESENTATION`), and every usage as a
/// `NEXT_ASSEMBLY_USAGE_OCCURRENCE` with a `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`
/// whose `ITEM_DEFINED_TRANSFORMATION` maps the child's frame to its placement
/// axis in the parent (the AP203/AP214 pattern OpenCascade writes and reads).
/// `root` is the top product. Refused: unknown indices, cyclic usage, a product
/// with neither solids nor children, non-rigid placements, no solid at all, or
/// any solid [`export_step_bodies`] refuses.
pub fn export_step_assembly(
    products: &[StepProduct],
    root: usize,
    organization_system: &str,
) -> Result<String> {
    let bad = |m: String| Err(KernelError::InvalidInput(m));
    if root >= products.len() {
        return bad("STEP assembly: root product index out of range".into());
    }
    for (i, p) in products.iter().enumerate() {
        if p.solids.is_empty() && p.children.is_empty() {
            return bad(format!("STEP assembly: product '{}' is empty", p.name));
        }
        for (c, n, t) in &p.children {
            if *c >= products.len() {
                return bad(format!("STEP assembly: '{n}' uses an unknown product"));
            }
            if *c == i {
                return bad(format!("STEP assembly: '{}' uses itself", p.name));
            }
            let r = t.r;
            let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
                - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
                + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
            let mut ok =
                (det - 1.0).abs() <= 1e-9 && r.iter().flatten().chain(&t.t).all(|x| x.is_finite());
            for a in 0..3 {
                for b in 0..3 {
                    let d: f64 = (0..3).map(|k| r[k][a] * r[k][b]).sum();
                    ok &= (d - if a == b { 1.0 } else { 0.0 }).abs() <= 1e-9;
                }
            }
            if !ok {
                return bad(format!("STEP assembly: placement of '{n}' is not rigid"));
            }
        }
    }
    // Cycles (depth-first with colours).
    fn visit(p: &[StepProduct], i: usize, st: &mut [u8]) -> bool {
        match st[i] {
            1 => return false,
            2 => return true,
            _ => {}
        }
        st[i] = 1;
        for (c, _, _) in &p[i].children {
            if !visit(p, *c, st) {
                return false;
            }
        }
        st[i] = 2;
        true
    }
    let mut st = vec![0u8; products.len()];
    for i in 0..products.len() {
        if !visit(products, i, &mut st) {
            return bad("STEP assembly: cyclic product usage".into());
        }
    }
    guarded("export_step_assembly", || -> Result<String> {
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut next: u64 = 0;
        // Shared contexts (from the first part).
        let mut ctx: Option<(u64, u64, u64)> = None; // product ctx, pd ctx, rep ctx
        let mut app_ctx: Option<u64> = None;
        // Per product: (product def id, shape rep id, extra items to list).
        let mut pd = vec![0u64; products.len()];
        let mut sr = vec![0u64; products.len()];
        let mut absr: Vec<Option<u64>> = vec![None; products.len()];
        let mut sdr_fix: Vec<Option<usize>> = vec![None; products.len()];
        for (i, p) in products.iter().enumerate() {
            if p.solids.is_empty() {
                continue;
            }
            let refs: Vec<(&str, &Body)> = p.solids.iter().map(|(n, b)| (n.as_str(), *b)).collect();
            let text = export_step_bodies(&refs, organization_system)?;
            let part = data_statements(&text)?;
            let first = app_ctx.is_none();
            let base = next;
            // In later parts, the application context (and its protocol
            // definition) map to the first part's.
            let app_local = part
                .iter()
                .find(|x| x.body.starts_with("APPLICATION_CONTEXT("))
                .map(|x| x.id);
            let app_prev = app_ctx;
            let map = |n: u64| -> u64 {
                if !first && Some(n) == app_local {
                    app_prev.unwrap_or(n + base)
                } else {
                    n + base
                }
            };
            for x in &part {
                if !first
                    && (Some(x.id) == app_local
                        || x.body.starts_with("APPLICATION_PROTOCOL_DEFINITION("))
                {
                    continue;
                }
                let id = map(x.id);
                next = next.max(id);
                let mut body = renumber(&x.body, &map);
                if body.starts_with("PRODUCT('','','',") {
                    let n = step_string(&p.name);
                    body = body.replacen("PRODUCT('','',", &format!("PRODUCT('{n}','{n}',"), 1);
                }
                if body.starts_with("PRODUCT_DEFINITION(") {
                    pd[i] = id;
                }
                if body.starts_with("SHAPE_DEFINITION_REPRESENTATION(") {
                    sdr_fix[i] = Some(stmts.len());
                }
                if body.starts_with("ADVANCED_BREP_SHAPE_REPRESENTATION(") {
                    absr[i] = Some(id);
                    if ctx.is_none() {
                        ctx = Some((0, 0, last_ref(&body).unwrap_or(0)));
                    }
                }
                if first && body.starts_with("APPLICATION_CONTEXT(") {
                    app_ctx = Some(id);
                }
                if let Some(c) = ctx.as_mut() {
                    if c.0 == 0 && body.starts_with("PRODUCT_CONTEXT(") {
                        c.0 = id;
                    }
                    if c.1 == 0 && body.starts_with("PRODUCT_DEFINITION_CONTEXT(") {
                        c.1 = id;
                    }
                }
                stmts.push(Stmt { id, body });
            }
            if pd[i] == 0 || absr[i].is_none() || sdr_fix[i].is_none() {
                return Err(KernelError::Internal(
                    "STEP writer output lacks the product entities".into(),
                ));
            }
        }
        // The first part's contexts (product/definition contexts appear
        // before the representation in truck's output, so look them up).
        let (pc, pdc, rc) = {
            let find = |k: &str| stmts.iter().find(|x| x.body.starts_with(k)).map(|x| x.id);
            match (
                find("PRODUCT_CONTEXT("),
                find("PRODUCT_DEFINITION_CONTEXT("),
                ctx.map(|c| c.2),
            ) {
                (Some(a), Some(b), Some(c)) if c != 0 => (a, b, c),
                _ => {
                    return Err(KernelError::InvalidInput(
                        "STEP assembly: no product has solids".into(),
                    ));
                }
            }
        };
        let mut push = |body: String, stmts: &mut Vec<Stmt>| -> u64 {
            next += 1;
            stmts.push(Stmt { id: next, body });
            next
        };
        let axis = |t: &crate::Rigid,
                    stmts: &mut Vec<Stmt>,
                    push: &mut dyn FnMut(String, &mut Vec<Stmt>) -> u64| {
            let r = |x: f64| step_real(x);
            let o = push(
                format!(
                    "CARTESIAN_POINT('', ({}, {}, {}));",
                    r(t.t[0]),
                    r(t.t[1]),
                    r(t.t[2])
                ),
                stmts,
            );
            let z = push(
                format!(
                    "DIRECTION('', ({}, {}, {}));",
                    r(t.r[0][2]),
                    r(t.r[1][2]),
                    r(t.r[2][2])
                ),
                stmts,
            );
            let x = push(
                format!(
                    "DIRECTION('', ({}, {}, {}));",
                    r(t.r[0][0]),
                    r(t.r[1][0]),
                    r(t.r[2][0])
                ),
                stmts,
            );
            push(format!("AXIS2_PLACEMENT_3D('', #{o}, #{z}, #{x});"), stmts)
        };
        // Product entities of pure assemblies.
        for (i, p) in products.iter().enumerate() {
            if !p.solids.is_empty() {
                continue;
            }
            let n = step_string(&p.name);
            let prod = push(format!("PRODUCT('{n}','{n}','', (#{pc}));"), &mut stmts);
            let pdf = push(
                format!("PRODUCT_DEFINITION_FORMATION('','', #{prod});"),
                &mut stmts,
            );
            pd[i] = push(
                format!("PRODUCT_DEFINITION('design','', #{pdf}, #{pdc});"),
                &mut stmts,
            );
        }
        // Placement axes: identity per product, one per usage.
        let mut items: Vec<Vec<u64>> = vec![vec![]; products.len()];
        let mut ident = vec![0u64; products.len()];
        for i in 0..products.len() {
            ident[i] = axis(&crate::Rigid::IDENTITY, &mut stmts, &mut push);
            items[i].push(ident[i]);
        }
        let mut usage_axes: Vec<Vec<u64>> = vec![vec![]; products.len()];
        for (i, p) in products.iter().enumerate() {
            for (_, _, t) in &p.children {
                let a = axis(t, &mut stmts, &mut push);
                items[i].push(a);
                usage_axes[i].push(a);
            }
        }
        // Shape representations (+ link to the solids) and SDRs.
        for (i, p) in products.iter().enumerate() {
            let list = items[i]
                .iter()
                .map(|x| format!("#{x}"))
                .collect::<Vec<_>>()
                .join(", ");
            sr[i] = push(
                format!(
                    "SHAPE_REPRESENTATION('{}', ({list}), #{rc});",
                    step_string(&p.name)
                ),
                &mut stmts,
            );
            match (absr[i], sdr_fix[i]) {
                (Some(a), Some(k)) => {
                    push(
                        format!(
                            "SHAPE_REPRESENTATION_RELATIONSHIP('','', #{a}, #{});",
                            sr[i]
                        ),
                        &mut stmts,
                    );
                    // The part's definition shape is now the shape representation.
                    let b = &stmts[k].body;
                    let pds = b
                        .trim_start_matches("SHAPE_DEFINITION_REPRESENTATION(")
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    stmts[k].body = format!("SHAPE_DEFINITION_REPRESENTATION({pds}, #{});", sr[i]);
                }
                _ => {
                    let pds = push(
                        format!("PRODUCT_DEFINITION_SHAPE('','', #{});", pd[i]),
                        &mut stmts,
                    );
                    push(
                        format!("SHAPE_DEFINITION_REPRESENTATION(#{pds}, #{});", sr[i]),
                        &mut stmts,
                    );
                }
            }
        }
        // Usages.
        for (i, p) in products.iter().enumerate() {
            for (k, (c, n, _)) in p.children.iter().enumerate() {
                let n = step_string(n);
                let idt = push(
                    format!(
                        "ITEM_DEFINED_TRANSFORMATION('','', #{}, #{});",
                        ident[*c], usage_axes[i][k]
                    ),
                    &mut stmts,
                );
                let rr = push(
                    format!(
                        "( REPRESENTATION_RELATIONSHIP('','', #{}, #{}) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt}) SHAPE_REPRESENTATION_RELATIONSHIP() );",
                        sr[*c], sr[i]
                    ),
                    &mut stmts,
                );
                let nauo = push(
                    format!(
                        "NEXT_ASSEMBLY_USAGE_OCCURRENCE('{n}','{n}','', #{}, #{}, $);",
                        pd[i], pd[*c]
                    ),
                    &mut stmts,
                );
                let pds = push(
                    format!("PRODUCT_DEFINITION_SHAPE('','', #{nauo});"),
                    &mut stmts,
                );
                push(
                    format!("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr}, #{pds});"),
                    &mut stmts,
                );
            }
        }
        let _ = root;
        // Reassemble with truck's header.
        let head_src = {
            let refs: Vec<(&str, &Body)> = products
                .iter()
                .flat_map(|p| p.solids.iter().map(|(n, b)| (n.as_str(), *b)))
                .take(1)
                .collect();
            export_step_bodies(&refs, organization_system)?
        };
        let head_end = head_src
            .find("\nDATA;\n")
            .ok_or_else(|| KernelError::Internal("STEP header".into()))?;
        let mut out = String::with_capacity(stmts.iter().map(|x| x.body.len() + 12).sum());
        out.push_str(&head_src[..head_end]);
        out.push_str("\nDATA;\n");
        for x in &stmts {
            out.push_str(&format!("#{} = {}\n", x.id, x.body));
        }
        out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        check_unique_ids(&out)?;
        Ok(round_reals(&out))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone whose generatrix is drawn at one angle while the face lies at
    /// others: the bounded generatrix must span the face's axial extent
    /// (z in [10, 12]), not the projections of its points onto the line.
    #[test]
    fn cone_generatrix_is_bounded_by_the_meridian_extent() {
        let gen_line = Line(Point3::new(3.0, 0.0, 10.0), Point3::new(1.0, 0.0, 12.0));
        let mut surface = Surface::RevolutedCurve(Processor::new(RevolutedCurve::by_revolution(
            Curve::Line(gen_line),
            Point3::origin(),
            Vector3::unit_z(),
        )));
        // Face samples at 60°–120° around the axis, on the cone.
        let samples: Vec<Point3> = (0..=8)
            .flat_map(|i| {
                let a = (60.0 + 7.5 * i as f64).to_radians();
                [(3.0, 10.0), (2.0, 11.0), (1.0, 12.0)]
                    .map(|(r, z)| Point3::new(r * a.cos(), r * a.sin(), z))
            })
            .collect();
        bounded_revolution_generatrix(&mut surface, &samples);
        let Surface::RevolutedCurve(p) = &surface else {
            panic!("not a revolution")
        };
        let c = p.entity().entity_curve();
        let (t0, t1) = c.range_tuple();
        let (a, b) = (c.subs(t0), c.subs(t1));
        assert!(
            (a.z - 10.0).abs() < 1e-5 && (b.z - 12.0).abs() < 1e-5,
            "{a:?} {b:?}"
        );
        assert!(
            (a.x - 3.0).abs() < 1e-5 && (b.x - 1.0).abs() < 1e-5,
            "{a:?} {b:?}"
        );
    }

    #[test]
    fn header_time_and_reals_are_canonical() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00");
        assert_eq!(iso8601_utc(1_791_759_825), "2026-10-11T23:03:45");
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(step_real(59.999999999999794), "60.0");
        assert_eq!(step_real(-2.1e-13), "0.0");
        assert_eq!(step_real(std::f64::consts::FRAC_1_SQRT_2), "0.707106781");
        assert_eq!(step_real(-1.5), "-1.5");
        let t = "HEADER;\nFILE_NAME('a 1.25', '');\nENDSEC;\nDATA;\n#7 = CARTESIAN_POINT('1.5', (4.0000000000000036, -1.3500311979441904E-13, 5.0000000000E-3));\n#8 = B_SPLINE_CURVE_WITH_KNOTS('', 2, (#1, #2), .UNSPECIFIED., .F., .F., (3, 3), (0.0, 1.0), .UNSPECIFIED.);\n";
        let r = round_reals(t);
        assert!(r.contains("('1.5', (4.0, 0.0, 0.005))"), "{r}");
        assert!(
            r.contains("2, (#1, #2)") && r.contains("(3, 3), (0.0, 1.0)"),
            "{r}"
        );
        assert!(r.contains("FILE_NAME('a 1.25', '')"));
    }

    #[test]
    fn file_name_lists_are_flat() {
        let t = "HEADER;\nFILE_NAME('', 'T', (('')), (('')), 'truck', 'R-CAD', '');\nENDSEC;\nDATA;\n#1 = X((('')));\n";
        let r = fix_file_name(t);
        assert!(
            r.contains("FILE_NAME('', 'T', (''), (''), 'truck', 'R-CAD', '');"),
            "{r}"
        );
        assert!(r.ends_with("#1 = X((('')));\n"), "data untouched: {r}");
        assert_eq!(fix_file_name(&r), r);
        assert_eq!(fix_file_name("DATA;\n"), "DATA;\n");
    }

    #[test]
    fn step_string_escapes() {
        assert_eq!(step_string("Plate 1"), "Plate 1");
        assert_eq!(step_string("it's a\\b"), "it\\X2\\0027\\X0\\s a\\\\b");
        assert_eq!(step_string("Ø10 hole"), "\\X2\\00D8\\X0\\10 hole");
        assert_eq!(step_string("a\u{1F600}"), "a\\X2\\D83DDE00\\X0\\");
        assert_eq!(step_string("x\ny"), "x\\X2\\000A\\X0\\y");
    }

    #[test]
    fn label_solids_counts_must_match() {
        let t = "#16 = MANIFOLD_SOLID_BREP('', #17);\n#40 = BREP_WITH_VOIDS('', #41, (#50));\n";
        assert_eq!(
            label_solids(t, &["a", "b c"]).unwrap(),
            "#16 = MANIFOLD_SOLID_BREP('a', #17);\n#40 = BREP_WITH_VOIDS('b c', #41, (#50));\n"
        );
        assert!(label_solids(t, &["a"]).is_err());
        assert!(label_solids(t, &["a", "b", "c"]).is_err());
    }
}
