//! Deterministic regeneration of the feature history into per-body geometry
//! (ADR 0003): B-rep bodies (modelled or imported STEP) and mesh bodies (STL).
use crate::hole::{check_hole_inside, to_kernel};
use crate::import::{self, ImportedPlanarFace};
use crate::{
    BodyId, DocError, Document, Extent, FaceRef, FaceRole, FeatureId, FeatureKind, Operation,
    Result, SketchPlane,
};
use f420_kernel::{self as k, Body, Frame, Measurements};
use f420_sketch::{EntityId, Profile as SketchProfile, SolveReport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// How a semantic face reference resolved in this evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFace {
    pub reference: FaceRef,
    /// Placement frame of the face (normal = outward face normal).
    pub frame: Frame,
    /// Number of B-rep planar faces of the owning body matching the frame at the
    /// time of resolution (must be exactly 1).
    pub matching_brep_faces: usize,
    /// Body the face belongs to.
    #[serde(default)]
    pub body: BodyId,
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
    /// Solid-producing features: measurements of the body after this feature
    /// (filled for the body's last feature only).
    pub measurements: Option<Measurements>,
    /// Sketch features: the solved sketch (display / editor seed; not authoritative).
    #[serde(default)]
    pub solved_sketch: Option<f420_sketch::Sketch>,
    /// Sketch features: why the geometry is not a usable profile (open loop,
    /// self-intersection, ...). Such sketches are kept but cannot be extruded/cut.
    #[serde(default)]
    pub profile_error: Option<String>,
    /// Body-producing / modifying features: the body acted on.
    #[serde(default)]
    pub body: Option<BodyId>,
    /// Import features: human-readable notes (units, counts, warnings).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A triangle-mesh body (STL import). Never treated as an exact solid.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshBody {
    /// Positions in mm; one normal per vertex (flat shading: vertices are not
    /// shared between triangles), `tri_face` all zero.
    pub mesh: k::TriMesh,
    pub diagnostics: import::MeshDiagnostics,
}

/// Geometry of one body.
#[derive(Debug, Clone)]
pub enum BodyGeometry {
    Brep(Body),
    Mesh(Arc<MeshBody>),
}

impl BodyGeometry {
    pub fn brep(&self) -> Option<&Body> {
        match self {
            BodyGeometry::Brep(b) => Some(b),
            BodyGeometry::Mesh(_) => None,
        }
    }
    pub fn mesh(&self) -> Option<&MeshBody> {
        match self {
            BodyGeometry::Brep(_) => None,
            BodyGeometry::Mesh(m) => Some(m),
        }
    }
}

/// Where a body came from (kept visible in the UI: imported bodies have no
/// recovered feature history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BodySource {
    Modeled,
    ImportedStep {
        file_name: String,
        solid_name: Option<String>,
    },
    ImportedMesh {
        file_name: String,
    },
}

/// One evaluated body.
#[derive(Debug, Clone)]
pub struct EvalBody {
    pub id: BodyId,
    pub name: String,
    pub visible: bool,
    pub geometry: BodyGeometry,
    pub created_by: FeatureId,
    /// Last feature (in evaluation order) that produced or modified the body.
    pub last_feature: FeatureId,
    pub source: BodySource,
    /// Exact B-rep measurements (None: deferred, failed, or a mesh body).
    pub measurements: Option<Measurements>,
    /// Why measuring this geometry failed (stable: not retried until the
    /// body's geometry changes; never blocks modeling or other bodies).
    pub measure_error: Option<String>,
}

/// A planar face that can host a sketch in the final model: its semantic
/// reference, its frame (z = outward normal) and owning body. Exactly one B-rep
/// face of the body lies in the frame's plane with that normal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceCandidate {
    pub reference: FaceRef,
    pub frame: Frame,
    pub body: BodyId,
}

/// Disposable geometry cache for a document.
#[derive(Debug, Clone)]
pub struct Evaluation {
    pub features: Vec<FeatureResult>,
    /// Bodies in creation order.
    pub bodies: Vec<EvalBody>,
    /// Referenceable planar faces of the final model (history order).
    pub faces: Vec<FaceCandidate>,
    /// Legacy single-body view: the B-rep of the body modified by the last
    /// solid feature in history (the only body of v1/v2 documents).
    pub body: Option<Body>,
    /// Measurements of [`Evaluation::body`].
    pub measurements: Option<Measurements>,
}

impl Evaluation {
    pub fn body_by_id(&self, id: BodyId) -> Option<&EvalBody> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// Id of the body behind the legacy [`Evaluation::body`] view.
    pub fn active_body(&self) -> Option<BodyId> {
        let last = self
            .features
            .iter()
            .rev()
            .find_map(|r| r.body.filter(|b| self.body_by_id(*b).is_some()))?;
        Some(last)
    }

    /// Recompute [`Evaluation::body`] / [`Evaluation::measurements`] from `bodies`.
    pub fn refresh_legacy_view(&mut self) {
        let view = self
            .active_body()
            .and_then(|id| self.body_by_id(id))
            .and_then(|b| {
                b.geometry
                    .brep()
                    .map(|body| (body.clone(), b.measurements.clone()))
            });
        (self.body, self.measurements) = match view {
            Some((b, m)) => (Some(b), m),
            None => (None, None),
        };
    }

    /// Install out-of-band measurements of body `id` (also on its last feature).
    pub fn set_body_measurements(&mut self, id: BodyId, m: Measurements) {
        let Some(b) = self.bodies.iter_mut().find(|b| b.id == id) else {
            return;
        };
        b.measurements = Some(m.clone());
        let last = b.last_feature;
        if let Some(r) = self.features.iter_mut().find(|r| r.id == last) {
            r.measurements = Some(m);
        }
        self.refresh_legacy_view();
    }

    /// Record that measuring body `id` failed (it is no longer pending).
    pub fn set_body_failure(&mut self, id: BodyId, why: String) {
        if let Some(b) = self.bodies.iter_mut().find(|b| b.id == id) {
            b.measure_error = Some(why);
        }
    }

    /// B-rep bodies still lacking measurements (excluding failed ones).
    pub fn unmeasured(&self) -> Vec<(BodyId, &Body)> {
        self.bodies
            .iter()
            .filter(|b| b.measurements.is_none() && b.measure_error.is_none())
            .filter_map(|b| b.geometry.brep().map(|g| (b.id, g)))
            .collect()
    }

    pub fn empty() -> Self {
        Self {
            features: vec![],
            bodies: vec![],
            faces: vec![],
            body: None,
            measurements: None,
        }
    }
}

#[derive(Clone)]
struct SketchOut {
    frame: Frame,
    /// `Err(reason)` when the sketch is not a closed profile region.
    profile: std::result::Result<f420_sketch::Region, String>,
    /// The solved sketch (region picking).
    solved: Arc<f420_sketch::Sketch>,
}

impl SketchOut {
    /// The region to extrude: `pick` resolved by identity among the sketch's
    /// bounded regions, else the default whole profile. A picked region that no
    /// longer exists (or a sketch whose regions cannot be enumerated any more)
    /// is a `BrokenReference`; another region is never substituted.
    fn region(
        &self,
        user: FeatureId,
        sketch: FeatureId,
        pick: Option<&f420_sketch::RegionRef>,
    ) -> Result<f420_sketch::Region> {
        let Some(pick) = pick else {
            return self.usable(user, sketch).cloned();
        };
        let broken = |why: String| DocError::BrokenReference {
            from: user,
            detail: format!(
                "region {} of sketch {sketch} no longer exists ({why}); pick a region again",
                pick.label()
            ),
        };
        match self.solved.region_by_ref(pick) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(broken(
                "its bounding loops changed: an entity or hole was added or removed".into(),
            )),
            Err(e) => Err(broken(format!("the sketch has no valid regions: {e}"))),
        }
    }

    /// The profile, or a structured error naming the consuming feature.
    fn usable(&self, user: FeatureId, sketch: FeatureId) -> Result<&f420_sketch::Region> {
        self.profile.as_ref().map_err(|why| {
            DocError::Validation(format!(
                "{user}: sketch {sketch} has no usable profile ({why}); \
                 close the loop or remove extra geometry"
            ))
        })
    }

    /// The single outer loop, refusing inner loops for features that do not
    /// support them (`what`).
    fn single_loop(
        &self,
        user: FeatureId,
        sketch: FeatureId,
        what: &str,
    ) -> Result<&SketchProfile> {
        let r = self.usable(user, sketch)?;
        if !r.holes.is_empty() {
            return Err(DocError::ProductLimit(format!(
                "{user}: sketch {sketch} has {} inner loop(s); {what} needs a single closed loop \
                 (inner loops are supported by New Body extrusions)",
                r.holes.len()
            )));
        }
        Ok(&r.outer)
    }
}

/// Definition data of an evaluated extrusion, enough to derive its faces.
#[derive(Clone)]
struct ExtrudeOut {
    /// Sketch frame.
    start: Frame,
    /// Extrusion distance (unused for through-all).
    distance: f64,
    /// +1 along the sketch normal, -1 against it.
    dir: f64,
    extent: Extent,
    profile: SketchProfile,
    entities: Vec<EntityId>,
    /// Inner loops (New Body only) with their entities.
    holes: Vec<(SketchProfile, Vec<EntityId>)>,
    /// Tool span `[a, b]` along the sketch normal actually used (after
    /// overlap/margin adjustments).
    span: (f64, f64),
    /// Join/Cut: the user's nominal span along the sketch normal, before the
    /// overlap/margin workarounds (re-derived for each pattern/mirror copy, whose
    /// surroundings differ from the seed's). Through-all cuts recompute it.
    nominal: (f64, f64),
    /// Outward side (+1/-1 along the sketch normal) of the target face in the
    /// sketch plane, if any.
    side: Option<f64>,
    /// New/Join (material added) vs Cut (material removed).
    removes: bool,
    /// Join: the start cap is not a face of the result (it lies in the support).
    joined: bool,
    body: BodyId,
}

#[derive(Clone)]
struct BodyState {
    geometry: BodyGeometry,
    created_by: FeatureId,
    last_feature: FeatureId,
    source: BodySource,
}

/// Regeneration state after a history prefix (cached by [`RegenCache`]).
#[derive(Clone)]
struct Snapshot {
    sketches: HashMap<FeatureId, SketchOut>,
    extrudes: HashMap<FeatureId, ExtrudeOut>,
    /// Hole features: their tools (counterbore, then hole), for patterns.
    holes: HashMap<FeatureId, Vec<ExtrudeOut>>,
    imported_faces: HashMap<(FeatureId, BodyId), Arc<Vec<ImportedPlanarFace>>>,
    bodies: BTreeMap<BodyId, BodyState>,
    /// Body ids in creation order.
    created: Vec<BodyId>,
    results: Vec<FeatureResult>,
}

impl Snapshot {
    fn new() -> Self {
        Self {
            sketches: HashMap::new(),
            extrudes: HashMap::new(),
            holes: HashMap::new(),
            imported_faces: HashMap::new(),
            bodies: BTreeMap::new(),
            created: Vec::new(),
            results: Vec::new(),
        }
    }
}

/// Memo of regeneration states keyed by the exact serialized feature definitions of
/// the evaluated prefix (so any upstream change misses). Bounded FIFO. Results are
/// identical to uncached regeneration because regeneration is deterministic and
/// B-rep results are never mutated after creation. Embedded files enter the key
/// as blob id plus length and FNV-1a hash (ids are reused after undo), so keys
/// stay small.
#[derive(Default)]
pub struct RegenCache {
    states: HashMap<String, Snapshot>,
    /// Measurements by (prefix key at the body's last feature, body).
    measurements: HashMap<(String, BodyId), Measurements>,
    /// Measurement failures, keyed like `measurements` (not retried).
    failed: HashMap<(String, BodyId), String>,
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
                    self.measurements.retain(|(k, _), _| k != &old);
                    self.failed.retain(|(k, _), _| k != &old);
                }
            }
        }
    }

    /// Remember measurements computed out of band for body `body` of `eval`,
    /// the current evaluation of `doc` (deferred measurement). They are filed
    /// under the prefix ending at the body's [`EvalBody::last_feature`], the
    /// same key regeneration itself reads, so the two cannot diverge.
    pub fn store_measurements(
        &mut self,
        doc: &Document,
        eval: &Evaluation,
        body: BodyId,
        m: Measurements,
    ) {
        if let Ok(Some(key)) = body_key(doc, eval, body)
            && self.states.contains_key(&key)
        {
            self.measurements.insert((key, body), m);
        }
    }
}

impl RegenCache {
    /// Remember that measuring body `body` of `eval` failed, under the same
    /// key as [`RegenCache::store_measurements`].
    pub fn store_failure(&mut self, doc: &Document, eval: &Evaluation, body: BodyId, why: String) {
        if let Ok(Some(key)) = body_key(doc, eval, body)
            && self.states.contains_key(&key)
        {
            self.failed.insert((key, body), why);
        }
    }
}

/// Serialized-history keys of every prefix of the evaluation order. A feature
/// that reads an embedded file also contributes the file's length and hash:
/// blob ids are reused after undo, so the id alone does not name the contents.
fn prefix_keys(doc: &Document, order: &[FeatureId]) -> Result<Vec<String>> {
    let mut keys = Vec::with_capacity(order.len());
    let mut key = String::new();
    for id in order {
        let feature = doc.feature(*id)?;
        let f = serde_json::to_string(feature)
            .map_err(|e| DocError::Validation(format!("cannot serialize feature {id}: {e}")))?;
        key.push_str(&f);
        // A missing blob fails regeneration of this feature anyway.
        if let Some(b) = feature.blob().and_then(|b| doc.blob(b).ok()) {
            key.push_str(&format!("\tblob {} {}", b.len, b.fnv64));
        }
        key.push('\n');
        keys.push(key.clone());
    }
    Ok(keys)
}

/// Cache key of the prefix ending at `body`'s last feature in `eval` (the
/// evaluation of `doc`).
fn body_key(doc: &Document, eval: &Evaluation, body: BodyId) -> Result<Option<String>> {
    let Some(last) = eval.body_by_id(body).map(|b| b.last_feature) else {
        return Ok(None);
    };
    let order = doc.evaluation_order()?;
    let keys = prefix_keys(doc, &order)?;
    Ok(order
        .iter()
        .position(|id| *id == last)
        .map(|i| keys[i].clone()))
}

/// Regenerate the whole document. Pure function of the document; deterministic.
pub fn regenerate(doc: &Document) -> Result<Evaluation> {
    regenerate_cached(doc, None)
}

/// Like [`regenerate`], reusing and filling `cache` for unchanged history prefixes.
pub fn regenerate_cached(doc: &Document, cache: Option<&mut RegenCache>) -> Result<Evaluation> {
    regenerate_cached_opts(doc, cache, true)
}

fn not_a_sketch(id: FeatureId, sketch: FeatureId) -> DocError {
    DocError::BrokenReference {
        from: id,
        detail: format!("{sketch} is not a sketch"),
    }
}

fn kernel(feature: FeatureId) -> impl Fn(k::KernelError) -> DocError {
    move |e| DocError::Kernel {
        feature,
        detail: e.to_string(),
    }
}

/// Exact B-rep of `target`, or a structured error (missing body, mesh body).
fn brep_target(st: &Snapshot, id: FeatureId, target: BodyId) -> Result<Body> {
    let b = st.bodies.get(&target).ok_or(DocError::BrokenReference {
        from: id,
        detail: format!("target body {target} does not exist at this point of the history"),
    })?;
    match &b.geometry {
        BodyGeometry::Brep(body) => Ok(body.clone()),
        BodyGeometry::Mesh(_) => Err(DocError::ProductLimit(format!(
            "{id}: body {target} is a mesh (STL) body; exact operations need a B-rep body"
        ))),
    }
}

/// Like [`regenerate_cached`]; with `measure == false` the (expensive) B-rep
/// measurements are skipped unless already cached, leaving
/// `EvalBody::measurements == None` (deferred measurement).
pub fn regenerate_cached_opts(
    doc: &Document,
    mut cache: Option<&mut RegenCache>,
    measure_body: bool,
) -> Result<Evaluation> {
    let order = doc.evaluation_order()?;
    doc.validate_bodies()?;
    let keys = prefix_keys(doc, &order)?;
    let mut st = Snapshot::new();
    let mut start = 0;
    if let Some(c) = cache.as_deref_mut() {
        if let Some(i) = (0..order.len())
            .rev()
            .find(|&i| c.states.contains_key(&keys[i]))
        {
            st = c.states[&keys[i]].clone();
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
            body: None,
            notes: vec![],
        };
        match &f.kind {
            FeatureKind::Sketch { plane, sketch } => {
                let (frame, resolved) = match plane {
                    SketchPlane::WorldXY => (Frame::XY, None),
                    SketchPlane::WorldXZ => (Frame::XZ, None),
                    SketchPlane::WorldYZ => (FRAME_YZ, None),
                    SketchPlane::Face(fr) => {
                        let rf = resolve_face(id, fr, &st)?;
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
                let profile = s.profile_region().map_err(|e| e.to_string());
                r.profile_error = profile.as_ref().err().cloned();
                r.solve = Some(report);
                r.frame = Some(frame);
                r.resolved_face = resolved;
                let solved = Arc::new(s.clone());
                r.solved_sketch = Some(s);
                st.sketches.insert(
                    id,
                    SketchOut {
                        frame,
                        profile,
                        solved,
                    },
                );
            }
            FeatureKind::Revolve {
                sketch,
                angle,
                body,
            } => {
                let s = st.sketches.get(sketch).ok_or(not_a_sketch(id, *sketch))?;
                let b = k::revolve(
                    &to_kernel(s.single_loop(id, *sketch, "Revolve")?),
                    &s.frame,
                    *angle,
                )
                .map_err(|e| DocError::Validation(format!("{id}: {e}")))?;
                create_body(
                    &mut st,
                    id,
                    *body,
                    BodyGeometry::Brep(b),
                    BodySource::Modeled,
                )?;
                r.body = Some(*body);
            }
            FeatureKind::Extrude {
                sketch,
                distance,
                operation,
                extent,
                reverse,
                region,
            } => {
                let out = extrude_feature(
                    &mut st,
                    id,
                    *sketch,
                    *distance,
                    *operation,
                    *extent,
                    *reverse,
                    region.as_ref(),
                )?;
                r.body = Some(out.body);
                st.extrudes.insert(id, out);
            }
            FeatureKind::Cut { sketch, target, .. } => {
                legacy_cut(doc, &mut st, id, *sketch, *target, None)?;
                r.body = Some(*target);
            }
            FeatureKind::Pocket {
                sketch,
                depth,
                target,
            } => {
                legacy_cut(doc, &mut st, id, *sketch, *target, Some(*depth))?;
                r.body = Some(*target);
            }
            FeatureKind::ImportStep {
                blob,
                unit_override_to_mm,
                bodies,
            } => {
                let b = doc.blob(*blob)?;
                let bytes = b.bytes()?;
                let imp = import::step(&bytes, *unit_override_to_mm).map_err(|e| match e {
                    DocError::Validation(m) => DocError::Validation(format!("{id}: {m}")),
                    other => other,
                })?;
                if imp.solids.len() != bodies.len() {
                    return Err(DocError::Validation(format!(
                        "{id}: {} contains {} solids but the feature records {} bodies",
                        b.file_name,
                        imp.solids.len(),
                        bodies.len()
                    )));
                }
                r.notes = imp.notes.clone();
                for (solid, body) in imp.solids.into_iter().zip(bodies) {
                    st.imported_faces
                        .insert((id, *body), Arc::new(solid.planar_faces));
                    create_body(
                        &mut st,
                        id,
                        *body,
                        BodyGeometry::Brep(solid.body),
                        BodySource::ImportedStep {
                            file_name: b.file_name.clone(),
                            solid_name: solid.name,
                        },
                    )?;
                }
                r.body = bodies.last().copied();
            }
            FeatureKind::LinearPattern {
                seed,
                count,
                spacing,
                direction,
            } => {
                r.body = Some(linear_pattern(
                    &mut st, id, *seed, *count, *spacing, *direction,
                )?);
            }
            FeatureKind::CircularPattern {
                seed,
                count,
                angle,
                center,
            } => {
                r.body = Some(circular_pattern(
                    &mut st, id, *seed, *count, *angle, *center,
                )?);
            }
            FeatureKind::Mirror { seed, plane } => {
                let frame = match plane {
                    SketchPlane::WorldXY => Frame::XY,
                    SketchPlane::WorldXZ => Frame::XZ,
                    SketchPlane::WorldYZ => FRAME_YZ,
                    SketchPlane::Face(fr) => resolve_face(id, fr, &st)?.frame,
                };
                r.frame = Some(frame);
                r.body = Some(mirror_feature(&mut st, id, *seed, &frame)?);
            }
            FeatureKind::Chamfer { a, b, distance } => {
                let (fa, fb) = (resolve_face(id, a, &st)?, resolve_face(id, b, &st)?);
                if fa.body != fb.body {
                    return Err(DocError::Validation(format!(
                        "{id}: the two faces belong to different bodies ({} and {})",
                        fa.body, fb.body
                    )));
                }
                let body = brep_target(&st, id, fa.body)?;
                let out =
                    k::chamfer_edge(&body, &fa.frame, &fb.frame, *distance).map_err(kernel(id))?;
                replace_body(&mut st, id, fa.body, out);
                r.body = Some(fa.body);
            }
            FeatureKind::Hole {
                plane,
                center,
                diameter,
                depth,
                counterbore,
                target,
            } => {
                let frame = match plane {
                    SketchPlane::WorldXY => Frame::XY,
                    SketchPlane::WorldXZ => Frame::XZ,
                    SketchPlane::WorldYZ => FRAME_YZ,
                    SketchPlane::Face(fr) => {
                        let rf = resolve_face(id, fr, &st)?;
                        r.resolved_face = Some(rf.clone());
                        rf.frame
                    }
                };
                r.frame = Some(frame);
                let tools = hole_tools(
                    &st,
                    id,
                    &frame,
                    *center,
                    *diameter,
                    *depth,
                    *counterbore,
                    *target,
                )?;
                let mut body = brep_target(&st, id, *target)?;
                let labels: &[&str] = if tools.len() == 2 {
                    &["counterbore", "hole"]
                } else {
                    &["hole"]
                };
                for (what, tool) in labels.iter().zip(&tools) {
                    body = apply_tool(&body, tool, &frame, Some(what), id, None)?.0;
                }
                replace_body(&mut st, id, *target, body);
                st.holes.insert(id, tools);
                r.body = Some(*target);
            }
            FeatureKind::Shell {
                extrude,
                open,
                thickness,
            } => {
                let (body, tool) = shell_tool(doc, &st, id, *extrude, *open, *thickness)?;
                let target = brep_target(&st, id, body)?;
                r.frame = Some(tool.start);
                let out = apply_tool(&target, &tool, &tool.start, Some("shell"), id, None)?.0;
                replace_body(&mut st, id, body, out);
                r.body = Some(body);
            }
            FeatureKind::Import3mf { blob, bodies } => {
                let b = doc.blob(*blob)?;
                let bytes = b.bytes()?;
                let (objects, notes) = import::threemf(&bytes)?;
                if objects.len() != bodies.len() {
                    return Err(DocError::Validation(format!(
                        "{id}: {} contains {} build items but the feature records {} bodies",
                        b.file_name,
                        objects.len(),
                        bodies.len()
                    )));
                }
                r.notes = notes;
                for (o, body) in objects.into_iter().zip(bodies) {
                    create_body(
                        &mut st,
                        id,
                        *body,
                        BodyGeometry::Mesh(Arc::new(o.mesh)),
                        BodySource::ImportedMesh {
                            file_name: b.file_name.clone(),
                        },
                    )?;
                }
                r.body = bodies.last().copied();
            }
            FeatureKind::ImportMesh { blob, to_mm, body } => {
                let b = doc.blob(*blob)?;
                let bytes = b.bytes()?;
                let (mesh, notes) = import::stl(&bytes, *to_mm)?;
                r.notes = notes;
                create_body(
                    &mut st,
                    id,
                    *body,
                    BodyGeometry::Mesh(Arc::new(mesh)),
                    BodySource::ImportedMesh {
                        file_name: b.file_name.clone(),
                    },
                )?;
                r.body = Some(*body);
            }
        }
        st.results.push(r);
        if let Some(c) = cache.as_deref_mut() {
            c.insert(keys[i].clone(), st.clone());
        }
    }

    // Per-body measurements (cached by the prefix that last touched the body).
    let pos: HashMap<FeatureId, usize> = order.iter().enumerate().map(|(i, f)| (*f, i)).collect();
    let mut bodies = Vec::with_capacity(st.created.len());
    for bid in &st.created {
        let Some(bs) = st.bodies.get(bid) else {
            continue;
        };
        let info = doc.body(*bid)?;
        let key = (keys[pos[&bs.last_feature]].clone(), *bid);
        let mut measure_error = cache.as_deref().and_then(|c| c.failed.get(&key).cloned());
        let measurements = match &bs.geometry {
            BodyGeometry::Mesh(_) => None,
            BodyGeometry::Brep(_) if measure_error.is_some() => None,
            BodyGeometry::Brep(b) => {
                let cached = cache
                    .as_deref()
                    .and_then(|c| c.measurements.get(&key).cloned());
                let m = match cached {
                    Some(m) => Some(m),
                    // A failed measurement is a stable per-body state, not a
                    // failed edit: the geometry itself regenerated.
                    None if measure_body => match measure(bs.last_feature, b) {
                        Ok(m) => Some(m),
                        Err(e) => {
                            let why = e.to_string();
                            if let Some(c) = cache.as_deref_mut()
                                && c.states.contains_key(&key.0)
                            {
                                c.failed.insert(key.clone(), why.clone());
                            }
                            measure_error = Some(why);
                            None
                        }
                    },
                    None => None,
                };
                if let (Some(c), Some(m)) = (cache.as_deref_mut(), &m)
                    && c.states.contains_key(&key.0)
                {
                    c.measurements.insert(key, m.clone());
                }
                m
            }
        };
        if let Some(r) = st.results.iter_mut().find(|r| r.id == bs.last_feature)
            && (r.body == Some(*bid) || r.measurements.is_none())
        {
            r.measurements = measurements.clone();
        }
        bodies.push(EvalBody {
            id: *bid,
            name: info.name.clone(),
            visible: info.visible,
            geometry: bs.geometry.clone(),
            created_by: bs.created_by,
            last_feature: bs.last_feature,
            source: bs.source.clone(),
            measurements,
            measure_error,
        });
    }
    let faces = face_candidates(&st, &order);
    let mut eval = Evaluation {
        features: st.results,
        bodies,
        faces,
        body: None,
        measurements: None,
    };
    eval.refresh_legacy_view();
    Ok(eval)
}

/// Every semantic planar face (extrusion caps and line sides, imported planar
/// faces) that resolves uniquely on the final geometry.
fn face_candidates(st: &Snapshot, order: &[FeatureId]) -> Vec<FaceCandidate> {
    let mut out = Vec::new();
    let mut push = |reference: FaceRef, frame: Frame, body: BodyId| {
        if let Some(BodyState {
            geometry: BodyGeometry::Brep(b),
            ..
        }) = st.bodies.get(&body)
            && k::planar_faces_on(b, &frame) == 1
        {
            out.push(FaceCandidate {
                reference,
                frame,
                body,
            });
        }
    };
    for fid in order {
        if let Some(e) = st.extrudes.get(fid) {
            let roles = [FaceRole::ExtrudeEndCap, FaceRole::ExtrudeStartCap]
                .into_iter()
                .chain(
                    e.entities
                        .iter()
                        .chain(e.holes.iter().flat_map(|(_, ents)| ents.iter()))
                        .map(|id| FaceRole::ExtrudeSide { entity: id.0 }),
                );
            for role in roles {
                let fr = FaceRef {
                    feature: *fid,
                    role,
                };
                if let Ok(frame) = extrude_face_frame(e, &fr, *fid) {
                    push(fr, frame, e.body);
                }
            }
        }
        let mut imported: Vec<_> = st
            .imported_faces
            .iter()
            .filter(|((f, _), _)| f == fid)
            .collect();
        imported.sort_by_key(|((_, b), _)| *b);
        for ((_, body), faces) in imported {
            for f in faces.iter() {
                push(
                    FaceRef {
                        feature: *fid,
                        role: FaceRole::Imported {
                            body: *body,
                            face: f.index,
                        },
                    },
                    f.frame,
                    *body,
                );
            }
        }
    }
    out
}

/// World YZ plane: local x = world Y, local y = world Z, normal +X (right-handed).
pub const FRAME_YZ: Frame = Frame {
    origin: [0.0, 0.0, 0.0],
    x: [0.0, 1.0, 0.0],
    y: [0.0, 0.0, 1.0],
    z: [1.0, 0.0, 0.0],
};

fn create_body(
    st: &mut Snapshot,
    id: FeatureId,
    body: BodyId,
    geometry: BodyGeometry,
    source: BodySource,
) -> Result<()> {
    if body.0 == 0 {
        return Err(DocError::Validation(format!("{id}: body id is unassigned")));
    }
    if st.bodies.contains_key(&body) {
        return Err(DocError::Validation(format!(
            "{id}: body {body} already exists"
        )));
    }
    st.bodies.insert(
        body,
        BodyState {
            geometry,
            created_by: id,
            last_feature: id,
            source,
        },
    );
    st.created.push(body);
    Ok(())
}

fn replace_body(st: &mut Snapshot, id: FeatureId, body: BodyId, b: Body) {
    if let Some(s) = st.bodies.get_mut(&body) {
        s.geometry = BodyGeometry::Brep(b);
        s.last_feature = id;
    }
}

/// Overlap / margin of a Join tool extended into its support face (ADR 0003).
const JOIN_OVERLAP: f64 = 0.5;

#[allow(clippy::too_many_arguments)]
fn extrude_feature(
    st: &mut Snapshot,
    id: FeatureId,
    sketch: FeatureId,
    distance: f64,
    operation: Operation,
    extent: Extent,
    reverse: bool,
    pick: Option<&f420_sketch::RegionRef>,
) -> Result<ExtrudeOut> {
    let s = st
        .sketches
        .get(&sketch)
        .ok_or(not_a_sketch(id, sketch))?
        .clone();
    let region = s.region(id, sketch, pick)?;
    let (profile, entities) = (region.outer, region.outer_entities);
    let holes = region.holes;
    let khole: Vec<k::Profile> = holes.iter().map(|(p, _)| to_kernel(p)).collect();
    if !holes.is_empty() && !matches!(operation, Operation::NewBody { .. }) {
        return Err(DocError::ProductLimit(format!(
            "{id}: sketch {sketch} has {} inner loop(s); Join and Cut need a single closed loop \
             (Truck 0.6 booleans fail for tools with inner loops); use New Body, or cut each \
             loop separately",
            holes.len()
        )));
    }
    let kp = to_kernel(&profile);
    let frame = s.frame;
    if extent == Extent::Distance && (!distance.is_finite() || distance <= 0.0) {
        return Err(DocError::Validation(format!(
            "{id}.distance: extrusion distance must be > 0 mm, got {distance}"
        )));
    }
    let target = match operation {
        Operation::NewBody { .. } => None,
        Operation::Join { target } | Operation::Cut { target } => {
            Some((target, brep_target(st, id, target)?))
        }
    };
    // Sketch on a face of the target: +1/-1 when the face's outward normal is
    // along/against the sketch normal (material lies on the other side).
    let side = match &target {
        Some((_, t)) => k::coplanar_face_side(t, &frame).map_err(kernel(id))?,
        None => None,
    };
    // Direction semantics (ADR 0003): along the sketch normal, except when the
    // sketch plane holds a face of the target. A Cut then goes into the material
    // by default (`reverse` flips it). A Join goes away from the material and
    // ignores `reverse`: the other way only adds nothing, and documents saved
    // before this rule set `reverse` exactly when it pointed away.
    let base = match (operation, side) {
        (Operation::Cut { .. }, Some(side)) => -side,
        (Operation::Join { .. }, Some(side)) => side,
        _ => 1.0,
    };
    let dir = match (operation, side) {
        (Operation::Join { .. }, Some(side)) => side,
        _ if reverse => -base,
        _ => base,
    };
    // Tool = prism of the profile over [a, b] along the sketch normal.
    let (mut a, mut b) = if dir > 0.0 {
        (0.0, distance)
    } else {
        (-distance, 0.0)
    };
    let mut out = ExtrudeOut {
        start: frame,
        distance,
        dir,
        extent,
        profile,
        entities,
        holes,
        span: (0.0, 0.0),
        nominal: (0.0, 0.0),
        side,
        removes: false,
        joined: false,
        body: BodyId(0),
    };
    match (operation, target) {
        (Operation::NewBody { body }, _) => {
            if extent == Extent::ThroughAll {
                return Err(DocError::Validation(format!(
                    "{id}: 'through all' needs a Cut target body"
                )));
            }
            let solid =
                k::extrude_region(&kp, &khole, &frame.offset(a), b - a).map_err(kernel(id))?;
            create_body(st, id, body, BodyGeometry::Brep(solid), BodySource::Modeled)?;
            out.body = body;
        }
        (Operation::Join { .. }, Some((target, t))) => {
            if extent == Extent::ThroughAll {
                return Err(DocError::Validation(format!(
                    "{id}: 'through all' is only available for Cut"
                )));
            }
            out.joined = true;
            out.nominal = (a, b);
            if side.is_some() {
                // Report a tool that misses the target as such, before the
                // face-containment rule (the sketch plane may merely be coplanar
                // with some face of the target).
                let tool = k::extrude(&kp, &frame.offset(a), b - a).map_err(kernel(id))?;
                let (tb, ob) = (
                    k::bounds(&t).map_err(kernel(id))?,
                    k::bounds(&tool).map_err(kernel(id))?,
                );
                let eps = k::tol::KERNEL_LINEAR;
                if (0..3).any(|i| ob.min[i] > tb.max[i] + eps || ob.max[i] < tb.min[i] - eps) {
                    return Err(DocError::Kernel {
                        feature: id,
                        detail: "join: the extrusion does not touch the target body".into(),
                    });
                }
            }
            let (joined, used) = apply_tool(&t, &out, &frame, None, id, None)?;
            (a, b) = used;
            replace_body(st, id, target, joined);
            out.body = target;
        }
        (Operation::Cut { .. }, Some((target, t))) => {
            out.removes = true;
            if extent == Extent::Distance
                && let Some(side) = side
                && dir * side > 0.0
            {
                return Err(DocError::Validation(format!(
                    "{id}: the cut points away from the material of {target}; clear 'reverse'"
                )));
            }
            out.nominal = (a, b);
            let (cut, used) = apply_tool(&t, &out, &frame, None, id, None)?;
            (a, b) = used;
            replace_body(st, id, target, cut);
            out.body = target;
        }
        (Operation::Join { .. } | Operation::Cut { .. }, None) => {
            return Err(DocError::Validation(format!("{id}: missing target body")));
        }
    }
    out.span = (a, b);
    Ok(out)
}

/// Which end of a blind cut stayed on a coplanar face because moving it would
/// remove material that must stay.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CoplanarEnd {
    Start,
    Floor,
}

/// Margin adjustments of a blind Cut's nominal span `(a, b)` (along `frame.z`,
/// tool direction `dir`, `side` = outward side of the target face in the
/// sketch plane). A tool end lying on a face of `t` whose material is on the
/// tool's side would make coplanar boolean faces (Truck 0.6 returns no result),
/// so that end is moved `margin` past the face, but only where that removes
/// nothing extra ([`margin_is_free`]). Returns the span and the end that had to
/// stay coplanar, if any.
fn blind_cut_span(
    t: &Body,
    frame: &Frame,
    kp: &k::Profile,
    (mut a, mut b): (f64, f64),
    dir: f64,
    side: Option<f64>,
    margin: f64,
) -> k::Result<((f64, f64), Option<CoplanarEnd>)> {
    let mut refused = None;
    // Start: a cut sketched on a face of the target starts on that face.
    if let Some(side) = side {
        if margin_is_free(t, frame, kp, side, margin)? {
            if dir > 0.0 {
                a = side * margin;
            } else {
                b = side * margin;
            }
        } else {
            refused = Some(CoplanarEnd::Start);
        }
    }
    // Floor: e.g. the bottom face for a cut through the full thickness.
    let far = if dir > 0.0 { b } else { a };
    let far_frame = frame.offset(far);
    if let Ok(Some(side_far)) = k::coplanar_face_side(t, &far_frame)
        && side_far * dir > 0.0
    {
        if margin_is_free(t, &far_frame, kp, dir, margin)? {
            if dir > 0.0 {
                b += margin;
            } else {
                a -= margin;
            }
        } else {
            refused = Some(CoplanarEnd::Floor);
        }
    }
    Ok(((a, b), refused))
}

/// Whether `t` has no material in the prism of `kp` from `plane` to `margin`
/// beyond it (`beyond` = +1/-1 along `plane.z`), so a cutter end may be moved
/// there without removing anything extra. Cheap sufficient tests first: the
/// profile lies inside a face of the plane facing `beyond` (air directly past
/// it; material returning within `margin` is not detected), or nothing of `t`
/// lies past the plane at all. Otherwise the exact [`k::prism_is_empty`] test
/// (no surface crosses the slab and it lies in air).
fn margin_is_free(
    t: &Body,
    plane: &Frame,
    kp: &k::Profile,
    beyond: f64,
    margin: f64,
) -> k::Result<bool> {
    if matches!(k::coplanar_face_side(t, plane), Ok(Some(s)) if s * beyond > 0.0)
        && k::profile_within_face(t, plane, kp)?
    {
        return Ok(true);
    }
    let (lo, hi) = k::extent_along(t, plane)?;
    let eps = 10.0 * k::tol::KERNEL_LINEAR;
    if (beyond > 0.0 && hi <= eps) || (beyond < 0.0 && lo >= -eps) {
        return Ok(true);
    }
    // Exact test (no boolean): no surface crosses the slab and it lies in air.
    // (Run 3 probed with a cut, which Truck can fail or panic on for a slab
    // wholly in air beside a coplanar face; that refused valid cuts.)
    k::prism_is_empty(t, plane, kp, beyond, margin)
}

/// A cut whose end had to stay on a coplanar face failed in the kernel: say why.
fn coplanar_cut_error(id: FeatureId, end: Option<CoplanarEnd>, e: k::KernelError) -> DocError {
    let what = match end {
        None => return kernel(id)(e),
        Some(CoplanarEnd::Start) => {
            "the sketch plane holds a face under part of the profile while material rises \
             above the plane elsewhere under it"
        }
        Some(CoplanarEnd::Floor) => {
            "the cut's floor lies on a face that covers only part of the profile while \
             material continues past the floor elsewhere"
        }
    };
    DocError::ProductLimit(format!(
        "{id}: {what}; the cutter cannot start or stop exactly there (Truck 0.6 fails on \
         coplanar faces) and moving it would remove material that must stay. Change the \
         depth, or split the profile so each part is wholly on or off the face ({e})"
    ))
}

/// One tool application, recorded for the independent result check.
struct Applied {
    frame: Frame,
    profile: k::Profile,
    /// The user's span along `frame.z`.
    nominal: (f64, f64),
    /// The span handed to the kernel.
    used: (f64, f64),
    op: k::PrismOp,
}

/// Independent check that `result` is `target` with all `applied` tools
/// (ADR 0005); a failure is a `GeometryCheck` refusal.
fn verify_applied(
    id: FeatureId,
    prefix: &str,
    target: &Body,
    result: &Body,
    applied: &[Applied],
) -> Result<()> {
    let Some(op) = applied.first().map(|a| a.op) else {
        return Ok(());
    };
    let starts: Vec<Frame> = applied
        .iter()
        .map(|a| a.frame.offset(a.nominal.0))
        .collect();
    let prisms: Vec<k::Prism> = applied
        .iter()
        .zip(&starts)
        .map(|(a, start)| k::Prism {
            profile: &a.profile,
            start,
            length: a.nominal.1 - a.nominal.0,
        })
        .collect();
    let tools: Vec<(&k::Prism, (f64, f64))> = prisms
        .iter()
        .zip(applied)
        .map(|(p, a)| (p, (a.used.0 - a.nominal.0, a.used.1 - a.nominal.0)))
        .collect();
    k::verify_prisms_boolean(target, result, op, &tools)
        .map(|_| ())
        .map_err(|x| DocError::GeometryCheck {
            feature: id,
            detail: format!(
                "{prefix}{}; refused (the model is unchanged). {}",
                x.to_string()
                    .trim_start_matches("result is not a valid solid: "),
                match op {
                    k::PrismOp::Cut => {
                        "Known Truck 0.6 causes: a cut that fully contains an existing hole or \
                         pocket (enlarge that feature by editing its dimension instead), or a cut \
                         whose walls meet existing ones only along an edge or at a point \
                         (non-manifold; move it slightly)"
                    }
                    k::PrismOp::Join => "Change the profile or the support face",
                }
            ),
        })
}

/// Apply extrusion `e`'s tool, placed on `frame` (its sketch frame for the
/// seed, a transformed frame for a pattern/mirror copy), to `body`. This is the
/// one path for seeds and copies (SESSION_4 §1): the workarounds for Truck's
/// coplanar-face failures are re-derived for each placement from the material
/// around it, and the result is checked against the nominal operation
/// ([`k::verify_prism_boolean`]) before it is accepted.
///
/// * Join from a face (`e.side`): the profile must lie inside a face of `body`
///   in the plane; the tool's start moves into the support by an overlap no
///   deeper than half the material that is free of other surfaces under the
///   profile ([`k::clear_depth`]), at most [`JOIN_OVERLAP`].
/// * Blind cut: the start/floor margins of [`blind_cut_span`].
/// * Through-all cut: the span covers `body` along the normal plus a margin.
///
/// Returns the new body and the span `(a, b)` along `frame.z` actually used.
fn apply_tool(
    body: &Body,
    e: &ExtrudeOut,
    frame: &Frame,
    what: Option<&str>,
    id: FeatureId,
    defer: Option<&mut Vec<Applied>>,
) -> Result<(Body, (f64, f64))> {
    let kp = to_kernel(&e.profile);
    let prefix = what.map(|w| format!("{w}: ")).unwrap_or_default();
    let err = |x: k::KernelError| DocError::Kernel {
        feature: id,
        detail: format!("{prefix}{x}"),
    };
    // Check now, or (patterns, mirrors) record the application and check the
    // final body once against all of them (`verify_applied`).
    let mut defer = defer;
    let mut check = |result: &Body, op: k::PrismOp, nominal: (f64, f64), used: (f64, f64)| {
        let a = Applied {
            frame: *frame,
            profile: kp.clone(),
            nominal,
            used,
            op,
        };
        match defer.as_deref_mut() {
            Some(v) => {
                v.push(a);
                Ok(())
            }
            None => verify_applied(id, &prefix, body, result, std::slice::from_ref(&a)),
        }
    };
    if e.removes {
        let margin = k::through_all_margin(body).map_err(err)?;
        let (nominal, (span, refused)) = if e.extent == Extent::ThroughAll {
            let (lo, hi) = k::extent_along(body, frame).map_err(err)?;
            let span = (lo - margin, hi + margin);
            (span, (span, None))
        } else {
            (
                e.nominal,
                blind_cut_span(body, frame, &kp, e.nominal, e.dir, e.side, margin).map_err(err)?,
            )
        };
        let out =
            k::cut_prism(body, &kp, &frame.offset(span.0), span.1 - span.0).map_err(
                |x| match x {
                    x if refused.is_some() => coplanar_cut_error(id, refused, x),
                    x => err(x),
                },
            )?;
        check(&out, k::PrismOp::Cut, nominal, span)?;
        return Ok((out, span));
    }
    let (a, b) = e.nominal;
    let span = match e.side {
        None => (a, b),
        Some(side) => {
            // A tool end lying on a face of the target would make coplanar
            // boolean faces (Truck 0.6 returns no result): move that end into
            // the material, which is only exact when the profile lies on the
            // face (material under the whole profile) and the overlap stays
            // inside material that no other surface crosses.
            if !k::profile_within_face(body, frame, &kp).map_err(err)? {
                return Err(DocError::ProductLimit(format!(
                    "{id}: {prefix}a Join sketched on a face of the target must lie inside \
                     that face; the profile is not inside a face here (it crosses the face \
                     boundary or a hole). Use New Body, or sketch on another plane"
                )));
            }
            let face = Frame {
                z: [side * frame.z[0], side * frame.z[1], side * frame.z[2]],
                ..*frame
            };
            let clear = k::clear_depth(body, &face, &kp, 2.0 * JOIN_OVERLAP).map_err(err)?;
            if clear < 2.0 * k::tol::MIN_LENGTH {
                return Err(DocError::ProductLimit(format!(
                    "{id}: {prefix}the material under the joined profile is only {clear:.4} \
                     mm deep before another surface (a cavity or hole just under the face); \
                     a Join from this face cannot be built exactly. Sketch the join \
                     elsewhere or thicken the support"
                )));
            }
            let delta = JOIN_OVERLAP.min((b - a) / 2.0).min(clear / 2.0);
            if e.dir > 0.0 {
                (-side * delta, b)
            } else {
                (a, -side * delta)
            }
        }
    };
    let out = k::join_prism(body, &kp, &frame.offset(span.0), span.1 - span.0).map_err(err)?;
    check(&out, k::PrismOp::Join, (a, b), span)?;
    Ok((out, span))
}

/// The cut of a scoped shell (docs/SHELL.md): the extrusion's profile offset
/// inward by `t`, from the open cap into the material, `distance − t` deep.
/// Returns the body and the tool. Every unsupported case is a `ProductLimit`
/// naming the reason; an open cap that no longer resolves is a
/// `BrokenReference`.
fn shell_tool(
    doc: &Document,
    st: &Snapshot,
    id: FeatureId,
    extrude: FeatureId,
    open: FaceRole,
    t: f64,
) -> Result<(BodyId, ExtrudeOut)> {
    let limit = |m: String| DocError::ProductLimit(format!("{id}: shell: {m}"));
    if !t.is_finite() || t <= 0.0 {
        return Err(DocError::Validation(format!(
            "{id}.thickness: shell wall thickness must be > 0 mm, got {t}"
        )));
    }
    let picked = matches!(
        &doc.feature(extrude)?.kind,
        FeatureKind::Extrude {
            region: Some(_),
            ..
        }
    );
    let e = st
        .extrudes
        .get(&extrude)
        .filter(|e| !e.removes && !e.joined && e.extent == Extent::Distance)
        .ok_or_else(|| {
            limit(format!(
                "{extrude} is not a New Body extrusion; the scoped shell hollows a body made by \
                 one New Body extrusion"
            ))
        })?
        .clone();
    if picked || !e.holes.is_empty() {
        return Err(limit(format!(
            "the profile of {extrude} has inner loops or a picked region; the scoped shell needs \
             a single closed loop (lines/arcs) or a circle"
        )));
    }
    if !matches!(open, FaceRole::ExtrudeEndCap | FaceRole::ExtrudeStartCap) {
        return Err(limit(
            "the open face must be the extrusion's end cap or start cap".into(),
        ));
    }
    if t >= e.distance {
        return Err(limit(format!(
            "thickness {t} mm must be less than the extrusion distance {} mm (no floor would \
             remain)",
            e.distance
        )));
    }
    let fr = FaceRef {
        feature: extrude,
        role: open,
    };
    let cap = extrude_face_frame(&e, &fr, id)?;
    let body = brep_target(st, id, e.body)?;
    if k::planar_faces_on(&body, &cap) != 1 {
        return Err(DocError::BrokenReference {
            from: id,
            detail: format!(
                "the open face ({open:?} of {extrude}) no longer resolves to one planar face of \
                 {}; a feature before the shell changed it",
                e.body
            ),
        });
    }
    let inner = f420_sketch::offset_profile(&e.profile, -t).map_err(|x| {
        limit(format!(
            "thickness {t} mm does not fit the profile of {extrude}: the inward offset {x}"
        ))
    })?;
    // The tool frame: the sketch frame moved to the open cap's plane (same x/y,
    // so the profile coordinates carry over).
    let along = if open == FaceRole::ExtrudeEndCap {
        e.dir * e.distance
    } else {
        0.0
    };
    let start = e.start.offset(along);
    let side = k::coplanar_face_side(&body, &start)
        .map_err(kernel(id))?
        .ok_or_else(|| limit("the open cap is not a face of the body".into()))?;
    let dir = -side;
    let depth = e.distance - t;
    let span = if dir > 0.0 {
        (0.0, depth)
    } else {
        (-depth, 0.0)
    };
    Ok((
        e.body,
        ExtrudeOut {
            start,
            distance: depth,
            dir,
            extent: Extent::Distance,
            profile: inner,
            entities: vec![],
            holes: vec![],
            span,
            nominal: span,
            side: Some(side),
            removes: true,
            joined: false,
            body: e.body,
        },
    ))
}

/// Tools of a Hole, in application order (`[counterbore, hole]` or `[hole]`),
/// as blind/through cuts from the face in `frame` into the material of
/// `target`.
#[allow(clippy::too_many_arguments)]
fn hole_tools(
    st: &Snapshot,
    id: FeatureId,
    frame: &Frame,
    center: [f64; 2],
    diameter: f64,
    depth: Option<f64>,
    counterbore: Option<crate::Counterbore>,
    target: BodyId,
) -> Result<Vec<ExtrudeOut>> {
    let t = brep_target(st, id, target)?;
    let side = k::coplanar_face_side(&t, frame)
        .map_err(kernel(id))?
        .ok_or_else(|| {
            DocError::ProductLimit(format!(
                "{id}: a hole starts on a planar face of its target body; the chosen plane holds \
                 no face of {target}"
            ))
        })?;
    let dir = -side;
    let finite = |v: f64, what: &str| {
        (v.is_finite() && v > 0.0)
            .then_some(v)
            .ok_or_else(|| DocError::Validation(format!("{id}.{what}: must be > 0 mm, got {v}")))
    };
    finite(diameter, "diameter")?;
    if !(center[0].is_finite() && center[1].is_finite()) {
        return Err(DocError::Validation(format!("{id}: centre must be finite")));
    }
    let tool = |d: f64, len: Option<f64>| {
        let span = match len {
            Some(l) if dir > 0.0 => (0.0, l),
            Some(l) => (-l, 0.0),
            None => (0.0, 0.0),
        };
        ExtrudeOut {
            start: *frame,
            distance: len.unwrap_or(0.0),
            dir,
            extent: if len.is_some() {
                Extent::Distance
            } else {
                Extent::ThroughAll
            },
            profile: SketchProfile::Circle {
                center,
                radius: d / 2.0,
            },
            entities: vec![],
            holes: vec![],
            span,
            nominal: span,
            side: Some(side),
            removes: true,
            joined: false,
            body: target,
        }
    };
    let mut v = Vec::new();
    if let Some(c) = counterbore {
        finite(c.diameter, "cbore_diameter")?;
        finite(c.depth, "cbore_depth")?;
        if c.diameter <= diameter || depth.is_some_and(|d| c.depth >= d) {
            return Err(DocError::Validation(format!(
                "{id}: the counterbore must be wider than the hole and shallower than its depth"
            )));
        }
        v.push(tool(c.diameter, Some(c.depth)));
    }
    if let Some(d) = depth {
        finite(d, "depth")?;
    }
    v.push(tool(diameter, depth));
    Ok(v)
}

/// Pattern/mirror seeds: an evaluated Extrude Join or Cut, or a Hole (its
/// counterbore and hole tools, applied in that order).
fn copy_seed(st: &Snapshot, id: FeatureId, seed: FeatureId) -> Result<Vec<ExtrudeOut>> {
    if let Some(tools) = st.holes.get(&seed) {
        return Ok(tools.clone());
    }
    let e = st
        .extrudes
        .get(&seed)
        .cloned()
        .ok_or(DocError::BrokenReference {
            from: id,
            detail: format!("{seed} is not an evaluated extrusion or hole"),
        })?;
    if !e.removes && !e.joined {
        return Err(DocError::ProductLimit(format!(
            "{id}: patterns and mirrors copy Extrude Join or Cut features ({seed} creates a \
             body)"
        )));
    }
    Ok(vec![e])
}

/// Linear pattern of an Extrude Join/Cut: the seed's exact tool (same span along
/// the sketch normal) translated in the sketch plane, applied to the seed's target.
fn linear_pattern(
    st: &mut Snapshot,
    id: FeatureId,
    seed: FeatureId,
    count: u32,
    spacing: f64,
    direction: [f64; 2],
) -> Result<BodyId> {
    let tools = copy_seed(st, id, seed)?;
    let e = tools[0].clone();
    if !(2..=crate::MAX_PATTERN_COUNT).contains(&count) {
        return Err(DocError::Validation(format!(
            "{id}.count: must be in 2..={}, got {count}",
            crate::MAX_PATTERN_COUNT
        )));
    }
    if !spacing.is_finite() || spacing <= 0.0 {
        return Err(DocError::Validation(format!(
            "{id}.spacing: must be > 0 mm, got {spacing}"
        )));
    }
    let len = direction[0].hypot(direction[1]);
    if !len.is_finite() || len <= 0.0 {
        return Err(DocError::Validation(format!(
            "{id}: pattern direction must be a non-zero vector"
        )));
    }
    let f = e.start;
    let d = [direction[0] / len, direction[1] / len];
    let world = [
        f.x[0] * d[0] + f.y[0] * d[1],
        f.x[1] * d[0] + f.y[1] * d[1],
        f.x[2] * d[0] + f.y[2] * d[1],
    ];
    let before = brep_target(st, id, e.body)?;
    let mut body = before.clone();
    let mut applied = Vec::new();
    for i in 1..count {
        let t = spacing * f64::from(i);
        let frame = Frame {
            origin: [
                f.origin[0] + world[0] * t,
                f.origin[1] + world[1] * t,
                f.origin[2] + world[2] * t,
            ],
            ..f
        };
        for tool in &tools {
            body = apply_tool(
                &body,
                tool,
                &frame,
                Some(&format!("instance {} of {count}", i + 1)),
                id,
                Some(&mut applied),
            )?
            .0;
        }
    }
    // One independent check of the whole pattern (ADR 0005).
    verify_applied(id, "pattern: ", &before, &body, &applied)?;
    replace_body(st, id, e.body, body);
    Ok(e.body)
}

/// Circular pattern of an Extrude Join/Cut: the seed's exact tool rotated about
/// the axis through `center` (seed sketch coordinates) along the sketch normal.
fn circular_pattern(
    st: &mut Snapshot,
    id: FeatureId,
    seed: FeatureId,
    count: u32,
    angle: f64,
    center: [f64; 2],
) -> Result<BodyId> {
    let tools = copy_seed(st, id, seed)?;
    let e = tools[0].clone();
    if !(2..=crate::MAX_PATTERN_COUNT).contains(&count) {
        return Err(DocError::Validation(format!(
            "{id}.count: must be in 2..={}, got {count}",
            crate::MAX_PATTERN_COUNT
        )));
    }
    if !angle.is_finite() || angle <= 0.0 || angle >= std::f64::consts::TAU {
        return Err(DocError::Validation(format!(
            "{id}.angle_deg: must be in (0, 360), got {}",
            angle.to_degrees()
        )));
    }
    // A full turn puts the last instance back on the seed.
    if f64::from(count - 1) * angle >= std::f64::consts::TAU - 1e-9 {
        return Err(DocError::Validation(format!(
            "{id}: {count} instances {:.6}° apart overlap after a full turn",
            angle.to_degrees()
        )));
    }
    if !(center[0].is_finite() && center[1].is_finite()) {
        return Err(DocError::Validation(format!("{id}: centre must be finite")));
    }
    let f = e.start;
    let c = f.to_world(center);
    let rot = |v: [f64; 3], t: f64| {
        // Rodrigues about f.z (v ⟂ f.z for in-plane vectors, but keep it general).
        let (s, co) = t.sin_cos();
        let k = f.z;
        let kxv = cross(k, v);
        let kd = dot(k, v);
        [
            v[0] * co + kxv[0] * s + k[0] * kd * (1.0 - co),
            v[1] * co + kxv[1] * s + k[1] * kd * (1.0 - co),
            v[2] * co + kxv[2] * s + k[2] * kd * (1.0 - co),
        ]
    };
    let before = brep_target(st, id, e.body)?;
    let mut body = before.clone();
    let mut applied = Vec::new();
    for i in 1..count {
        let t = angle * f64::from(i);
        let o = rot(sub(f.origin, c), t);
        let frame = Frame {
            origin: [c[0] + o[0], c[1] + o[1], c[2] + o[2]],
            x: rot(f.x, t),
            y: rot(f.y, t),
            z: f.z,
        };
        for tool in &tools {
            body = apply_tool(
                &body,
                tool,
                &frame,
                Some(&format!("instance {} of {count}", i + 1)),
                id,
                Some(&mut applied),
            )?
            .0;
        }
    }
    // One independent check of the whole pattern (ADR 0005).
    verify_applied(id, "pattern: ", &before, &body, &applied)?;
    replace_body(st, id, e.body, body);
    Ok(e.body)
}

/// Mirror of an Extrude Join/Cut (or a Hole) across the plane of `mirror`
/// (z = plane normal): the reflected tools, applied to the seed's target. A
/// reflection reverses orientation, so each copy uses the right-handed frame
/// (R o, R x, R y, R x × R y = −R z) with the same 2D profile, and the spans
/// along the normal are negated (−b..−a) so each tool still extends along R z.
fn mirror_feature(
    st: &mut Snapshot,
    id: FeatureId,
    seed: FeatureId,
    mirror: &Frame,
) -> Result<BodyId> {
    let tools = copy_seed(st, id, seed)?;
    let n = mirror.z;
    let refl_v = |v: [f64; 3]| {
        let d = 2.0 * dot(v, n);
        [v[0] - d * n[0], v[1] - d * n[1], v[2] - d * n[2]]
    };
    let refl_p = |p: [f64; 3]| {
        let d = 2.0 * dot(sub(p, mirror.origin), n);
        [p[0] - d * n[0], p[1] - d * n[1], p[2] - d * n[2]]
    };
    let target = tools[0].body;
    let before = brep_target(st, id, target)?;
    let mut body = before.clone();
    let mut applied = Vec::new();
    for mut e in tools {
        let f = e.start;
        let (x, y) = (refl_v(f.x), refl_v(f.y));
        let frame = Frame {
            origin: refl_p(f.origin),
            x,
            y,
            z: cross(x, y),
        };
        // Along the reflected frame's normal (−R z) the tool and the face sides flip.
        e.nominal = (-e.nominal.1, -e.nominal.0);
        e.dir = -e.dir;
        e.side = e.side.map(|s| -s);
        body = apply_tool(
            &body,
            &e,
            &frame,
            Some("mirrored copy"),
            id,
            Some(&mut applied),
        )?
        .0;
    }
    verify_applied(id, "mirror: ", &before, &body, &applied)?;
    replace_body(st, id, target, body);
    Ok(target)
}

/// Run-1/2 circular cut (`depth == None`: through all) and pocket features on
/// their explicit target, with the original product limits.
fn legacy_cut(
    doc: &Document,
    st: &mut Snapshot,
    id: FeatureId,
    sketch: FeatureId,
    target: BodyId,
    depth: Option<f64>,
) -> Result<()> {
    let s = st
        .sketches
        .get(&sketch)
        .ok_or(not_a_sketch(id, sketch))?
        .clone();
    let t = brep_target(st, id, target)?;
    let host = match depth {
        // Through cuts: the extrusion that created the target.
        None => st
            .extrudes
            .values()
            .find(|e| !e.removes && !e.joined && e.body == target && e.extent == Extent::Distance)
            .cloned()
            .ok_or(DocError::ProductLimit(
                "circular cuts are only supported on an extruded base body".into(),
            ))?,
        // Pockets: the extrusion whose cap the sketch is on.
        Some(_) => {
            let host_id = match &doc.feature(sketch)?.kind {
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
            st.extrudes
                .get(&host_id)
                .filter(|e| !e.removes)
                .cloned()
                .ok_or(DocError::ProductLimit(
                    "pockets are only supported on an extruded base body".into(),
                ))?
        }
    };
    if let Some(depth) = depth {
        if !depth.is_finite() || depth <= 0.0 {
            return Err(DocError::Validation(format!(
                "{id}.depth: pocket depth must be > 0 mm, got {depth}"
            )));
        }
        if depth >= host.distance - k::tol::MIN_LENGTH {
            return Err(DocError::ProductLimit(format!(
                "{id}.depth: a blind pocket must be shallower than the {} mm thickness \
                 (got {depth} mm); use Cut Through All for a through hole",
                host.distance
            )));
        }
    }
    let cut_profile = s.single_loop(id, sketch, "Cut Through All / Pocket")?;
    let hole = hole_in_host_frame(cut_profile, &s.frame, &host.start)?;
    check_hole_inside(&hole, &host.profile)?;
    let kp = to_kernel(cut_profile);
    let b = match depth {
        None => k::cut_through_all(&t, &kp, &s.frame),
        Some(d) => k::cut_blind(&t, &kp, &s.frame, d),
    }
    .map_err(kernel(id))?;
    // Independent check against the nominal cut (through all: the body's whole
    // extent along the normal; pocket: from the face down to `depth`).
    let (nominal, used) = match depth {
        None => {
            let (lo, hi) = k::extent_along(&t, &s.frame).map_err(kernel(id))?;
            let m = k::through_all_margin(&t).map_err(kernel(id))?;
            ((lo - m, hi + m), (lo - m, hi + m))
        }
        Some(d) => {
            let m = k::through_all_margin(&t).map_err(kernel(id))?;
            ((-d, 0.0), (-d, m))
        }
    };
    let start = s.frame.offset(nominal.0);
    k::verify_prism_boolean(
        &t,
        &b,
        k::PrismOp::Cut,
        &k::Prism {
            profile: &kp,
            start: &start,
            length: nominal.1 - nominal.0,
        },
        (used.0 - nominal.0, used.1 - nominal.0),
    )
    .map_err(|x| DocError::GeometryCheck {
        feature: id,
        detail: format!(
            "{}; refused (the model is unchanged). Known Truck 0.6 case: a cut that fully \
             contains an existing hole or pocket; enlarge that feature by editing its \
             dimension instead",
            x.to_string()
                .trim_start_matches("result is not a valid solid: ")
        ),
    })?;
    // A cut that removes material always adds faces; containment (checked
    // above) guarantees the cutter intersects the body.
    if k::face_count(&b) <= k::face_count(&t) {
        return Err(DocError::Kernel {
            feature: id,
            detail: format!(
                "{} removed no material",
                if depth.is_some() { "pocket" } else { "cut" }
            ),
        });
    }
    replace_body(st, id, target, b);
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
            "only circular cuts are supported by Cut Through All / Pocket; use Extrude > Cut \
             for other profiles"
                .into(),
        ));
    };
    if (dot(cut_frame.z, host.z).abs() - 1.0).abs() > 1e-9 {
        return Err(DocError::ProductLimit(
            "cut axis must be parallel to the base extrusion direction".into(),
        ));
    }
    let w = cut_frame.to_world(*center);
    let d = sub(w, host.origin);
    Ok(SketchProfile::Circle {
        center: [dot(d, host.x), dot(d, host.y)],
        radius: *radius,
    })
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn measure(id: FeatureId, b: &Body) -> Result<Measurements> {
    k::measure(b).map_err(|e| DocError::Kernel {
        feature: id,
        detail: e.to_string(),
    })
}

/// Frame of a face generated by an extrusion, from its definition only.
fn extrude_face_frame(e: &ExtrudeOut, fr: &FaceRef, from: FeatureId) -> Result<Frame> {
    let broken = |detail: String| DocError::BrokenReference { from, detail };
    let f = e.start;
    let z_dir = scale(f.z, e.dir);
    let frame_from = |origin: [f64; 3], z: [f64; 3], x: [f64; 3]| Frame {
        origin,
        x,
        y: cross(z, x),
        z,
    };
    match fr.role {
        FaceRole::ExtrudeEndCap | FaceRole::ExtrudeStartCap if e.extent == Extent::ThroughAll => {
            Err(broken(format!(
                "{} is a through-all cut; it has no cap faces",
                fr.feature
            )))
        }
        FaceRole::ExtrudeEndCap => {
            let origin = f.to_world([0.0, 0.0]);
            let origin = [
                origin[0] + z_dir[0] * e.distance,
                origin[1] + z_dir[1] * e.distance,
                origin[2] + z_dir[2] * e.distance,
            ];
            // New/Join: outward along the extrusion; cut floor: back toward the opening.
            let z = if e.removes { scale(z_dir, -1.0) } else { z_dir };
            Ok(frame_from(origin, z, f.x))
        }
        FaceRole::ExtrudeStartCap => {
            if e.removes || e.joined {
                return Err(broken(format!(
                    "{} has no start cap face (a {} starts on its support)",
                    fr.feature,
                    if e.removes { "cut" } else { "join" }
                )));
            }
            Ok(frame_from(f.origin, scale(z_dir, -1.0), f.x))
        }
        FaceRole::ExtrudeSide { entity } => {
            // Outer loop first, then the holes (whose walls face into the hole).
            let (profile, i, hole) = std::iter::once((&e.profile, &e.entities, false))
                .chain(e.holes.iter().map(|(p, ents)| (p, ents, true)))
                .find_map(|(p, ents, hole)| {
                    ents.iter()
                        .position(|x| x.0 == entity)
                        .map(|i| (p, i, hole))
                })
                .ok_or_else(|| {
                    broken(format!(
                        "side face of sketch entity {entity}: the entity is no longer part of \
                         the profile of {}",
                        fr.feature
                    ))
                })?;
            let (p, q) = match profile {
                SketchProfile::Polygon(v) => (v[i], v[(i + 1) % v.len()]),
                SketchProfile::Loop(segs) => match segs[i] {
                    f420_sketch::LoopSegment::Line { start } => {
                        (start, segs[(i + 1) % segs.len()].start())
                    }
                    f420_sketch::LoopSegment::Arc { .. } => {
                        return Err(broken(format!(
                            "sketch entity {entity} is an arc; its side face is curved"
                        )));
                    }
                },
                SketchProfile::Circle { .. } => {
                    return Err(broken("a circle's side face is curved".into()));
                }
            };
            let (a, b) = (f.to_world(p), f.to_world(q));
            let d = sub(b, a);
            let len = dot(d, d).sqrt();
            if len <= 0.0 {
                return Err(broken("degenerate profile edge".into()));
            }
            let x = scale(d, 1.0 / len);
            // CCW loop about the sketch normal: outward in-plane normal x × z; a
            // hole's wall faces into the hole (the loop's inside).
            let n = cross(x, f.z);
            let z = if e.removes != hole { scale(n, -1.0) } else { n };
            Ok(frame_from(a, z, x))
        }
        FaceRole::Imported { .. } => Err(broken(format!(
            "{} is an extrusion, not an import",
            fr.feature
        ))),
    }
}

/// Resolve a semantic face reference from the generating feature's definition, then
/// confirm it against the owning body's current B-rep: exactly one planar face
/// must coincide (same plane, same outward normal). Never guesses.
fn resolve_face(from: FeatureId, fr: &FaceRef, st: &Snapshot) -> Result<ResolvedFace> {
    let (frame, owner) = match fr.role {
        FaceRole::Imported { body, face } => {
            let faces =
                st.imported_faces
                    .get(&(fr.feature, body))
                    .ok_or(DocError::BrokenReference {
                        from,
                        detail: format!("{} did not import body {body}", fr.feature),
                    })?;
            let f = faces
                .iter()
                .find(|f| f.index == face)
                .ok_or(DocError::BrokenReference {
                    from,
                    detail: format!(
                        "imported face {face} of {body} is not planar or does not exist"
                    ),
                })?;
            (f.frame, body)
        }
        _ => {
            let e = st
                .extrudes
                .get(&fr.feature)
                .ok_or(DocError::BrokenReference {
                    from,
                    detail: format!("{} is not an evaluated extrusion", fr.feature),
                })?;
            (extrude_face_frame(e, fr, from)?, e.body)
        }
    };
    let state = st.bodies.get(&owner).ok_or(DocError::BrokenReference {
        from,
        detail: format!("body {owner} no longer exists"),
    })?;
    let BodyGeometry::Brep(body) = &state.geometry else {
        return Err(DocError::BrokenReference {
            from,
            detail: format!("body {owner} is a mesh; mesh faces cannot host sketches"),
        });
    };
    let n = k::planar_faces_on(body, &frame);
    let role = match fr.role {
        FaceRole::ExtrudeSide { entity } => format!("side face of entity {entity}"),
        FaceRole::Imported { face, .. } => format!("imported face {face}"),
        r => format!("{r:?}"),
    };
    if n == 0 {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "the {role} of {} no longer exists on body {owner} (removed by a later \
                 feature or edit); re-attach the sketch to another face",
                fr.feature
            ),
        });
    }
    if n > 1 {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "the {role} of {} is ambiguous: {n} faces of body {owner} lie in its plane \
                 (split or coplanar faces); refusing to guess — re-attach the sketch",
                fr.feature
            ),
        });
    }
    Ok(ResolvedFace {
        reference: *fr,
        frame,
        matching_brep_faces: n,
        body: owner,
    })
}
