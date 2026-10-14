//! Mesh repair commands (docs/MESH.md):
//! `repair_mesh` previews (report only, nothing changes) or applies a bounded
//! repair of a mesh body as a new body (`FeatureKind::MeshRepair`, one undo
//! step); the source body is kept (optionally hidden).
use crate::{Command, Response, Session};
use rcad_document::{BodyId, DocError, FeatureKind};
use rcad_exchange::mesh_repair::{RepairOptions, RepairReport, repair};
use serde::{Deserialize, Serialize};

/// Arguments of `repair_mesh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairMeshArgs {
    pub body: BodyId,
    #[serde(default)]
    pub options: RepairOptions,
    /// Only report what the repair would change (no feature, no undo step).
    #[serde(default)]
    pub preview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_name: Option<String>,
    /// Hide the source body after a committed repair.
    #[serde(default)]
    pub hide_source: bool,
}

impl Session {
    /// Mesh commands; `Ok(false)` when `cmd` is not one.
    pub(crate) fn run_mesh(&mut self, cmd: &Command, r: &mut Response) -> Result<bool, DocError> {
        let Command::RepairMesh(a) = cmd else {
            return Ok(false);
        };
        let a = a.clone();
        a.options.validate().map_err(DocError::Validation)?;
        let m = self.model_mut()?;
        let eb = m.evaluation().body_by_id(a.body).ok_or_else(|| {
            DocError::Validation(format!("{} is not in the evaluated model", a.body))
        })?;
        let mesh = eb.geometry.mesh().ok_or_else(|| {
            DocError::ProductLimit(format!(
                "{} is an exact (B-rep) body; mesh repair applies to mesh bodies (STL/3MF)",
                a.body
            ))
        })?;
        let (_, _, report): (_, _, RepairReport) =
            repair(&mesh.mesh.positions, &mesh.mesh.triangles, &a.options)
                .map_err(DocError::Validation)?;
        if a.preview {
            r.message = Some(summary(&report, true));
            r.mesh_repair = Some(report);
            return Ok(true);
        }
        let id = m.transact("Repair mesh", |d| {
            let src = d.body(a.body)?.name.clone();
            let n = match &a.body_name {
                Some(n) => crate::valid_name(n, "body")?,
                None => crate::unique_body_name(d, &format!("{src} repaired")),
            };
            if d.bodies.iter().any(|b| b.name == n) {
                return Err(DocError::Validation(format!(
                    "another body is already named '{n}'"
                )));
            }
            // The repaired copy takes the source's place in
            // its component (if any), never joins the active one by accident.
            let body = d.add_derived_body(n, a.body);
            if a.hide_source {
                d.body_mut(a.body)?.visible = false;
            }
            let name = d.default_name("Mesh repair");
            Ok(d.push(
                name,
                FeatureKind::MeshRepair {
                    source: a.body,
                    options: a.options,
                    body,
                },
            ))
        })?;
        r.feature = Some(id);
        r.message = Some(summary(&report, false));
        r.mesh_repair = Some(report);
        Ok(true)
    }
}

/// One-line summary of a repair report.
pub fn summary(r: &RepairReport, preview: bool) -> String {
    format!(
        "{}{} merged vertices ({} by tolerance), {} degenerate, {} duplicate, {} flipped, \
         {} holes filled, {} left open; before: {}, after: {}",
        if preview { "preview: " } else { "" },
        r.welded_vertices,
        r.welded_by_tolerance,
        r.removed_degenerate,
        r.removed_duplicates,
        r.flipped_triangles,
        r.filled_holes,
        r.unfilled_holes,
        state(&r.before),
        state(&r.after)
    )
}

fn state(d: &rcad_exchange::mesh::MeshDiagnostics) -> String {
    if d.closed {
        format!("closed, {} triangles", d.triangles)
    } else {
        format!(
            "open ({} boundary, {} non-manifold, {} misoriented edges), {} triangles",
            d.boundary_edges, d.non_manifold_edges, d.inconsistent_orientation_edges, d.triangles
        )
    }
}
