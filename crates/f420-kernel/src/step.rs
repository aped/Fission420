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

/// Export `body` as a STEP file string. Fails (never approximates) on unsupported edges.
pub fn export_step(body: &Body, organization_system: &str) -> Result<String> {
    guarded("export_step", || -> Result<String> {
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
