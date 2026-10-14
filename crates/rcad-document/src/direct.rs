//! Direct edits of imported STEP bodies in the document (ADR 0010).
//!
//! An imported solid's faces are named by their *import index*: the position
//! in the solid's face order right after import (deterministic for the same
//! embedded bytes). While a body has been changed by nothing but its import
//! and direct edits, [`ImportedFaceIds`] maps every current face position to
//! that index: topology-preserving edits (offset, resize) keep it; a removal
//! retires the indices of the faces it deletes, and later references to them
//! are `BrokenReference`s, never another face. Once any other feature changes
//! the body (a Join/Cut, a fillet, ...), the map is no longer valid and a
//! later direct edit is refused: we never guess which new face an index means.
use crate::import::ImportedPlanarFace;
use crate::{DirectOp, DocError, FaceRef, FaceRole, FeatureId, Result};
use rcad_kernel as k;

/// Import identity of the faces of one imported body.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedFaceIds {
    /// The `ImportStep` feature that created the body.
    pub import: FeatureId,
    /// Last feature (the import or a direct edit) after which `ids` is valid.
    pub valid_at: FeatureId,
    /// Import index of each current B-rep face, in `face_iter` order (the
    /// tessellation's `tri_face`).
    pub ids: Vec<u32>,
}

impl ImportedFaceIds {
    /// Identity right after import: face `i` has import index `i`.
    pub fn at_import(import: FeatureId, faces: usize) -> Self {
        ImportedFaceIds {
            import,
            valid_at: import,
            ids: (0..faces as u32).collect(),
        }
    }

    /// The semantic reference of the B-rep face at position `pos`.
    pub fn reference(&self, body: crate::BodyId, pos: usize) -> Option<FaceRef> {
        self.ids.get(pos).map(|face| FaceRef {
            feature: self.import,
            role: FaceRole::Imported { body, face: *face },
        })
    }
}

/// Result of one direct edit: the new body, the updated face identity, and
/// the import indices retired by it.
pub(crate) struct Applied {
    pub body: k::Body,
    pub ids: ImportedFaceIds,
    pub removed: Vec<u32>,
}

/// Apply direct edit `op` of feature `id` to `body` (whose last modifying
/// feature is `last`) at the imported face `face`.
pub(crate) fn apply(
    id: FeatureId,
    face: &FaceRef,
    op: DirectOp,
    body: &k::Body,
    last: FeatureId,
    ids: Option<&ImportedFaceIds>,
    pivot: Option<rcad_kernel::Frame>,
) -> Result<Applied> {
    let FaceRole::Imported {
        body: bid,
        face: index,
    } = face.role
    else {
        return Err(DocError::Validation(format!(
            "{id}: a direct edit needs a face of an imported body"
        )));
    };
    let broken = |detail: String| DocError::BrokenReference { from: id, detail };
    let ids = ids
        .filter(|p| p.import == face.feature)
        .ok_or_else(|| broken(format!("{} did not import body {bid}", face.feature)))?;
    if ids.valid_at != last {
        return Err(broken(format!(
            "body {bid} was changed by {last} after its import and direct edits; imported \
             face {index} can no longer be identified (direct edits must come before other \
             features on an imported body)"
        )));
    }
    let pos = ids.ids.iter().position(|i| *i == index).ok_or_else(|| {
        broken(format!(
            "imported face {index} of body {bid} no longer exists (removed by an earlier \
             direct edit)"
        ))
    })?;
    let what = match op {
        DirectOp::OffsetFace { .. } => "offset face",
        DirectOp::ResizeCylinder { .. } => "resize hole/boss",
        DirectOp::RemoveCylinder => "remove hole/boss",
        DirectOp::ReplaceFace { .. } => "replace face",
    };
    let out = match op {
        DirectOp::OffsetFace { distance } => k::offset_face(body, pos, distance),
        DirectOp::ResizeCylinder { diameter } => k::resize_cylinder(body, pos, diameter),
        DirectOp::RemoveCylinder => k::remove_cylinder(body, pos),
        DirectOp::ReplaceFace { distance, angle } => {
            let Some(frame) = pivot else {
                return Err(DocError::ProductLimit(format!(
                    "{id}: imported face {index} is not planar; only planar faces can be replaced"
                )));
            };
            if !(distance.is_finite() && angle.is_finite() && angle.abs() < 89f64.to_radians()) {
                return Err(DocError::Validation(format!(
                    "{id}: replace face: distance must be finite and the tilt below 89 degrees"
                )));
            }
            let f = k::tilt_offset_frame(&frame, distance, angle);
            k::replace_face(body, pos, f.origin, f.z)
        }
    }
    .map_err(|e| match e {
        k::KernelError::InvalidInput(m) => {
            DocError::ProductLimit(format!("{id}: {what} (imported face {index}): {m}"))
        }
        other => DocError::Kernel {
            feature: id,
            detail: format!("{what} (imported face {index}): {other}"),
        },
    })?;
    let removed: Vec<u32> = out.removed_faces.iter().map(|p| ids.ids[*p]).collect();
    let kept = ids
        .ids
        .iter()
        .enumerate()
        .filter(|(p, _)| out.removed_faces.binary_search(p).is_err())
        .map(|(_, i)| *i)
        .collect();
    Ok(Applied {
        body: out.body,
        ids: ImportedFaceIds {
            import: ids.import,
            valid_at: id,
            ids: kept,
        },
        removed,
    })
}

/// Keep the imported planar-face table (sketch planes) in step with an edit:
/// an offset face's frame moves with it (sketches on it follow); faces removed
/// by the edit leave the table, so references to them are broken.
pub(crate) fn update_planar_table(
    table: &mut Vec<ImportedPlanarFace>,
    face: u32,
    op: DirectOp,
    removed: &[u32],
) {
    if let Some(f) = table.iter_mut().find(|f| f.index == face) {
        match op {
            DirectOp::OffsetFace { distance } => f.frame = f.frame.offset(distance),
            DirectOp::ReplaceFace { distance, angle } => {
                f.frame = rcad_kernel::tilt_offset_frame(&f.frame, distance, angle)
            }
            _ => {}
        }
    }
    table.retain(|f| !removed.contains(&f.index));
}
