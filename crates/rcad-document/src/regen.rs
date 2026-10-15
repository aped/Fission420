//! Deterministic regeneration of the feature history into per-body geometry
//! (ADR 0003): B-rep bodies (modelled or imported STEP) and mesh bodies (STL).
use crate::hole::{check_hole_inside, to_kernel};
use crate::import::{self, ImportedPlanarFace};
use crate::{
    BodyId, CopiedRole, DocError, Document, Extent, ExtrudeOptions, FaceRef, FaceRole, FeatureId,
    FeatureKind, Operation, Result, SketchPlane,
};
use crate::{LinkedCurve, ProjectSource, ProjectionLink};
use rcad_kernel::{self as k, Body, Frame, Measurements};
use rcad_sketch::{EntityId, Profile as SketchProfile, SolveReport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// When several faces of the body lie in the plane: a world point
    /// inside the referenced face (from its generating tool's footprint), which
    /// tells it apart from the others. None when the plane holds one face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<[f64; 3]>,
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
    pub solved_sketch: Option<rcad_sketch::Sketch>,
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
    /// Health after this regeneration (never persisted in documents).
    #[serde(default)]
    pub health: crate::FeatureHealth,
    /// Cuts and splits that separate a body: a point inside each piece, by
    /// body id ([`crate::Feature::piece_seeds`], adopted by the model).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub piece_seeds: BTreeMap<BodyId, Vec<[f64; 3]>>,
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
    /// A repaired copy of mesh body `source` (FeatureKind::MeshRepair).
    RepairedMesh {
        source: BodyId,
    },
    /// An edited copy of mesh body `source` (FeatureKind::MeshEdit);
    /// `operation`: "transform", "plane cut" or "decimate".
    EditedMesh {
        source: BodyId,
        operation: String,
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
    /// See [`ResolvedFace::sample`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<[f64; 3]>,
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
    /// Import identity of the faces of imported bodies changed by
    /// nothing but their import and direct edits (for picking a face to edit
    /// directly). Absent for any other body.
    pub imported_face_ids: BTreeMap<BodyId, crate::direct::ImportedFaceIds>,
    /// Planar faces of each imported body as imported, by (import feature,
    /// body): the source of [`crate::Feature::face_prints`].
    pub import_planes: BTreeMap<(FeatureId, BodyId), Arc<Vec<ImportedPlanarFace>>>,
    /// Solved placements of the component occurrences (docs/ASSEMBLIES.md).
    pub occurrences: Vec<crate::assembly::EvalOccurrence>,
}

impl Evaluation {
    /// Features that did not evaluate (failed themselves or depend on a
    /// failure), in evaluation order.
    pub fn failures(&self) -> impl Iterator<Item = &FeatureResult> {
        self.features.iter().filter(|r| r.health.is_error())
    }

    /// Whether every feature evaluated.
    pub fn is_healthy(&self) -> bool {
        self.failures().next().is_none()
    }

    /// The structured error of the first feature that failed itself.
    pub fn first_error(&self) -> Option<&DocError> {
        self.features.iter().find_map(|r| r.health.error.as_ref())
    }

    pub fn body_by_id(&self, id: BodyId) -> Option<&EvalBody> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// The semantic reference of face `face` of `body`: `face` is the index in
    /// the body's B-rep face order, the same index as the display mesh's
    /// `tri_face` and the kernel's edge and vertex tables, so a viewport pick
    /// maps to a [`FaceRef`] here. Some only when the face is planar, exactly
    /// one referenceable face ([`Evaluation::faces`]) lies in its plane with
    /// its outward normal, and that plane holds no other face of the body (the
    /// rule references resolve by, ADR 0003). None for curved faces, faces
    /// without a role and ambiguous planes: a pick is never bound by guess.
    pub fn face_ref_of(&self, body: BodyId, face: usize) -> Option<FaceRef> {
        let b = self.body_by_id(body)?.geometry.brep()?;
        if face >= k::face_count(b) {
            return None;
        }
        let k::DirectFace::Plane { origin, normal } = k::face_geometry(b, face).ok()? else {
            return None;
        };
        let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
        let mut hits = self.faces.iter().filter(|c| {
            let d = [0, 1, 2].map(|i| origin[i] - c.frame.origin[i]);
            c.body == body
                && dot(c.frame.z, normal) > 1.0 - 1e-9
                && dot(d, c.frame.z).abs() <= k::tol::KERNEL_LINEAR
                && k::planar_faces_on(b, &c.frame) == 1
        });
        let hit = hits.next()?;
        hits.next().is_none().then_some(hit.reference)
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
            imported_face_ids: BTreeMap::new(),
            import_planes: BTreeMap::new(),
            occurrences: vec![],
        }
    }
}

#[derive(Clone)]
struct SketchOut {
    frame: Frame,
    /// `Err(reason)` when the sketch is not a closed profile region.
    profile: std::result::Result<rcad_sketch::Region, String>,
    /// The solved sketch (region picking).
    solved: Arc<rcad_sketch::Sketch>,
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
        pick: Option<&rcad_sketch::RegionRef>,
    ) -> Result<rcad_sketch::Region> {
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
    fn usable(&self, user: FeatureId, sketch: FeatureId) -> Result<&rcad_sketch::Region> {
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
    /// Intersect: no face of the result is referencable by role.
    intersect: bool,
    /// No extent option (start on the sketch plane, one side, no thin/taper).
    plain: bool,
    /// Several regions in one feature (patterns/mirrors do not copy it).
    multi: bool,
    /// Draft angle (radians) of a tapered Join/Cut (patterns/mirrors
    /// do not copy it: their copies would be straight).
    taper: f64,
    /// Cut: accept a result that falls apart into several separate solids
    /// (one shell each; the caller splits them into bodies). Set only on the
    /// tool passed to [`apply_tool`], never on the stored feature output, so
    /// pattern/mirror copies keep refusing such results.
    pieces: bool,
    body: BodyId,
}

#[derive(Clone)]
struct BodyState {
    geometry: BodyGeometry,
    created_by: FeatureId,
    last_feature: FeatureId,
    source: BodySource,
    /// Check tessellation of exactly this geometry, left by the result check
    /// that produced it (reused as the next check's target mesh). Cleared by
    /// every other geometry change (`replace_body`).
    check_mesh: Option<k::CheckMesh>,
}

/// Regeneration state after a history prefix (cached by [`RegenCache`]).
#[derive(Clone)]
struct Snapshot {
    sketches: HashMap<FeatureId, SketchOut>,
    extrudes: HashMap<FeatureId, ExtrudeOut>,
    /// Hole features: their tools (counterbore, then hole), for patterns.
    holes: HashMap<FeatureId, Vec<ExtrudeOut>>,
    /// Pattern/mirror occurrences: (instance, tool index, the tool as applied,
    /// with its start frame moved to the occurrence), for [`FaceRole::Copy`].
    copies: HashMap<FeatureId, Vec<(u32, u32, ExtrudeOut)>>,
    /// Chamfer features: the chamfer face's frame (outward normal) and body.
    chamfers: HashMap<FeatureId, (Frame, BodyId)>,
    imported_faces: HashMap<(FeatureId, BodyId), Arc<Vec<ImportedPlanarFace>>>,
    /// Imported faces whose plane no longer matches their stored print
    /// ([`crate::Feature::face_prints`]): (import, body, face) -> why.
    import_moved: HashMap<(FeatureId, BodyId, u32), String>,
    /// Planar faces of each imported body as imported (never changed by
    /// direct edits, unlike `imported_faces`).
    import_planes: BTreeMap<(FeatureId, BodyId), Arc<Vec<ImportedPlanarFace>>>,
    /// Face identity of imported bodies (direct edits).
    face_ids: BTreeMap<BodyId, crate::direct::ImportedFaceIds>,
    bodies: BTreeMap<BodyId, BodyState>,
    /// Body ids in creation order.
    created: Vec<BodyId>,
    results: Vec<FeatureResult>,
    /// Evaluation index of every evaluated feature.
    index: HashMap<FeatureId, usize>,
    /// Rigid moves of bodies (body, evaluation index of the move, transform),
    /// in order: faces made before a move are found moved with the body.
    moves: Vec<(BodyId, usize, k::Rigid)>,
    /// Sheet-metal bodies (definition state for flanges / flat pattern).
    sheets: BTreeMap<BodyId, crate::sheet::SheetModel>,
    /// Partial regeneration: features not evaluated because they failed or
    /// depend on a failure, each with the failed feature at the root.
    failed: BTreeMap<FeatureId, FeatureId>,
    /// Bodies a failed feature should have made or changed (with that
    /// feature): later features reading them are not evaluated.
    tainted: BTreeMap<BodyId, FeatureId>,
    /// First error of a partial regeneration of this prefix.
    first_error: Option<DocError>,
}

impl Snapshot {
    /// The transform carrying faces made by `feature` on `body` to where
    /// `body` is now (the moves after `feature`).
    fn placement(&self, body: BodyId, feature: FeatureId) -> k::Rigid {
        let at = self.index.get(&feature).copied().unwrap_or(0);
        self.moves
            .iter()
            .filter(|(b, i, _)| *b == body && *i > at)
            .fold(k::Rigid::IDENTITY, |acc, (_, _, t)| t.after(&acc))
    }

    /// Whether `body` was moved after `feature`.
    fn moved_since(&self, body: Option<BodyId>, feature: FeatureId) -> bool {
        body.is_some_and(|b| !self.placement(b, feature).is_identity())
    }

    fn body_of_extrude(&self, f: FeatureId) -> Option<BodyId> {
        self.extrudes.get(&f).map(|e| e.body)
    }

    fn new() -> Self {
        Self {
            sketches: HashMap::new(),
            extrudes: HashMap::new(),
            holes: HashMap::new(),
            copies: HashMap::new(),
            chamfers: HashMap::new(),
            imported_faces: HashMap::new(),
            import_moved: HashMap::new(),
            import_planes: BTreeMap::new(),
            face_ids: BTreeMap::new(),
            bodies: BTreeMap::new(),
            created: Vec::new(),
            results: Vec::new(),
            index: HashMap::new(),
            moves: Vec::new(),
            sheets: BTreeMap::new(),
            failed: BTreeMap::new(),
            tainted: BTreeMap::new(),
            first_error: None,
        }
    }

    /// The failed feature `f` depends on (explicitly, or through a body it
    /// reads or changes that a failed feature should have made or changed).
    fn failed_upstream(&self, doc: &Document, f: &crate::Feature) -> Option<FeatureId> {
        if self.failed.is_empty() {
            return None;
        }
        let deps = doc.dependencies_of(f);
        if let Some(root) = deps.iter().find_map(|d| self.failed.get(d)) {
            return Some(*root);
        }
        doc.read_bodies(f)
            .iter()
            .find_map(|b| self.tainted.get(b))
            .copied()
    }

    /// Record that `f` was not evaluated because of failed feature `root`.
    fn mark_failed(&mut self, doc: &Document, f: &crate::Feature, root: FeatureId) {
        self.failed.insert(f.id, root);
        for b in f.created_bodies().into_iter().chain(doc.read_bodies(f)) {
            self.tainted.entry(b).or_insert(root);
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
        if doc.is_rolled_back(*id) {
            key.push_str("\trolled back");
        }
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

/// The sheet model of body `body` made by base `base`, refused when
/// another feature changed the body since its last sheet feature.
fn sheet_of(
    st: &Snapshot,
    id: FeatureId,
    base: FeatureId,
    body: BodyId,
) -> Result<crate::sheet::SheetModel> {
    let m = st
        .sheets
        .get(&body)
        .filter(|m| m.base == base)
        .ok_or(DocError::BrokenReference {
            from: id,
            detail: format!("body {body} is not the sheet body of {base} at this point"),
        })?;
    let last = st.bodies.get(&body).map(|b| b.last_feature);
    if last != Some(m.last_feature) {
        return Err(DocError::ProductLimit(format!(
            "{id}: body {body} was changed by {} after its last sheet feature; sheet features must come before other edits of the body",
            last.map(|f| f.to_string()).unwrap_or_default()
        )));
    }
    Ok(m.clone())
}

/// The B-rep of surface-operation source `source`.
fn surface_source(st: &Snapshot, id: FeatureId, source: BodyId) -> Result<Body> {
    match st.bodies.get(&source).map(|b| &b.geometry) {
        Some(BodyGeometry::Brep(b)) => Ok(b.clone()),
        Some(BodyGeometry::Mesh(_)) => Err(DocError::ProductLimit(format!(
            "{id}: body {source} is a mesh; surface operations need an exact B-rep"
        ))),
        None => Err(DocError::BrokenReference {
            from: id,
            detail: format!("source body {source} does not exist at this point of the history"),
        }),
    }
}

/// Exact B-rep of `target`, or a structured error (missing body, mesh body,
/// open surface body).
fn brep_target(st: &Snapshot, id: FeatureId, target: BodyId) -> Result<Body> {
    let b = st.bodies.get(&target).ok_or(DocError::BrokenReference {
        from: id,
        detail: format!("target body {target} does not exist at this point of the history"),
    })?;
    match &b.geometry {
        BodyGeometry::Brep(body) if body.is_open_shell() => Err(DocError::ProductLimit(format!(
            "{id}: body {target} is an open surface body; solid operations need a closed solid (stitch, fill or thicken it first)"
        ))),
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
    cache: Option<&mut RegenCache>,
    measure_body: bool,
) -> Result<Evaluation> {
    match regen_impl(doc, cache, measure_body, None, false)? {
        Regen::Done(e) => Ok(*e),
        Regen::Stopped(_) => Err(DocError::Validation(
            "internal: regeneration stopped early".into(),
        )),
    }
}

/// Regenerate as far as possible: a feature that fails is recorded in its
/// [`FeatureResult::health`] (with the state just before it kept), its
/// dependents are flagged as failed upstream and not evaluated, and every
/// independent feature is still evaluated. Fails only for structural
/// problems (dependency cycles, inconsistent body tables, cancellation,
/// unassigned split pieces). Deterministic, like [`regenerate`].
pub fn regenerate_partial(
    doc: &Document,
    cache: Option<&mut RegenCache>,
    measure_body: bool,
) -> Result<Evaluation> {
    match regen_impl(doc, cache, measure_body, None, true)? {
        Regen::Done(e) => Ok(*e),
        Regen::Stopped(_) => Err(DocError::Validation(
            "internal: regeneration stopped early".into(),
        )),
    }
}

/// How to repair feature `id` that failed with `e`, when there is a
/// generic answer.
fn repair_hint(doc: &Document, id: FeatureId, e: &DocError) -> Option<String> {
    let name = doc.feature(id).map(|f| f.name.as_str()).unwrap_or("");
    match e {
        DocError::BrokenReference { .. } => Some(format!(
            "edit {id} {name} and select its references again, or delete it"
        )),
        DocError::SketchSolve { .. } => Some(format!(
            "edit sketch {id} {name}: remove or change the conflicting constraints"
        )),
        DocError::Kernel { .. } | DocError::ProductLimit(_) => Some(format!(
            "edit {id} {name} (change its size or operation), roll the marker back before \
             it, or suppress it"
        )),
        _ => None,
    }
}

/// A full evaluation, or the state just before feature `stop`.
enum Regen {
    Done(Box<Evaluation>),
    Stopped(Box<Snapshot>),
}

fn regen_impl(
    doc: &Document,
    mut cache: Option<&mut RegenCache>,
    measure_body: bool,
    stop: Option<FeatureId>,
    partial: bool,
) -> Result<Regen> {
    let order = doc.evaluation_order()?;
    doc.validate_body_tables()?;
    // Strict: a dependency on a suppressed/rolled-back feature fails the
    // whole regeneration. Partial: that feature (and its dependents) is
    // flagged and the rest evaluates.
    let mut bad_refs: HashMap<FeatureId, DocError> = HashMap::new();
    for (id, e) in doc
        .missing_body_refs()
        .into_iter()
        .chain(doc.suppression_violations()?)
    {
        if !partial {
            return Err(e);
        }
        bad_refs.entry(id).or_insert(e);
    }
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
    // A cached prefix that holds a failure (stored by a partial
    // regeneration) fails a strict one, as evaluating it would.
    if !partial && let Some(e) = &st.first_error {
        return Err(e.clone());
    }

    for (i, id) in order.iter().copied().enumerate().skip(start) {
        if stop == Some(id) {
            return Ok(Regen::Stopped(Box::new(st)));
        }
        crate::check_cancel(id)?;
        st.index.insert(id, i);
        let f = doc.feature(id)?;
        let t_feature = std::time::Instant::now();
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
            health: crate::FeatureHealth::healthy(),
            piece_seeds: BTreeMap::new(),
        };
        if f.suppressed || doc.is_rolled_back(id) {
            // Not evaluated (no dependent is evaluated: `check_suppression`).
            r.notes.push(if f.suppressed {
                "suppressed: not evaluated".into()
            } else {
                "rolled back: after the timeline marker, not evaluated".into()
            });
            r.health = crate::FeatureHealth {
                state: if f.suppressed {
                    crate::HealthState::Suppressed
                } else {
                    crate::HealthState::RolledBack
                },
                ..Default::default()
            };
            st.results.push(r);
            if let Some(c) = cache.as_deref_mut() {
                c.insert(keys[i].clone(), st.clone());
            }
            continue;
        }
        // Partial regeneration: a feature that reads a failed feature, or
        // a body a failed feature should have made or changed, is not
        // evaluated (never built on a state the history does not describe).
        let upstream = if partial {
            st.failed_upstream(doc, f)
        } else {
            None
        };
        let outcome = match (bad_refs.remove(&id), upstream) {
            (Some(e), _) => Err(e),
            (None, Some(up)) => {
                r.health = crate::FeatureHealth::upstream(up);
                st.mark_failed(doc, f, up);
                Ok(false)
            }
            (None, None) => Ok(true),
        };
        // Partial regeneration restores this state when the feature fails.
        let pre = partial.then(|| st.clone());
        let evaluated = match outcome {
            Ok(true) => {
                // A panic inside one feature's evaluation (a kernel defect)
                // fails that feature like any other error; the state it was
                // building is discarded (restored from `pre`, or dropped with
                // the strict regeneration's error).
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    eval_feature(doc, id, i, f, &mut st, &mut r)
                }))
                .unwrap_or_else(|p| {
                    Err(DocError::Kernel {
                        feature: id,
                        detail: format!(
                            "internal error while evaluating this feature ({}); nothing was \
                             changed",
                            panic_text(&p)
                        ),
                    })
                })
            }
            Ok(false) => Ok(()),
            Err(e) => Err(e),
        };
        if let Err(e) = evaluated {
            match e {
                // Structural outcomes the caller acts on, never per-feature health.
                DocError::NeedsPieces { .. } | DocError::Cancelled(_) => return Err(e),
                e if !partial => return Err(e),
                e => {
                    if let Some(pre) = pre {
                        st = pre;
                    }
                    r = FeatureResult {
                        id,
                        name: f.name.clone(),
                        solve: r.solve,
                        frame: None,
                        resolved_face: None,
                        measurements: None,
                        solved_sketch: r.solved_sketch,
                        profile_error: r.profile_error,
                        body: None,
                        notes: r.notes,
                        health: crate::FeatureHealth::failed(e.clone(), repair_hint(doc, id, &e)),
                        piece_seeds: BTreeMap::new(),
                    };
                    st.mark_failed(doc, f, id);
                    st.first_error.get_or_insert(e);
                }
            }
        }
        st.results.push(r);
        trace_regen(id, "feature", t_feature);
        let t_cache = std::time::Instant::now();
        if let Some(c) = cache.as_deref_mut() {
            c.insert(keys[i].clone(), st.clone());
        }
        trace_regen(id, "cache_insert", t_cache);
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
    let imported_face_ids = st
        .face_ids
        .iter()
        .filter(|(b, f)| {
            st.bodies
                .get(b)
                .is_some_and(|s| s.last_feature == f.valid_at)
        })
        .map(|(b, f)| (*b, f.clone()))
        .collect();
    let mut eval = Evaluation {
        features: st.results,
        bodies,
        faces,
        body: None,
        measurements: None,
        imported_face_ids,
        import_planes: st.import_planes,
        occurrences: doc.assembly.evaluate()?,
    };
    eval.refresh_legacy_view();
    Ok(Regen::Done(Box::new(eval)))
}

/// The message of a caught panic payload (`&str` or `String`), else a placeholder.
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-text payload".into())
}

/// Imported faces of import `import` that any feature references
/// (`FaceRole::Imported`), found in the serialized definitions so every kind
/// of reference counts.
pub(crate) fn imported_face_refs(doc: &Document, import: FeatureId) -> BTreeSet<(BodyId, u32)> {
    fn walk(v: &serde_json::Value, import: u64, out: &mut BTreeSet<(BodyId, u32)>) {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("feature").and_then(|f| f.as_u64()) == Some(import)
                    && let Some(i) = m.get("role").and_then(|r| r.get("Imported"))
                    && let (Some(b), Some(f)) = (
                        i.get("body").and_then(|b| b.as_u64()),
                        i.get("face").and_then(|f| f.as_u64()),
                    )
                    && let (Ok(b), Ok(f)) = (u32::try_from(b), u32::try_from(f))
                {
                    out.insert((BodyId(b), f));
                }
                for x in m.values() {
                    walk(x, import, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, import, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    for f in &doc.features {
        if let Ok(v) = serde_json::to_value(&f.kind) {
            walk(&v, u64::from(import.0), &mut out);
        }
    }
    out
}

/// A cut's or split's piece ids: the ids the feature holds, its stored
/// seeds, and where to put the seeds of this evaluation.
type Pieces<'a> = (
    &'a [BodyId],
    &'a BTreeMap<BodyId, Vec<[f64; 3]>>,
    &'a mut BTreeMap<BodyId, Vec<[f64; 3]>>,
);

/// Body ids for the pieces of a cut or split (ADR 0003, piece identity).
/// `parts` come in the positional order and `required` (the target; for a
/// split the target and the new body) must each receive a piece; `spare`
/// are the feature's extra piece ids.
///
/// Ids with seeds are matched first: repeatedly, the (id, piece) pair with
/// the most of the id's seed points inside the piece (the deep point counts
/// twice) is taken, provided
/// that count is the unique maximum for the id (over free pieces) and the
/// maximum for the piece (over unmatched ids; when pieces rejoined, the
/// earliest id, the target first, keeps the piece). Ids without seeds (new ids,
/// documents written before seeds) then take the remaining pieces in
/// positional order. A required id left unmatched by its seeds is a broken
/// reference (the piece is never guessed); a spare one makes no body (a
/// later feature that targets it fails explicitly). More unclaimed pieces
/// than free ids is [`DocError::NeedsPieces`]. On success `seeds_out`
/// receives fresh seeds for every id that got a piece (when the result has
/// several pieces) and keeps the old seeds of the others.
fn assign_pieces(
    id: FeatureId,
    parts: Vec<Body>,
    required: &[BodyId],
    spare: &[BodyId],
    seeds: &BTreeMap<BodyId, Vec<[f64; 3]>>,
    seeds_out: &mut BTreeMap<BodyId, Vec<[f64; 3]>>,
) -> Result<Vec<(BodyId, Body)>> {
    if parts.len() < required.len() {
        return Err(DocError::Kernel {
            feature: id,
            detail: format!(
                "the result has {} piece(s), {} expected",
                parts.len(),
                required.len()
            ),
        });
    }
    let ids: Vec<BodyId> = required.iter().chain(spare).copied().collect();
    let mut owner: Vec<Option<BodyId>> = vec![None; parts.len()];
    // Ids whose identity comes from seeds (matched or not).
    let mut seeded: BTreeSet<BodyId> = BTreeSet::new();
    let with_seed: Vec<(BodyId, &[[f64; 3]])> = ids
        .iter()
        .filter_map(|b| seeds.get(b).filter(|v| !v.is_empty()).map(|v| (*b, &v[..])))
        .collect();
    // A single piece needs no matching: it keeps the target.
    if parts.len() > 1 && !with_seed.is_empty() {
        let points: Vec<[f64; 3]> = with_seed
            .iter()
            .flat_map(|(_, v)| v.iter().copied())
            .collect();
        let inside: Vec<Vec<Option<bool>>> = parts
            .iter()
            .map(|p| k::point_membership(p, &points))
            .collect::<std::result::Result<_, _>>()
            .map_err(kernel(id))?;
        // count[k][j]: weight of id k's seeds inside piece j; the deep
        // point (first) weighs 2, the point away from the cut 1.
        let mut count = vec![vec![0usize; parts.len()]; with_seed.len()];
        let mut at = 0;
        for (k, (_, v)) in with_seed.iter().enumerate() {
            for n in 0..v.len() {
                for (j, row) in inside.iter().enumerate() {
                    if row[at] == Some(true) {
                        count[k][j] += if n == 0 { 2 } else { 1 };
                    }
                }
                at += 1;
            }
        }
        let mut matched = vec![false; with_seed.len()];
        loop {
            let unique_max = |vals: &mut dyn Iterator<Item = usize>, v: usize| {
                let mut n = 0;
                for x in vals {
                    if x > v {
                        return false;
                    }
                    n += usize::from(x == v);
                }
                n == 1
            };
            let mut best: Option<(usize, usize, usize)> = None;
            for k in (0..with_seed.len()).filter(|&k| !matched[k]) {
                for j in (0..parts.len()).filter(|&j| owner[j].is_none()) {
                    let c = count[k][j];
                    if c == 0 || best.is_some_and(|(b, _, _)| c <= b) {
                        continue;
                    }
                    let row_ok = unique_max(
                        &mut (0..parts.len())
                            .filter(|&jj| owner[jj].is_none())
                            .map(|jj| count[k][jj]),
                        c,
                    );
                    // Pieces that rejoined hold the seeds of several ids:
                    // the earliest id (the target first) keeps the piece.
                    let col_ok = (0..with_seed.len())
                        .filter(|&kk| kk != k && !matched[kk])
                        .all(|kk| count[kk][j] < c || (count[kk][j] == c && kk > k));
                    if row_ok && col_ok {
                        best = Some((c, k, j));
                    }
                }
            }
            let Some((_, k, j)) = best else { break };
            matched[k] = true;
            owner[j] = Some(with_seed[k].0);
        }
        for (k, (b, _)) in with_seed.iter().enumerate() {
            seeded.insert(*b);
            if !matched[k] && required.contains(b) {
                let holders = count[k].iter().filter(|&&c| c > 0).count();
                let why = if holders == 0 {
                    "no piece of the result contains its reference points".to_string()
                } else {
                    format!("its reference points do not single out one of {holders} pieces")
                };
                return Err(DocError::BrokenReference {
                    from: id,
                    detail: format!(
                        "the piece that is {b} can no longer be identified ({why}); edit the \
                         feature to assign the pieces again"
                    ),
                });
            }
        }
    }
    // Ids without seeds take the unclaimed pieces in positional order.
    let mut free = ids.iter().filter(|b| !seeded.contains(b)).copied();
    let mut short = 0;
    for o in owner.iter_mut().filter(|o| o.is_none()) {
        match free.next() {
            Some(b) => *o = Some(b),
            None => short += 1,
        }
    }
    if short > 0 {
        return Err(DocError::NeedsPieces {
            feature: id,
            pieces: spare.len() + short + 1,
            assigned: spare.len(),
        });
    }
    if let Some(b) = required.iter().find(|b| !owner.contains(&Some(**b))) {
        return Err(DocError::BrokenReference {
            from: id,
            detail: format!("no piece of the result is left for {b}"),
        });
    }
    let mut out: Vec<(BodyId, Body)> = owner.into_iter().flatten().zip(parts).collect();
    // Required ids first, then the others in the feature's order.
    out.sort_by_key(|(b, _)| ids.iter().position(|x| x == b));
    // Ids without a piece now (rejoined, or the cut no longer separates the
    // body) keep their seeds, so they find their pieces again when the cut
    // separates the body again.
    for b in &ids {
        if !out.iter().any(|(o, _)| o == b)
            && let Some(v) = seeds.get(b)
        {
            seeds_out.insert(*b, v.clone());
        }
    }
    if out.len() == 1
        && let Some(v) = seeds.get(&out[0].0)
    {
        seeds_out.insert(out[0].0, v.clone());
    }
    if out.len() > 1 {
        for (i, (b, piece)) in out.iter().enumerate() {
            let others: Vec<&Body> = out
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, (_, p))| p)
                .collect();
            // A piece too thin for seeds keeps positional identity.
            if let Ok(v) = k::piece_seeds(piece, &others) {
                let round = |x: f64| (x * 1e6).round() / 1e6 + 0.0;
                seeds_out.insert(*b, v.into_iter().map(|p| p.map(round)).collect());
            }
        }
    }
    Ok(out)
}

/// Evaluate feature `f` (at evaluation index `i`) on `st`, filling `r`.
/// On error `st` may be partially changed; the caller restores it.
fn eval_feature(
    doc: &Document,
    id: FeatureId,
    i: usize,
    f: &crate::Feature,
    st: &mut Snapshot,
    r: &mut FeatureResult,
) -> Result<()> {
    match &f.kind {
        FeatureKind::Sketch {
            plane,
            sketch,
            links,
        } => {
            let (frame, resolved) = plane_frame(id, plane, st)?;
            let mut s = sketch.clone();
            for (li, link) in links.iter().enumerate() {
                let curves = link_curves(id, &frame, &link.source, st)?;
                bind_link(id, li, link, &curves, &mut s)?;
            }
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
            axis,
            reverse,
            merge,
        } => {
            let s = st.sketches.get(sketch).ok_or(not_a_sketch(id, *sketch))?;
            let profile = s.single_loop(id, *sketch, "Revolve")?.clone();
            // Runs 1-5: about the sketch Y axis with the profile on +X,
            // straight to the kernel (unchanged results); otherwise (other
            // axes, the profile on −X, reverse) through `revolve_about`.
            let legacy = (axis.is_default() && !*reverse)
                .then(|| k::revolve(&to_kernel(&profile), &s.frame, *angle));
            let b = match legacy {
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    revolve_about(id, &profile, &s.frame, &s.solved, *axis, *angle, *reverse)
                        .map_err(|_| DocError::Validation(format!("{id}: {e}")))?
                }
                None => revolve_about(id, &profile, &s.frame, &s.solved, *axis, *angle, *reverse)?,
            };
            match merge {
                None | Some(Operation::NewBody { .. }) => {
                    create_body(st, id, *body, BodyGeometry::Brep(b), BodySource::Modeled)?;
                    r.body = Some(*body);
                }
                Some(Operation::Join { target } | Operation::Cut { target })
                | Some(Operation::Intersect { target }) => {
                    let op = match merge {
                        Some(Operation::Join { .. }) => k::CombineOp::Join,
                        Some(Operation::Cut { .. }) => k::CombineOp::Cut,
                        _ => k::CombineOp::Intersect,
                    };
                    let t = brep_target(st, id, *target)?;
                    // A full turn is the same solid whatever its seam:
                    // Truck often fails when the tool's seam lies on the
                    // target's (coaxial revolutions), so retry with the
                    // seam turned about the axis.
                    let (ao, ad) = revolve_axis_world(id, &s.frame, &s.solved, *axis)?;
                    let full = *angle >= 2.0 * std::f64::consts::PI - 1e-12;
                    let turns: &[f64] = if full {
                        &[0.0, 0.37, 1.13, 2.71]
                    } else {
                        &[0.0]
                    };
                    let mut last = None;
                    let mut done = None;
                    for &a in turns {
                        let tool = if a == 0.0 {
                            b.clone()
                        } else {
                            let rot = k::Rigid::new([0.0; 3], ao, ad, a).map_err(kernel(id))?;
                            k::transform_body(&b, &rot).map_err(kernel(id))?
                        };
                        match k::combine(&t, std::slice::from_ref(&tool), op) {
                            Ok(x) => {
                                done = Some(x);
                                break;
                            }
                            Err(e) => last = Some(e),
                        }
                    }
                    let (out, rep) = done.ok_or_else(|| DocError::Kernel {
                        feature: id,
                        detail: format!(
                            "revolve {op:?}: {}",
                            last.map(|e| e.to_string()).unwrap_or_default()
                        ),
                    })?;
                    if !rep.passed() {
                        return Err(DocError::GeometryCheck {
                            feature: id,
                            detail: format!(
                                "revolve {op:?}: {}; refused (the model is unchanged)",
                                rep.failure().unwrap_or_default()
                            ),
                        });
                    }
                    replace_body(st, id, *target, out);
                    r.body = Some(*target);
                }
            }
        }
        FeatureKind::Sweep {
            profile,
            path,
            body,
        } => {
            let sp = st.sketches.get(profile).ok_or(not_a_sketch(id, *profile))?;
            let pp = st.sketches.get(path).ok_or(not_a_sketch(id, *path))?;
            let prof = to_kernel(sp.single_loop(id, *profile, "Sweep")?);
            let segs = path_segments(&pp.solved, &pp.frame, sp.frame.origin)
                .map_err(|m| DocError::ProductLimit(format!("{id}: sweep path: {m}")))?;
            let solid = k::sweep(&prof, &sp.frame, &segs).map_err(|e| match e {
                k::KernelError::InvalidInput(m) => {
                    // The kernel message already starts with "sweep: ".
                    DocError::ProductLimit(format!("{id}: {m}"))
                }
                other => DocError::Kernel {
                    feature: id,
                    detail: other.to_string(),
                },
            })?;
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(solid),
                BodySource::Modeled,
            )?;
            r.body = Some(*body);
        }
        FeatureKind::Loft { a, b, body, via } => {
            let mut sections = Vec::new();
            for f in std::iter::once(a).chain(via.iter()).chain([b]) {
                let so = st.sketches.get(f).ok_or(not_a_sketch(id, *f))?;
                sections.push((to_kernel(so.single_loop(id, *f, "Loft")?), so.frame));
            }
            let solid = k::loft_sections(&sections).map_err(|e| match e {
                k::KernelError::InvalidInput(m) => {
                    DocError::ProductLimit(format!("{id}: loft: {m}"))
                }
                other => DocError::Kernel {
                    feature: id,
                    detail: other.to_string(),
                },
            })?;
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(solid),
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
            options,
            pieces,
        } => {
            let out = extrude_feature(
                st,
                id,
                *sketch,
                *distance,
                *operation,
                *extent,
                *reverse,
                region.as_ref(),
                options,
                (pieces, &f.piece_seeds, &mut r.piece_seeds),
            )?;
            r.body = Some(out.body);
            st.extrudes.insert(id, out);
        }
        FeatureKind::Cut { sketch, target, .. } => {
            legacy_cut(doc, st, id, *sketch, *target, None)?;
            r.body = Some(*target);
        }
        FeatureKind::Pocket {
            sketch,
            depth,
            target,
        } => {
            legacy_cut(doc, st, id, *sketch, *target, Some(*depth))?;
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
                let planes = Arc::new(solid.planar_faces);
                for p in f.face_prints.iter().filter(|p| p.body == *body) {
                    let now = planes.iter().find(|x| x.index == p.face);
                    if !now.is_some_and(|x| p.matches_face(x)) {
                        st.import_moved.insert(
                            (id, *body, p.face),
                            format!(
                                "imported face {} of {body} is no longer the face that was \
                                 referenced (the file now imports another face at that \
                                 index); pick the face again",
                                p.face
                            ),
                        );
                    }
                }
                st.import_planes.insert((id, *body), planes.clone());
                st.imported_faces.insert((id, *body), planes);
                st.face_ids.insert(
                    *body,
                    crate::direct::ImportedFaceIds::at_import(id, k::face_count(&solid.body)),
                );
                create_body(
                    st,
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
            second,
        } => {
            r.body = Some(linear_pattern(
                st, id, *seed, *count, *spacing, *direction, *second,
            )?);
        }
        FeatureKind::CircularPattern {
            seed,
            count,
            angle,
            center,
        } => {
            r.body = Some(circular_pattern(st, id, *seed, *count, *angle, *center)?);
        }
        FeatureKind::Mirror { seed, plane } => {
            let frame = plane_frame(id, plane, st)?.0;
            r.frame = Some(frame);
            r.body = Some(mirror_feature(st, id, *seed, &frame)?);
        }
        FeatureKind::Chamfer { a, b, distance } => {
            let (fa, fb) = (resolve_face(id, a, st)?, resolve_face(id, b, st)?);
            if fa.body != fb.body {
                return Err(DocError::Validation(format!(
                    "{id}: the two faces belong to different bodies ({} and {})",
                    fa.body, fb.body
                )));
            }
            // The kernel finds the edge along the whole line where the two
            // planes meet; with other faces in either plane (pattern
            // occurrences, split faces) that line spans several edges.
            if fa.sample.is_some() || fb.sample.is_some() {
                return Err(DocError::ProductLimit(format!(
                    "{id}: this chamfer edge lies on a plane that other faces of the body \
                     share (pattern copies, split faces), so the edge between the two picked \
                     faces is not unique (product limit). For patterned pockets, round the \
                     seed sketch's corners (sketch fillet) so every copy is rounded"
                )));
            }
            let body = brep_target(st, id, fa.body)?;
            let out =
                k::chamfer_edge(&body, &fa.frame, &fb.frame, *distance).map_err(kernel(id))?;
            replace_body(st, id, fa.body, out);
            if let Some(frame) = chamfer_face_frame(&fa.frame, &fb.frame, *distance) {
                st.chamfers.insert(id, (frame, fa.body));
            }
            r.body = Some(fa.body);
        }
        FeatureKind::Fillet { a, b, radius } => {
            let (fa, fb) = (resolve_face(id, a, st)?, resolve_face(id, b, st)?);
            if fa.body != fb.body {
                return Err(DocError::Validation(format!(
                    "{id}: the two faces belong to different bodies ({} and {})",
                    fa.body, fb.body
                )));
            }
            if fa.sample.is_some() || fb.sample.is_some() {
                return Err(DocError::ProductLimit(format!(
                    "{id}: this fillet edge lies on a plane that other faces of the body \
                     share (pattern copies, split faces), so the edge between the two picked \
                     faces is not unique (product limit). For patterned pockets, round the \
                     seed sketch's corners (sketch fillet) so every copy is rounded"
                )));
            }
            let body = brep_target(st, id, fa.body)?;
            let out = k::fillet_edge(&body, &fa.frame, &fb.frame, *radius).map_err(kernel(id))?;
            replace_body(st, id, fa.body, out);
            r.body = Some(fa.body);
        }
        FeatureKind::FilletLoop {
            face,
            radius,
            holes,
        } => {
            let fa = resolve_face(id, face, st)?;
            let body = brep_target(st, id, fa.body)?;
            let out = if *holes {
                k::fillet_cap_face_edges(&body, &fa.frame, *radius)
            } else {
                k::fillet_cap_loop(&body, &fa.frame, *radius)
            }
            .map_err(kernel(id))?;
            replace_body(st, id, fa.body, out);
            r.body = Some(fa.body);
        }
        FeatureKind::ChamferLoop {
            face,
            distance,
            holes,
        } => {
            let fa = resolve_face(id, face, st)?;
            let body = brep_target(st, id, fa.body)?;
            let out = if *holes {
                k::chamfer_cap_face_edges(&body, &fa.frame, *distance)
            } else {
                k::chamfer_cap_loop(&body, &fa.frame, *distance)
            }
            .map_err(kernel(id))?;
            replace_body(st, id, fa.body, out);
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
            let (frame, rf) = plane_frame(id, plane, st)?;
            r.resolved_face = rf;
            r.frame = Some(frame);
            let tools = hole_tools(
                st,
                id,
                &frame,
                *center,
                *diameter,
                *depth,
                *counterbore,
                *target,
            )?;
            let mut body = brep_target(st, id, *target)?;
            let labels: &[&str] = if tools.len() == 2 {
                &["counterbore", "hole"]
            } else {
                &["hole"]
            };
            for (what, tool) in labels.iter().zip(&tools) {
                body = apply_tool(&body, tool, &frame, Some(what), id, None)?.0;
            }
            replace_body(st, id, *target, body);
            st.holes.insert(id, tools);
            r.body = Some(*target);
        }
        FeatureKind::MoveBody {
            body,
            translate,
            origin,
            axis,
            angle,
            copy,
        } => {
            let t = k::Rigid::new(*translate, *origin, *axis, *angle)
                .map_err(|e| DocError::Validation(format!("{id}: {e}")))?;
            let b = brep_target(st, id, *body)?;
            let moved = k::transform_body(&b, &t).map_err(kernel(id))?;
            match copy {
                Some(c) => {
                    create_body(st, id, *c, BodyGeometry::Brep(moved), BodySource::Modeled)?;
                    r.body = Some(*c);
                }
                None => {
                    replace_body(st, id, *body, moved);
                    st.moves.push((*body, i, t));
                    r.body = Some(*body);
                }
            }
        }
        FeatureKind::SheetMetal {
            sketch,
            thickness,
            radius,
            k_factor,
            width,
            flip,
            body,
            geometric_order,
        } => {
            let so = st.sketches.get(sketch).ok_or(not_a_sketch(id, *sketch))?;
            let chain = crate::sheet::flange_chain(&so.solved, *geometric_order)
                .map_err(|m| DocError::Validation(format!("{id}: sheet metal profile: {m}")))?;
            let part =
                crate::sheet::contour_flange(id, &chain, *thickness, *radius, *k_factor, *flip)?;
            if !(width.is_finite() && *width > 0.0) {
                return Err(DocError::Validation(format!(
                    "{id}.width: must be > 0 mm, got {width}"
                )));
            }
            let solid = k::extrude_region(&to_kernel(&part.section), &[], &so.frame, *width)
                .map_err(kernel(id))?;
            r.frame = Some(so.frame);
            r.notes.push(format!(
                "flat length {:.6} mm (K {}), bend lines at {}",
                part.flat_length,
                k_factor,
                part.bend_lines
                    .iter()
                    .map(|x| format!("{x:.6}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(solid),
                BodySource::Modeled,
            )?;
            r.body = Some(*body);
        }
        // Sheet metal (logic in `crate::sheet`).
        FeatureKind::SheetBase {
            sketch,
            rule,
            flip,
            body,
        } => {
            let so = st.sketches.get(sketch).ok_or(not_a_sketch(id, *sketch))?;
            let region = so.usable(id, *sketch)?;
            let model =
                crate::sheet::base_model(id, *body, so.frame, *flip, rule, region, &so.solved)?;
            let built = crate::sheet::build(&model)?;
            r.frame = Some(so.frame);
            r.notes = crate::sheet::notes(&built, rule);
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(built.folded),
                BodySource::Modeled,
            )?;
            st.sheets.insert(*body, model);
            r.body = Some(*body);
        }
        FeatureKind::SheetFlange { base, body, .. } => {
            let mut model = sheet_of(st, id, *base, *body)?;
            // A flange after the flat pattern would leave the
            // displayed flat out of date (the DXF includes every flange).
            if let Some(flat) = model.flat {
                return Err(DocError::Validation(format!(
                    "{id}: this flange comes after the flat pattern {flat} of the same sheet, which would be out of date; keep flanges before the flat pattern"
                )));
            }
            model.flanges.extend(crate::sheet::flange_def(id, &f.kind));
            let built = crate::sheet::build(&model)?;
            r.notes = crate::sheet::notes(&built, &model.rule);
            replace_body(st, id, *body, built.folded);
            model.last_feature = id;
            st.sheets.insert(*body, model);
            r.body = Some(*body);
        }
        FeatureKind::SheetFlat { base, body } => {
            let sheet_body = doc
                .feature(*base)?
                .created_bodies()
                .first()
                .copied()
                .ok_or(DocError::BrokenReference {
                    from: id,
                    detail: format!("{base} is not a sheet base"),
                })?;
            let mut model = sheet_of(st, id, *base, sheet_body)?;
            let built = crate::sheet::build(&model)?;
            let flat = crate::sheet::flat_body(&built, &model.rule, id)?;
            r.notes = crate::sheet::notes(&built, &model.rule);
            model.flat = Some(id);
            st.sheets.insert(sheet_body, model);
            create_body(st, id, *body, BodyGeometry::Brep(flat), BodySource::Modeled)?;
            r.body = Some(*body);
        }
        // Surfaces (logic in `crate::surface`).
        FeatureKind::ImportSurfaces {
            blob,
            unit_override_to_mm,
            body,
        } => {
            let b = doc.blob(*blob)?;
            let (geom, name, notes) =
                crate::surface::import(id, &b.bytes()?, *unit_override_to_mm)?;
            r.notes = notes;
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(geom),
                BodySource::ImportedStep {
                    file_name: b.file_name.clone(),
                    solid_name: name,
                },
            )?;
            r.body = Some(*body);
        }
        FeatureKind::SurfaceStitch {
            source,
            tolerance,
            body,
        }
        | FeatureKind::SurfaceFill {
            source,
            tolerance,
            body,
        } => {
            let src = surface_source(st, id, *source)?;
            let (geom, notes) = if matches!(f.kind, FeatureKind::SurfaceStitch { .. }) {
                crate::surface::stitch(id, &src, *tolerance)?
            } else {
                crate::surface::fill(id, &src, *tolerance)?
            };
            r.notes = notes;
            create_body(st, id, *body, BodyGeometry::Brep(geom), BodySource::Modeled)?;
            r.body = Some(*body);
        }
        FeatureKind::MeshEdit {
            source,
            op,
            body,
            second_body,
        } => {
            let bs = st
                .bodies
                .get(source)
                .ok_or_else(|| DocError::BrokenReference {
                    from: id,
                    detail: format!("mesh edit source {source} is not in the model"),
                })?;
            let BodyGeometry::Mesh(m) = &bs.geometry else {
                return Err(DocError::ProductLimit(format!(
                    "{id}: {source} is an exact (B-rep) body; mesh edits apply to mesh \
                     bodies (STL/3MF imports)"
                )));
            };
            let (meshes, notes) = crate::mesh_edit::edited(m, op)?;
            if meshes.len() != 1 + usize::from(second_body.is_some()) {
                return Err(DocError::Validation(format!(
                    "{id}: the mesh edit made {} meshes for {} bodies",
                    meshes.len(),
                    1 + usize::from(second_body.is_some())
                )));
            }
            r.notes = notes;
            for (mesh, b) in meshes
                .into_iter()
                .zip(std::iter::once(*body).chain(*second_body))
            {
                create_body(
                    st,
                    id,
                    b,
                    BodyGeometry::Mesh(Arc::new(mesh)),
                    BodySource::EditedMesh {
                        source: *source,
                        operation: op.name().into(),
                    },
                )?;
            }
            r.body = Some(*body);
        }
        FeatureKind::MeshRepair {
            source,
            options,
            body,
        } => {
            let bs = st
                .bodies
                .get(source)
                .ok_or_else(|| DocError::BrokenReference {
                    from: id,
                    detail: format!("mesh repair source {source} is not in the model"),
                })?;
            let BodyGeometry::Mesh(m) = &bs.geometry else {
                return Err(DocError::ProductLimit(format!(
                    "{id}: {source} is an exact (B-rep) body; mesh repair applies to \
                     mesh bodies (STL/3MF imports)"
                )));
            };
            let (mesh, notes) = import::repaired(m, options)?;
            r.notes = notes;
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Mesh(Arc::new(mesh)),
                BodySource::RepairedMesh { source: *source },
            )?;
            r.body = Some(*body);
        }
        FeatureKind::SurfacePatch { sketch, body }
        | FeatureKind::SurfaceExtrude { sketch, body, .. } => {
            let so = st.sketches.get(sketch).ok_or(not_a_sketch(id, *sketch))?;
            let geom = crate::surface::from_sketch(
                id,
                &f.kind,
                so.profile.as_ref().ok(),
                &so.solved,
                &so.frame,
            )?;
            r.frame = Some(so.frame);
            create_body(st, id, *body, BodyGeometry::Brep(geom), BodySource::Modeled)?;
            r.body = Some(*body);
        }
        FeatureKind::Thicken {
            source,
            distance,
            reverse,
            body,
        } => {
            let src = surface_source(st, id, *source)?;
            let (geom, notes) = crate::surface::thicken(id, &src, *distance, *reverse)?;
            r.notes = notes;
            create_body(st, id, *body, BodyGeometry::Brep(geom), BodySource::Modeled)?;
            r.body = Some(*body);
        }
        FeatureKind::AlignBody { from, to, offset } => {
            let a = resolve_face(id, from, st)?;
            let b = resolve_face(id, to, st)?;
            if a.body == b.body {
                return Err(DocError::Validation(format!(
                    "{id}: align needs faces of two different bodies"
                )));
            }
            if !offset.is_finite() {
                return Err(DocError::Validation(format!("{id}.offset must be finite")));
            }
            // Rotate n_a onto −n_b (smallest rotation), then translate a's
            // origin onto b's plane (+offset along n_b).
            let (na, nb) = (a.frame.z, b.frame.z);
            let want = scale(nb, -1.0);
            let axis = cross(na, want);
            let s = dot(axis, axis).sqrt();
            let c = dot(na, want).clamp(-1.0, 1.0);
            let (ax, ang) = if s > 1e-12 {
                (scale(axis, 1.0 / s), s.atan2(c))
            } else if c > 0.0 {
                ([0.0, 0.0, 1.0], 0.0)
            } else {
                (a.frame.x, std::f64::consts::PI)
            };
            let rot = k::Rigid::new([0.0; 3], a.frame.origin, ax, ang).map_err(kernel(id))?;
            let pa = rot.point(a.frame.origin);
            let h = dot(sub(pa, b.frame.origin), nb) - offset;
            let t = k::Rigid::new(scale(nb, -h), [0.0; 3], [0.0, 0.0, 1.0], 0.0)
                .map_err(kernel(id))?
                .after(&rot);
            let body = brep_target(st, id, a.body)?;
            let moved = k::transform_body(&body, &t).map_err(kernel(id))?;
            replace_body(st, id, a.body, moved);
            st.moves.push((a.body, i, t));
            r.body = Some(a.body);
        }
        FeatureKind::ScaleBody {
            body,
            center,
            factor,
        } => {
            let t = k::Rigid::scaling(*center, *factor)
                .map_err(|e| DocError::Validation(format!("{id}: {e}")))?;
            let b = brep_target(st, id, *body)?;
            let scaled = k::transform_body(&b, &t).map_err(kernel(id))?;
            // Keep every dimension above the modelling
            // tolerances.
            let bb = k::bounds(&scaled).map_err(kernel(id))?;
            let small = (0..3)
                .map(|i| bb.max[i] - bb.min[i])
                .fold(f64::INFINITY, f64::min);
            if small < 10.0 * k::tol::MIN_LENGTH {
                return Err(DocError::Validation(format!(
                    "{id}: scaled by {factor} the body would be {small:.6} mm thin, below the \
                     modelling tolerances"
                )));
            }
            replace_body(st, id, *body, scaled);
            st.moves.push((*body, i, t));
            r.body = Some(*body);
        }
        FeatureKind::ShellFace { face, thickness } => {
            let rf = resolve_face(id, face, st)?;
            r.frame = Some(rf.frame);
            let t = brep_target(st, id, rf.body)?;
            let out = shell_from_face(id, &t, &rf.frame, *thickness)?;
            replace_body(st, id, rf.body, out);
            r.body = Some(rf.body);
        }
        FeatureKind::SplitBody {
            body,
            plane,
            new_body,
            pieces,
        } => {
            let (pf, rf) = plane_frame(id, plane, st)?;
            r.frame = Some(pf);
            r.resolved_face = rf;
            let t = brep_target(st, id, *body)?;
            let (behind, front) = split_by_plane(&t, &pf, id, true)?;
            let mut behind = k::split_pieces(&behind).map_err(kernel(id))?.into_iter();
            let mut front = k::split_pieces(&front).map_err(kernel(id))?.into_iter();
            let (Some(b0), Some(f0)) = (behind.next(), front.next()) else {
                return Err(DocError::Kernel {
                    feature: id,
                    detail: "split: an empty side".into(),
                });
            };
            // Positional order: the first piece of each side, then the other
            // pieces behind-side first (used only for ids without a seed).
            let parts: Vec<Body> = [b0, f0].into_iter().chain(behind).chain(front).collect();
            let assigned = assign_pieces(
                id,
                parts,
                &[*body, *new_body],
                pieces,
                &f.piece_seeds,
                &mut r.piece_seeds,
            )?;
            for (b, piece) in assigned {
                if b == *body {
                    replace_body(st, id, b, piece);
                } else {
                    create_body(st, id, b, BodyGeometry::Brep(piece), BodySource::Modeled)?;
                }
            }
            r.body = Some(*new_body);
        }
        FeatureKind::Shell {
            extrude,
            open,
            thickness,
        } => {
            if st.moved_since(st.body_of_extrude(*extrude), *extrude) {
                return Err(DocError::ProductLimit(format!(
                    "{id}: shell: the body of {extrude} was moved after it; the scoped \
                     shell needs it in place (shell before the move)"
                )));
            }
            let (body, tool) = shell_tool(doc, st, id, *extrude, *open, *thickness)?;
            let target = brep_target(st, id, body)?;
            r.frame = Some(tool.start);
            let out = apply_tool(&target, &tool, &tool.start, Some("shell"), id, None)?.0;
            replace_body(st, id, body, out);
            r.body = Some(body);
        }
        FeatureKind::Thread {
            plane,
            center,
            diameter,
            pitch,
            length,
            left_hand,
            chord_tol,
            nut,
            body,
        } => {
            let (frame, rf) = plane_frame(id, plane, st)?;
            r.resolved_face = rf;
            let frame = Frame {
                origin: frame.to_world(*center),
                ..frame
            };
            r.frame = Some(frame);
            let spec = k::ThreadSpec {
                major_diameter: *diameter,
                pitch: *pitch,
                length: *length,
                left_hand: *left_hand,
                chord_tol: *chord_tol,
            };
            let built = match nut {
                None => k::threaded_rod(&frame, &spec),
                Some(n) => k::threaded_nut(&frame, &spec, n.width, n.clearance),
            };
            let (solid, report) = built.map_err(|e| match e {
                k::KernelError::InvalidInput(m) => DocError::ProductLimit(format!("{id}: {m}")),
                other => DocError::Kernel {
                    feature: id,
                    detail: other.to_string(),
                },
            })?;
            r.notes = vec![format!(
                "faceted thread: {} segments per turn, {} planar faces, largest facet \
                 deviation {:.4} mm (tolerance {} mm); not an exact helicoid",
                report.segments_per_turn, report.faces, report.max_facet_deviation, chord_tol
            )];
            create_body(
                st,
                id,
                *body,
                BodyGeometry::Brep(solid),
                BodySource::Modeled,
            )?;
            r.body = Some(*body);
        }
        FeatureKind::Countersink {
            hole,
            diameter,
            angle,
        } => {
            let target = doc
                .feature(*hole)
                .ok()
                .and_then(|f| match f.kind {
                    FeatureKind::Hole {
                        counterbore: None,
                        target,
                        ..
                    } => Some(Ok(target)),
                    FeatureKind::Hole { .. } => Some(Err(DocError::ProductLimit(format!(
                        "{id}: a countersink on a counterbored hole is not supported"
                    )))),
                    _ => None,
                })
                .ok_or_else(|| {
                    DocError::Validation(format!("{id}: {hole} is not a Hole feature"))
                })??;
            // The hole's tool is where the hole was made;
            // after a move/scale of its body that is not where it is now.
            if st.moved_since(Some(target), *hole) {
                return Err(DocError::ProductLimit(format!(
                    "{id}: the body of {hole} was moved or scaled after it; add the \
                     countersink before the move"
                )));
            }
            let tool = st.holes.get(hole).and_then(|t| t.last()).ok_or_else(|| {
                DocError::BrokenReference {
                    from: id,
                    detail: format!("{hole} is not an evaluated hole"),
                }
            })?;
            let SketchProfile::Circle { center, radius } = tool.profile else {
                return Err(DocError::Validation(format!("{id}: hole profile")));
            };
            let frame = tool.start;
            let body = brep_target(st, id, target)?;
            let out = k::countersink(
                &body,
                frame.to_world(center),
                frame.z,
                2.0 * radius,
                *diameter,
                *angle,
            )
            .map_err(|e| match e {
                k::KernelError::InvalidInput(m) => DocError::ProductLimit(format!("{id}: {m}")),
                other => DocError::Kernel {
                    feature: id,
                    detail: other.to_string(),
                },
            })?;
            replace_body(st, id, target, out);
            r.frame = Some(frame);
            r.body = Some(target);
        }
        FeatureKind::Combine {
            target,
            tools,
            op,
            keep_tools,
        } => {
            let a = brep_target(st, id, *target)?;
            let bs = tools
                .iter()
                .map(|t| brep_target(st, id, *t))
                .collect::<Result<Vec<_>>>()?;
            let (out, notes) = crate::combine::evaluate(id, *target, tools, &a, &bs, *op)?;
            replace_body(st, id, *target, out);
            if !keep_tools {
                for t in tools {
                    st.bodies.remove(t);
                }
            }
            r.notes = notes;
            r.body = Some(*target);
        }
        FeatureKind::DirectEdit { face, op } => {
            let bid = doc.face_body(face).ok_or_else(|| {
                DocError::Validation(format!("{id}: a direct edit needs an imported face"))
            })?;
            if let FaceRole::Imported { body, face: index } = face.role
                && let Some(why) = st.import_moved.get(&(face.feature, body, index))
            {
                return Err(DocError::BrokenReference {
                    from: id,
                    detail: why.clone(),
                });
            }
            let target = brep_target(st, id, bid)?;
            let last = st.bodies.get(&bid).map_or(id, |b| b.last_feature);
            let pivot = match face.role {
                FaceRole::Imported { face: index, .. } => st
                    .imported_faces
                    .get(&(face.feature, bid))
                    .and_then(|t| t.iter().find(|f| f.index == index))
                    .map(|f| f.frame),
                _ => None,
            };
            let out =
                crate::direct::apply(id, face, *op, &target, last, st.face_ids.get(&bid), pivot)?;
            replace_body(st, id, bid, out.body);
            st.face_ids.insert(bid, out.ids);
            if let (FaceRole::Imported { face: index, .. }, Some(t)) =
                (face.role, st.imported_faces.get_mut(&(face.feature, bid)))
            {
                crate::direct::update_planar_table(Arc::make_mut(t), index, *op, &out.removed);
            }
            r.body = Some(bid);
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
                    st,
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
                st,
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
    Ok(())
}

/// Every semantic planar face (extrusion caps and line sides, pattern/mirror
/// occurrence faces, chamfer faces, imported planar faces) that resolves
/// uniquely on the final geometry, by the same rule as [`resolve_face`].
fn face_candidates(st: &Snapshot, order: &[FeatureId]) -> Vec<FaceCandidate> {
    let mut out = Vec::new();
    let mut push = |reference: FaceRef| {
        if let Ok(r) = resolve_face(reference.feature, &reference, st)
            && r.matching_brep_faces == 1
        {
            out.push(FaceCandidate {
                reference,
                frame: r.frame,
                body: r.body,
                sample: r.sample,
            });
        }
    };
    let side_roles = |e: &ExtrudeOut| -> Vec<u32> {
        e.entities
            .iter()
            .chain(e.holes.iter().flat_map(|(_, ents)| ents.iter()))
            .map(|id| id.0)
            .collect()
    };
    for fid in order {
        if let Some(e) = st.extrudes.get(fid) {
            let roles = [FaceRole::ExtrudeEndCap, FaceRole::ExtrudeStartCap]
                .into_iter()
                .chain(
                    side_roles(e)
                        .into_iter()
                        .map(|entity| FaceRole::ExtrudeSide { entity }),
                );
            for role in roles {
                push(FaceRef {
                    feature: *fid,
                    role,
                });
            }
        }
        if let Some(copies) = st.copies.get(fid) {
            for (instance, tool, e) in copies {
                let roles = [CopiedRole::EndCap, CopiedRole::StartCap]
                    .into_iter()
                    .chain(
                        side_roles(e)
                            .into_iter()
                            .map(|entity| CopiedRole::Side { entity }),
                    );
                for of in roles {
                    push(FaceRef {
                        feature: *fid,
                        role: FaceRole::Copy {
                            instance: *instance,
                            tool: *tool,
                            of,
                        },
                    });
                }
            }
        }
        if st.chamfers.contains_key(fid) {
            push(FaceRef {
                feature: *fid,
                role: FaceRole::Chamfer,
            });
        }
        let mut imported: Vec<_> = st
            .imported_faces
            .iter()
            .filter(|((f, _), _)| f == fid)
            .collect();
        imported.sort_by_key(|((_, b), _)| *b);
        for ((_, body), faces) in imported {
            for f in faces.iter() {
                push(FaceRef {
                    feature: *fid,
                    role: FaceRole::Imported {
                        body: *body,
                        face: f.index,
                    },
                });
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
            check_mesh: None,
        },
    );
    st.created.push(body);
    Ok(())
}

fn replace_body(st: &mut Snapshot, id: FeatureId, body: BodyId, b: Body) {
    replace_body_checked(st, id, body, b, None);
}

/// [`replace_body`] keeping the check tessellation of the new geometry.
fn replace_body_checked(
    st: &mut Snapshot,
    id: FeatureId,
    body: BodyId,
    b: Body,
    mesh: Option<k::CheckMesh>,
) {
    if let Some(s) = st.bodies.get_mut(&body) {
        s.geometry = BodyGeometry::Brep(b);
        s.last_feature = id;
        s.check_mesh = mesh;
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
    pick: Option<&rcad_sketch::RegionRef>,
    opts: &ExtrudeOptions,
    (piece_ids, seeds, seeds_out): Pieces<'_>,
) -> Result<ExtrudeOut> {
    let s = st
        .sketches
        .get(&sketch)
        .ok_or(not_a_sketch(id, sketch))?
        .clone();
    // Thin extrusion of an open line chain (ribs/webs): the band of
    // width `thin` centred on the chain, mitred at its corners, square ends.
    let open_band = match (opts.thin, pick, &s.profile) {
        (Some(t), None, Err(_)) => match crate::sheet::line_chain(&s.solved) {
            Ok(chain) => Some(open_band(id, &chain, t)?),
            Err(_) => None,
        },
        _ => None,
    };
    let region = match &open_band {
        Some(band) => rcad_sketch::Region {
            outer: band.clone(),
            outer_entities: Vec::new(),
            holes: Vec::new(),
        },
        None => s.region(id, sketch, pick)?,
    };
    let (profile, entities) = (region.outer, region.outer_entities);
    let mut holes = region.holes;
    let mut extra = Vec::new();
    for r in &opts.regions {
        let x = s.region(id, sketch, Some(r))?;
        extra.push((x.outer, x.outer_entities, x.holes));
    }
    let frame = s.frame;
    let limit = |m: String| DocError::ProductLimit(format!("{id}: {m}"));
    let invalid = |m: String| DocError::Validation(format!("{id}: {m}"));
    // Option combinations (ExtrudeOptions): one way to give the far end.
    if opts.symmetric && (opts.distance2 != 0.0 || opts.to.is_some()) {
        return Err(invalid(
            "symmetric extrusions have one length; clear 'two sides' / 'to plane'".into(),
        ));
    }
    if opts.to.is_some() && opts.distance2 != 0.0 {
        return Err(invalid(
            "'to plane' and 'two sides' cannot be combined".into(),
        ));
    }
    if extent == Extent::ThroughAll && !opts.is_plain() {
        return Err(invalid(
            "'through all' takes no symmetric/two-sides/offset/to-plane/thin/taper option".into(),
        ));
    }
    for (name, v) in [
        ("distance2", opts.distance2),
        ("start_offset", opts.start_offset),
        ("taper", opts.taper),
    ] {
        if !v.is_finite() {
            return Err(invalid(format!("{name} must be finite, got {v}")));
        }
    }
    if opts.distance2 < 0.0 {
        return Err(invalid(format!(
            "distance2: the second side's distance must be > 0 mm, got {}",
            opts.distance2
        )));
    }
    // Thin: the band between the outer loop and its inward offset.
    if let Some(t) = opts.thin.filter(|_| open_band.is_none()) {
        if !t.is_finite() || t <= 0.0 {
            return Err(invalid(format!(
                "thin: wall thickness must be > 0 mm, got {t}"
            )));
        }
        if !holes.is_empty() {
            return Err(limit(
                "a thin extrusion needs a single closed loop (the region has inner loops)".into(),
            ));
        }
        let inner = rcad_sketch::offset_profile(&profile, -t).map_err(|x| {
            limit(format!(
                "thin: wall thickness {t} mm does not fit the profile (inward offset: {x})"
            ))
        })?;
        holes.push((inner, Vec::new()));
        for (p, _, h) in extra.iter_mut() {
            if !h.is_empty() {
                return Err(limit(
                    "a thin extrusion needs single closed loops (a region has inner loops)".into(),
                ));
            }
            let inner = rcad_sketch::offset_profile(p, -t).map_err(|x| {
                limit(format!(
                    "thin: wall thickness {t} mm does not fit a region (inward offset: {x})"
                ))
            })?;
            h.push((inner, Vec::new()));
        }
    }
    if !extra.is_empty() && !matches!(operation, Operation::Join { .. } | Operation::Cut { .. }) {
        return Err(limit(
            "several regions are extruded together only by Join or Cut (New Body would make \
             separate solids; Intersect keeps one region)"
                .into(),
        ));
    }
    let khole: Vec<k::Profile> = holes.iter().map(|(p, _)| to_kernel(p)).collect();
    let kp = to_kernel(&profile);
    if opts.to.is_none() && extent == Extent::Distance && (!distance.is_finite() || distance <= 0.0)
    {
        return Err(DocError::Validation(format!(
            "{id}.distance: extrusion distance must be > 0 mm, got {distance}"
        )));
    }
    let target = match operation {
        Operation::NewBody { .. } => None,
        Operation::Join { target }
        | Operation::Cut { target }
        | Operation::Intersect { target } => Some((target, brep_target(st, id, target)?)),
    };
    // Sketch on a face of the target: +1/-1 when the face's outward normal is
    // along/against the sketch normal (material lies on the other side).
    let side = match &target {
        Some((_, t)) => k::coplanar_face_side(t, &frame).map_err(kernel(id))?,
        None => None,
    };
    // Direction semantics (ADR 0003): along the sketch normal, except when the
    // sketch plane holds a face of the target. A Cut (or Intersect) then goes
    // into the material by default (`reverse` flips it). A Join goes away from
    // the material and ignores `reverse`: the other way only adds nothing, and
    // documents saved before this rule set `reverse` exactly when it pointed
    // away.
    let base = match (operation, side) {
        (Operation::Cut { .. } | Operation::Intersect { .. }, Some(side)) => -side,
        (Operation::Join { .. }, Some(side)) => side,
        _ => 1.0,
    };
    let mut dir = match (operation, side) {
        (Operation::Join { .. }, Some(side)) => side,
        _ if reverse => -base,
        _ => base,
    };
    // Span along the direction, u ∈ [u0, u1] (ExtrudeOptions).
    let s0 = opts.start_offset;
    let mut distance = distance;
    let (u0, u1) = if let Some(to) = &opts.to {
        let (tf, _) = plane_frame(id, to, st)?;
        if dot(tf.z, frame.z).abs() < 1.0 - 1e-9 {
            return Err(limit(
                "'to plane' needs a plane parallel to the sketch plane (curved or oblique \
                 targets are not supported)"
                    .into(),
            ));
        }
        let h = dot(sub(tf.origin, frame.origin), frame.z);
        if h.abs() <= k::tol::MIN_LENGTH {
            return Err(invalid("the 'to' plane is the sketch plane itself".into()));
        }
        dir = h.signum();
        // `start_offset` is measured along the direction.
        if s0 >= h.abs() - k::tol::MIN_LENGTH {
            return Err(invalid(format!(
                "the start offset {s0} mm reaches the 'to' plane ({:.6} mm away)",
                h.abs()
            )));
        }
        distance = h.abs() - s0;
        (s0, h.abs())
    } else if opts.symmetric {
        (s0 - distance / 2.0, s0 + distance / 2.0)
    } else {
        (s0 - opts.distance2, s0 + distance)
    };
    if extent == Extent::Distance && u1 - u0 <= k::tol::MIN_LENGTH {
        return Err(invalid("the extrusion has no length".into()));
    }
    // Tool = prism of the profile over [a, b] along the sketch normal.
    let (mut a, mut b) = if dir > 0.0 { (u0, u1) } else { (-u1, -u0) };
    let mut out = ExtrudeOut {
        start: frame,
        distance,
        dir,
        extent,
        profile,
        entities,
        holes,
        span: (0.0, 0.0),
        nominal: (a, b),
        side,
        removes: false,
        joined: false,
        intersect: false,
        plain: opts.is_plain(),
        taper: opts.taper,
        pieces: false,
        multi: false,
        body: BodyId(0),
    };
    if opts.taper != 0.0 {
        if matches!(operation, Operation::Intersect { .. }) || !extra.is_empty() {
            return Err(limit(
                "a tapered Intersect or multi-region extrusion is not supported (New Body, Join \
                 and Cut of one region can be tapered)"
                    .into(),
            ));
        }
        if !khole.is_empty() {
            return Err(limit(
                "a tapered extrusion needs a region without inner loops".into(),
            ));
        }
        // The taper is anchored at the near end of the span;
        // only a span starting on the sketch plane keeps the sketch profile
        // exact there.
        if opts.symmetric || opts.distance2 != 0.0 || opts.start_offset != 0.0 {
            return Err(limit(
                "a taper with symmetric, two-sided or offset-start extents is not supported \
                 (the sketch profile would not be the section at the sketch plane)"
                    .into(),
            ));
        }
    }
    if matches!(operation, Operation::Intersect { .. }) && extent == Extent::ThroughAll {
        return Err(invalid(
            "'through all' is not available for Intersect".into(),
        ));
    }
    match (operation, target) {
        (Operation::NewBody { body }, _) => {
            if extent == Extent::ThroughAll {
                return Err(DocError::Validation(format!(
                    "{id}: 'through all' needs a Cut target body"
                )));
            }
            let solid = if opts.taper != 0.0 {
                tapered_solid(id, &out.profile, &frame, (a, b), dir, opts.taper)?
            } else {
                k::extrude_region(&kp, &khole, &frame.offset(a), b - a).map_err(kernel(id))?
            };
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
            let (mut joined, used) = if opts.taper != 0.0 {
                let r = tapered_apply(id, &t, &out, (a, b), dir, opts.taper, k::PrismOp::Join)?;
                (r, (a, b))
            } else {
                apply_tool(&t, &out, &frame, None, id, None)?
            };
            (a, b) = used;
            for (i, (p, ents, h)) in extra.iter().enumerate() {
                let o = ExtrudeOut {
                    profile: p.clone(),
                    entities: ents.clone(),
                    holes: h.clone(),
                    ..out.clone()
                };
                let w = format!("region {}", i + 2);
                joined = apply_tool(&joined, &o, &frame, Some(&w), id, None)?.0;
            }
            replace_body(st, id, target, joined);
            out.body = target;
            out.multi = !extra.is_empty();
        }
        (Operation::Cut { .. }, Some((target, t))) => {
            out.removes = true;
            if extent == Extent::Distance
                && opts.is_plain()
                && let Some(side) = side
                && dir * side > 0.0
            {
                return Err(DocError::Validation(format!(
                    "{id}: the cut points away from the material of {target}; clear 'reverse'"
                )));
            }
            let t_apply = std::time::Instant::now();
            // One region, straight walls: a cut that separates the target
            // into pieces makes one body per piece (as Fusion does).
            let split = opts.taper == 0.0 && extra.is_empty();
            let (mut cut, used) = if opts.taper != 0.0 {
                let r = tapered_apply(id, &t, &out, (a, b), dir, opts.taper, k::PrismOp::Cut)?;
                (r, (a, b))
            } else {
                let tool = ExtrudeOut {
                    pieces: split,
                    ..out.clone()
                };
                apply_tool(&t, &tool, &frame, None, id, None)?
            };
            trace_regen(id, "apply_tool", t_apply);
            (a, b) = used;
            for (i, (p, ents, h)) in extra.iter().enumerate() {
                let o = ExtrudeOut {
                    profile: p.clone(),
                    entities: ents.clone(),
                    holes: h.clone(),
                    ..out.clone()
                };
                let w = format!("region {}", i + 2);
                cut = apply_tool(&cut, &o, &frame, Some(&w), id, None)?.0;
            }
            let parts = k::split_pieces(&cut).map_err(kernel(id))?;
            if parts.is_empty() {
                return Err(DocError::Kernel {
                    feature: id,
                    detail: "cut: empty result".into(),
                });
            }
            let mut assigned =
                assign_pieces(id, parts, &[target], piece_ids, seeds, seeds_out)?.into_iter();
            if let Some((_, first)) = assigned.next() {
                replace_body(st, id, target, first);
            }
            for (body, piece) in assigned {
                create_body(st, id, body, BodyGeometry::Brep(piece), BodySource::Modeled)?;
            }
            out.body = target;
            out.multi = !extra.is_empty();
        }
        (Operation::Intersect { .. }, Some((target, t))) => {
            out.intersect = true;
            let kept = intersect_tool(&t, &out, &frame, id)?;
            replace_body(st, id, target, kept);
            out.body = target;
        }
        (Operation::Join { .. } | Operation::Cut { .. } | Operation::Intersect { .. }, None) => {
            return Err(DocError::Validation(format!("{id}: missing target body")));
        }
    }
    out.span = (a, b);
    Ok(out)
}

/// Join/Cut with a drafted tool: the exact frustum of
/// the profile over `(a, b)` along the sketch normal, exact at the near end
/// (along `dir`) and offset inward by `w·tan(taper)` at distance `w` from it,
/// applied with Truck's boolean and checked against that nominal tool
/// (`rcad_kernel::tapered_boolean`; the slab construction only handles walls
/// parallel to the axis). When the near end lies on a face of the target the
/// tool starts `min(0.5 mm, length / 2)` behind it (outside the material for a
/// Cut, inside the support for a Join), continuing the same draft, so no
/// boolean faces are coplanar; the check still uses the nominal span.
fn tapered_apply(
    id: FeatureId,
    t: &Body,
    out: &ExtrudeOut,
    (a, b): (f64, f64),
    dir: f64,
    taper: f64,
    op: k::PrismOp,
) -> Result<Body> {
    let limit = |m: String| DocError::ProductLimit(format!("{id}: taper: {m}"));
    let kp = to_kernel(&out.profile);
    let len = b - a;
    let frame = out.start;
    // Law frame: at the near end, z along the direction (right-handed: a
    // reversed direction also flips y, and the profile is mirrored in y).
    let (lf, lp) = if dir > 0.0 {
        (frame.offset(a), kp)
    } else {
        let f = frame.offset(b);
        let flipped = Frame {
            origin: f.origin,
            x: f.x,
            y: [-f.y[0], -f.y[1], -f.y[2]],
            z: [-f.z[0], -f.z[1], -f.z[2]],
        };
        let mirrored = match kp {
            k::Profile::Polygon(v) => {
                k::Profile::Polygon(v.iter().rev().map(|p| [p[0], -p[1]]).collect())
            }
            k::Profile::Circle { center, radius } => k::Profile::Circle {
                center: [center[0], -center[1]],
                radius,
            },
            k::Profile::Loop(_) => {
                return Err(limit(
                    "rounded (line/arc) profiles cannot be tapered exactly yet; polygons and \
                     circles only"
                        .into(),
                ));
            }
        };
        (flipped, mirrored)
    };
    let near = if dir > 0.0 { a } else { b };
    let on_face = out.side.is_some() && near.abs() <= k::tol::FACE_MATCH;
    let back = if on_face {
        JOIN_OVERLAP.min(len / 2.0)
    } else {
        0.0
    };
    k::tapered_boolean(t, &lp, &lf, len, (-back, len), taper, op)
        .map(|(body, _)| body)
        .map_err(|e| match e {
            k::KernelError::InvalidInput(m) => limit(m),
            other => DocError::Kernel {
                feature: id,
                detail: format!(
                    "tapered {}: {other}",
                    match op {
                        k::PrismOp::Join => "join",
                        k::PrismOp::Cut => "cut",
                    }
                ),
            },
        })
}

/// Hollow `t` under the planar face in `frame` (outward normal z), leaving
/// it open: its outer boundary offset inward by `th`, cut to one `th` above
/// the opposite cap (see `FeatureKind::ShellFace`).
fn shell_from_face(id: FeatureId, t: &Body, frame: &Frame, th: f64) -> Result<Body> {
    let limit = |m: String| DocError::ProductLimit(format!("{id}: shell: {m}"));
    if !(th.is_finite() && th > 0.0) {
        return Err(DocError::Validation(format!(
            "{id}.thickness: shell wall thickness must be > 0 mm, got {th}"
        )));
    }
    let loops = k::face_boundary_curves(t, frame).map_err(|e| limit(e.to_string()))?;
    if loops.len() != 1 {
        return Err(limit(format!(
            "the open face has {} boundary loops; a shell needs a face without holes",
            loops.len()
        )));
    }
    let outer = curves_to_profile(&loops[0]).map_err(limit)?;
    let inner = rcad_sketch::offset_profile(&outer, -th)
        .map_err(|x| limit(format!("thickness {th} mm does not fit the face ({x})")))?;
    // The opposite cap: the lowest planar face parallel to the open face.
    let floor = k::face_planes(t)
        .iter()
        .filter(|p| (dot(p.normal, frame.z).abs() - 1.0).abs() < 1e-9)
        .map(|p| dot(sub(p.point, frame.origin), frame.z))
        .fold(f64::INFINITY, f64::min);
    if !floor.is_finite() || floor >= -th {
        return Err(limit(format!(
            "the body is {:.4} mm deep under the face; it must be deeper than the thickness",
            -floor
        )));
    }
    let kp = to_kernel(&inner);
    if let Some(why) = k::prismatic::not_applicable(t, &kp, frame) {
        return Err(limit(format!(
            "the body is not prismatic along the face normal ({why}); walls would not be \
             {th} mm thick"
        )));
    }
    let depth = -floor - th;
    let piece = ExtrudeOut {
        start: *frame,
        distance: depth,
        dir: -1.0,
        extent: Extent::Distance,
        profile: inner,
        entities: vec![],
        holes: vec![],
        span: (-depth, 0.0),
        nominal: (-depth, 0.0),
        side: Some(1.0),
        removes: true,
        joined: false,
        intersect: false,
        plain: true,
        multi: false,
        taper: 0.0,
        pieces: false,
        body: BodyId(0),
    };
    Ok(apply_tool(t, &piece, frame, Some("shell"), id, None)?.0)
}

/// A closed loop of projected curves as a sketch profile.
fn curves_to_profile(l: &[k::Curve2]) -> std::result::Result<SketchProfile, String> {
    if let [k::Curve2::Circle { c, r }] = l {
        return Ok(SketchProfile::Circle {
            center: *c,
            radius: *r,
        });
    }
    let mut segs = Vec::new();
    let mut any_arc = false;
    for c in l {
        match *c {
            k::Curve2::Line { a, .. } => segs.push(rcad_sketch::LoopSegment::Line { start: a }),
            k::Curve2::Arc { c, a, ccw, .. } => {
                any_arc = true;
                segs.push(rcad_sketch::LoopSegment::Arc {
                    start: a,
                    center: c,
                    ccw,
                })
            }
            k::Curve2::Circle { .. } => return Err("a circle inside a loop".into()),
            k::Curve2::Ellipse { .. } => return Err("an ellipse in a face boundary".into()),
        }
    }
    let poly: Vec<[f64; 2]> = segs.iter().map(|s| s.start()).collect();
    let area: f64 = (0..poly.len())
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if !any_arc {
        let mut p = poly;
        if area < 0.0 {
            p.reverse();
        }
        return Ok(SketchProfile::Polygon(p));
    }
    // Arcs: orientation from a fine polyline.
    let prof = SketchProfile::Loop(segs.clone());
    let (pts, _) = k::profile_outline(&to_kernel(&prof));
    let a2: f64 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if a2 >= 0.0 {
        return Ok(prof);
    }
    let n = segs.len();
    let starts: Vec<[f64; 2]> = segs.iter().map(|s| s.start()).collect();
    let mut out = Vec::with_capacity(n);
    for i in (0..n).rev() {
        let to = starts[(i + 1) % n];
        out.push(match segs[i] {
            rcad_sketch::LoopSegment::Line { .. } => rcad_sketch::LoopSegment::Line { start: to },
            rcad_sketch::LoopSegment::Arc { center, ccw, .. } => rcad_sketch::LoopSegment::Arc {
                start: to,
                center,
                ccw: !ccw,
            },
            rcad_sketch::LoopSegment::Cubic { c1, c2, w, .. } => rcad_sketch::LoopSegment::Cubic {
                start: to,
                c1: c2,
                c2: c1,
                w: [w[1], w[0]],
            },
        });
    }
    Ok(SketchProfile::Loop(out))
}

/// Split `t` by the plane of `pf`: (the part behind the plane, the part in
/// front of it), each an exact cut by a half-space box through
/// [`apply_tool`] (result-checked; ADR 0005/0006).
/// The parts of `t` behind and in front of `pf`'s plane. With `pieces`, a
/// side that falls apart is returned as one body with a closed shell per
/// piece (see `rcad_kernel::split_pieces`); otherwise it is refused.
pub(crate) fn split_by_plane(
    t: &Body,
    pf: &Frame,
    id: FeatureId,
    pieces: bool,
) -> Result<(Body, Body)> {
    let bb = k::bounds(t).map_err(kernel(id))?;
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in 0..8 {
        let p = [
            if i & 1 == 0 { bb.min[0] } else { bb.max[0] },
            if i & 2 == 0 { bb.min[1] } else { bb.max[1] },
            if i & 4 == 0 { bb.min[2] } else { bb.max[2] },
        ];
        let d = sub(p, pf.origin);
        let q = [dot(d, pf.x), dot(d, pf.y), dot(d, pf.z)];
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let eps = k::tol::KERNEL_LINEAR;
    if lo[2] >= -eps || hi[2] <= eps {
        return Err(DocError::Validation(format!(
            "{id}: the split plane does not cross the body (it lies {} it)",
            if lo[2] >= -eps { "below" } else { "above" }
        )));
    }
    let m = k::through_all_margin(t).map_err(kernel(id))?;
    let (x0, x1, y0, y1) = (lo[0] - m, hi[0] + m, lo[1] - m, hi[1] + m);
    let boxp = SketchProfile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
    let piece = |span: (f64, f64)| ExtrudeOut {
        start: *pf,
        distance: span.1 - span.0,
        dir: 1.0,
        extent: Extent::Distance,
        profile: boxp.clone(),
        entities: vec![],
        holes: vec![],
        span,
        nominal: span,
        side: None,
        removes: true,
        joined: false,
        intersect: false,
        plain: true,
        multi: false,
        taper: 0.0,
        pieces,
        body: BodyId(0),
    };
    let cut = |span: (f64, f64), what: &str| {
        apply_tool(t, &piece(span), pf, Some(what), id, None)
            .map(|(b, _)| b)
            .map_err(|x| match x {
                DocError::Kernel { detail, .. } if detail.contains("disconnected") => {
                    DocError::ProductLimit(format!(
                        "{id}: split: the {what} falls apart into several solids ({detail}); \
                         split it further or choose another plane"
                    ))
                }
                other => other,
            })
    };
    // The same half-space box extruded along an in-plane axis instead
    // of the plane normal. When the body's curved faces (holes, bosses) are
    // parallel to that axis the exact slab construction applies where Truck's
    // boolean fails (e.g. a section plane crossing a hole off its axis).
    // `normal_side`: (u0, u1) along the plane normal.
    let in_plane = |normal_side: (f64, f64), what: &str| -> Result<Body> {
        let mut last: Option<DocError> = None;
        // (extrusion axis, in-profile axis, its index in lo/hi, extrusion index)
        for (ax, other, oi, ei) in [(pf.y, pf.x, 0usize, 1usize), (pf.x, pf.y, 1, 0)] {
            // Right-handed frame with z = ax and the plane normal in it.
            let (fx, fy) = if ei == 1 {
                (pf.z, other)
            } else {
                (other, pf.z)
            };
            let f2 = Frame {
                origin: pf.origin,
                x: fx,
                y: fy,
                z: ax,
            };
            let (n0, n1) = normal_side;
            let (o0, o1) = (lo[oi] - m, hi[oi] + m);
            let pts = if ei == 1 {
                // x = normal, y = other in-plane axis.
                vec![[n0, o0], [n1, o0], [n1, o1], [n0, o1]]
            } else {
                // x = other in-plane axis, y = normal.
                vec![[o0, n0], [o1, n0], [o1, n1], [o0, n1]]
            };
            let span = (lo[ei] - m, hi[ei] + m);
            let tool = ExtrudeOut {
                start: f2,
                distance: span.1 - span.0,
                profile: SketchProfile::Polygon(pts),
                span,
                nominal: span,
                ..piece(span)
            };
            match apply_tool(t, &tool, &f2, Some(what), id, None) {
                Ok((b, _)) => return Ok(b),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| DocError::Validation(format!("{id}: split failed"))))
    };
    let behind = cut((0.0, hi[2] + m), "part behind the plane")
        .or_else(|e| in_plane((0.0, hi[2] + m), "part behind the plane").map_err(|_| e))?;
    let front = cut((lo[2] - m, 0.0), "part in front of the plane")
        .or_else(|e| in_plane((lo[2] - m, 0.0), "part in front of the plane").map_err(|_| e))?;
    Ok((behind, front))
}

/// Band of width `t` centred on the open chain `pts`: offset lines ±t/2,
/// mitred at interior vertices (refused when a corner is sharper than a
/// miter of 4·t), square ends; counter-clockwise polygon.
#[allow(clippy::needless_range_loop)] // vertex i and its neighbours
fn open_band(id: FeatureId, pts: &[[f64; 2]], t: f64) -> Result<SketchProfile> {
    let bad = |m: String| DocError::Validation(format!("{id}: thin open profile: {m}"));
    if !(t.is_finite() && t > 0.0) {
        return Err(bad(format!("wall thickness must be > 0 mm, got {t}")));
    }
    let n = pts.len();
    let dir = |i: usize| {
        let d = [pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]];
        let l = d[0].hypot(d[1]);
        [d[0] / l, d[1] / l]
    };
    let side = |sgn: f64| -> Result<Vec<[f64; 2]>> {
        let h = sgn * t / 2.0;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let p = pts[i];
            let off = |d: [f64; 2]| [p[0] - d[1] * h, p[1] + d[0] * h];
            if i == 0 {
                out.push(off(dir(0)));
            } else if i == n - 1 {
                out.push(off(dir(n - 2)));
            } else {
                let (a, b) = (dir(i - 1), dir(i));
                let m = [-(a[1] + b[1]), a[0] + b[0]];
                let ml = m[0].hypot(m[1]);
                let cosh = (m[0] * -a[1] + m[1] * a[0]) / ml;
                if ml < 1e-9 || cosh.abs() < 0.125 {
                    return Err(bad(format!(
                        "the corner at vertex {i} is too sharp to miter"
                    )));
                }
                let k = h / cosh;
                out.push([p[0] + m[0] / ml * k, p[1] + m[1] / ml * k]);
            }
        }
        Ok(out)
    };
    let mut poly = side(1.0)?;
    let mut right = side(-1.0)?;
    right.reverse();
    poly.extend(right);
    let area: f64 = (0..poly.len())
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area < 0.0 {
        poly.reverse();
    }
    // A chain folding back over itself makes a crossing band.
    let m = poly.len();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    for i in 0..m {
        for j in i + 2..m {
            if i == 0 && j == m - 1 {
                continue;
            }
            let (a, b, c, d) = (poly[i], poly[(i + 1) % m], poly[j], poly[(j + 1) % m]);
            if cross(a, b, c) * cross(a, b, d) < 0.0 && cross(c, d, a) * cross(c, d, b) < 0.0 {
                return Err(bad("the band crosses itself (the chain folds back)".into()));
            }
        }
    }
    Ok(SketchProfile::Polygon(poly))
}

/// New-body tapered extrusion of `profile` over `(a, b)` along `frame.z`:
/// the far end (along `dir`) is the profile offset by `−(b − a)·tan(taper)`
/// (positive taper shrinks it), lofted exactly (ADR 0008: polygons have
/// parallel corresponding edges, circles give a cone frustum).
fn tapered_solid(
    id: FeatureId,
    profile: &SketchProfile,
    frame: &Frame,
    (a, b): (f64, f64),
    dir: f64,
    taper: f64,
) -> Result<Body> {
    let limit = |m: String| DocError::ProductLimit(format!("{id}: taper: {m}"));
    let delta = -(b - a) * taper.tan();
    let far = rcad_sketch::offset_profile(profile, delta).map_err(|x| {
        limit(format!(
            "the profile offset by {delta:.6} mm at the far end is not valid ({x}); reduce the \
             taper or the distance"
        ))
    })?;
    let (near_at, far_at) = if dir > 0.0 { (a, b) } else { (b, a) };
    k::loft(
        &to_kernel(profile),
        &frame.offset(near_at),
        &to_kernel(&far),
        &frame.offset(far_at),
    )
    .map_err(|e| {
        limit(format!(
            "{e} (tapered profiles are lofted: polygons and circles)"
        ))
    })
}

/// Intersect: keep the material of `t` inside `e`'s tool. Built as cuts by
/// the tool's complement within a box around `t` (each an exact slab cut
/// checked against its nominal tool, ADR 0005/0006): the box with the profile
/// as a hole over the tool span, every inner loop of the profile over the
/// span, and the whole box below / above the span.
fn intersect_tool(t: &Body, e: &ExtrudeOut, frame: &Frame, id: FeatureId) -> Result<Body> {
    let err = kernel(id);
    let bb = k::bounds(t).map_err(&err)?;
    // Box around the target in sketch-frame coordinates.
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for i in 0..8 {
        let p = [
            if i & 1 == 0 { bb.min[0] } else { bb.max[0] },
            if i & 2 == 0 { bb.min[1] } else { bb.max[1] },
            if i & 4 == 0 { bb.min[2] } else { bb.max[2] },
        ];
        let d = sub(p, frame.origin);
        let q = [dot(d, frame.x), dot(d, frame.y), dot(d, frame.z)];
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let m = k::through_all_margin(t).map_err(&err)?;
    let (x0, x1, y0, y1) = (lo[0] - m, hi[0] + m, lo[1] - m, hi[1] + m);
    let boxp = SketchProfile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
    let (a, b) = e.nominal;
    let piece = |profile: SketchProfile,
                 holes: Vec<(SketchProfile, Vec<EntityId>)>,
                 span: (f64, f64)| ExtrudeOut {
        start: *frame,
        distance: span.1 - span.0,
        dir: 1.0,
        extent: Extent::Distance,
        profile,
        entities: vec![],
        holes,
        span,
        nominal: span,
        side: None,
        removes: true,
        joined: false,
        intersect: false,
        plain: true,
        taper: 0.0,
        pieces: false,
        multi: false,
        body: e.body,
    };
    let mut pieces = vec![piece(
        boxp.clone(),
        vec![(e.profile.clone(), e.entities.clone())],
        (a, b),
    )];
    for (h, ents) in &e.holes {
        pieces.push(piece(h.clone(), vec![], (a, b)));
        let _ = ents;
    }
    if a > lo[2] + k::tol::KERNEL_LINEAR {
        pieces.push(piece(boxp.clone(), vec![], (lo[2] - m, a)));
    }
    if b < hi[2] - k::tol::KERNEL_LINEAR {
        pieces.push(piece(boxp, vec![], (b, hi[2] + m)));
    }
    let mut body = t.clone();
    for p in &pieces {
        if k::prismatic::not_applicable(&body, &to_kernel(&p.profile), frame).is_some()
            && !p.holes.is_empty()
        {
            let why = k::prismatic::not_applicable(&body, &to_kernel(&p.profile), frame)
                .unwrap_or_default();
            return Err(DocError::ProductLimit(format!(
                "{id}: intersect is built from exact slab cuts, which do not apply to this \
                 target ({why})"
            )));
        }
        let (next, _) =
            apply_tool(&body, p, frame, Some("intersect"), id, None).map_err(|x| match x {
                DocError::Kernel { detail, .. } if detail.contains("disconnected") => {
                    DocError::ProductLimit(format!(
                        "{id}: intersect leaves disconnected pieces of the target ({detail})"
                    ))
                }
                other => other,
            })?;
        body = next;
    }
    Ok(body)
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
    // (A probe cut is avoided: Truck can fail or panic on one for a slab
    // wholly in air beside a coplanar face, refusing valid cuts.)
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
    /// Inner loops of the tool region (empty for a single closed loop).
    holes: Vec<k::Profile>,
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
            holes: &a.holes,
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
/// one path for seeds and copies: the workarounds for Truck's
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
/// Opt-in timing of regeneration steps (`RCAD_TRACE_REGEN=1`, stderr).
fn trace_regen(id: FeatureId, what: &str, t: std::time::Instant) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("RCAD_TRACE_REGEN").is_some_and(|v| v == "1")) {
        eprintln!(
            "regen {id} {what} {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }
}

fn apply_tool(
    body: &Body,
    e: &ExtrudeOut,
    frame: &Frame,
    what: Option<&str>,
    id: FeatureId,
    defer: Option<&mut Vec<Applied>>,
) -> Result<(Body, (f64, f64))> {
    let kp = to_kernel(&e.profile);
    let khole: Vec<k::Profile> = e.holes.iter().map(|(p, _)| to_kernel(p)).collect();
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
            holes: khole.clone(),
            nominal,
            used,
            op,
        };
        match defer.as_deref_mut() {
            Some(v) => {
                v.push(a);
                Ok(())
            }
            None => {
                let t = std::time::Instant::now();
                let r = verify_applied(id, &prefix, body, result, std::slice::from_ref(&a));
                trace_regen(id, "verify_applied", t);
                r
            }
        }
    };
    if !khole.is_empty() {
        // A tool region with inner loops (Truck 0.6 booleans fail for those):
        // only the exact slab construction, over the nominal span (it needs no
        // overlap or margin), checked against the holed nominal tool.
        let (op, kop, span) = if e.removes {
            let span = if e.extent == Extent::ThroughAll {
                let margin = k::through_all_margin(body).map_err(err)?;
                let (lo, hi) = k::extent_along(body, frame).map_err(err)?;
                (lo - margin, hi + margin)
            } else {
                e.nominal
            };
            (k::prismatic::SlabOp::Cut, k::PrismOp::Cut, span)
        } else {
            (k::prismatic::SlabOp::Join, k::PrismOp::Join, e.nominal)
        };
        if let Some(why) = k::prismatic::not_applicable(body, &kp, frame) {
            // Not prismatic (a countersink cone, a revolved or filleted
            // body): Truck's boolean with the holed tool (our truck-shapeops
            // fork builds such tools), placed as for a single loop and checked
            // against the holed nominal tool.
            return holed_truck(body, e, frame, &kp, &khole, &mut check).map_err(|t| {
                DocError::ProductLimit(format!(
                    "{id}: {prefix}the sketch region has {} inner loop(s); the exact slab \
                         construction does not apply here ({why}) and the general boolean \
                         failed ({t}). Use New Body, or cut each loop separately",
                    khole.len()
                ))
            });
        }
        let region_boolean = if e.pieces && e.removes {
            k::prismatic::prism_region_boolean_pieces
        } else {
            k::prismatic::prism_region_boolean
        };
        let out = region_boolean(
            body,
            &kp,
            &khole,
            &frame.offset(span.0),
            span.1 - span.0,
            op,
        )
        .map_err(|x| DocError::Kernel {
            feature: id,
            detail: format!("{prefix}exact slab construction (region with inner loops): {x}"),
        })?;
        check(&out, kop, span, span).map_err(slab_checked)?;
        return Ok((out, span));
    }
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
        let t = std::time::Instant::now();
        let (start, len) = (frame.offset(span.0), span.1 - span.0);
        let truck = if e.pieces {
            k::cut_region_pieces(body, &kp, &[], &start, len)
        } else {
            k::cut_prism(body, &kp, &start, len)
        }
        .map_err(|x| match x {
            x if refused.is_some() => coplanar_cut_error(id, refused, x),
            x => err(x),
        });
        trace_regen(id, "cut_prism", t);
        let truck = truck.and_then(|out| check(&out, k::PrismOp::Cut, nominal, span).map(|()| out));
        let e_pieces = e.pieces;
        let out = match truck {
            Ok(out) => {
                let t = std::time::Instant::now();
                let r = exact_instead(
                    id, &prefix, body, out, &kp, frame, nominal, span, true, e.pieces,
                );
                trace_regen(id, "exact_instead", t);
                r
            }
            Err(e) => {
                let pieces = e_pieces;
                let out =
                    slab_fallback(body, &kp, frame, span, k::prismatic::SlabOp::Cut, e, pieces)?;
                check(&out, k::PrismOp::Cut, nominal, span).map_err(slab_checked)?;
                out
            }
        };
        return Ok((out, span));
    }
    let (a, b) = e.nominal;
    // The support face matters only when the tool starts on it (a
    // symmetric, two-sided or offset join crosses or leaves the face).
    let on_face = (e.dir > 0.0 && a == 0.0) || (e.dir < 0.0 && b == 0.0);
    let span = match e.side {
        Some(side) if on_face => {
            // A tool end lying on a face of the target would make coplanar
            // boolean faces (Truck 0.6 returns no result): move that end into
            // the material, which is only exact when the profile lies on the
            // face (material under the whole profile) and the overlap stays
            // inside material that no other surface crosses.
            if !k::profile_within_face(body, frame, &kp).map_err(err)? {
                let limit = |why: String| {
                    DocError::ProductLimit(format!(
                        "{id}: {prefix}a Join sketched on a face of the target must lie inside \
                         that face; the profile is not inside a face here (it crosses the face \
                         boundary or a hole). Use New Body, or sketch on another plane \
                         (exact slab construction: {why})"
                    ))
                };
                if let Some(why) = k::prismatic::not_applicable(body, &kp, frame) {
                    return Err(limit(format!("not applicable: {why}")));
                }
                // Prismatic target: the exact slab construction needs no
                // overlap into the support, so the profile may leave the face.
                let out = k::prismatic::prism_boolean(
                    body,
                    &kp,
                    &frame.offset(a),
                    b - a,
                    k::prismatic::SlabOp::Join,
                )
                .map_err(|x| limit(x.to_string()))?;
                check(&out, k::PrismOp::Join, (a, b), (a, b)).map_err(slab_checked)?;
                return Ok((out, (a, b)));
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
        _ => (a, b),
    };
    let truck = k::join_prism(body, &kp, &frame.offset(span.0), span.1 - span.0)
        .map_err(err)
        .and_then(|out| check(&out, k::PrismOp::Join, (a, b), span).map(|()| out));
    let out = match truck {
        Ok(out) => exact_instead(
            id,
            &prefix,
            body,
            out,
            &kp,
            frame,
            (a, b),
            span,
            false,
            false,
        ),
        Err(e) => {
            let out = slab_fallback(body, &kp, frame, span, k::prismatic::SlabOp::Join, e, false)?;
            check(&out, k::PrismOp::Join, (a, b), span).map_err(slab_checked)?;
            out
        }
    };
    Ok((out, span))
}

/// The per-application result check of [`apply_tool`]: (result, operation,
/// nominal span, span used).
type ResultCheck<'a> = dyn FnMut(&Body, k::PrismOp, (f64, f64), (f64, f64)) -> Result<()> + 'a;

/// Join/Cut of a region with inner loops on a target where the exact slab
/// construction does not apply: Truck's boolean of the holed prism. The span
/// is placed as for a single loop: a Join from a face starts a little inside
/// the support (no deeper than half the clear material under the outer
/// loop), a cut gets the same margins as a blind/through cut. Checked against
/// the nominal holed tool by `check`. Returns the failure text otherwise.
#[allow(clippy::too_many_arguments)]
fn holed_truck(
    body: &Body,
    e: &ExtrudeOut,
    frame: &Frame,
    kp: &k::Profile,
    khole: &[k::Profile],
    check: &mut ResultCheck<'_>,
) -> std::result::Result<(Body, (f64, f64)), String> {
    let text = |x: k::KernelError| x.to_string();
    if e.removes {
        let margin = k::through_all_margin(body).map_err(text)?;
        let (nominal, span) = if e.extent == Extent::ThroughAll {
            let (lo, hi) = k::extent_along(body, frame).map_err(text)?;
            let span = (lo - margin, hi + margin);
            (span, span)
        } else {
            let (span, _) =
                blind_cut_span(body, frame, kp, e.nominal, e.dir, e.side, margin).map_err(text)?;
            (e.nominal, span)
        };
        let (start, len) = (frame.offset(span.0), span.1 - span.0);
        let out = if e.pieces {
            k::cut_region_pieces(body, kp, khole, &start, len)
        } else {
            k::cut_region(body, kp, khole, &start, len)
        }
        .map_err(text)?;
        check(&out, k::PrismOp::Cut, nominal, span).map_err(|x| x.to_string())?;
        return Ok((out, span));
    }
    let (a, b) = e.nominal;
    let on_face = (e.dir > 0.0 && a == 0.0) || (e.dir < 0.0 && b == 0.0);
    let span = match e.side {
        Some(side) if on_face => {
            let face = Frame {
                z: [side * frame.z[0], side * frame.z[1], side * frame.z[2]],
                ..*frame
            };
            let clear = k::clear_depth(body, &face, kp, 2.0 * JOIN_OVERLAP).map_err(text)?;
            if clear < 2.0 * k::tol::MIN_LENGTH {
                return Err(format!(
                    "the material under the profile is only {clear:.4} mm deep before another \
                     surface"
                ));
            }
            let delta = JOIN_OVERLAP.min((b - a) / 2.0).min(clear / 2.0);
            if e.dir > 0.0 {
                (-side * delta, b)
            } else {
                (a, -side * delta)
            }
        }
        _ => (a, b),
    };
    let out =
        k::join_region(body, kp, khole, &frame.offset(span.0), span.1 - span.0).map_err(text)?;
    check(&out, k::PrismOp::Join, (a, b), span).map_err(|x| x.to_string())?;
    Ok((out, span))
}

/// Truck's (checked) result `out`, or, when it keeps implicit intersection
/// edges without an exact replacement (e.g. cylinder ∩ cylinder: approximated,
/// not exportable to STEP) and the case is prismatic, the exact slab
/// construction's result, if that passes the result check on its own
/// (ADR 0007). Any failure of the alternative keeps Truck's result.
#[allow(clippy::too_many_arguments)]
fn exact_instead(
    id: FeatureId,
    prefix: &str,
    body: &Body,
    out: Body,
    kp: &k::Profile,
    frame: &Frame,
    nominal: (f64, f64),
    span: (f64, f64),
    removes: bool,
    pieces: bool,
) -> Body {
    if !k::has_implicit_edges(&out) || k::prismatic::not_applicable(body, kp, frame).is_some() {
        return out;
    }
    let (slab_op, op) = if removes {
        (k::prismatic::SlabOp::Cut, k::PrismOp::Cut)
    } else {
        (k::prismatic::SlabOp::Join, k::PrismOp::Join)
    };
    let boolean = if pieces {
        k::prismatic::prism_region_boolean_pieces
    } else {
        k::prismatic::prism_region_boolean
    };
    let Ok(alt) = boolean(
        body,
        kp,
        &[],
        &frame.offset(span.0),
        span.1 - span.0,
        slab_op,
    ) else {
        return out;
    };
    let applied = Applied {
        frame: *frame,
        profile: kp.clone(),
        holes: Vec::new(),
        nominal,
        used: span,
        op,
    };
    match verify_applied(id, prefix, body, &alt, std::slice::from_ref(&applied)) {
        Ok(()) => alt,
        Err(_) => out,
    }
}

/// A result check failure of the exact slab construction (says so: it is a
/// defect of that construction, not Truck's).
fn slab_checked(e: DocError) -> DocError {
    match e {
        DocError::GeometryCheck { feature, detail } => DocError::GeometryCheck {
            feature,
            detail: format!("exact slab construction: {detail}"),
        },
        other => other,
    }
}

/// When Truck fails (or its result fails the check) on a prismatic target
/// (every face planar, perpendicular or parallel to the tool axis) with a
/// straight-edged profile, build the result exactly from slab regions instead
/// (`k::prismatic`, ADR 0006): coplanar and coincident faces are exactly what
/// Truck cannot handle. Otherwise, or if that fails too, the original error,
/// with the slab construction's reason appended.
fn slab_fallback(
    body: &Body,
    kp: &k::Profile,
    frame: &Frame,
    span: (f64, f64),
    op: k::prismatic::SlabOp,
    truck: DocError,
    pieces: bool,
) -> Result<Body> {
    let note = |m: String| match truck.clone() {
        DocError::Kernel { feature, detail } => DocError::Kernel {
            feature,
            detail: format!("{detail}; exact slab construction: {m}"),
        },
        DocError::ProductLimit(detail) => {
            DocError::ProductLimit(format!("{detail}; exact slab construction: {m}"))
        }
        DocError::GeometryCheck { feature, detail } => DocError::GeometryCheck {
            feature,
            detail: format!("{detail}; exact slab construction: {m}"),
        },
        other => other,
    };
    if let Some(why) = k::prismatic::not_applicable(body, kp, frame) {
        return Err(note(format!("not applicable: {why}")));
    }
    let boolean = if pieces {
        k::prismatic::prism_region_boolean_pieces
    } else {
        k::prismatic::prism_region_boolean
    };
    boolean(body, kp, &[], &frame.offset(span.0), span.1 - span.0, op)
        .map_err(|e| note(e.to_string()))
}

/// Frame of a sketch/mirror/hole plane: a world plane, a resolved face, or a
/// construction plane offset from one of those along its normal (same in-plane
/// axes). The resolved face (when the plane is on or offset from a face) is
/// returned for reporting.
fn plane_frame(
    id: FeatureId,
    plane: &SketchPlane,
    st: &Snapshot,
) -> Result<(Frame, Option<ResolvedFace>)> {
    let (base, d) = plane.base_and_offset();
    if !d.is_finite() {
        return Err(DocError::Validation(format!(
            "{id}.plane_offset: the plane offset must be finite, got {d}"
        )));
    }
    let (frame, rf) = match base {
        SketchPlane::WorldXY => (Frame::XY, None),
        SketchPlane::WorldXZ => (Frame::XZ, None),
        SketchPlane::WorldYZ => (FRAME_YZ, None),
        SketchPlane::Face(fr) => {
            let rf = resolve_face(id, &fr, st)?;
            (rf.frame, Some(rf))
        }
        SketchPlane::Offset { .. } => unreachable!("base_and_offset returns a base plane"),
    };
    let a = plane.angle();
    if !a.is_finite() {
        return Err(DocError::Validation(format!(
            "{id}.plane_angle_deg: the plane angle must be finite"
        )));
    }
    // Tilt about the base's local X axis through its origin.
    let frame = if a == 0.0 {
        frame
    } else {
        let (s, c) = a.sin_cos();
        let y = [
            c * frame.y[0] + s * frame.z[0],
            c * frame.y[1] + s * frame.z[1],
            c * frame.y[2] + s * frame.z[2],
        ];
        let z = [
            -s * frame.y[0] + c * frame.z[0],
            -s * frame.y[1] + c * frame.z[1],
            -s * frame.y[2] + c * frame.z[2],
        ];
        Frame { y, z, ..frame }
    };
    Ok((if d == 0.0 { frame } else { frame.offset(d) }, rf))
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
        .filter(|e| !e.removes && !e.joined && e.plain && e.extent == Extent::Distance)
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
    let inner = rcad_sketch::offset_profile(&e.profile, -t).map_err(|x| {
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
            intersect: false,
            plain: true,
            taper: 0.0,
            pieces: false,
            multi: false,
            body: e.body,
        },
    ))
}

/// Tools of a Hole, in application order (`[counterbore, hole]` or `[hole]`),
/// as blind/through cuts from the face in `frame` into the material of
/// `target`.
/// +1/-1 when the faces of `body` in `frame`'s plane that the hole's circle
/// (`center`, `diameter`) overlaps all face along / against `frame.z`; None
/// when it overlaps none (or the diameter is not usable yet). Faces facing
/// both ways under the hole: refused as ambiguous.
fn hole_face_side(
    body: &Body,
    frame: &Frame,
    center: [f64; 2],
    diameter: f64,
) -> std::result::Result<Option<f64>, k::KernelError> {
    if !(diameter.is_finite() && diameter > 0.0) {
        return Ok(None);
    }
    let (pts, curved) = k::profile_outline(&k::Profile::Circle {
        center,
        radius: diameter / 2.0,
    });
    let outline: Vec<[f64; 3]> = pts.into_iter().map(|p| frame.to_world(p)).collect();
    let flipped = Frame {
        x: frame.y,
        y: frame.x,
        z: scale(frame.z, -1.0),
        ..*frame
    };
    let (pos, _) = k::planar_faces_overlapping(body, frame, &outline, curved)?;
    let (neg, _) = k::planar_faces_overlapping(body, &flipped, &outline, curved)?;
    match (pos > 0, neg > 0) {
        (true, false) => Ok(Some(1.0)),
        (false, true) => Ok(Some(-1.0)),
        (false, false) => Ok(None),
        (true, true) => Err(k::KernelError::InvalidInput(
            "the faces under the hole face both ways; the material side is ambiguous".into(),
        )),
    }
}

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
    // The material side from the faces under the hole itself (not any
    // coplanar face of the body: a plane holding faces facing both ways
    // elsewhere would refuse every hole on it).
    let side = match hole_face_side(&t, frame, center, diameter).map_err(kernel(id))? {
        Some(s) => Some(s),
        None => k::coplanar_face_side(&t, frame).map_err(kernel(id))?,
    }
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
            intersect: false,
            plain: true,
            taper: 0.0,
            pieces: false,
            multi: false,
            body: target,
        }
    };
    // "Through all" goes into the material only, through everything of the
    // target in that direction (a two-sided span must not drill
    // an overhang above the face).
    let (lo, hi) = k::extent_along(&t, frame).map_err(kernel(id))?;
    let through = if dir > 0.0 { hi } else { -lo };
    if through <= k::tol::MIN_LENGTH {
        return Err(DocError::ProductLimit(format!(
            "{id}: there is no material of {target} behind the hole's face"
        )));
    }
    let depth_eff = depth.unwrap_or(through);
    let mut v = Vec::new();
    if let Some(c) = counterbore {
        finite(c.diameter, "cbore_diameter")?;
        finite(c.depth, "cbore_depth")?;
        if c.diameter <= diameter || c.depth >= depth_eff {
            return Err(DocError::Validation(format!(
                "{id}: the counterbore must be wider than the hole and shallower than its depth \
                 ({depth_eff} mm here)"
            )));
        }
        v.push(tool(c.diameter, Some(c.depth)));
    }
    if let Some(d) = depth {
        finite(d, "depth")?;
    }
    v.push(tool(diameter, Some(depth_eff)));
    Ok(v)
}

/// Pattern/mirror seeds: an evaluated Extrude Join or Cut, or a Hole (its
/// counterbore and hole tools, applied in that order).
fn copy_seed(st: &Snapshot, id: FeatureId, seed: FeatureId) -> Result<Vec<ExtrudeOut>> {
    let body = st
        .holes
        .get(&seed)
        .and_then(|t| t.first())
        .map(|e| e.body)
        .or(st.body_of_extrude(seed));
    if st.moved_since(body, seed) {
        return Err(DocError::ProductLimit(format!(
            "{id}: the body of {seed} was moved after it; pattern or mirror it before the move"
        )));
    }
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
    if e.taper != 0.0 {
        return Err(DocError::ProductLimit(format!(
            "{id}: patterns and mirrors do not copy tapered extrusions ({seed}); their copies \
             would not be drafted"
        )));
    }
    if e.intersect || e.multi {
        return Err(DocError::ProductLimit(format!(
            "{id}: patterns and mirrors copy single-region Extrude Join or Cut features ({seed} \
             is {})",
            if e.intersect {
                "an intersect"
            } else {
                "a multi-region extrusion"
            }
        )));
    }
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
    second: Option<crate::PatternDirection>,
) -> Result<BodyId> {
    let tools = copy_seed(st, id, seed)?;
    let e = tools[0].clone();
    if !(1..=crate::MAX_PATTERN_COUNT).contains(&count) {
        return Err(DocError::Validation(format!(
            "{id}.count: must be in 1..={} (1 = the seed only), got {count}",
            crate::MAX_PATTERN_COUNT
        )));
    }
    if !spacing.is_finite() || spacing <= 0.0 {
        return Err(DocError::Validation(format!(
            "{id}.spacing: must be > 0 mm, got {spacing}"
        )));
    }
    let f = e.start;
    let world_dir = |direction: [f64; 2]| -> Result<[f64; 3]> {
        let len = direction[0].hypot(direction[1]);
        if !len.is_finite() || len <= 0.0 {
            return Err(DocError::Validation(format!(
                "{id}: pattern direction must be a non-zero vector"
            )));
        }
        let d = [direction[0] / len, direction[1] / len];
        Ok([
            f.x[0] * d[0] + f.y[0] * d[1],
            f.x[1] * d[0] + f.y[1] * d[1],
            f.x[2] * d[0] + f.y[2] * d[1],
        ])
    };
    let world = world_dir(direction)?;
    let (count2, spacing2, world2) = match second {
        None => (1, 0.0, [0.0; 3]),
        Some(s) => {
            if !(1..=crate::MAX_PATTERN_COUNT).contains(&s.count) {
                return Err(DocError::Validation(format!(
                    "{id}.count2: must be in 1..={}, got {}",
                    crate::MAX_PATTERN_COUNT,
                    s.count
                )));
            }
            if !s.spacing.is_finite() || s.spacing <= 0.0 {
                return Err(DocError::Validation(format!(
                    "{id}.spacing2: must be > 0 mm, got {}",
                    s.spacing
                )));
            }
            let w2 = world_dir(s.direction)?;
            let c = world[0] * w2[1] - world[1] * w2[0];
            let c = [
                world[1] * w2[2] - world[2] * w2[1],
                world[2] * w2[0] - world[0] * w2[2],
                c,
            ];
            if (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() < 1e-6 {
                return Err(DocError::Validation(format!(
                    "{id}: the second pattern direction is parallel to the first"
                )));
            }
            if u64::from(count) * u64::from(s.count) > u64::from(crate::MAX_PATTERN_COUNT) {
                return Err(DocError::Validation(format!(
                    "{id}: {count} x {} = {} instances; at most {} in one pattern",
                    s.count,
                    u64::from(count) * u64::from(s.count),
                    crate::MAX_PATTERN_COUNT
                )));
            }
            (s.count, s.spacing, w2)
        }
    };
    let total = count * count2;
    let before = brep_target(st, id, e.body)?;
    let mut body = before.clone();
    let mut applied = Vec::new();
    let mut copies = Vec::new();
    for k in 1..total {
        crate::check_cancel(id)?;
        let (i, j) = (k % count, k / count);
        let t = spacing * f64::from(i);
        let t2 = spacing2 * f64::from(j);
        let frame = Frame {
            origin: [
                f.origin[0] + world[0] * t + world2[0] * t2,
                f.origin[1] + world[1] * t + world2[1] * t2,
                f.origin[2] + world[2] * t + world2[2] * t2,
            ],
            ..f
        };
        for (ti, tool) in tools.iter().enumerate() {
            body = apply_tool(
                &body,
                tool,
                &frame,
                Some(&format!("instance {} of {total}", k + 1)),
                id,
                Some(&mut applied),
            )?
            .0;
            // Stable instance number: `i + j·MAX_PATTERN_COUNT`, so a
            // FaceRole::Copy reference keeps its (i, j) occurrence when either
            // count changes (one row: `i`, as before).
            copies.push((
                i + j * crate::MAX_PATTERN_COUNT,
                ti as u32,
                ExtrudeOut {
                    start: frame,
                    ..tool.clone()
                },
            ));
        }
    }
    // One independent check of the whole pattern (ADR 0005). Count 1 applies
    // nothing: the body is the seed's result, unchanged.
    if !applied.is_empty() {
        verify_applied(id, "pattern: ", &before, &body, &applied)?;
    }
    replace_body(st, id, e.body, body);
    st.copies.insert(id, copies);
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
    if !(1..=crate::MAX_PATTERN_COUNT).contains(&count) {
        return Err(DocError::Validation(format!(
            "{id}.count: must be in 1..={} (1 = the seed only), got {count}",
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
    let mut copies = Vec::new();
    for i in 1..count {
        crate::check_cancel(id)?;
        let t = angle * f64::from(i);
        let o = rot(sub(f.origin, c), t);
        let frame = Frame {
            origin: [c[0] + o[0], c[1] + o[1], c[2] + o[2]],
            x: rot(f.x, t),
            y: rot(f.y, t),
            z: f.z,
        };
        for (ti, tool) in tools.iter().enumerate() {
            body = apply_tool(
                &body,
                tool,
                &frame,
                Some(&format!("instance {} of {count}", i + 1)),
                id,
                Some(&mut applied),
            )?
            .0;
            copies.push((
                i,
                ti as u32,
                ExtrudeOut {
                    start: frame,
                    ..tool.clone()
                },
            ));
        }
    }
    // One independent check of the whole pattern (ADR 0005). Count 1 applies
    // nothing: the body is the seed's result, unchanged.
    if !applied.is_empty() {
        verify_applied(id, "pattern: ", &before, &body, &applied)?;
    }
    replace_body(st, id, e.body, body);
    st.copies.insert(id, copies);
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
    let mut copies = Vec::new();
    for (ti, mut e) in tools.into_iter().enumerate() {
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
        e.start = frame;
        copies.push((1, ti as u32, e));
    }
    verify_applied(id, "mirror: ", &before, &body, &applied)?;
    replace_body(st, id, target, body);
    st.copies.insert(id, copies);
    Ok(target)
}

/// True when `result` encloses measurably less volume than `target`: the exact boundary
/// integrals differ by more than three times their combined error estimates (and 1e-9 of the
/// target, for float noise). False when either volume is unavailable, so the caller refuses.
fn removes_volume(target: &Body, result: &Body) -> bool {
    let vol = |b: &Body| match k::measure(b).map(|m| m.volume) {
        Ok(k::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        }) => Some((volume, error_estimate)),
        _ => None,
    };
    match (vol(target), vol(result)) {
        (Some((vt, et)), Some((vr, er))) => vt - vr > (3.0 * (et + er)).max(1.0e-9 * vt.abs()),
        _ => false,
    }
}

/// Legacy (v1/v2) circular cut (`depth == None`: through all) and pocket features on
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
    // Nominal cut (through all: the body's whole extent along the normal plus
    // the margin; pocket: from the face down to `depth`) and the span handed
    // to Truck (a pocket's top is moved a margin above the face).
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
    // The target's check tessellation from the check that produced it.
    let target_mesh = st.bodies.get(&target).and_then(|b| b.check_mesh.clone());
    // Independent check against the nominal cut (ADR 0005).
    let verify = |b: &Body, used: (f64, f64)| {
        k::verify_prism_boolean_reusing(
            &t,
            target_mesh.as_ref(),
            b,
            k::PrismOp::Cut,
            &k::Prism {
                profile: &kp,
                holes: &[],
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
        })
    };
    let t0 = std::time::Instant::now();
    let truck = match depth {
        None => k::cut_through_all(&t, &kp, &s.frame),
        Some(d) => k::cut_blind(&t, &kp, &s.frame, d),
    }
    .map_err(kernel(id));
    trace_regen(id, "truck_cut", t0);
    let t0 = std::time::Instant::now();
    let truck = truck.and_then(|b| verify(&b, used).map(|(_, m)| (b, m)));
    trace_regen(id, "verify", t0);
    // As for Extrude Cut (ADR 0006/0007): when Truck fails or its result fails
    // the check on a prismatic target, the exact slab construction over the
    // nominal span, checked the same way.
    let (b, mesh) = match truck {
        Ok(x) => x,
        Err(e) => {
            let b = slab_fallback(
                &t,
                &kp,
                &s.frame,
                nominal,
                k::prismatic::SlabOp::Cut,
                e,
                false,
            )?;
            let (_, m) = verify(&b, nominal).map_err(slab_checked)?;
            (b, m)
        }
    };
    // A cut that removes material usually adds faces; containment (checked
    // above) guarantees the cutter intersects the body. A cut engulfing an
    // existing hole replaces its walls and can keep or lower the face count, so
    // then the exact volumes decide (rare path; the measure is not cheap).
    if k::face_count(&b) <= k::face_count(&t) && !removes_volume(&t, &b) {
        return Err(DocError::Kernel {
            feature: id,
            detail: format!(
                "{} removed no material",
                if depth.is_some() { "pocket" } else { "cut" }
            ),
        });
    }
    replace_body_checked(st, id, target, b, Some(mesh));
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
    if e.intersect {
        return Err(broken(format!(
            "{} is an intersect extrusion; its faces are not referencable by role",
            fr.feature
        )));
    }
    // Cap positions along the sketch normal: the far end along the direction
    // (end cap) and the near end (start cap) of the nominal span.
    let (lo, hi) = e.nominal;
    let (end_at, start_at) = if e.dir > 0.0 { (hi, lo) } else { (lo, hi) };
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
                origin[0] + f.z[0] * end_at,
                origin[1] + f.z[1] * end_at,
                origin[2] + f.z[2] * end_at,
            ];
            // New/Join: outward along the extrusion; cut floor: back toward the opening.
            let z = if e.removes { scale(z_dir, -1.0) } else { z_dir };
            Ok(frame_from(origin, z, f.x))
        }
        FaceRole::ExtrudeStartCap => {
            if (e.removes || e.joined) && start_at == 0.0 {
                return Err(broken(format!(
                    "{} has no start cap face (a {} starts on its support)",
                    fr.feature,
                    if e.removes { "cut" } else { "join" }
                )));
            }
            let o = f.origin;
            let origin = [
                o[0] + f.z[0] * start_at,
                o[1] + f.z[1] * start_at,
                o[2] + f.z[2] * start_at,
            ];
            Ok(frame_from(origin, scale(z_dir, -1.0), f.x))
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
                    rcad_sketch::LoopSegment::Line { start } => {
                        (start, segs[(i + 1) % segs.len()].start())
                    }
                    rcad_sketch::LoopSegment::Arc { .. } => {
                        return Err(broken(format!(
                            "sketch entity {entity} is an arc; its side face is curved"
                        )));
                    }
                    rcad_sketch::LoopSegment::Cubic { .. } => {
                        return Err(broken(format!(
                            "sketch entity {entity} is an ellipse or spline; its side face is \
                             curved"
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
        FaceRole::Copy { .. } | FaceRole::Chamfer => Err(broken(format!(
            "{} is an extrusion, not a pattern, mirror or chamfer",
            fr.feature
        ))),
    }
}

/// The sweep path of `sketch` (on `frame`): its non-construction lines and
/// arcs must form one open chain; it is walked from the end at `start` (the
/// profile sketch's origin, world mm) to the other end. Circles, branches,
/// closed loops or a chain not starting at `start` are refused.
fn path_segments(
    sketch: &rcad_sketch::Sketch,
    frame: &Frame,
    start: [f64; 3],
) -> std::result::Result<Vec<k::PathSegment>, String> {
    use rcad_sketch::Entity;
    let edges: Vec<&Entity> = sketch
        .entities
        .iter()
        .filter(|e| !sketch.is_construction(e.id()))
        .collect();
    if edges.is_empty() {
        return Err("the path sketch has no lines or arcs".into());
    }
    let ends = |e: &Entity| -> std::result::Result<_, String> {
        match e {
            Entity::Line { a, b, .. } => Ok((*a, *b)),
            Entity::Arc { start, end, .. } => Ok((*start, *end)),
            Entity::Circle { .. } => Err("a circle cannot be a sweep path".to_string()),
            Entity::Ellipse { .. } | Entity::Spline { .. } => {
                Err("an ellipse or spline cannot be a sweep path (lines and arcs only)".to_string())
            }
        }
    };
    let mut degree: HashMap<rcad_sketch::PointId, usize> = HashMap::new();
    for e in &edges {
        let (a, b) = ends(e)?;
        *degree.entry(a).or_default() += 1;
        *degree.entry(b).or_default() += 1;
    }
    if degree.values().any(|&d| d > 2) {
        return Err("the path branches (a point joins more than two curves)".into());
    }
    let tips: Vec<_> = degree
        .iter()
        .filter(|&(_, &d)| d == 1)
        .map(|(p, _)| *p)
        .collect();
    if tips.len() != 2 {
        return Err("the path must be one open chain of lines and arcs".into());
    }
    let world = |p: rcad_sketch::PointId| -> std::result::Result<[f64; 3], String> {
        let q = sketch.point_xy(p).ok_or("missing path point")?;
        Ok(frame.to_world(q))
    };
    let dist = |a: [f64; 3], b: [f64; 3]| dot(sub(a, b), sub(a, b)).sqrt();
    let tol = 1e-6 * (1.0 + dot(start, start).sqrt());
    let first = *tips
        .iter()
        .find(|p| world(**p).is_ok_and(|w| dist(w, start) <= tol))
        .ok_or("the path must start at the profile sketch's origin")?;
    let mut out = Vec::new();
    let mut used = vec![false; edges.len()];
    let mut at = first;
    while out.len() < edges.len() {
        let i = (0..edges.len())
            .find(|&i| !used[i] && ends(edges[i]).is_ok_and(|(a, b)| a == at || b == at))
            .ok_or("the path is not one connected chain")?;
        used[i] = true;
        let (a, b) = ends(edges[i])?;
        let next = if a == at { b } else { a };
        out.push(match edges[i] {
            Entity::Line { .. } => k::PathSegment::Line { end: world(next)? },
            Entity::Arc { center, .. } => k::PathSegment::Arc {
                center: world(*center)?,
                end: world(next)?,
            },
            Entity::Circle { .. } | Entity::Ellipse { .. } | Entity::Spline { .. } => {
                unreachable!("refused above")
            }
        });
        at = next;
    }
    Ok(out)
}

/// Area of a closed planar polygon in 3-D (Newell), mm².
fn polygon_area(p: &[[f64; 3]]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = [0.0; 3];
    for i in 0..n {
        let c = cross(p[i], p[(i + 1) % n]);
        for k in 0..3 {
            a[k] += c[k];
        }
    }
    0.5 * dot(a, a).sqrt()
}

/// Footprint of the face `role` of extrusion tool `e` (whose resolved plane is
/// `frame`), as a closed world polygon in that plane, and whether it samples
/// curves: the profile on a cap; the generating edge swept over the nominal
/// span on a side face. Empty when unknown.
fn footprint(e: &ExtrudeOut, role: FaceRole, frame: &Frame) -> (Vec<[f64; 3]>, bool) {
    let f = e.start;
    match role {
        FaceRole::ExtrudeEndCap | FaceRole::ExtrudeStartCap => {
            let shift = sub(frame.origin, f.to_world([0.0, 0.0]));
            let (pts, curved) = k::profile_outline(&to_kernel(&e.profile));
            let out = pts
                .into_iter()
                .map(|p| {
                    let w = f.to_world(p);
                    [w[0] + shift[0], w[1] + shift[1], w[2] + shift[2]]
                })
                .collect();
            (out, curved)
        }
        FaceRole::ExtrudeSide { entity } => {
            let edge = std::iter::once((&e.profile, &e.entities))
                .chain(e.holes.iter().map(|(p, ents)| (p, ents)))
                .find_map(|(p, ents)| {
                    let i = ents.iter().position(|x| x.0 == entity)?;
                    match p {
                        SketchProfile::Polygon(v) => Some((v[i], v[(i + 1) % v.len()])),
                        SketchProfile::Loop(segs) => match segs[i] {
                            rcad_sketch::LoopSegment::Line { start } => {
                                Some((start, segs[(i + 1) % segs.len()].start()))
                            }
                            rcad_sketch::LoopSegment::Arc { .. }
                            | rcad_sketch::LoopSegment::Cubic { .. } => None,
                        },
                        SketchProfile::Circle { .. } => None,
                    }
                });
            let Some((p, q)) = edge else {
                return (Vec::new(), false);
            };
            // Through-all cuts span the whole body: their nominal span is the
            // body's extent plus margins, which only enlarges the footprint
            // past the material (no other face there).
            let (lo, hi) = if e.removes || e.joined {
                e.nominal
            } else {
                e.span
            };
            let (a, b) = (f.to_world(p), f.to_world(q));
            let at =
                |w: [f64; 3], t: f64| [w[0] + f.z[0] * t, w[1] + f.z[1] * t, w[2] + f.z[2] * t];
            (vec![at(a, lo), at(b, lo), at(b, hi), at(a, hi)], false)
        }
        _ => (Vec::new(), false),
    }
}

/// Occurrence `instance` (tool `tool`) of pattern/mirror `feature`, or a
/// `BrokenReference` naming why it does not exist (never another occurrence).
fn copy_occurrence(
    st: &Snapshot,
    from: FeatureId,
    feature: FeatureId,
    instance: u32,
    tool: u32,
) -> Result<&ExtrudeOut> {
    let copies = st.copies.get(&feature).ok_or(DocError::BrokenReference {
        from,
        detail: format!("{feature} is not an evaluated pattern or mirror"),
    })?;
    copies
        .iter()
        .find(|(i, t, _)| *i == instance && *t == tool)
        .map(|(_, _, e)| e)
        .ok_or_else(|| {
            let n = copies.iter().map(|(i, _, _)| *i).max().map_or(1, |m| m + 1);
            DocError::BrokenReference {
                from,
                detail: format!(
                    "occurrence {} (tool {tool}) of {feature} no longer exists ({feature} now \
                     has {n} occurrence(s)); re-attach to an existing face",
                    instance + 1
                ),
            }
        })
}

/// Outward frame of the face an equal-distance chamfer of `d` mm makes on the
/// convex edge between planar faces `a` and `b` (outward normals `na`, `nb`):
/// it contains the edge direction `e` and the points `P + d ta`, `P + d tb`,
/// where `ta`/`tb` are the in-face directions away from the edge (`ta ⟂ e,
/// na`, pointing against `nb` on a convex edge). Matches `k::chamfer_edge`.
fn chamfer_face_frame(a: &Frame, b: &Frame, d: f64) -> Option<Frame> {
    let (na, nb) = (a.z, b.z);
    let e = cross(na, nb);
    let len = dot(e, e).sqrt();
    if len.is_nan() || len <= 1e-9 {
        return None;
    }
    let e = scale(e, 1.0 / len);
    let (ha, hb) = (dot(na, a.origin), dot(nb, b.origin));
    let den = dot(cross(na, nb), e);
    let p0 = scale(
        [
            cross(nb, e)[0] * ha + cross(e, na)[0] * hb,
            cross(nb, e)[1] * ha + cross(e, na)[1] * hb,
            cross(nb, e)[2] * ha + cross(e, na)[2] * hb,
        ],
        1.0 / den,
    );
    let away = |n: [f64; 3], other: [f64; 3]| {
        let t = cross(n, e);
        if dot(t, other) < 0.0 {
            t
        } else {
            scale(t, -1.0)
        }
    };
    let (ta, tb) = (away(na, nb), away(nb, na));
    let pa = [p0[0] + d * ta[0], p0[1] + d * ta[1], p0[2] + d * ta[2]];
    let mut z = cross(sub(ta, tb), e);
    let zl = dot(z, z).sqrt();
    if zl.is_nan() || zl <= 1e-12 {
        return None;
    }
    z = scale(z, 1.0 / zl);
    if dot(z, [na[0] + nb[0], na[1] + nb[1], na[2] + nb[2]]) < 0.0 {
        z = scale(z, -1.0);
    }
    Some(Frame {
        origin: pa,
        x: e,
        y: cross(z, e),
        z,
    })
}

/// Resolve a semantic face reference from the generating feature's definition, then
/// confirm it against the owning body's current B-rep: exactly one planar face
/// must coincide (same plane, same outward normal). Never guesses.
/// Revolve `profile` (sketch coordinates on `frame`) by `angle` about
/// `axis`. The kernel revolves about a frame's local Y with the
/// profile on its +X side and sweeps toward that frame's −Z; the frame is
/// chosen so the profile is on +X (a proper rotation of the sketch frame), and
/// the result is turned back about the axis when the sweep must leave toward
/// the other side (`reverse` false: toward −normal, as runs 1-5).
fn revolve_about(
    id: FeatureId,
    profile: &SketchProfile,
    frame: &Frame,
    sketch: &rcad_sketch::Sketch,
    axis: crate::RevolveAxis,
    angle: f64,
    reverse: bool,
) -> Result<Body> {
    let invalid = |m: String| DocError::Validation(format!("{id}: revolve: {m}"));
    // Axis in sketch coordinates: point p, unit direction d.
    let (p, d) = match axis {
        crate::RevolveAxis::SketchY => ([0.0, 0.0], [0.0, 1.0]),
        crate::RevolveAxis::SketchX => ([0.0, 0.0], [1.0, 0.0]),
        crate::RevolveAxis::Line(e) => {
            let Some(rcad_sketch::Entity::Line { a, b, .. }) =
                sketch.entities.iter().find(|x| x.id() == e)
            else {
                return Err(DocError::BrokenReference {
                    from: id,
                    detail: format!("revolve axis {e:?} is not a line of the sketch"),
                });
            };
            let pt = |q: rcad_sketch::PointId| {
                sketch
                    .points
                    .iter()
                    .find(|x| x.id == q)
                    .map(|x| [x.x, x.y])
                    .ok_or_else(|| invalid(format!("axis point {q:?} missing")))
            };
            let (a, b) = (pt(*a)?, pt(*b)?);
            let l = (b[0] - a[0]).hypot(b[1] - a[1]);
            if l < k::tol::MIN_LENGTH {
                return Err(invalid("the axis line has no length".into()));
            }
            (a, [(b[0] - a[0]) / l, (b[1] - a[1]) / l])
        }
    };
    // Side of the profile: the sign of (q − p) × d over its outline.
    let (outline, _) = k::profile_outline(&to_kernel(profile));
    let side = |q: [f64; 2]| (q[0] - p[0]) * d[1] - (q[1] - p[1]) * d[0];
    let (lo, hi) = outline
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), q| {
            let v = side(*q);
            (l.min(v), h.max(v))
        });
    // Touching the axis (within the kernel tolerance) is allowed: the kernel
    // revolves such profiles when they meet the axis along a straight edge.
    let eps = k::tol::KERNEL_LINEAR;
    let s = if lo > -eps && hi > eps {
        1.0
    } else if hi < eps && lo < -eps {
        -1.0
    } else {
        return Err(invalid(
            "the profile crosses the axis; it must lie on one side of it".into(),
        ));
    };
    // New 2-D frame: x' = s·(d.y, −d.x) (toward the profile), y' = s·d: a
    // rotation of the sketch axes (by 180° when s < 0), so the profile keeps
    // its orientation and the normal is unchanged.
    let xp = [s * d[1], -s * d[0]];
    let yp = [s * d[0], s * d[1]];
    let to = |q: [f64; 2]| {
        let w = [q[0] - p[0], q[1] - p[1]];
        [w[0] * xp[0] + w[1] * xp[1], w[0] * yp[0] + w[1] * yp[1]]
    };
    let mapped = map_profile(profile, &to);
    let origin = frame.to_world(p);
    let wx = sub(frame.to_world([p[0] + xp[0], p[1] + xp[1]]), origin);
    let wy = sub(frame.to_world([p[0] + yp[0], p[1] + yp[1]]), origin);
    let f2 = Frame {
        origin,
        x: wx,
        y: wy,
        z: frame.z,
    };
    let body = k::revolve(&to_kernel(&mapped), &f2, angle).map_err(|e| invalid(e.to_string()))?;
    // The kernel's sweep leaves toward −f2.z = −frame.z; flip for `reverse`.
    if reverse && angle < 2.0 * std::f64::consts::PI - 1e-12 {
        let t = k::Rigid::new([0.0; 3], origin, wy, -angle).map_err(kernel(id))?;
        return k::transform_body(&body, &t).map_err(kernel(id));
    }
    Ok(body)
}

/// The revolve axis in world coordinates (point, unit direction).
fn revolve_axis_world(
    id: FeatureId,
    frame: &Frame,
    sketch: &rcad_sketch::Sketch,
    axis: crate::RevolveAxis,
) -> Result<([f64; 3], [f64; 3])> {
    let (p, q) = match axis {
        crate::RevolveAxis::SketchY => ([0.0, 0.0], [0.0, 1.0]),
        crate::RevolveAxis::SketchX => ([0.0, 0.0], [1.0, 0.0]),
        crate::RevolveAxis::Line(e) => {
            let Some(rcad_sketch::Entity::Line { a, b, .. }) =
                sketch.entities.iter().find(|x| x.id() == e)
            else {
                return Err(DocError::BrokenReference {
                    from: id,
                    detail: format!("revolve axis {e:?} is not a line of the sketch"),
                });
            };
            let xy = |q: rcad_sketch::PointId| {
                sketch
                    .points
                    .iter()
                    .find(|x| x.id == q)
                    .map(|x| [x.x, x.y])
                    .unwrap_or([0.0, 0.0])
            };
            (xy(*a), xy(*b))
        }
    };
    let (o, w) = (frame.to_world(p), frame.to_world(q));
    let d = sub(w, o);
    let l = dot(d, d).sqrt();
    if l == 0.0 {
        return Err(DocError::Validation(format!(
            "{id}: revolve axis has no length"
        )));
    }
    Ok((o, scale(d, 1.0 / l)))
}

/// `profile` with every point mapped by `f` (a proper rigid motion of the
/// plane: orientation and arc senses are kept).
fn map_profile(profile: &SketchProfile, f: &dyn Fn([f64; 2]) -> [f64; 2]) -> SketchProfile {
    match profile {
        SketchProfile::Polygon(v) => SketchProfile::Polygon(v.iter().map(|q| f(*q)).collect()),
        SketchProfile::Circle { center, radius } => SketchProfile::Circle {
            center: f(*center),
            radius: *radius,
        },
        SketchProfile::Loop(segs) => SketchProfile::Loop(
            segs.iter()
                .map(|s| match *s {
                    rcad_sketch::LoopSegment::Line { start } => {
                        rcad_sketch::LoopSegment::Line { start: f(start) }
                    }
                    rcad_sketch::LoopSegment::Arc { start, center, ccw } => {
                        rcad_sketch::LoopSegment::Arc {
                            start: f(start),
                            center: f(center),
                            ccw,
                        }
                    }
                    rcad_sketch::LoopSegment::Cubic { start, c1, c2, w } => {
                        rcad_sketch::LoopSegment::Cubic {
                            start: f(start),
                            c1: f(c1),
                            c2: f(c2),
                            w,
                        }
                    }
                })
                .collect(),
        ),
    }
}

/// `frame` carried back through `t` (the inverse of a rigid transform).
fn moved_back(t: &k::Rigid, frame: &Frame) -> Frame {
    let tr = |v: [f64; 3]| {
        [
            t.r[0][0] * v[0] + t.r[1][0] * v[1] + t.r[2][0] * v[2],
            t.r[0][1] * v[0] + t.r[1][1] * v[1] + t.r[2][1] * v[2],
            t.r[0][2] * v[0] + t.r[1][2] * v[1] + t.r[2][2] * v[2],
        ]
    };
    // A similarity s·R (moves: s = 1; Scale: s ≠ 1): its inverse is Rᵀ/s, so
    // Mᵀ q / s² for points; frame axes are re-normalized.
    let s2 = t.r[0][0] * t.r[0][0] + t.r[1][0] * t.r[1][0] + t.r[2][0] * t.r[2][0];
    let unit = |v: [f64; 3]| scale(v, 1.0 / dot(v, v).sqrt());
    Frame {
        origin: scale(tr(sub(frame.origin, t.t)), 1.0 / s2),
        x: unit(tr(frame.x)),
        y: unit(tr(frame.y)),
        z: unit(tr(frame.z)),
    }
}

/// Projection source curves, loop by loop, each with its source entity.
pub type SourceCurves = Vec<Vec<(k::Curve2, Option<EntityId>)>>;

/// Tag of an edge of an oblique face that cannot be projected exactly (an
/// arc: its projection is an elliptical arc). It stands in the source list
/// as its chord so a pick near it is refused by name, and it is never bound.
pub const UNPROJECTABLE: EntityId = EntityId(u32::MAX);

/// Source curves of a projection into the sketch on `frame` (2-D sketch
/// coordinates), loop by loop, each with its source entity (sketch sources).
/// The source plane must be parallel to the sketch plane (projection along
/// the sketch normal is then a translation: lines and arcs stay exact).
fn link_curves(
    id: FeatureId,
    frame: &Frame,
    source: &ProjectSource,
    st: &Snapshot,
) -> Result<SourceCurves> {
    let broken = |detail: String| DocError::BrokenReference { from: id, detail };
    let (src_frame, loops): (Frame, SourceCurves) = match source {
        ProjectSource::Edge(r) => {
            let rf = resolve_face(id, r, st)?;
            let body = brep_target(st, id, rf.body)?;
            let loops = k::face_boundary_curves(&body, &rf.frame)
                .map_err(|e| broken(format!("projection of {} {:?}: {e}", r.feature, r.role)))?;
            let mut loops: SourceCurves = loops
                .into_iter()
                .map(|l| {
                    merged_circle(&l)
                        .map_or(l, |c| vec![c])
                        .into_iter()
                        .map(|c| (c, None))
                        .collect()
                })
                .collect();
            annotate_cap_edges(st, r, rf.body, &rf.frame, &mut loops);
            return project_oblique(id, &rf.frame, frame, loops);
        }
        ProjectSource::Section { body, .. } => {
            let b = brep_target(st, id, *body)?;
            let loops = k::section_curves(&b, frame)
                .map_err(|e| broken(format!("section of body {body}: {e}")))?;
            (
                *frame,
                loops
                    .into_iter()
                    .map(|l| l.into_iter().map(|c| (c, None)).collect())
                    .collect(),
            )
        }
        ProjectSource::Face(r) => {
            let rf = resolve_face(id, r, st)?;
            let body = brep_target(st, id, rf.body)?;
            let loops = k::face_boundary_curves(&body, &rf.frame)
                .map_err(|e| broken(format!("projection of {} {:?}: {e}", r.feature, r.role)))?;
            (
                rf.frame,
                loops
                    .into_iter()
                    .map(|l| l.into_iter().map(|c| (c, None)).collect())
                    .collect(),
            )
        }
        ProjectSource::Sketch(f) => {
            let so = st.sketches.get(f).ok_or_else(|| {
                broken(format!("projection source {f} is not an evaluated sketch"))
            })?;
            let sk = &so.solved;
            let pt = |p: rcad_sketch::PointId| -> Result<[f64; 2]> {
                sk.points
                    .iter()
                    .find(|q| q.id == p)
                    .map(|q| [q.x, q.y])
                    .ok_or_else(|| broken(format!("sketch {f} lost point {p:?}")))
            };
            let mut out = Vec::new();
            for e in sk
                .entities
                .iter()
                .filter(|e| !sk.construction.contains(&e.id()))
            {
                let c = match e {
                    rcad_sketch::Entity::Line { a, b, .. } => k::Curve2::Line {
                        a: pt(*a)?,
                        b: pt(*b)?,
                    },
                    rcad_sketch::Entity::Arc {
                        center, start, end, ..
                    } => k::Curve2::Arc {
                        c: pt(*center)?,
                        a: pt(*start)?,
                        b: pt(*end)?,
                        ccw: true,
                    },
                    rcad_sketch::Entity::Circle { center, radius, .. } => k::Curve2::Circle {
                        c: pt(*center)?,
                        r: *radius,
                    },
                    rcad_sketch::Entity::Ellipse { .. } | rcad_sketch::Entity::Spline { .. } => {
                        return Err(DocError::ProductLimit(format!(
                            "{id}: projecting sketch ellipses and splines is not supported \
                             (lines, arcs and circles only)"
                        )));
                    }
                };
                out.push((c, Some(e.id())));
            }
            (so.frame, vec![out])
        }
    };
    if dot(src_frame.z, frame.z).abs() < 1.0 - 1e-9 {
        return Err(DocError::ProductLimit(format!(
            "{id}: projection needs a source parallel to the sketch plane (an oblique source \
             would project arcs to ellipses)"
        )));
    }
    let to2 = |p: [f64; 2]| {
        let w = sub(src_frame.to_world(p), frame.origin);
        [dot(w, frame.x), dot(w, frame.y)]
    };
    // A mirrored in-plane orientation flips arc directions.
    let flip = dot(src_frame.z, frame.z) < 0.0;
    Ok(loops
        .into_iter()
        .map(|l| {
            l.into_iter()
                .map(|(c, e)| {
                    let c = match c {
                        k::Curve2::Line { a, b } => k::Curve2::Line {
                            a: to2(a),
                            b: to2(b),
                        },
                        k::Curve2::Arc { c, a, b, ccw } => k::Curve2::Arc {
                            c: to2(c),
                            a: to2(a),
                            b: to2(b),
                            ccw: ccw != flip,
                        },
                        k::Curve2::Circle { c, r } => k::Curve2::Circle { c: to2(c), r },
                        k::Curve2::Ellipse { c, major, minor } => k::Curve2::Ellipse {
                            c: to2(c),
                            major: to2(major),
                            minor,
                        },
                    };
                    (c, e)
                })
                .collect()
        })
        .collect())
}

/// Semantic identity of the boundary edges of an extrusion cap: the profile
/// entity whose side face meets the cap along the edge (the edge lies on that
/// entity's line or circle, moved like the body). Set only when exactly one
/// profile entity carries the edge; other faces and edges stay `None` (bound
/// by position, strictly, in `bind_link`).
fn annotate_cap_edges(
    st: &Snapshot,
    r: &FaceRef,
    body: BodyId,
    cap: &Frame,
    loops: &mut SourceCurves,
) {
    if !matches!(r.role, FaceRole::ExtrudeEndCap | FaceRole::ExtrudeStartCap) {
        return;
    }
    let Some(e) = st.extrudes.get(&r.feature) else {
        return;
    };
    let moved = st.placement(body, r.feature);
    let loc = |p: [f64; 2]| {
        let w = sub(moved.point(e.start.to_world(p)), cap.origin);
        [dot(w, cap.x), dot(w, cap.y)]
    };
    // Carriers: (entity, Line(a, b) or Circle(c, r)) in cap coordinates.
    let mut carriers: Vec<(EntityId, k::Curve2)> = Vec::new();
    for (prof, ents) in
        std::iter::once((&e.profile, &e.entities)).chain(e.holes.iter().map(|(p, en)| (p, en)))
    {
        match prof {
            SketchProfile::Polygon(v) => {
                for (i, ent) in ents.iter().enumerate().take(v.len()) {
                    carriers.push((
                        *ent,
                        k::Curve2::Line {
                            a: loc(v[i]),
                            b: loc(v[(i + 1) % v.len()]),
                        },
                    ));
                }
            }
            SketchProfile::Circle { center, radius } => {
                if let Some(ent) = ents.first() {
                    carriers.push((
                        *ent,
                        k::Curve2::Circle {
                            c: loc(*center),
                            r: *radius,
                        },
                    ));
                }
            }
            SketchProfile::Loop(segs) => {
                for (i, (seg, ent)) in segs.iter().zip(ents).enumerate() {
                    let next = segs[(i + 1) % segs.len()].start();
                    match *seg {
                        rcad_sketch::LoopSegment::Line { start } => carriers.push((
                            *ent,
                            k::Curve2::Line {
                                a: loc(start),
                                b: loc(next),
                            },
                        )),
                        rcad_sketch::LoopSegment::Arc { start, center, .. } => carriers.push((
                            *ent,
                            k::Curve2::Circle {
                                c: loc(center),
                                r: (start[0] - center[0]).hypot(start[1] - center[1]),
                            },
                        )),
                        rcad_sketch::LoopSegment::Cubic { .. } => {}
                    }
                }
            }
        }
    }
    let t = 10.0 * k::tol::KERNEL_LINEAR;
    let on_line = |p: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        l > 0.0 && ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / l <= t
    };
    for l in loops.iter_mut() {
        for (c, tag) in l.iter_mut() {
            let hits: Vec<EntityId> = carriers
                .iter()
                .filter(|(_, k)| match (*c, *k) {
                    (k::Curve2::Line { a, b }, k::Curve2::Line { a: p, b: q }) => {
                        on_line(a, p, q) && on_line(b, p, q)
                    }
                    (k::Curve2::Arc { c, a, .. }, k::Curve2::Circle { c: cc, r: rr }) => {
                        (c[0] - cc[0]).hypot(c[1] - cc[1]) <= t
                            && ((a[0] - c[0]).hypot(a[1] - c[1]) - rr).abs() <= t
                    }
                    (k::Curve2::Circle { c, r }, k::Curve2::Circle { c: cc, r: rr }) => {
                        (c[0] - cc[0]).hypot(c[1] - cc[1]) <= t && (r - rr).abs() <= t
                    }
                    _ => false,
                })
                .map(|(e, _)| *e)
                .collect();
            if let [only] = hits.as_slice() {
                *tag = Some(*only);
            }
        }
    }
}

/// A boundary loop made only of arcs of one circle (the kernel splits a
/// full circle into arcs) as that full circle: a picked hole rim projects as
/// one circle.
fn merged_circle(l: &[k::Curve2]) -> Option<k::Curve2> {
    let mut circle: Option<([f64; 2], f64)> = None;
    for c in l {
        let k::Curve2::Arc { c, a, .. } = *c else {
            return None;
        };
        let r = (a[0] - c[0]).hypot(a[1] - c[1]);
        match circle {
            None => circle = Some((c, r)),
            Some((c0, r0)) => {
                if (c[0] - c0[0]).hypot(c[1] - c0[1]) > k::tol::KERNEL_LINEAR
                    || (r - r0).abs() > k::tol::KERNEL_LINEAR
                {
                    return None;
                }
            }
        }
    }
    circle.map(|(c, r)| k::Curve2::Circle { c, r })
}

/// Source curves (in `src`'s 2-D coordinates) projected along the sketch
/// normal into the sketch on `frame`. Lines stay lines; arcs and circles
/// stay circular on a parallel plane; on an oblique plane a circle becomes
/// the exact ellipse with the principal axes of `r [X Y]` (`X`, `Y` the
/// projected unit axes of the source plane: one semi-axis is `r`, the other
/// `r |cos φ|`); an arc there would be an elliptical arc (refused), and a
/// plane perpendicular to the sketch flattens circles to segments (refused).
fn project_oblique(
    id: FeatureId,
    src: &Frame,
    frame: &Frame,
    loops: SourceCurves,
) -> Result<SourceCurves> {
    let cosine = dot(src.z, frame.z);
    let refuse = |why: &str| DocError::ProductLimit(format!("{id}: projected edge: {why}"));
    let to2 = |p: [f64; 2]| {
        let w = sub(src.to_world(p), frame.origin);
        [dot(w, frame.x), dot(w, frame.y)]
    };
    let parallel = cosine.abs() >= 1.0 - 1e-9;
    let flip = cosine < 0.0;
    let (ax, ay) = (
        [dot(src.x, frame.x), dot(src.x, frame.y)],
        [dot(src.y, frame.x), dot(src.y, frame.y)],
    );
    let mut out = Vec::new();
    for l in loops {
        let mut ol = Vec::new();
        for (c, e) in l {
            let c = match c {
                k::Curve2::Line { a, b } => k::Curve2::Line {
                    a: to2(a),
                    b: to2(b),
                },
                k::Curve2::Arc { c, a, b, ccw } if parallel => k::Curve2::Arc {
                    c: to2(c),
                    a: to2(a),
                    b: to2(b),
                    ccw: ccw != flip,
                },
                k::Curve2::Circle { c, r } if parallel => k::Curve2::Circle { c: to2(c), r },
                // Refused only when picked or bound (see `UNPROJECTABLE`).
                k::Curve2::Arc { a, b, .. } => {
                    ol.push((
                        k::Curve2::Line {
                            a: to2(a),
                            b: to2(b),
                        },
                        Some(UNPROJECTABLE),
                    ));
                    continue;
                }
                k::Curve2::Circle { c, r } => {
                    // M = r [ax ay]; M Mᵀ = r² [[p, q], [q, t]].
                    let (p, q, t) = (
                        ax[0] * ax[0] + ay[0] * ay[0],
                        ax[0] * ax[1] + ay[0] * ay[1],
                        ax[1] * ax[1] + ay[1] * ay[1],
                    );
                    let (m, d) = ((p + t) / 2.0, (((p - t) / 2.0).powi(2) + q * q).sqrt());
                    let (l1, l2) = (m + d, (m - d).max(0.0));
                    if l2.sqrt() < 1e-9 {
                        return Err(refuse(
                            "a circle on a face perpendicular to the sketch projects to a segment",
                        ));
                    }
                    let v = if d <= 1e-15 {
                        [1.0, 0.0]
                    } else if (l1 - p).abs() >= (l1 - t).abs() {
                        [q, l1 - p]
                    } else {
                        [l1 - t, q]
                    };
                    let n = v[0].hypot(v[1]);
                    let cc = to2(c);
                    let a = r * l1.sqrt();
                    k::Curve2::Ellipse {
                        c: cc,
                        major: [cc[0] + a * v[0] / n, cc[1] + a * v[1] / n],
                        minor: r * l2.sqrt(),
                    }
                }
                k::Curve2::Ellipse { c, major, minor } if parallel => k::Curve2::Ellipse {
                    c: to2(c),
                    major: to2(major),
                    minor,
                },
                k::Curve2::Ellipse { .. } => {
                    return Err(refuse(
                        "an ellipse on a face oblique to the sketch is not projected (only \
                         parallel ellipses are)",
                    ));
                }
            };
            ol.push((c, e));
        }
        out.push(ol);
    }
    Ok(out)
}

/// The points a curve pins: line a, b; arc centre, start, end (sketch arcs
/// run counter-clockwise from start to end); circle centre and rim point.
fn curve_points(c: &k::Curve2) -> Vec<[f64; 2]> {
    match *c {
        k::Curve2::Line { a, b } => vec![a, b],
        k::Curve2::Arc { c, a, b, ccw } => {
            if ccw {
                vec![c, a, b]
            } else {
                vec![c, b, a]
            }
        }
        k::Curve2::Circle { c, r } => vec![c, [c[0] + r, c[1]]],
        k::Curve2::Ellipse { c, major, .. } => vec![c, major],
    }
}

fn curve_kind(c: &k::Curve2) -> u8 {
    match c {
        k::Curve2::Line { .. } => 0,
        k::Curve2::Arc { .. } => 1,
        k::Curve2::Circle { .. } => 2,
        k::Curve2::Ellipse { .. } => 3,
    }
}

/// Move the pinned points of `link` in `s` to the source curves. Sketch
/// sources match by source entity; face sources loop by loop (same count and
/// sequence of curve kinds, up to where the loop starts), choosing the
/// rotation nearest to the stored positions. Anything else is a
/// BrokenReference naming the link.
fn bind_link(
    id: FeatureId,
    li: usize,
    link: &ProjectionLink,
    curves: &[Vec<(k::Curve2, Option<EntityId>)>],
    s: &mut rcad_sketch::Sketch,
) -> Result<()> {
    let broken = |why: String| DocError::BrokenReference {
        from: id,
        detail: format!(
            "projection {} (from {}) no longer matches its source: {why}; unlink it or project \
             again",
            li + 1,
            link.source_feature()
        ),
    };
    // Stored pin of a projected point: (x, y), either may be absent
    // (FixedX / FixedY pin one coordinate).
    let stored = |p: rcad_sketch::PointId, s: &rcad_sketch::Sketch| {
        let (mut x, mut y) = (None, None);
        for c in &s.constraints {
            match c {
                rcad_sketch::Constraint::FixedPoint { p: q, x: a, y: b } if *q == p => {
                    (x, y) = (Some(*a), Some(*b));
                }
                rcad_sketch::Constraint::FixedX { p: q, x: a } if *q == p => x = Some(*a),
                rcad_sketch::Constraint::FixedY { p: q, y: b } if *q == p => y = Some(*b),
                _ => {}
            }
        }
        (x, y)
    };
    // Assignment: for every linked curve, its new geometry.
    let mut assigned: Vec<(&LinkedCurve, k::Curve2)> = Vec::new();
    match link.source {
        ProjectSource::Sketch(_) => {
            let all: Vec<&(k::Curve2, Option<EntityId>)> = curves.iter().flatten().collect();
            for lc in link.loops.iter().flatten() {
                let from = lc
                    .from
                    .ok_or_else(|| broken("no source entity recorded".into()))?;
                let (c, _) = all
                    .iter()
                    .find(|(_, e)| *e == Some(from))
                    .ok_or_else(|| broken(format!("source entity {from:?} was removed")))?;
                assigned.push((lc, *c));
            }
        }
        ProjectSource::Edge(_) => {
            // One curve. Identity first: an extrusion cap edge carries the
            // profile entity that generates it (`annotate_cap_edges`); it must
            // be exactly one current edge of the same kind. Otherwise (other
            // faces, untagged edges) the nearest same-kind edge to the pins,
            // in either direction, only when clearly nearer than any other
            // (at most half the runner-up's distance) and not moved more than
            // its own size; anything else is a broken reference.
            for lc in link.loops.iter().flatten() {
                let kind = match s.entities.iter().find(|x| x.id() == lc.entity) {
                    Some(rcad_sketch::Entity::Line { .. }) => 0u8,
                    Some(rcad_sketch::Entity::Arc { .. }) => 1,
                    Some(rcad_sketch::Entity::Circle { .. }) => 2,
                    Some(rcad_sketch::Entity::Ellipse { .. }) => 3,
                    _ => return Err(broken(format!("{:?} is not a projected curve", lc.entity))),
                };
                let same: Vec<&(k::Curve2, Option<EntityId>)> = curves
                    .iter()
                    .flatten()
                    .filter(|(c, e)| curve_kind(c) == kind && *e != Some(UNPROJECTABLE))
                    .collect();
                let pin_dist = |c: &k::Curve2| -> f64 {
                    curve_points(c)
                        .iter()
                        .zip(&lc.points)
                        .map(|(p, q)| match stored(*q, s) {
                            (Some(x), Some(y)) => (p[0] - x).hypot(p[1] - y),
                            (Some(x), None) => (p[0] - x).abs(),
                            (None, Some(y)) => (p[1] - y).abs(),
                            (None, None) => 0.0,
                        })
                        .sum()
                };
                // A line may be found reversed: keep the pins' direction.
                let oriented = |c: k::Curve2| -> (f64, k::Curve2) {
                    match c {
                        k::Curve2::Line { a, b } => {
                            let r = k::Curve2::Line { a: b, b: a };
                            let (d0, d1) = (pin_dist(&c), pin_dist(&r));
                            if d1 < d0 { (d1, r) } else { (d0, c) }
                        }
                        other => (pin_dist(&other), other),
                    }
                };
                if let Some(from) = lc.from {
                    let hits: Vec<k::Curve2> = same
                        .iter()
                        .filter(|(_, e)| *e == Some(from))
                        .map(|(c, _)| oriented(*c).1)
                        .collect();
                    match hits.as_slice() {
                        [c] => assigned.push((lc, *c)),
                        [] => {
                            return Err(broken(format!(
                                "the face no longer has the edge generated by sketch entity \
                                 {from:?}"
                            )));
                        }
                        many => {
                            return Err(broken(format!(
                                "the edge generated by sketch entity {from:?} is now {} separate \
                                 edges; which one to follow is ambiguous",
                                many.len()
                            )));
                        }
                    }
                    continue;
                }
                let mut ranked: Vec<(f64, k::Curve2)> =
                    same.iter().map(|(c, _)| oriented(*c)).collect();
                ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
                let Some(&(bd, c)) = ranked.first() else {
                    return Err(broken("the face has no edge of that kind any more".into()));
                };
                let size = match c {
                    k::Curve2::Line { a, b } => (b[0] - a[0]).hypot(b[1] - a[1]),
                    k::Curve2::Arc { c, a, .. } => (a[0] - c[0]).hypot(a[1] - c[1]),
                    k::Curve2::Circle { r, .. } => r,
                    k::Curve2::Ellipse { minor, .. } => minor,
                };
                let clear = ranked
                    .get(1)
                    .is_none_or(|(sd, _)| bd <= 0.5 * sd && sd - bd > k::tol::KERNEL_LINEAR);
                if !clear || bd > size {
                    return Err(broken(
                        "the projected edge cannot be told apart from another edge of the face \
                         after the change (no generating entity; the nearest edge is not clearly \
                         nearer); unlink it or project again"
                            .into(),
                    ));
                }
                assigned.push((lc, c));
            }
        }
        ProjectSource::Face(_) | ProjectSource::Section { .. } => {
            if curves.len() != link.loops.len() {
                return Err(broken(format!(
                    "the face had {} boundary loop(s), now {}",
                    link.loops.len(),
                    curves.len()
                )));
            }
            let mut used = vec![false; curves.len()];
            let entity_kind = |e: EntityId| match s.entities.iter().find(|x| x.id() == e) {
                Some(rcad_sketch::Entity::Line { .. }) => Some(0u8),
                Some(rcad_sketch::Entity::Arc { .. }) => Some(1),
                Some(rcad_sketch::Entity::Circle { .. }) => Some(2),
                Some(rcad_sketch::Entity::Ellipse { .. }) => Some(3),
                Some(rcad_sketch::Entity::Spline { .. }) => Some(4),
                None => None,
            };
            for ll in &link.loops {
                let kinds: Vec<Option<u8>> = ll.iter().map(|lc| entity_kind(lc.entity)).collect();
                // Best and second-best placement (distance, loop, rotation).
                let mut best: Option<(f64, usize, usize)> = None;
                let mut second: Option<f64> = None;
                for (ci, cl) in curves.iter().enumerate() {
                    if used[ci] || cl.len() != ll.len() {
                        continue;
                    }
                    let n = cl.len();
                    for rot in 0..n {
                        let ok = (0..n).all(|j| Some(curve_kind(&cl[(j + rot) % n].0)) == kinds[j]);
                        if !ok {
                            continue;
                        }
                        let mut d = 0.0;
                        for j in 0..n {
                            let pts = curve_points(&cl[(j + rot) % n].0);
                            for (p, q) in pts.iter().zip(&ll[j].points) {
                                d += match stored(*q, s) {
                                    (Some(x), Some(y)) => (p[0] - x).hypot(p[1] - y),
                                    (Some(x), None) => (p[0] - x).abs(),
                                    (None, Some(y)) => (p[1] - y).abs(),
                                    (None, None) => 0.0,
                                };
                            }
                        }
                        match best {
                            Some((bd, _, _)) if d >= bd => {
                                if second.is_none_or(|sd| d < sd) {
                                    second = Some(d);
                                }
                            }
                            _ => {
                                if let Some((bd, _, _)) = best {
                                    second = Some(bd);
                                }
                                best = Some((d, ci, rot));
                            }
                        }
                    }
                }
                let (bd, ci, rot) = best.ok_or_else(|| {
                    broken("a boundary loop changed its number or kinds of edges".into())
                })?;
                // Two placements equally close to the stored points (a
                // symmetric outline with too few pins): which sketch curve
                // drives which edge would depend on the kernel's loop order.
                if second.is_some_and(|sd| sd - bd <= k::tol::KERNEL_LINEAR) {
                    return Err(broken(
                        "a boundary loop matches its stored points equally well in two \
                         placements (symmetric outline); the edge correspondence is ambiguous"
                            .into(),
                    ));
                }
                used[ci] = true;
                let n = ll.len();
                for (j, lc) in ll.iter().enumerate() {
                    assigned.push((lc, curves[ci][(j + rot) % n].0));
                }
            }
        }
    }
    for (lc, c) in assigned {
        let pts = curve_points(&c);
        if pts.len() != lc.points.len() {
            return Err(broken(format!("{:?} changed kind", lc.entity)));
        }
        for (p, q) in pts.iter().zip(&lc.points) {
            let mut pinned = false;
            for c in s.constraints.iter_mut() {
                match c {
                    rcad_sketch::Constraint::FixedPoint { p: r, x, y } if r == q => {
                        (*x, *y) = (p[0], p[1]);
                        pinned = true;
                    }
                    rcad_sketch::Constraint::FixedX { p: r, x } if r == q => {
                        *x = p[0];
                        pinned = true;
                    }
                    rcad_sketch::Constraint::FixedY { p: r, y } if r == q => {
                        *y = p[1];
                        pinned = true;
                    }
                    _ => {}
                }
            }
            if !pinned {
                return Err(broken(format!(
                    "pinned point {q:?} lost its fixed constraint"
                )));
            }
            if let Some(sp) = s.points.iter_mut().find(|sp| sp.id == *q) {
                (sp.x, sp.y) = (p[0], p[1]);
            }
        }
        if let k::Curve2::Circle { r, .. } = c
            && let Some(rcad_sketch::Entity::Circle { radius, .. }) =
                s.entities.iter_mut().find(|e| e.id() == lc.entity)
        {
            *radius = r;
        }
        if let k::Curve2::Ellipse { minor: b, .. } = c {
            if let Some(rcad_sketch::Entity::Ellipse { minor, .. }) =
                s.entities.iter_mut().find(|e| e.id() == lc.entity)
            {
                *minor = b;
            }
            for k in s.constraints.iter_mut() {
                if let rcad_sketch::Constraint::EllipseRadius {
                    ellipse,
                    major: false,
                    dim,
                } = k
                    && *ellipse == lc.entity
                {
                    dim.value = b;
                }
            }
        }
    }
    Ok(())
}

/// Source curves for a new projection into sketch `sketch` (the history
/// state just before it, in its frame).
pub fn projection_curves(
    doc: &Document,
    sketch: FeatureId,
    source: &ProjectSource,
) -> Result<SourceCurves> {
    let FeatureKind::Sketch { plane, .. } = &doc.feature(sketch)?.kind else {
        return Err(DocError::Validation(format!("{sketch} is not a sketch")));
    };
    // The source must come first in history (no cycle).
    let pos = |f: FeatureId| doc.features.iter().position(|x| x.id == f);
    let src = match source {
        ProjectSource::Face(r) | ProjectSource::Edge(r) => r.feature,
        ProjectSource::Sketch(f) => *f,
        ProjectSource::Section { after, .. } => *after,
    };
    if src == sketch || pos(src) > pos(sketch) || doc.with_dependents(sketch).contains(&src) {
        return Err(DocError::Validation(format!(
            "{sketch} cannot project {src}: it comes later in history or depends on the sketch \
             (a cycle)"
        )));
    }
    match regen_impl(doc, None, false, Some(sketch), false)? {
        Regen::Stopped(st) => {
            let (frame, _) = plane_frame(sketch, plane, &st)?;
            link_curves(sketch, &frame, source, &st)
        }
        Regen::Done(_) => Err(DocError::Validation(format!("{sketch} was not evaluated"))),
    }
}

fn resolve_face(from: FeatureId, fr: &FaceRef, st: &Snapshot) -> Result<ResolvedFace> {
    // The generating tool and its extrusion role, for telling coplanar faces
    // apart by the tool's own footprint.
    let mut tool_of: Option<(&ExtrudeOut, FaceRole)> = None;
    let (frame, owner) = match fr.role {
        FaceRole::Imported { body, face } => {
            if let Some(why) = st.import_moved.get(&(fr.feature, body, face)) {
                return Err(DocError::BrokenReference {
                    from,
                    detail: why.clone(),
                });
            }
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
        FaceRole::Copy { instance, tool, of } => {
            let e = copy_occurrence(st, from, fr.feature, instance, tool)?;
            let role = FaceRef {
                feature: fr.feature,
                role: of.extrude_role(),
            };
            let out = (extrude_face_frame(e, &role, from)?, e.body);
            tool_of = Some((e, role.role));
            out
        }
        FaceRole::Chamfer => *st
            .chamfers
            .get(&fr.feature)
            .ok_or(DocError::BrokenReference {
                from,
                detail: format!("{} is not an evaluated chamfer", fr.feature),
            })?,
        _ => {
            let e = st
                .extrudes
                .get(&fr.feature)
                .ok_or(DocError::BrokenReference {
                    from,
                    detail: format!("{} is not an evaluated extrusion", fr.feature),
                })?;
            tool_of = Some((e, fr.role));
            (extrude_face_frame(e, fr, from)?, e.body)
        }
    };
    let tool = tool_of;
    // Faces made before a move of their body are found moved with it.
    let moved = st.placement(owner, fr.feature);
    let frame = moved.frame(&frame);
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
    let in_plane = k::planar_faces_on(body, &frame);
    let mut n = in_plane;
    let mut sample = None;
    let mut gone_from_footprint = false;
    if n > 0
        && let Some((e, role)) = tool
    {
        // The face is the one this tool's footprint overlaps,
        // also when the plane holds one face, which may be another tool's —
        // e.g. an occurrence's floor cut away and the seed's still there.
        // The footprint is computed where the tool was made, then moved.
        let (outline, curved) = footprint(e, role, &moved_back(&moved, &frame));
        let outline: Vec<[f64; 3]> = outline.into_iter().map(|p| moved.point(p)).collect();
        if polygon_area(&outline) > 0.0 {
            let (hits, inside) = k::planar_faces_overlapping(body, &frame, &outline, curved)
                .map_err(kernel(from))?;
            n = hits;
            gone_from_footprint = hits == 0;
            if hits == 1 && in_plane > 1 {
                sample = inside;
            }
        }
    }
    // A later cut that split `owner` into pieces may have divided this face
    // between the bodies: count the piece bodies' faces in the plane (where
    // the tool made it) too, so such a face is ambiguous, not silently the
    // target's fragment.
    let mut on_pieces = 0;
    if n > 0 {
        for (bid, bs) in &st.bodies {
            let split_off = *bid != owner
                && bs.created_by != fr.feature
                && st
                    .extrudes
                    .get(&bs.created_by)
                    .is_some_and(|e| e.body == owner);
            let BodyGeometry::Brep(pb) = &bs.geometry else {
                continue;
            };
            if !split_off || k::planar_faces_on(pb, &frame) == 0 {
                continue;
            }
            on_pieces += match tool {
                Some((e, role)) => {
                    let (outline, curved) = footprint(e, role, &moved_back(&moved, &frame));
                    let outline: Vec<[f64; 3]> =
                        outline.into_iter().map(|p| moved.point(p)).collect();
                    if polygon_area(&outline) > 0.0 {
                        k::planar_faces_overlapping(pb, &frame, &outline, curved)
                            .map_err(kernel(from))?
                            .0
                    } else {
                        k::planar_faces_on(pb, &frame)
                    }
                }
                None => k::planar_faces_on(pb, &frame),
            };
        }
    }
    let role = match fr.role {
        FaceRole::ExtrudeSide { entity } => format!("side face of entity {entity}"),
        FaceRole::Imported { face, .. } => format!("imported face {face}"),
        FaceRole::Copy { instance, of, .. } => {
            format!("{of:?} face of occurrence {}", instance + 1)
        }
        r => format!("{r:?}"),
    };
    if gone_from_footprint {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "the {role} of {} no longer exists on body {owner}: {in_plane} face(s) lie in \
                 its plane, none where its own tool made it (removed or replaced by a later \
                 feature); re-attach to an existing face",
                fr.feature
            ),
        });
    }
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
    if on_pieces > 0 {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "the {role} of {} is ambiguous: a later cut split body {owner} into separate \
                 bodies and the face now lies on {} of them; refusing to guess — re-attach \
                 the sketch",
                fr.feature,
                on_pieces + n
            ),
        });
    }
    if n > 1 {
        return Err(DocError::BrokenReference {
            from,
            detail: format!(
                "the {role} of {} is ambiguous: {n} faces of body {owner} lie in its plane \
                 where its tool made it (a split face); refusing to guess — re-attach the sketch",
                fr.feature
            ),
        });
    }
    Ok(ResolvedFace {
        reference: *fr,
        frame,
        matching_brep_faces: n,
        body: owner,
        sample,
    })
}
