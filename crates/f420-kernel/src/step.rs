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
        let cs = exact_compressed(body)?;
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
                exact_compressed(body).map_err(|e| match e {
                    KernelError::InvalidInput(m) => {
                        KernelError::InvalidInput(format!("body {i} ('{name}'): {m}"))
                    }
                    other => other,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let models: out::StepModels<_, _, _> = solids.iter().collect();
        let text = out::CompleteStepDisplay::new(
            models,
            out::StepHeaderDescriptor {
                organization_system: organization_system.into(),
                ..Default::default()
            },
        )
        .to_string();
        check_unique_ids(&text)?;
        let names: Vec<&str> = bodies.iter().map(|(n, _)| *n).collect();
        label_solids(&text, &names)
    })?
}

/// Compressed copy of `body` with every `IntersectionCurve` edge replaced by its
/// verified exact curve; refuses (InvalidInput) when an edge cannot be rebuilt.
fn exact_compressed(body: &Body) -> Result<CompressedSolid<Point3, Curve, Surface>> {
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
