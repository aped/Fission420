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
        let text =
            out::CompleteStepDisplay::new(out::StepModel::from(&cs), header(organization_system))
                .to_string();
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
        let models: out::StepModels<_, _, _> = solids.iter().collect();
        let text = out::CompleteStepDisplay::new(models, header(organization_system)).to_string();
        check_unique_ids(&text)?;
        let names: Vec<&str> = bodies.iter().map(|(n, _)| *n).collect();
        label_solids(&round_reals(&fix_file_name(&text)), &names)
    })?
}

/// Compressed copy of `body` with every `IntersectionCurve` edge replaced by its
/// verified exact curve; refuses (InvalidInput) when an edge cannot be rebuilt.
fn exact_compressed(body: &Body) -> Result<CompressedSolid<Point3, Curve, Surface>> {
    // Run 7: an open surface body is not a solid; writing it as a
    // MANIFOLD_SOLID_BREP would be a malformed file.
    if body.is_open_shell() {
        return Err(KernelError::InvalidInput(
            "the body is an open surface (no solid): STEP export of surface bodies is not supported; stitch, fill or thicken it first".into(),
        ));
    }
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
    }
    Ok(cs)
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
    const SOLIDS: [&str; 2] = ["MANIFOLD_SOLID_BREP('',", "BREP_WITH_VOIDS('',"];
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

#[cfg(test)]
mod tests {
    use super::*;

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
