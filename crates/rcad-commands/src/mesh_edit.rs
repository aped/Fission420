//! Mesh edit and inspection commands (docs/MESH.md):
//! * `edit_mesh` previews (report only, nothing changes) or applies a
//!   transform, plane cut or decimation of a mesh body as new body/bodies
//!   (`FeatureKind::MeshEdit`, one undo step); the source is kept (optionally
//!   hidden).
//! * `inspect_mesh` runs the bounded self-intersection check of a mesh body
//!   and returns it with the body's diagnosis (read only).
use crate::{Command, Response, Session};
use rcad_document::{BodyId, DocError, FeatureKind};
use rcad_exchange::mesh::{
    MeshDiagnostics, SelfIntersectionLimits, SelfIntersectionReport, self_intersections,
};
use rcad_exchange::mesh_edit::{CutKeep, MeshEditOp, edit};
use serde::{Deserialize, Serialize};

/// Arguments of `edit_mesh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditMeshArgs {
    pub body: BodyId,
    pub op: MeshEditOp,
    /// Only report what the edit would do (no feature, no undo step).
    #[serde(default)]
    pub preview: bool,
    /// Name of the new body (a two-sided cut adds " (above)" for the second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_name: Option<String>,
    /// Hide the source body after a committed edit.
    #[serde(default)]
    pub hide_source: bool,
}

/// Arguments of `inspect_mesh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectMeshArgs {
    pub body: BodyId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<SelfIntersectionLimits>,
}

/// `Response.mesh_inspection`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshInspection {
    pub body: BodyId,
    pub diagnostics: MeshDiagnostics,
    pub self_intersection: SelfIntersectionReport,
}

impl Session {
    fn source_mesh(
        &mut self,
        body: BodyId,
        what: &str,
    ) -> Result<rcad_document::MeshBody, DocError> {
        let m = self.model_mut()?;
        let eb = m
            .evaluation()
            .body_by_id(body)
            .ok_or_else(|| DocError::Validation(format!("{body} is not in the evaluated model")))?;
        let mesh = eb.geometry.mesh().ok_or_else(|| {
            DocError::ProductLimit(format!(
                "{body} is an exact (B-rep) body; {what} applies to mesh bodies (STL/3MF)"
            ))
        })?;
        Ok(mesh.clone())
    }

    /// `edit_mesh` / `inspect_mesh`; `Ok(false)` when `cmd` is neither.
    pub(crate) fn run_mesh_edit(
        &mut self,
        cmd: &Command,
        r: &mut Response,
    ) -> Result<bool, DocError> {
        match cmd {
            Command::InspectMesh(a) => {
                let mesh = self.source_mesh(a.body, "mesh inspection")?;
                let lim = a.limits.unwrap_or_default();
                let si = self_intersections(&mesh.mesh.positions, &mesh.mesh.triangles, &lim);
                r.message = Some(inspection_summary(&si));
                r.mesh_inspection = Some(MeshInspection {
                    body: a.body,
                    diagnostics: mesh.diagnostics.clone(),
                    self_intersection: si,
                });
                Ok(true)
            }
            Command::EditMesh(a) => {
                let a = a.clone();
                a.op.validate().map_err(DocError::Validation)?;
                let mesh = self.source_mesh(a.body, "mesh editing")?;
                let (_, report) = edit(&mesh.mesh.positions, &mesh.mesh.triangles, &a.op)
                    .map_err(|e| DocError::Validation(format!("mesh {}: {e}", a.op.name())))?;
                let text = rcad_document::mesh_edit::summary(&report);
                if a.preview {
                    r.message = Some(format!("preview: {text}"));
                    r.mesh_edit = Some(report);
                    return Ok(true);
                }
                let two = a.op.outputs() == 2;
                let m = self.model_mut()?;
                let id = m.transact(&format!("Mesh {}", a.op.name()), |d| {
                    let src = d.body(a.body)?.name.clone();
                    let suffix = match a.op {
                        MeshEditOp::Transform(_) => "transformed",
                        MeshEditOp::PlaneCut(c) if c.keep == CutKeep::Above => "above",
                        MeshEditOp::PlaneCut(_) if two => "below",
                        MeshEditOp::PlaneCut(_) => "cut",
                        MeshEditOp::Decimate(_) => "decimated",
                    };
                    let n = match &a.body_name {
                        Some(n) => crate::valid_name(n, "body")?,
                        None => crate::unique_body_name(d, &format!("{src} {suffix}")),
                    };
                    if d.bodies.iter().any(|b| b.name == n) {
                        return Err(DocError::Validation(format!(
                            "another body is already named '{n}'"
                        )));
                    }
                    let body = d.add_derived_body(n.clone(), a.body);
                    let second_body = if two {
                        let n2 = crate::unique_body_name(
                            d,
                            &match &a.body_name {
                                Some(n) => format!("{n} (above)"),
                                None => format!("{src} above"),
                            },
                        );
                        let b2 = d.add_body_unowned(n2);
                        if let Some(c) = d
                            .assembly
                            .components
                            .iter_mut()
                            .find(|c| c.bodies.contains(&body))
                        {
                            c.bodies.push(b2);
                        }
                        Some(b2)
                    } else {
                        None
                    };
                    if a.hide_source {
                        d.body_mut(a.body)?.visible = false;
                    }
                    let name = d.default_name(match a.op {
                        MeshEditOp::Transform(_) => "Mesh transform",
                        MeshEditOp::PlaneCut(_) => "Mesh plane cut",
                        MeshEditOp::Decimate(_) => "Mesh decimate",
                    });
                    Ok(d.push(
                        name,
                        FeatureKind::MeshEdit {
                            source: a.body,
                            op: a.op,
                            body,
                            second_body,
                        },
                    ))
                })?;
                r.feature = Some(id);
                r.message = Some(text);
                r.mesh_edit = Some(report);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// One-line summary of a self-intersection check.
pub fn inspection_summary(s: &SelfIntersectionReport) -> String {
    let verdict = match (s.intersecting_pairs, s.complete) {
        (0, true) => {
            "no self-intersections found (checked to tolerance; not an exact proof)".to_string()
        }
        (0, false) => "none found in a PARTIAL check (bounded; not a proof)".to_string(),
        (n, true) => format!("{n} intersecting triangle pairs"),
        (n, false) => format!("at least {n} intersecting triangle pairs (partial check)"),
    };
    format!(
        "self-intersection: {verdict}; {} of {} triangles, {} pair tests",
        s.triangles_checked, s.triangles_total, s.pairs_tested
    )
}
