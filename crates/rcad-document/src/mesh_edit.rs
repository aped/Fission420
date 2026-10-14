//! Mesh edit features (`FeatureKind::MeshEdit`, docs/MESH.md): editable
//! parameters of each operation and evaluation through
//! `rcad_exchange::mesh_edit` (which defines every operation exactly).
use crate::regen::MeshBody;
use crate::{DocError, FeatureId, Result};
use rcad_exchange::mesh_edit::{MeshEditOp, MeshEditReport, edit};

/// Named parameters of a mesh edit (lengths mm, angles degrees).
pub(crate) fn parameters(op: &MeshEditOp) -> Vec<(String, f64)> {
    match op {
        MeshEditOp::Transform(t) => vec![
            ("sx".into(), t.scale[0]),
            ("sy".into(), t.scale[1]),
            ("sz".into(), t.scale[2]),
            ("angle_deg".into(), t.angle_deg),
            ("tx".into(), t.translate[0]),
            ("ty".into(), t.translate[1]),
            ("tz".into(), t.translate[2]),
        ],
        MeshEditOp::PlaneCut(c) => vec![("offset".into(), c.offset)],
        MeshEditOp::Decimate(d) => vec![
            ("target_triangles".into(), d.target_triangles as f64),
            ("max_deviation".into(), d.max_deviation),
        ],
    }
}

/// Set one parameter; the edited operation must validate.
pub(crate) fn set_parameter(
    id: FeatureId,
    op: &mut MeshEditOp,
    name: &str,
    value: f64,
) -> Result<()> {
    let mut trial = *op;
    let unknown = || DocError::UnknownParameter {
        feature: id,
        name: name.into(),
    };
    match &mut trial {
        MeshEditOp::Transform(t) => match name {
            "sx" => t.scale[0] = value,
            "sy" => t.scale[1] = value,
            "sz" => t.scale[2] = value,
            "angle_deg" => t.angle_deg = value,
            "tx" => t.translate[0] = value,
            "ty" => t.translate[1] = value,
            "tz" => t.translate[2] = value,
            _ => return Err(unknown()),
        },
        MeshEditOp::PlaneCut(c) => match name {
            "offset" => c.offset = value,
            _ => return Err(unknown()),
        },
        MeshEditOp::Decimate(d) => match name {
            "target_triangles" => {
                if !(value >= 0.0 && value.fract() == 0.0 && value <= 1e9) {
                    return Err(DocError::Validation(format!(
                        "{id}.target_triangles: a whole number >= 0 (0 = no target), got {value}"
                    )));
                }
                d.target_triangles = value as u64;
            }
            "max_deviation" => d.max_deviation = value,
            _ => return Err(unknown()),
        },
    }
    trial
        .validate()
        .map_err(|e| DocError::Validation(format!("{id}.{name}: {e}")))?;
    *op = trial;
    Ok(())
}

/// One-line summary of an edit report (shared by notes, command messages and UI).
pub fn summary(r: &MeshEditReport) -> String {
    let mut parts = vec![format!(
        "{}: {} triangles in, {} out",
        r.op.name(),
        r.before.triangles,
        r.after
            .iter()
            .map(|d| d.triangles.to_string())
            .collect::<Vec<_>>()
            .join(" + ")
    )];
    if r.vertices_moved > 0 {
        parts.push(format!("{} vertices moved", r.vertices_moved));
    }
    if r.triangles_cut > 0 || r.cut_loops > 0 {
        parts.push(format!(
            "{} triangles clipped, {} removed, {} cut loops ({} capped, {} cap triangles)",
            r.triangles_cut, r.triangles_removed, r.cut_loops, r.capped_loops, r.triangles_added
        ));
    }
    if r.collapses > 0 || matches!(r.op, MeshEditOp::Decimate(_)) {
        parts.push(format!(
            "{} edge collapses; refused: {} topology, {} normal flip, {} deviation",
            r.collapses, r.refused_topology, r.refused_normal, r.refused_deviation
        ));
    }
    if let Some(d) = &r.deviation {
        parts.push(format!(
            "measured deviation max {:.6} mm, mean {:.6} mm (sampled)",
            d.max,
            0.5 * (d.source_to_result_mean + d.result_to_source_mean)
        ));
    }
    for (i, a) in r.after.iter().enumerate() {
        let tag = if r.after.len() > 1 {
            format!("result {}", i + 1)
        } else {
            "result".into()
        };
        parts.push(match a.signed_volume {
            Some(v) => format!("{tag} closed, volume {v:.6} mm³"),
            None => format!("{tag} open ({} boundary edges)", a.boundary_edges),
        });
    }
    parts.join("; ")
}

/// Evaluate a mesh edit of a mesh body: the new mesh bodies and notes.
pub(crate) fn edited(m: &MeshBody, op: &MeshEditOp) -> Result<(Vec<MeshBody>, Vec<String>)> {
    let (meshes, rep) = edit(&m.mesh.positions, &m.mesh.triangles, op)
        .map_err(|e| DocError::Validation(format!("mesh {}: {e}", op.name())))?;
    let mut notes = vec![format!("mesh {}", summary(&rep))];
    notes.extend(rep.notes.iter().cloned());
    let mut out = vec![];
    for ((p, t), d) in meshes.iter().zip(rep.after) {
        notes.extend(crate::import::mesh_notes(&d));
        out.push(crate::import::mesh_body(p, t, d));
    }
    Ok((out, notes))
}
