//! Deterministic regeneration of the feature history into B-rep geometry.
use crate::hole::{check_hole_inside, to_kernel};
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
    /// Sketch features: the solved sketch (display / editor seed; not authoritative).
    #[serde(default)]
    pub solved_sketch: Option<f420_sketch::Sketch>,
    /// Sketch features: why the geometry is not a usable profile (open loop,
    /// self-intersection, ...). Such sketches are kept but cannot be extruded/cut.
    #[serde(default)]
    pub profile_error: Option<String>,
}

/// Disposable geometry cache for a document.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub features: Vec<FeatureResult>,
    pub body: Option<Body>,
    pub measurements: Option<Measurements>,
}

#[derive(Clone)]
struct SketchOut {
    frame: Frame,
    /// `Err(reason)` when the sketch is not a single closed profile.
    profile: std::result::Result<SketchProfile, String>,
}

impl SketchOut {
    /// The profile, or a structured error naming the consuming feature.
    fn usable(&self, user: FeatureId, sketch: FeatureId) -> Result<&SketchProfile> {
        self.profile.as_ref().map_err(|why| {
            DocError::Validation(format!(
                "{user}: sketch {sketch} has no usable profile ({why}); \
                 close the loop or remove extra geometry"
            ))
        })
    }
}

#[derive(Clone)]
struct ExtrudeOut {
    start: Frame,
    distance: f64,
    profile: SketchProfile,
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

/// Regeneration state after a history prefix (cached by [`RegenCache`]).
#[derive(Clone)]
struct Snapshot {
    sketches: HashMap<FeatureId, SketchOut>,
    extrudes: HashMap<FeatureId, ExtrudeOut>,
    body: Option<Body>,
    last_solid: Option<usize>,
    results: Vec<FeatureResult>,
}

/// Memo of regeneration states keyed by the exact serialized feature definitions of
/// the evaluated prefix (so any upstream change misses). Bounded FIFO. Results are
/// identical to uncached regeneration because regeneration is deterministic and
/// B-rep results are never mutated after creation.
#[derive(Default)]
pub struct RegenCache {
    states: HashMap<String, Snapshot>,
    measurements: HashMap<String, Measurements>,
    order: std::collections::VecDeque<String>,
    pub hits: u64,
    pub misses: u64,
}

const REGEN_CACHE_CAPACITY: usize = 128;

impl RegenCache {
    fn insert(&mut self, key: String, snap: Snapshot) {
        if self.states.insert(key.clone(), snap).is_none() {
            self.order.push_back(key);
            while self.order.len() > REGEN_CACHE_CAPACITY {
                if let Some(old) = self.order.pop_front() {
                    self.states.remove(&old);
                    self.measurements.remove(&old);
                }
            }
        }
    }
}

/// Regenerate the whole document. Pure function of the document; deterministic.
pub fn regenerate(doc: &Document) -> Result<Evaluation> {
    regenerate_cached(doc, None)
}

/// Like [`regenerate`], reusing and filling `cache` for unchanged history prefixes.
pub fn regenerate_cached(doc: &Document, cache: Option<&mut RegenCache>) -> Result<Evaluation> {
    regenerate_cached_opts(doc, cache, true)
}

/// Serialized-history key of the whole document (the cache key of its full prefix).
fn full_key(doc: &Document) -> Result<String> {
    let mut key = String::new();
    for id in doc.evaluation_order()? {
        let f = serde_json::to_string(doc.feature(id)?)
            .map_err(|e| DocError::Validation(format!("cannot serialize feature {id}: {e}")))?;
        key.push_str(&f);
        key.push('\n');
    }
    Ok(key)
}

impl RegenCache {
    /// Remember measurements computed out of band for `doc`'s current geometry
    /// (deferred measurement); later regenerations of the same history reuse them.
    pub fn store_measurements(&mut self, doc: &Document, m: Measurements) {
        if let Ok(key) = full_key(doc)
            && self.states.contains_key(&key)
        {
            self.measurements.insert(key, m);
        }
    }
}

/// Like [`regenerate_cached`]; with `measure == false` the (expensive) B-rep
/// measurements are skipped unless already cached, leaving
/// `Evaluation::measurements == None` for a body (deferred measurement).
pub fn regenerate_cached_opts(
    doc: &Document,
    mut cache: Option<&mut RegenCache>,
    measure_body: bool,
) -> Result<Evaluation> {
    let order = doc.evaluation_order()?;
    let mut keys = Vec::with_capacity(order.len());
    let mut key = String::new();
    for id in &order {
        let f = serde_json::to_string(doc.feature(*id)?)
            .map_err(|e| DocError::Validation(format!("cannot serialize feature {id}: {e}")))?;
        key.push_str(&f);
        key.push('\n');
        keys.push(key.clone());
    }
    let mut sketches: HashMap<FeatureId, SketchOut> = HashMap::new();
    let mut extrudes: HashMap<FeatureId, ExtrudeOut> = HashMap::new();
    let mut body: Option<Body> = None;
    // Index (into `results`) of the feature that produced the current body. Only the
    // final body is measured: intermediate measurements cost ~1 s each.
    let mut last_solid: Option<usize> = None;
    let mut results = Vec::new();

    let mut start = 0;
    if let Some(c) = cache.as_deref_mut() {
        if let Some(i) = (0..order.len())
            .rev()
            .find(|&i| c.states.contains_key(&keys[i]))
        {
            let snap = c.states[&keys[i]].clone();
            (sketches, extrudes, body, last_solid, results) = (
                snap.sketches,
                snap.extrudes,
                snap.body,
                snap.last_solid,
                snap.results,
            );
            start = i + 1;
            c.hits += 1;
        } else {
            c.misses += 1;
        }
    }

    for (i, id) in order.iter().copied().enumerate().skip(start) {
        let f = doc.feature(id)?;
        let mut r = FeatureResult {
            id,
            name: f.name.clone(),
            solve: None,
            frame: None,
            resolved_face: None,
            measurements: None,
            solved_sketch: None,
            profile_error: None,
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
                    let culprits: Vec<String> = report
                        .conflicting
                        .iter()
                        .map(|&i| sketch.constraint_label(i))
                        .collect();
                    return Err(DocError::SketchSolve {
                        feature: id,
                        detail: format!(
                            "{:?}, residual {:.3e} mm, conflicting constraints: {}",
                            report.status,
                            report.residual,
                            if culprits.is_empty() {
                                "not identified".to_string()
                            } else {
                                culprits.join("; ")
                            }
                        ),
                    });
                }
                let profile = s.profile().map_err(|e| e.to_string());
                r.profile_error = profile.as_ref().err().cloned();
                r.solve = Some(report);
                r.frame = Some(frame);
                r.resolved_face = resolved;
                r.solved_sketch = Some(s);
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
                let b = k::revolve(&to_kernel(s.usable(id, *sketch)?), &s.frame, *angle)
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
                let profile = s.usable(id, *sketch)?.clone();
                let b = k::extrude(&to_kernel(&profile), &s.frame, *distance).map_err(|e| {
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
                        profile,
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
                let cut_profile = s.usable(id, *sketch)?;
                let hole = hole_in_host_frame(cut_profile, &s.frame, &host.start)?;
                check_hole_inside(&hole, &host.profile)?;
                let b =
                    k::cut_through_all(target, &to_kernel(cut_profile), &s.frame).map_err(|e| {
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
            FeatureKind::Pocket { sketch, depth } => {
                let s = sketches.get(sketch).ok_or(DocError::BrokenReference {
                    from: id,
                    detail: format!("{sketch} is not a sketch"),
                })?;
                let target = body.as_ref().ok_or(DocError::BrokenReference {
                    from: id,
                    detail: "pocket has no body to cut".into(),
                })?;
                // Product limit: the sketch must sit on a cap of an evaluated extrusion;
                // the pocket goes from that cap into the material.
                let host_id = match &doc.feature(*sketch)?.kind {
                    FeatureKind::Sketch {
                        plane: SketchPlane::Face(fr),
                        ..
                    } => fr.feature,
                    _ => {
                        return Err(DocError::ProductLimit(format!(
                            "{id}: a pocket must be sketched on an extrusion end or start cap \
                             (sketch {sketch} is on a datum plane); use Cut Through All"
                        )));
                    }
                };
                let host = extrudes.get(&host_id).ok_or(DocError::ProductLimit(
                    "pockets are only supported on an extruded base body".into(),
                ))?;
                if !depth.is_finite() || *depth <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "{id}.depth: pocket depth must be > 0 mm, got {depth}"
                    )));
                }
                if *depth >= host.distance - k::tol::MIN_LENGTH {
                    return Err(DocError::ProductLimit(format!(
                        "{id}.depth: a blind pocket must be shallower than the {} mm thickness \
                         (got {depth} mm); use Cut Through All for a through hole",
                        host.distance
                    )));
                }
                let cut_profile = s.usable(id, *sketch)?;
                let hole = hole_in_host_frame(cut_profile, &s.frame, &host.start)?;
                check_hole_inside(&hole, &host.profile)?;
                let b = k::cut_blind(target, &to_kernel(cut_profile), &s.frame, *depth).map_err(
                    |e| DocError::Kernel {
                        feature: id,
                        detail: e.to_string(),
                    },
                )?;
                if k::face_count(&b) <= k::face_count(target) {
                    return Err(DocError::Kernel {
                        feature: id,
                        detail: "pocket removed no material".into(),
                    });
                }
                last_solid = Some(results.len());
                body = Some(b);
            }
        }
        results.push(r);
        if let Some(c) = cache.as_deref_mut() {
            c.insert(
                keys[i].clone(),
                Snapshot {
                    sketches: sketches.clone(),
                    extrudes: extrudes.clone(),
                    body: body.clone(),
                    last_solid,
                    results: results.clone(),
                },
            );
        }
    }
    let full = keys.last().cloned().unwrap_or_default();
    let measurements = match (&body, last_solid) {
        (Some(b), Some(i)) => {
            let cached = cache
                .as_deref()
                .and_then(|c| c.measurements.get(&full).cloned());
            let m = match cached {
                Some(m) => Some(m),
                None if measure_body => Some(measure(results[i].id, b)?),
                None => None,
            };
            if let (Some(c), Some(m)) = (cache, &m)
                && c.states.contains_key(&full)
            {
                c.measurements.insert(full, m.clone());
            }
            results[i].measurements = m.clone();
            m
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
