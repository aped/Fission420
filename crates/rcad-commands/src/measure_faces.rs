//! Exact face measurements (`measure_faces`): the analytic surface
//! of a picked face (plane or circular cylinder, with radius and diameter) and the
//! distance or angle to a second face. World frame; mm and degrees. Faces are named
//! by body and B-rep face index (the display face order of the body).
use rcad_document::{BodyId, DocError, Model};
use rcad_kernel::{self as k, DirectFace, FacePairMeasure};
use serde::{Deserialize, Serialize};

/// A face of a body: B-rep face index in face order (= the body's display face
/// order, so a client maps a picked display group `g` to `g - groups.start`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacePick {
    pub body: BodyId,
    pub face: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceMeasureReport {
    /// "world; mm, degrees".
    pub units: String,
    pub a: DirectFace,
    /// Cylinder `a`: radius and diameter (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<DirectFace>,
    /// Distance / angle between `a` and `b`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<FacePairMeasure>,
    /// Area of planar face `a` (mm², exact for line and arc
    /// boundaries, holes subtracted); None for other faces and for planar
    /// faces whose boundary has other curves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<f64>,
}

pub(crate) fn classify(model: &Model, p: FacePick) -> Result<DirectFace, DocError> {
    let ev = model.evaluation();
    let b = ev.body_by_id(p.body).ok_or_else(|| {
        DocError::Validation(format!("body {} is not in the evaluated model", p.body))
    })?;
    let g = b.geometry.brep().ok_or_else(|| {
        DocError::ProductLimit(format!(
            "body {} is a triangle mesh: no exact faces to measure",
            p.body
        ))
    })?;
    let n = k::face_count(g);
    if p.face as usize >= n {
        return Err(DocError::Validation(format!(
            "body {} has {n} faces; face {} does not exist",
            p.body, p.face
        )));
    }
    k::face_geometry(g, p.face as usize).map_err(|e| DocError::Validation(e.to_string()))
}

pub fn report(
    model: &Model,
    a: FacePick,
    b: Option<FacePick>,
) -> Result<FaceMeasureReport, DocError> {
    let fa = classify(model, a)?;
    let (radius, diameter) = match fa {
        DirectFace::Cylinder { radius, .. } => (Some(radius), Some(2.0 * radius)),
        _ => (None, None),
    };
    let (fb, pair) = match b {
        Some(b) => {
            let fb = classify(model, b)?;
            let pair = k::measure_face_pair(&fa, &fb).map_err(|e| match e {
                k::KernelError::InvalidInput(m) => DocError::ProductLimit(m),
                other => DocError::Validation(other.to_string()),
            })?;
            (Some(fb), Some(pair))
        }
        None => (None, None),
    };
    if matches!(fa, DirectFace::Other) && b.is_none() {
        return Err(DocError::ProductLimit(
            "exact face measurement supports planes and circular cylinders only".into(),
        ));
    }
    let area = match fa {
        DirectFace::Plane { .. } => {
            let g = model
                .evaluation()
                .body_by_id(a.body)
                .and_then(|b| b.geometry.brep());
            // Faces the exact area does not support
            // (ellipse/spline boundaries, multi-shell bodies) still measure
            // their plane; the area is then omitted, never estimated.
            g.and_then(|g| k::planar_face_area(g, a.face as usize).ok())
        }
        _ => None,
    };
    Ok(FaceMeasureReport {
        units: "world; mm, degrees".into(),
        a: fa,
        radius,
        diameter,
        b: fb,
        pair,
        area,
    })
}
