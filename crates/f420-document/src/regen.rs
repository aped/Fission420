//! Deterministic regeneration of the feature history into B-rep geometry.
use crate::{DocError, Document, FaceRef, FaceRole, FeatureId, FeatureKind, Result, SketchPlane};
use f420_kernel::{self as k, Body, Frame, Measurements};
use f420_sketch::{Profile as SketchProfile, SolveReport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a semantic face reference resolved in this evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFace {
    pub reference: FaceRef,
    /// Placement frame of the face (normal = outward face normal).
    pub frame: Frame,
    /// Number of B-rep planar faces matching the frame in the body at the time of
    /// resolution (must be exactly 1).
    pub matching_brep_faces: usize,
}

/// Per-feature evaluation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureResult {
    pub id: FeatureId,
    pub name: String,
    /// Sketch features: solver report (residual, DOF, status).
    pub solve: Option<SolveReport>,
    /// Sketch features: placement frame actually used.
    pub frame: Option<Frame>,
    /// Sketch features on faces: how the reference resolved.
    pub resolved_face: Option<ResolvedFace>,
    /// Solid-producing features: measurements of the body after this feature.
    pub measurements: Option<Measurements>,
}

/// Disposable geometry cache for a document.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub features: Vec<FeatureResult>,
    pub body: Option<Body>,
    pub measurements: Option<Measurements>,
}

struct SketchOut {
    frame: Frame,
    profile: SketchProfile,
}

struct ExtrudeOut {
    start: Frame,
    distance: f64,
    profile: SketchProfile,
}

fn to_kernel(p: &SketchProfile) -> k::Profile {
    match p {
        SketchProfile::Polygon(v) => k::Profile::Polygon(v.clone()),
        SketchProfile::Circle { center, radius } => k::Profile::Circle {
            center: *center,
            radius: *radius,
        },
    }
}

/// Product limit (not a general CAD rule): a circular cut must lie strictly inside the
/// polygonal profile of the extrusion whose face it is sketched on, in that face's frame.
/// For a centred rectangle this means diameter < min(width, height).
fn check_hole_inside(hole: &SketchProfile, host: &SketchProfile) -> Result<()> {
    let (SketchProfile::Circle { center, radius }, SketchProfile::Polygon(poly)) = (hole, host)
    else {
        return Err(DocError::ProductLimit(
            "only a circular cut sketched on a polygonal extrusion face is supported".into(),
        ));
    };
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let len = ex.hypot(ey);
        // Signed distance of the centre to the edge's supporting line (inside > 0 for CCW).
        let d = (ex * (center[1] - a[1]) - ey * (center[0] - a[0])) / len;
        if d <= *radius + k::tol::MIN_LENGTH {
            return Err(DocError::ProductLimit(format!(
                "hole (diameter {:.6} mm) must lie strictly inside the face profile; \
                 clearance to an edge is {:.6} mm (initial product limit)",
                2.0 * radius,
                d - radius
            )));
        }
    }
    Ok(())
}

/// Express a circular cut profile (in `cut_frame`) in the host extrusion's start frame.
/// The cut axis must be parallel to the extrusion direction (product limit).
fn hole_in_host_frame(
    hole: &SketchProfile,
    cut_frame: &Frame,
    host: &Frame,
) -> Result<SketchProfile> {
    let SketchProfile::Circle { center, radius } = hole else {
        return Err(DocError::ProductLimit(
            "only circular cuts are supported".into(),
        ));
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    if (dot(cut_frame.z, host.z).abs() - 1.0).abs() > 1e-9 {
        return Err(DocError::ProductLimit(
            "cut axis must be parallel to the base extrusion direction".into(),
        ));
    }
    let w = cut_frame.to_world(*center);
    let d = [
        w[0] - host.origin[0],
        w[1] - host.origin[1],
        w[2] - host.origin[2],
    ];
    Ok(SketchProfile::Circle {
        center: [dot(d, host.x), dot(d, host.y)],
        radius: *radius,
    })
}

/// Regenerate the whole document. Pure function of the document; deterministic.
pub fn regenerate(doc: &Document) -> Result<Evaluation> {
    let order = doc.evaluation_order()?;
    let mut sketches: HashMap<FeatureId, SketchOut> = HashMap::new();
    let mut extrudes: HashMap<FeatureId, ExtrudeOut> = HashMap::new();
    let mut body: Option<Body> = None;
    // Index (into `results`) of the feature that produced the current body. Only the
    // final body is measured: intermediate measurements cost ~1 s each.
    let mut last_solid: Option<usize> = None;
    let mut results = Vec::new();

    for id in order {
        let f = doc.feature(id)?;
        let mut r = FeatureResult {
            id,
            name: f.name.clone(),
            solve: None,
            frame: None,
            resolved_face: None,
            measurements: None,
        };
        match &f.kind {
            FeatureKind::Sketch { plane, sketch } => {
                let (frame, resolved) = match plane {
                    SketchPlane::WorldXY => (Frame::XY, None),
                    SketchPlane::WorldXZ => (Frame::XZ, None),
                    SketchPlane::Face(fr) => {
                        let rf = resolve_face(id, fr, &extrudes, body.as_ref())?;
                        (rf.frame, Some(rf))
                    }
                };
                let mut s = sketch.clone();
                let report = s.solve();
                if !report.is_success() {
                    return Err(DocError::SketchSolve {
                        feature: id,
                        detail: format!(
                            "{:?}, residual {:.3e} mm, conflicting constraints {:?}",
                            report.status, report.residual, report.conflicting
                        ),
                    });
                }
                let profile = s.profile().map_err(|e| DocError::SketchSolve {
                    feature: id,
                    detail: e.to_string(),
                })?;
                r.solve = Some(report);
                r.frame = Some(frame);
                r.resolved_face = resolved;
                sketches.insert(id, SketchOut { frame, profile });
            }
            FeatureKind::Revolve { sketch, angle } => {
                let s = sketches.get(sketch).ok_or(DocError::BrokenReference {
                    from: id,
                    detail: format!("{sketch} is not a sketch"),
                })?;
                if body.is_some() {
                    return Err(DocError::ProductLimit(
                        "only one base body (extrude or revolve) per document is supported".into(),
                    ));
                }
                let b = k::revolve(&to_kernel(&s.profile), &s.frame, *angle)
                    .map_err(|e| DocError::Validation(format!("{id}: {e}")))?;
                last_solid = Some(results.len());
                body = Some(b);
            }
            FeatureKind::Extrude { sketch, distance } => {
                let s = sketches.get(sketch).ok_or(DocError::BrokenReference {
                    from: id,
                    detail: format!("{sketch} is not a sketch"),
                })?;
                if body.is_some() {
                    return Err(DocError::ProductLimit(
                        "only one base extrusion per document is supported".into(),
                    ));
                }
                if !distance.is_finite() || *distance <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "{id}.distance: extrusion thickness must be > 0 mm, got {distance}"
                    )));
                }
                let b = k::extrude(&to_kernel(&s.profile), &s.frame, *distance).map_err(|e| {
                    DocError::Kernel {
                        feature: id,
                        detail: e.to_string(),
                    }
                })?;
                extrudes.insert(
                    id,
                    ExtrudeOut {
                        start: s.frame,
                        distance: *distance,
                        profile: s.profile.clone(),
                    },
                );
                last_solid = Some(results.len());
                body = Some(b);
            }
            FeatureKind::Cut { sketch, .. } => {
                let s = sketches.get(sketch).ok_or(DocError::BrokenReference {
                    from: id,
                    detail: format!("{sketch} is not a sketch"),
                })?;
                let target = body.as_ref().ok_or(DocError::BrokenReference {
                    from: id,
                    detail: "cut has no body to cut".into(),
                })?;
                // Product limit: every cut must lie inside the base extrusion's profile,
                // evaluated in the base profile's own frame (not the cut sketch's frame).
                let host = extrudes.values().next().ok_or(DocError::ProductLimit(
                    "cuts are only supported on an extruded base body".into(),
                ))?;
                let hole = hole_in_host_frame(&s.profile, &s.frame, &host.start)?;
                check_hole_inside(&hole, &host.profile)?;
                let b =
                    k::cut_through_all(target, &to_kernel(&s.profile), &s.frame).map_err(|e| {
                        DocError::Kernel {
                            feature: id,
                            detail: e.to_string(),
                        }
                    })?;
                // A cut that removes material always adds faces; containment (checked
                // above) guarantees the cutter intersects the body.
                if k::face_count(&b) <= k::face_count(target) {
                    return Err(DocError::Kernel {
                        feature: id,
                        detail: "cut removed no material".into(),
                    });
                }
                last_solid = Some(results.len());
                body = Some(b);
            }
        }
        results.push(r);
    }
    let measurements = match (&body, last_solid) {
        (Some(b), Some(i)) => {
            let m = measure(results[i].id, b)?;
            results[i].measurements = Some(m.clone());
            Some(m)
        }
        _ => None,
    };
    Ok(Evaluation {
        features: results,
        body,
        measurements,
    })
}

fn measure(id: FeatureId, b: &Body) -> Result<Measurements> {
    k::measure(b).map_err(|e| DocError::Kernel {
        feature: id,
        detail: e.to_string(),
    })
}

/// Resolve a semantic face reference from the generating feature's definition, then
/// confirm it against the current B-rep: exactly one planar face must coincide.
fn resolve_face(
    from: FeatureId,
    fr: &FaceRef,
    extrudes: &HashMap<FeatureId, ExtrudeOut>,
    body: Option<&Body>,
) -> Result<ResolvedFace> {
    let e = extrudes.get(&fr.feature).ok_or(DocError::BrokenReference {
        from,
        detail: format!("{} is not an evaluated extrusion", fr.feature),
    })?;
    let frame = match fr.role {
        FaceRole::ExtrudeEndCap => e.start.offset(e.distance),
        FaceRole::ExtrudeStartCap => {
            // Outward normal of the start cap points against the extrusion direction;
            // flip y to keep the frame right-handed.
            let s = e.start;
            Frame {
                origin: s.origin,
                x: s.x,
                y: s.y.map(|c| -c),
                z: s.z.map(|c| -c),
            }
        }
    };
    let body = body.ok_or(DocError::BrokenReference {
        from,
        detail: "no body to reference".into(),
    })?;
    let n = k::planar_faces_on(body, &frame);
    if n != 1 {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "{:?} of {} matches {n} B-rep faces (expected exactly 1); refusing to guess",
                fr.role, fr.feature
            ),
        });
    }
    Ok(ResolvedFace {
        reference: *fr,
        frame,
        matching_brep_faces: n,
    })
}
