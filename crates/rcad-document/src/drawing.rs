//! Associative drawings (run 6, SESSION_7 §4 foundation; docs/DRAWINGS.md).
//!
//! A [`Drawing`] is stored in the document (format v6): sheet size, projection
//! convention, title block fields and views. Views reference bodies by
//! [`BodyId`] and carry only placement (orientation, scale, position) and
//! semantic annotations; nothing geometric is stored. Every line, dimension
//! and centre mark is computed by [`render`] from the *current* regenerated
//! geometry at export time, so a stale dimension cannot exist: nothing is
//! cached. A view whose body no longer exists is reported and drawn as a red
//! flag, never redrawn from old data; a hole callout whose feature was
//! deleted, suppressed or no longer appears end-on is reported as dangling and
//! drawn in red without a value.
//!
//! Hidden-line removal is *sampled*: every sharp B-rep edge (exact lines and
//! arcs from `rcad_kernel::sharp_edges`) and every silhouette is sampled at
//! [`sample_spacing`] and each sample is tested against a tessellation of the
//! view's bodies along the view direction (chord [`hlr_chord`]); transitions
//! between visible and hidden samples are refined by bisection to
//! [`TRANSITION_TOL`]. The drawn curves are the exact projected edges cut at
//! those transitions. See docs/DRAWINGS.md for the stated tolerances.
//!
//! Section views cut the bodies with the exact Split Body cut
//! (`regen::split_by_plane`), draw the part behind the plane the same way and
//! hatch the faces lying in the plane ([`HATCH_SPACING`]).
use crate::{BodyId, DocError, Document, Evaluation, FeatureId, FeatureKind, Result};
use rcad_exchange::drawing::{Anchor, Group, LineStyle, Prim, SheetPlot};
use rcad_kernel::{self as k, EdgeCurve};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::fmt;

/// Identifier of a drawing in a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DrawingId(pub u32);

impl fmt::Display for DrawingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// Identifier of a view within a drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ViewId(pub u32);

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "V{}", self.0)
    }
}

/// Sheet size (landscape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetSize {
    #[default]
    A4Landscape,
    A3Landscape,
}

impl SheetSize {
    /// Width and height in mm.
    pub fn mm(self) -> (f64, f64) {
        match self {
            SheetSize::A4Landscape => (297.0, 210.0),
            SheetSize::A3Landscape => (420.0, 297.0),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SheetSize::A4Landscape => "A4",
            SheetSize::A3Landscape => "A3",
        }
    }
}

/// Projection convention, recorded on the sheet (title block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// ISO E / ASME: the view from above is placed above, from the right to
    /// the right.
    #[default]
    ThirdAngle,
    /// ISO A: the view from above is placed below, from the right to the left.
    FirstAngle,
}

impl Projection {
    pub fn label(self) -> &'static str {
        match self {
            Projection::ThirdAngle => "THIRD ANGLE PROJECTION",
            Projection::FirstAngle => "FIRST ANGLE PROJECTION",
        }
    }
}

/// Standard orientation of a base view (Z up, the viewport's presets).
///
/// | view   | looks along | right | up  |
/// |--------|-------------|-------|-----|
/// | Front  | +Y          | +X    | +Z  |
/// | Back   | -Y          | -X    | +Z  |
/// | Top    | -Z          | +X    | +Y  |
/// | Bottom | +Z          | +X    | -Y  |
/// | Right  | -X          | +Y    | +Z  |
/// | Left   | +X          | -Y    | +Z  |
/// | Iso    | (-1, 1, -1) | (1, 1, 0) | Z-ish |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Front,
    Back,
    Top,
    Bottom,
    Right,
    Left,
    Iso,
}

impl Orientation {
    pub const ALL: [Orientation; 7] = [
        Orientation::Front,
        Orientation::Top,
        Orientation::Right,
        Orientation::Left,
        Orientation::Back,
        Orientation::Bottom,
        Orientation::Iso,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Orientation::Front => "Front",
            Orientation::Back => "Back",
            Orientation::Top => "Top",
            Orientation::Bottom => "Bottom",
            Orientation::Right => "Right",
            Orientation::Left => "Left",
            Orientation::Iso => "Iso",
        }
    }
    /// The view frame.
    pub fn frame(self) -> ViewFrame {
        let (r, u) = match self {
            Orientation::Front => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Back => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Top => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Orientation::Bottom => ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            Orientation::Right => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Left => ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Iso => {
                let s = std::f64::consts::FRAC_1_SQRT_2;
                let toward = norm([1.0, -1.0, 1.0]);
                let r = [s, s, 0.0];
                (r, cross(toward, r))
            }
        };
        ViewFrame::from_right_up(r, u)
    }
}

/// Which side the parent is seen from in a projected view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Above,
    Below,
    Left,
    Right,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Above => "from above",
            Side::Below => "from below",
            Side::Left => "from the left",
            Side::Right => "from the right",
        }
    }
    /// Unit direction on the sheet where the projected view is placed
    /// relative to its parent under `p`.
    pub fn placement(self, p: Projection) -> [f64; 2] {
        let d = match self {
            Side::Above => [0.0, 1.0],
            Side::Below => [0.0, -1.0],
            Side::Left => [-1.0, 0.0],
            Side::Right => [1.0, 0.0],
        };
        match p {
            Projection::ThirdAngle => d,
            Projection::FirstAngle => [-d[0], -d[1]],
        }
    }
}

/// Orthonormal view frame: `right`, `up` on the sheet, `toward` the viewer
/// (`right × up = toward`; the view looks along `-toward`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewFrame {
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub toward: [f64; 3],
}

impl ViewFrame {
    fn from_right_up(r: [f64; 3], u: [f64; 3]) -> Self {
        let r = norm(r);
        let u = norm(u);
        ViewFrame {
            right: r,
            up: u,
            toward: cross(r, u),
        }
    }
    /// The parent seen from `side` (orthographic projection by 90°).
    pub fn seen_from(&self, side: Side) -> ViewFrame {
        let neg = |v: [f64; 3]| v.map(|x| -x);
        let (toward, up) = match side {
            Side::Above => (self.up, neg(self.toward)),
            Side::Below => (neg(self.up), self.toward),
            Side::Right => (self.right, self.up),
            Side::Left => (neg(self.right), self.up),
        };
        ViewFrame {
            right: cross(up, toward),
            up,
            toward,
        }
    }
    /// View coordinates (x right, y up, z toward the viewer), model mm.
    pub fn project(&self, p: [f64; 3]) -> [f64; 3] {
        [dot(p, self.right), dot(p, self.up), dot(p, self.toward)]
    }
    /// Named standard direction of `toward`, if axis-aligned.
    pub fn name(&self) -> &'static str {
        let t = self.toward;
        let names = [
            ([0.0, -1.0, 0.0], "Front"),
            ([0.0, 1.0, 0.0], "Back"),
            ([0.0, 0.0, 1.0], "Top"),
            ([0.0, 0.0, -1.0], "Bottom"),
            ([1.0, 0.0, 0.0], "Right"),
            ([-1.0, 0.0, 0.0], "Left"),
        ];
        names
            .iter()
            .find(|(d, _)| dot(*d, t) > 1.0 - 1e-9)
            .map(|(_, n)| *n)
            .unwrap_or("Iso")
    }
    /// World axis index of `v` when it is axis-aligned.
    fn axis_of(v: [f64; 3]) -> Option<usize> {
        (0..3).find(|&i| v[i].abs() > 1.0 - 1e-12)
    }
}

/// What a view shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// Bodies seen in a standard orientation at `scale` (sheet mm per model
    /// mm), centred at `position` (sheet mm from the bottom-left corner).
    Base {
        bodies: Vec<BodyId>,
        orientation: Orientation,
        scale: f64,
        position: [f64; 2],
    },
    /// The parent seen from `from`, at the parent's scale, aligned with it and
    /// placed by the drawing's projection convention.
    Projected { parent: ViewId, from: Side },
    /// Full section `label`-`label`: the parent cut by the plane through its
    /// cutting line and seen from `from` (placed like a projected view). The
    /// cutting line lies in the parent view at `offset` model mm along the
    /// parent's up axis (`from` above/below: a horizontal line) or right axis
    /// (`from` left/right: a vertical line), in world coordinates projected on
    /// that axis. The material between the plane and the viewer is removed;
    /// faces in the plane are hatched.
    Section {
        parent: ViewId,
        from: Side,
        offset: f64,
        label: String,
    },
}

impl ViewKind {
    /// The parent and side of a projected or section view.
    pub fn parent(&self) -> Option<(ViewId, Side)> {
        match self {
            ViewKind::Base { .. } => None,
            ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                Some((*parent, *from))
            }
        }
    }
}

/// A semantic annotation of a view, resolved at every render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Annotation {
    /// Diameter callout of Hole feature `feature`, attached to its circle
    /// where the hole is seen end-on.
    HoleCallout { feature: FeatureId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingView {
    pub id: ViewId,
    pub kind: ViewKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

/// Title block fields. Empty `part_number` / `material`: taken from the
/// model (the active configuration's part number; the bodies' materials).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TitleBlock {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub part_number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub material: String,
}

/// A drawing sheet (format v6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub name: String,
    #[serde(default)]
    pub sheet: SheetSize,
    #[serde(default)]
    pub projection: Projection,
    #[serde(default)]
    pub title: TitleBlock,
    #[serde(default)]
    pub views: Vec<DrawingView>,
    #[serde(default = "one")]
    pub next_view: u32,
    /// Overall bounding dimensions (each world axis once) and hole diameters.
    #[serde(default = "yes")]
    pub auto_dimensions: bool,
}

fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

/// Accepted scale range (sheet mm per model mm).
pub const MIN_SCALE: f64 = 1.0e-3;
pub const MAX_SCALE: f64 = 1.0e3;
/// Default sheet gap (mm) between a view's outline and a projected view's.
pub const VIEW_GAP: f64 = 22.0;

impl Drawing {
    pub fn new(id: DrawingId, name: &str) -> Self {
        Drawing {
            id,
            name: name.to_string(),
            sheet: SheetSize::default(),
            projection: Projection::default(),
            title: TitleBlock {
                title: name.to_string(),
                ..Default::default()
            },
            views: Vec::new(),
            next_view: 1,
            auto_dimensions: true,
        }
    }

    pub fn view(&self, id: ViewId) -> Result<&DrawingView> {
        self.views
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| DocError::Validation(format!("{} has no view {id}", self.id)))
    }

    pub fn view_mut(&mut self, id: ViewId) -> Result<&mut DrawingView> {
        let did = self.id;
        self.views
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or_else(|| DocError::Validation(format!("{did} has no view {id}")))
    }

    /// Frame, scale and the base view of `id` (following projected parents).
    pub fn resolve(&self, id: ViewId) -> Result<(ViewFrame, f64, ViewId)> {
        let mut chain = Vec::new();
        let mut cur = id;
        loop {
            if chain.len() > self.views.len() {
                return Err(DocError::Validation(format!(
                    "{}: projected views form a cycle at {id}",
                    self.id
                )));
            }
            let v = self.view(cur)?;
            match &v.kind {
                ViewKind::Base {
                    orientation, scale, ..
                } => {
                    let mut f = orientation.frame();
                    for side in chain.iter().rev() {
                        f = f.seen_from(*side);
                    }
                    return Ok((f, *scale, cur));
                }
                ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                    chain.push(*from);
                    cur = *parent;
                }
            }
        }
    }

    /// Bodies shown by view `id` (a projected view shows its base's bodies).
    pub fn bodies(&self, id: ViewId) -> Result<Vec<BodyId>> {
        let (_, _, base) = self.resolve(id)?;
        match &self.view(base)?.kind {
            ViewKind::Base { bodies, .. } => Ok(bodies.clone()),
            _ => unreachable!("resolve returns a base view"),
        }
    }

    /// Structural checks (every edit): finite scale/position in range, at
    /// least one body per base view, parents earlier in the list, no two
    /// projected views of one parent from the same side.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(DocError::Validation(format!("{}: {m}", self.id)));
        if self.name.trim().is_empty() {
            return bad("the drawing needs a name".into());
        }
        let (w, h) = self.sheet.mm();
        for (i, v) in self.views.iter().enumerate() {
            if self.views[..i].iter().any(|o| o.id == v.id) {
                return bad(format!("duplicate view id {}", v.id));
            }
            match &v.kind {
                ViewKind::Base {
                    bodies,
                    scale,
                    position,
                    ..
                } => {
                    if bodies.is_empty() {
                        return bad(format!("{} shows no body", v.id));
                    }
                    if !(scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(scale)) {
                        return bad(format!(
                            "{} scale must be in {MIN_SCALE}..={MAX_SCALE}, got {scale}",
                            v.id
                        ));
                    }
                    if !(position[0].is_finite()
                        && position[1].is_finite()
                        && (0.0..=w).contains(&position[0])
                        && (0.0..=h).contains(&position[1]))
                    {
                        return bad(format!(
                            "{} position {position:?} is outside the {} sheet",
                            v.id,
                            self.sheet.label()
                        ));
                    }
                }
                ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                    let Some(pv) = self.views[..i].iter().find(|o| o.id == *parent) else {
                        return bad(format!(
                            "{} projects from {parent}, which is not an earlier view",
                            v.id
                        ));
                    };
                    if matches!(pv.kind, ViewKind::Section { .. }) {
                        return bad(format!(
                            "{} cannot be derived from the section view {parent}",
                            v.id
                        ));
                    }
                    let section = matches!(v.kind, ViewKind::Section { .. });
                    if !section
                        && self.views[..i].iter().any(|o| {
                            matches!(o.kind, ViewKind::Projected { .. })
                                && o.kind.parent() == Some((*parent, *from))
                        })
                    {
                        return bad(format!("{parent} already has a view {}", from.label()));
                    }
                    if let ViewKind::Section { offset, label, .. } = &v.kind {
                        if !offset.is_finite() {
                            return bad(format!("{} section offset must be finite", v.id));
                        }
                        if label.is_empty()
                            || label.chars().count() > 3
                            || !label.chars().all(|c| c.is_ascii_alphanumeric())
                        {
                            return bad(format!(
                                "{} section label must be 1 to 3 letters/numbers, got '{label}'",
                                v.id
                            ));
                        }
                        if self.views[..i].iter().any(
                            |o| matches!(&o.kind, ViewKind::Section { label: l, .. } if l == label),
                        ) {
                            return bad(format!("section {label} already exists"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Document {
    pub fn drawing(&self, id: DrawingId) -> Result<&Drawing> {
        self.drawings
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown drawing {id}")))
    }
    pub fn drawing_mut(&mut self, id: DrawingId) -> Result<&mut Drawing> {
        self.drawings
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown drawing {id}")))
    }
    /// Next free drawing id.
    pub fn next_drawing_id(&self) -> DrawingId {
        DrawingId(self.drawings.iter().map(|d| d.id.0).max().unwrap_or(0) + 1)
    }
}

// ---------------------------------------------------------------- rendering

/// Bisection tolerance (model mm) on the position of a visible/hidden
/// transition along an edge.
pub const TRANSITION_TOL: f64 = 1.0e-4;

/// Chord tolerance (mm) of the occlusion tessellation for a part of bounding
/// diagonal `diag`: 2e-4 × diag, at least the kernel minimum.
pub fn hlr_chord(diag: f64) -> f64 {
    (diag * 2.0e-4)
        .max(k::tol::MIN_MESH_CHORD_ABS)
        .max(diag * k::tol::MIN_MESH_CHORD_REL * 2.0)
}

/// Sample spacing (model mm) along edges for the visibility test: diag/400,
/// at least 0.02 mm.
pub fn sample_spacing(diag: f64) -> f64 {
    (diag / 400.0).max(0.02)
}

/// How a view rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ViewStatus {
    Ok,
    /// Some or all of the view's bodies no longer exist (or are not
    /// evaluated): they are not drawn; the view shows a red flag.
    MissingBodies {
        bodies: Vec<BodyId>,
    },
    /// Bodies without exact edges (mesh bodies) are not drawn.
    Unsupported {
        bodies: Vec<BodyId>,
        reason: String,
    },
}

/// One dimension or callout as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionReport {
    /// "horizontal", "vertical", "diameter", "hole_callout".
    pub kind: String,
    /// Model value (mm); None for a dangling callout.
    pub value: Option<f64>,
    pub text: String,
    /// Why a callout could not be resolved (drawn in red, no value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangling: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewReport {
    pub id: ViewId,
    pub label: String,
    /// Named direction of the view ("Front", …, "Iso").
    pub direction: String,
    pub scale: f64,
    /// Sheet position of the view's centre (mm).
    pub center: [f64; 2],
    #[serde(flatten)]
    pub status: ViewStatus,
    pub visible_curves: usize,
    pub hidden_curves: usize,
    pub dimensions: Vec<DimensionReport>,
}

/// A rendered drawing: the sheet to write, per-view reports and every problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingRender {
    pub sheet: SheetPlot,
    pub views: Vec<ViewReport>,
    /// Flagged views and dangling annotations (empty: all references valid).
    pub issues: Vec<String>,
    /// Sampled-HLR parameters used (mm).
    pub sample_spacing: f64,
    pub chord: f64,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|x| x / l)
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Per-body data reused by every view of one render.
struct BodyData {
    edges: Vec<EdgeCurve>,
    /// Smooth edges (seams, tangent edges) with their surface normals.
    smooth: Vec<(EdgeCurve, [[f64; 3]; 3])>,
    mesh: k::TriMesh,
    /// (origin, unit axis, radius) of cylindrical faces, for exact silhouettes.
    cylinders: Vec<([f64; 3], [f64; 3], f64)>,
    bounds: k::Bounds,
}

fn body_data(b: &k::Body, chord: f64) -> Result<BodyData> {
    let kerr = |e: k::KernelError| DocError::Validation(format!("drawing: {e}"));
    let all = k::drawing_edges(b).map_err(kerr)?;
    let smooth = all
        .iter()
        .filter(|e| e.smooth)
        .filter_map(|e| e.normals.map(|n| (e.curve.clone(), n)))
        .collect();
    let edges = all
        .into_iter()
        .filter(|e| !e.smooth)
        .map(|e| e.curve)
        .collect();
    let mesh = k::tessellate_display(b, chord).map_err(kerr)?;
    let cylinders = k::classify_faces(b)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| match f {
            k::DirectFace::Cylinder {
                origin,
                axis,
                radius,
                ..
            } => Some((origin, norm(axis), radius)),
            _ => None,
        })
        .collect();
    let bounds = k::bounds(b).map_err(kerr)?;
    Ok(BodyData {
        edges,
        smooth,
        mesh,
        cylinders,
        bounds,
    })
}

/// Front-facing triangles in view coordinates with a uniform grid.
struct Occluder {
    tris: Vec<([[f64; 2]; 3], [f64; 3])>,
    lo: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    grid: Vec<Vec<u32>>,
    margin: f64,
    depth_eps: f64,
}

impl Occluder {
    fn new(meshes: &[&k::TriMesh], f: &ViewFrame, margin: f64, depth_eps: f64) -> Self {
        let mut tris = Vec::new();
        for m in meshes {
            for t in &m.triangles {
                let p = t.map(|i| f.project(m.positions[i as usize]));
                let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                    - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
                if area > 1e-12 {
                    tris.push((p.map(|q| [q[0], q[1]]), p.map(|q| q[2])));
                }
            }
        }
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for (p, _) in &tris {
            for q in p {
                for i in 0..2 {
                    lo[i] = lo[i].min(q[i]);
                    hi[i] = hi[i].max(q[i]);
                }
            }
        }
        if tris.is_empty() {
            lo = [0.0; 2];
            hi = [1.0; 2];
        }
        let n = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let cell = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / n as f64).max(1e-9);
        let nx = (((hi[0] - lo[0]) / cell).ceil() as usize).max(1);
        let ny = (((hi[1] - lo[1]) / cell).ceil() as usize).max(1);
        let mut grid = vec![Vec::new(); nx * ny];
        for (ti, (p, _)) in tris.iter().enumerate() {
            let (mut a, mut b) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in p {
                for i in 0..2 {
                    a[i] = a[i].min(q[i]);
                    b[i] = b[i].max(q[i]);
                }
            }
            let ix = |x: f64, n: usize| (((x - lo[0]) / cell).floor().max(0.0) as usize).min(n - 1);
            let iy = |y: f64, n: usize| (((y - lo[1]) / cell).floor().max(0.0) as usize).min(n - 1);
            for gy in iy(a[1], ny)..=iy(b[1], ny) {
                for gx in ix(a[0], nx)..=ix(b[0], nx) {
                    grid[gy * nx + gx].push(ti as u32);
                }
            }
        }
        Occluder {
            tris,
            lo,
            cell,
            nx,
            ny,
            grid,
            margin,
            depth_eps,
        }
    }

    /// Is the view-space point `q` (x, y, depth) behind some triangle?
    fn hidden(&self, q: [f64; 3]) -> bool {
        let gx = ((q[0] - self.lo[0]) / self.cell).floor();
        let gy = ((q[1] - self.lo[1]) / self.cell).floor();
        if gx < 0.0 || gy < 0.0 || gx >= self.nx as f64 || gy >= self.ny as f64 {
            return false;
        }
        let cell = &self.grid[gy as usize * self.nx + gx as usize];
        cell.iter().any(|&ti| {
            let (p, z) = &self.tris[ti as usize];
            // Signed distances to the three edges (counter-clockwise triangle).
            let mut bary = [0.0; 3];
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            for i in 0..3 {
                let (a, b) = (p[(i + 1) % 3], p[(i + 2) % 3]);
                let e = [b[0] - a[0], b[1] - a[1]];
                let len = e[0].hypot(e[1]);
                let c = e[0] * (q[1] - a[1]) - e[1] * (q[0] - a[0]);
                if c / len <= self.margin {
                    return false;
                }
                bary[i] = c / area;
            }
            let depth = bary[0] * z[0] + bary[1] * z[1] + bary[2] * z[2];
            depth > q[2] + self.depth_eps
        })
    }
}

/// A projected curve kind.
enum Proj {
    /// Collinear on the sheet: carrier through `o` with unit `d`.
    Line {
        o: [f64; 2],
        d: [f64; 2],
    },
    /// End-on circle: centre, radius, angle at s = 0, signed sweep.
    Circle {
        c: [f64; 2],
        r: f64,
        a0: f64,
        sweep: f64,
    },
    Curve,
}

/// A parameter interval.
type Span = (f64, f64);
/// A parameter interval and whether it is visible.
type Run = (f64, f64, bool);

/// Visible / hidden pieces collected per carrier.
#[derive(Default)]
struct Pieces {
    /// (origin, dir, intervals[(t0, t1, visible)])
    lines: Vec<([f64; 2], [f64; 2], Vec<Run>)>,
    /// (centre, r, angular intervals in [0, 2π): (a0, a1, visible))
    circles: Vec<([f64; 2], f64, Vec<Run>)>,
    curves: Vec<(Vec<[f64; 2]>, bool)>,
    tol: f64,
}

impl Pieces {
    fn line(&mut self, o: [f64; 2], d: [f64; 2], t0: f64, t1: f64, vis: bool) {
        let tol = self.tol;
        let (a, b) = (
            [o[0] + d[0] * t0, o[1] + d[1] * t0],
            [o[0] + d[0] * t1, o[1] + d[1] * t1],
        );
        // Canonical direction: angle in [0, π).
        let d = if d[1] < -1e-15 || (d[1].abs() <= 1e-15 && d[0] < 0.0) {
            [-d[0], -d[1]]
        } else {
            d
        };
        for (lo, ld, iv) in &mut self.lines {
            let par = (ld[0] * d[1] - ld[1] * d[0]).abs() < 1e-9;
            let off = |p: [f64; 2]| ((p[0] - lo[0]) * ld[1] - (p[1] - lo[1]) * ld[0]).abs();
            if par && off(a) <= tol && off(b) <= tol {
                let t = |p: [f64; 2]| (p[0] - lo[0]) * ld[0] + (p[1] - lo[1]) * ld[1];
                let (u0, u1) = (t(a).min(t(b)), t(a).max(t(b)));
                iv.push((u0, u1, vis));
                return;
            }
        }
        // New carrier, origin at the foot of the perpendicular from (0, 0).
        let s = a[0] * d[0] + a[1] * d[1];
        let lo = [a[0] - d[0] * s, a[1] - d[1] * s];
        let t = |p: [f64; 2]| (p[0] - lo[0]) * d[0] + (p[1] - lo[1]) * d[1];
        self.lines
            .push((lo, d, vec![(t(a).min(t(b)), t(a).max(t(b)), vis)]));
    }

    fn arc(&mut self, c: [f64; 2], r: f64, a0: f64, a1: f64, vis: bool) {
        let tol = self.tol;
        // Normalised counter-clockwise interval(s) in [0, 2π).
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        let mut ivs = Vec::new();
        if hi - lo >= TAU - 1e-12 {
            ivs.push((0.0, TAU));
        } else {
            let s = lo.rem_euclid(TAU);
            let e = s + (hi - lo);
            if e > TAU {
                ivs.push((s, TAU));
                ivs.push((0.0, e - TAU));
            } else {
                ivs.push((s, e));
            }
        }
        for (cc, cr, iv) in &mut self.circles {
            if (cc[0] - c[0]).hypot(cc[1] - c[1]) <= tol && (*cr - r).abs() <= tol {
                iv.extend(ivs.iter().map(|&(a, b)| (a, b, vis)));
                return;
            }
        }
        self.circles
            .push((c, r, ivs.into_iter().map(|(a, b)| (a, b, vis)).collect()));
    }
}

/// Union of intervals (gaps below `tol` closed).
fn union(mut iv: Vec<(f64, f64)>, tol: f64) -> Vec<(f64, f64)> {
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in iv {
        match out.last_mut() {
            Some(l) if a <= l.1 + tol => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// `iv` minus the union `cut`.
fn subtract(iv: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in iv {
        let mut pieces = vec![(a, b)];
        for &(c, d) in cut {
            pieces = pieces
                .into_iter()
                .flat_map(|(x, y)| {
                    let mut v = Vec::new();
                    if c > x {
                        v.push((x, y.min(c)));
                    }
                    if d < y {
                        v.push((x.max(d), y));
                    }
                    v.into_iter().filter(|(p, q)| q > p)
                })
                .collect();
        }
        out.extend(pieces);
    }
    out
}

/// Split intervals into (visible union, hidden minus visible).
fn resolve_intervals(iv: &[Run], tol: f64) -> (Vec<Span>, Vec<Span>) {
    let vis = union(iv.iter().filter(|x| x.2).map(|x| (x.0, x.1)).collect(), tol);
    let hid = union(
        iv.iter().filter(|x| !x.2).map(|x| (x.0, x.1)).collect(),
        tol,
    );
    let hid = subtract(&hid, &vis)
        .into_iter()
        .filter(|(a, b)| b - a > tol)
        .collect();
    (vis, hid)
}

/// Geometry of one view in view coordinates (model mm), before placement.
#[derive(Default)]
struct ViewGeom {
    /// (prim in view mm, visible)
    prims: Vec<(Prim, bool)>,
    /// Visible full circles: (centre, r).
    circles: Vec<([f64; 2], f64)>,
    lo: [f64; 2],
    hi: [f64; 2],
}

/// Project `curve`, classify its samples, and add the pieces.
#[allow(clippy::too_many_arguments)]
fn add_curve(
    out: &mut Pieces,
    curve: &EdgeCurve,
    f: &ViewFrame,
    occ: &Occluder,
    spacing: f64,
    lo: &mut [f64; 2],
    hi: &mut [f64; 2],
) {
    let len = curve.length();
    if len <= 0.0 {
        return;
    }
    let n = ((len / spacing).ceil() as usize).clamp(2, 4000);
    let pt = |s: f64| f.project(curve.point(s));
    let vis = |s: f64| !occ.hidden(pt(s));
    let samples: Vec<[f64; 3]> = (0..=n).map(|i| pt(i as f64 / n as f64)).collect();
    for q in &samples {
        for i in 0..2 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    // Projected kind.
    let a = samples[0];
    let (mut far, mut fd) = (a, 0.0);
    for q in &samples {
        let d = (q[0] - a[0]).hypot(q[1] - a[1]);
        if d > fd {
            (far, fd) = (*q, d);
        }
    }
    if fd <= out.tol {
        return; // seen end-on: a point
    }
    let dir = [(far[0] - a[0]) / fd, (far[1] - a[1]) / fd];
    let collinear = samples
        .iter()
        .all(|q| ((q[0] - a[0]) * dir[1] - (q[1] - a[1]) * dir[0]).abs() <= out.tol);
    let kind = if collinear {
        Proj::Line {
            o: [a[0], a[1]],
            d: dir,
        }
    } else if let EdgeCurve::Arc {
        center,
        normal,
        radius,
        start,
        sweep,
    } = curve
    {
        let nz = dot(*normal, f.toward);
        if nz.abs() > 1.0 - 1e-9 {
            let c = f.project(*center);
            let s = f.project(*start);
            Proj::Circle {
                c: [c[0], c[1]],
                r: *radius,
                a0: (s[1] - c[1]).atan2(s[0] - c[0]),
                sweep: sweep * nz.signum(),
            }
        } else {
            Proj::Curve
        }
    } else {
        Proj::Curve
    };
    // Runs of equal visibility with refined transitions.
    let flags: Vec<bool> = (0..=n).map(|i| vis(i as f64 / n as f64)).collect();
    let mut runs: Vec<(f64, f64, bool)> = Vec::new();
    let mut start = 0.0;
    let ds = TRANSITION_TOL / len;
    for i in 0..n {
        if flags[i] != flags[i + 1] {
            let (mut x0, mut x1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
            while x1 - x0 > ds {
                let m = 0.5 * (x0 + x1);
                if vis(m) == flags[i] {
                    x0 = m;
                } else {
                    x1 = m;
                }
            }
            let t = 0.5 * (x0 + x1);
            runs.push((start, t, flags[i]));
            start = t;
        }
    }
    runs.push((start, 1.0, flags[n]));
    // Runs shorter than the occlusion margin zone are artefacts of the
    // margin around triangle borders (an edge ending on an occluding face's
    // outline): they take the visibility of their longer neighbour.
    let short = 2.0 * occ.margin / len;
    while runs.len() > 1 {
        let Some(i) = (0..runs.len()).find(|&i| runs[i].1 - runs[i].0 < short) else {
            break;
        };
        let (s0, s1, _) = runs.remove(i);
        let j = if i == 0 {
            0
        } else if i >= runs.len() {
            runs.len() - 1
        } else if runs[i - 1].1 - runs[i - 1].0 >= runs[i].1 - runs[i].0 {
            i - 1
        } else {
            i
        };
        runs[j].0 = runs[j].0.min(s0);
        runs[j].1 = runs[j].1.max(s1);
        let mut k = 0;
        while k + 1 < runs.len() {
            if runs[k].2 == runs[k + 1].2 {
                runs[k].1 = runs[k + 1].1;
                runs.remove(k + 1);
            } else {
                k += 1;
            }
        }
    }
    for (s0, s1, v) in runs {
        match kind {
            Proj::Line { o, d } => {
                // Extent of the run along the carrier (folded curves seen
                // edge-on cover [min, max]).
                let m = ((s1 - s0) * n as f64).ceil().max(1.0) as usize;
                let (mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY);
                for j in 0..=m {
                    let q = pt(s0 + (s1 - s0) * j as f64 / m as f64);
                    let t = (q[0] - o[0]) * d[0] + (q[1] - o[1]) * d[1];
                    t0 = t0.min(t);
                    t1 = t1.max(t);
                }
                if t1 - t0 > out.tol {
                    out.line(o, d, t0, t1, v);
                }
            }
            Proj::Circle { c, r, a0, sweep } => {
                out.arc(c, r, a0 + sweep * s0, a0 + sweep * s1, v);
            }
            Proj::Curve => {
                let m = ((s1 - s0) * n as f64).ceil().max(1.0) as usize;
                let pts: Vec<[f64; 2]> = (0..=m)
                    .map(|j| {
                        let q = pt(s0 + (s1 - s0) * j as f64 / m as f64);
                        [q[0], q[1]]
                    })
                    .collect();
                out.curves.push((pts, v));
            }
        }
    }
}

/// Silhouette segments of curved faces: mesh edges inside one B-rep face
/// between a front- and a back-facing triangle; on a cylinder whose axis is
/// perpendicular to the view, snapped to the exact generator.
fn silhouettes(d: &BodyData, f: &ViewFrame, chord: f64) -> Vec<EdgeCurve> {
    let m = &d.mesh;
    let facing: Vec<f64> = m
        .triangles
        .iter()
        .map(|t| {
            let p = t.map(|i| m.positions[i as usize]);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(n, n).sqrt();
            if l <= 0.0 { 0.0 } else { dot(n, f.toward) / l }
        })
        .collect();
    let mut edges: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (ti, t) in m.triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(ti);
        }
    }
    const EDGE_ON: f64 = 1e-6;
    let mut out = Vec::new();
    for ((a, b), ts) in edges {
        let [t0, t1] = ts[..] else { continue };
        if m.tri_face[t0] != m.tri_face[t1] {
            continue;
        }
        let (f0, f1) = (facing[t0], facing[t1]);
        if !((f0 > EDGE_ON && f1 < -EDGE_ON) || (f0 < -EDGE_ON && f1 > EDGE_ON)) {
            continue;
        }
        let mut p = m.positions[a as usize];
        let mut q = m.positions[b as usize];
        // Exact cylinder generator.
        for (o, ax, r) in &d.cylinders {
            if dot(*ax, f.toward).abs() > 1e-9 {
                continue;
            }
            let foot = |x: [f64; 3]| {
                let t = dot(sub(x, *o), *ax);
                [o[0] + ax[0] * t, o[1] + ax[1] * t, o[2] + ax[2] * t]
            };
            let dist = |x: [f64; 3]| {
                let v = sub(x, foot(x));
                dot(v, v).sqrt()
            };
            let near = 3.0 * chord + 1e-6;
            if (dist(p) - r).abs() <= near && (dist(q) - r).abs() <= near {
                let s = norm(cross(*ax, f.toward));
                let snap = |x: [f64; 3]| {
                    let fo = foot(x);
                    let sg = dot(sub(x, fo), s).signum();
                    std::array::from_fn(|i| fo[i] + s[i] * r * sg)
                };
                (p, q) = (snap(p), snap(q));
                break;
            }
        }
        out.push(EdgeCurve::Line { a: p, b: q });
    }
    out
}

fn view_geometry(
    datas: &[&BodyData],
    f: &ViewFrame,
    chord: f64,
    spacing: f64,
    diag: f64,
) -> ViewGeom {
    let meshes: Vec<&k::TriMesh> = datas.iter().map(|d| &d.mesh).collect();
    let tol = 1e-6 * diag.max(1.0);
    let occ = Occluder::new(&meshes, f, 1.5 * chord + tol, 2.0 * chord + tol);
    let mut pieces = Pieces {
        tol: tol.max(1e-6),
        ..Default::default()
    };
    let mut g = ViewGeom {
        lo: [f64::INFINITY; 2],
        hi: [f64::NEG_INFINITY; 2],
        ..Default::default()
    };
    for d in datas {
        // Smooth edges are drawn only where they are silhouettes (the surface
        // normal along them is perpendicular to the view: seams and tangent
        // edges lying exactly on the outline).
        let smooth_sil = d.smooth.iter().filter_map(|(c, n)| {
            n.iter()
                .all(|n| dot(*n, f.toward).abs() <= 1e-9)
                .then_some(c)
        });
        let mesh_sil = silhouettes(d, f, chord);
        for e in d.edges.iter().chain(smooth_sil).chain(mesh_sil.iter()) {
            add_curve(&mut pieces, e, f, &occ, spacing, &mut g.lo, &mut g.hi);
        }
    }
    let ptol = pieces.tol;
    for (o, d, iv) in &pieces.lines {
        let (vis, hid) = resolve_intervals(iv, ptol);
        for (list, v) in [(vis, true), (hid, false)] {
            for (a, b) in list {
                if b - a <= 10.0 * ptol {
                    continue;
                }
                g.prims.push((
                    Prim::Line {
                        a: [o[0] + d[0] * a, o[1] + d[1] * a],
                        b: [o[0] + d[0] * b, o[1] + d[1] * b],
                        style: if v {
                            LineStyle::Visible
                        } else {
                            LineStyle::Hidden
                        },
                    },
                    v,
                ));
            }
        }
    }
    for (c, r, iv) in &pieces.circles {
        let atol = ptol / r.max(1e-9);
        let (vis, hid) = resolve_intervals(iv, atol);
        for (list, v) in [(vis, true), (hid, false)] {
            let mut list = list;
            // Join across angle 0.
            if list.len() >= 2
                && list[0].0 <= atol
                && list.last().is_some_and(|l| l.1 >= TAU - atol)
            {
                let first = list.remove(0);
                if let Some(l) = list.last_mut() {
                    l.1 = TAU + first.1;
                }
            }
            for (a, b) in list {
                if (b - a) * r <= 10.0 * ptol {
                    continue;
                }
                let full = b - a >= TAU - atol;
                if full && v {
                    g.circles.push((*c, *r));
                }
                g.prims.push((
                    Prim::Arc {
                        c: *c,
                        r: *r,
                        start: if full { 0.0 } else { a },
                        sweep: if full { TAU } else { b - a },
                        style: if v {
                            LineStyle::Visible
                        } else {
                            LineStyle::Hidden
                        },
                    },
                    v,
                ));
            }
        }
    }
    for (pts, v) in &pieces.curves {
        g.prims.push((
            Prim::Polyline {
                pts: pts.clone(),
                style: if *v {
                    LineStyle::Visible
                } else {
                    LineStyle::Hidden
                },
            },
            *v,
        ));
    }
    if !g.lo[0].is_finite() {
        g.lo = [0.0; 2];
        g.hi = [0.0; 2];
    }
    g
}

/// Format a model length for a dimension: up to 2 decimals, trailing zeros
/// removed.
pub fn fmt_len(x: f64) -> String {
    let s = format!("{:.2}", x + 0.0);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// "1:2", "2:1", "1:1", "1:2.5".
pub fn fmt_scale(s: f64) -> String {
    if s >= 1.0 {
        format!("{}:1", fmt_len(s))
    } else {
        format!("1:{}", fmt_len(1.0 / s))
    }
}

/// Largest standard scale at which an extent `ext` (mm) spans at most
/// `room` sheet mm.
pub fn fit_scale(ext: f64, room: f64) -> f64 {
    const STD: [f64; 13] = [
        10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01, 0.005, 0.002, 0.001,
    ];
    STD.into_iter()
        .find(|s| ext * s <= room)
        .unwrap_or(MIN_SCALE)
}

const TEXT: f64 = 3.5;
const ARROW: f64 = 2.5;

fn arrow(tip: [f64; 2], dir: [f64; 2]) -> Prim {
    // `dir`: unit direction the arrow points to.
    let (w, l) = (ARROW / 6.0, ARROW);
    let base = [tip[0] - dir[0] * l, tip[1] - dir[1] * l];
    let n = [-dir[1], dir[0]];
    Prim::Polygon {
        pts: vec![
            tip,
            [base[0] + n[0] * w, base[1] + n[1] * w],
            [base[0] - n[0] * w, base[1] - n[1] * w],
        ],
        fill: true,
        style: LineStyle::Thin,
    }
}

/// Linear dimension between sheet points `p0`, `p1` measured along unit
/// `along`, with the dimension line offset by `off` along unit `normal`.
fn linear_dim(
    out: &mut Vec<Prim>,
    p0: [f64; 2],
    p1: [f64; 2],
    normal: [f64; 2],
    off: f64,
    text: &str,
) {
    let q0 = [p0[0] + normal[0] * off, p0[1] + normal[1] * off];
    let q1 = [p1[0] + normal[0] * off, p1[1] + normal[1] * off];
    let ext = 2.0;
    for (p, q) in [(p0, q0), (p1, q1)] {
        out.push(Prim::Line {
            a: [p[0] + normal[0] * 1.5, p[1] + normal[1] * 1.5],
            b: [q[0] + normal[0] * ext, q[1] + normal[1] * ext],
            style: LineStyle::Thin,
        });
    }
    out.push(Prim::Line {
        a: q0,
        b: q1,
        style: LineStyle::Thin,
    });
    let l = (q1[0] - q0[0]).hypot(q1[1] - q0[1]).max(1e-9);
    let d = [(q1[0] - q0[0]) / l, (q1[1] - q0[1]) / l];
    out.push(arrow(q0, [-d[0], -d[1]]));
    out.push(arrow(q1, d));
    let mid = [(q0[0] + q1[0]) / 2.0, (q0[1] + q1[1]) / 2.0];
    // Text beside the line: below a horizontal dimension placed under the
    // view would collide with the arrows, so it sits on the outer side.
    let at = if normal[1] < 0.0 {
        [mid[0], mid[1] - 1.0 - TEXT]
    } else if normal[0] > 0.0 {
        [mid[0] + 1.0, mid[1] - TEXT / 2.0]
    } else {
        [mid[0], mid[1] + 1.0]
    };
    out.push(Prim::Text {
        at,
        size: TEXT,
        text: text.to_string(),
        anchor: if normal[0] > 0.0 {
            Anchor::Start
        } else {
            Anchor::Middle
        },
        style: LineStyle::Thin,
    });
}

fn diameter_callout(out: &mut Vec<Prim>, c: [f64; 2], r: f64, text: &str, style: LineStyle) {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let tip = [c[0] + r * s, c[1] + r * s];
    let knee = [tip[0] + 5.0, tip[1] + 5.0];
    let end = [knee[0] + 3.0, knee[1]];
    out.push(arrow(tip, [-s, -s]));
    out.push(Prim::Polyline {
        pts: vec![tip, knee, end],
        style,
    });
    out.push(Prim::Text {
        at: [end[0] + 0.8, end[1] - TEXT / 3.0],
        size: TEXT,
        text: text.to_string(),
        anchor: Anchor::Start,
        style,
    });
}

/// Canonical primitive order for deterministic output.
fn sort_key(p: &Prim) -> (u8, LineStyle, Vec<i64>) {
    let q = |x: f64| (x * 1e3).round() as i64;
    match p {
        Prim::Line { a, b, style } => {
            let (a, b) = if (q(a[0]), q(a[1])) <= (q(b[0]), q(b[1])) {
                (a, b)
            } else {
                (b, a)
            };
            (0, *style, vec![q(a[0]), q(a[1]), q(b[0]), q(b[1])])
        }
        Prim::Arc {
            c,
            r,
            start,
            sweep,
            style,
        } => (
            1,
            *style,
            vec![q(c[0]), q(c[1]), q(*r), q(*start), q(*sweep)],
        ),
        Prim::Polyline { pts, style } => (
            2,
            *style,
            pts.iter().flat_map(|p| [q(p[0]), q(p[1])]).collect(),
        ),
        Prim::Polygon { pts, style, .. } => (
            3,
            *style,
            pts.iter().flat_map(|p| [q(p[0]), q(p[1])]).collect(),
        ),
        Prim::Text { at, style, .. } => (4, *style, vec![q(at[0]), q(at[1])]),
    }
}

/// Map a view-space primitive to the sheet.
fn place(p: &Prim, s: f64, off: [f64; 2]) -> Prim {
    let m = |q: [f64; 2]| [off[0] + q[0] * s, off[1] + q[1] * s];
    match p {
        Prim::Line { a, b, style } => Prim::Line {
            a: m(*a),
            b: m(*b),
            style: *style,
        },
        Prim::Arc {
            c,
            r,
            start,
            sweep,
            style,
        } => Prim::Arc {
            c: m(*c),
            r: r * s,
            start: *start,
            sweep: *sweep,
            style: *style,
        },
        Prim::Polyline { pts, style } => Prim::Polyline {
            pts: pts.iter().map(|q| m(*q)).collect(),
            style: *style,
        },
        other => other.clone(),
    }
}

struct Placed {
    /// Sheet bounding box of the view.
    lo: [f64; 2],
    hi: [f64; 2],
    /// Sheet = off + scale * view coordinates.
    off: [f64; 2],
    scale: f64,
    frame: ViewFrame,
}

/// Sheet spacing (mm) of section hatch lines (45°).
pub const HATCH_SPACING: f64 = 2.5;

/// 45° hatch segments (view coordinates) over the faces of `datas` lying in
/// `plane` and facing the viewer (outward normal = plane normal): their
/// tessellation triangles, each hatch line clipped to them and the pieces
/// united, so the hatch follows the face boundaries to the chord tolerance.
fn hatch(
    datas: &[&BodyData],
    plane: &k::Frame,
    f: &ViewFrame,
    spacing: f64,
    tol: f64,
) -> Vec<([f64; 2], [f64; 2])> {
    let s2 = std::f64::consts::FRAC_1_SQRT_2;
    let (u, n) = ([s2, s2], [-s2, s2]);
    let mut lines: BTreeMap<i64, Vec<Span>> = BTreeMap::new();
    for d in datas {
        let m = &d.mesh;
        for t in &m.triangles {
            let p = t.map(|i| m.positions[i as usize]);
            if p.iter()
                .any(|q| dot(sub(*q, plane.origin), plane.z).abs() > tol)
            {
                continue;
            }
            let nn = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(nn, nn).sqrt();
            if l <= 0.0 || dot(nn, plane.z) / l < 0.999 {
                continue;
            }
            let q = p.map(|x| {
                let v = f.project(x);
                [v[0], v[1]]
            });
            let sv = q.map(|x| x[0] * n[0] + x[1] * n[1]);
            let tv = q.map(|x| x[0] * u[0] + x[1] * u[1]);
            let (lo, hi) = (
                sv.iter().copied().fold(f64::INFINITY, f64::min),
                sv.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            );
            let k0 = (lo / spacing).ceil() as i64;
            let k1 = (hi / spacing).floor() as i64;
            for kk in k0..=k1 {
                let s = kk as f64 * spacing;
                let mut ts = Vec::new();
                for i in 0..3 {
                    let (a, b) = (i, (i + 1) % 3);
                    let (da, db) = (sv[a] - s, sv[b] - s);
                    if (da <= 0.0 && db >= 0.0) || (da >= 0.0 && db <= 0.0) {
                        if (db - da).abs() < 1e-15 {
                            ts.push(tv[a]);
                            ts.push(tv[b]);
                        } else {
                            let w = da / (da - db);
                            ts.push(tv[a] + (tv[b] - tv[a]) * w);
                        }
                    }
                }
                if ts.len() >= 2 {
                    let t0 = ts.iter().copied().fold(f64::INFINITY, f64::min);
                    let t1 = ts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if t1 > t0 {
                        lines.entry(kk).or_default().push((t0, t1));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (kk, iv) in lines {
        let s = kk as f64 * spacing;
        for (t0, t1) in union(iv, 1e-9) {
            if t1 - t0 > 1e-6 {
                let at = |t: f64| [n[0] * s + u[0] * t, n[1] * s + u[1] * t];
                out.push((at(t0), at(t1)));
            }
        }
    }
    out
}

/// The cutting line of section `label` on its parent view `p`, with arrows
/// in the direction of sight and the label at both ends.
fn cutting_line(
    parent: ViewId,
    from: Side,
    offset: f64,
    label: &str,
    p: &Placed,
    section: &ViewFrame,
) -> Group {
    let look = section.toward.map(|x| -x);
    let dir = [dot(look, p.frame.right), dot(look, p.frame.up)];
    let ext = 6.0;
    let (a, b) = match from {
        Side::Above | Side::Below => {
            let y = p.off[1] + offset * p.scale;
            ([p.lo[0] - ext, y], [p.hi[0] + ext, y])
        }
        Side::Left | Side::Right => {
            let x = p.off[0] + offset * p.scale;
            ([x, p.lo[1] - ext], [x, p.hi[1] + ext])
        }
    };
    let mut prims = vec![Prim::Line {
        a,
        b,
        style: LineStyle::Center,
    }];
    for e in [a, b] {
        // Thick end strokes, arrow, letter.
        let along = [b[0] - a[0], b[1] - a[1]];
        let l = along[0].hypot(along[1]).max(1e-9);
        let along = [along[0] / l, along[1] / l];
        let inward = if e == a { 1.0 } else { -1.0 };
        prims.push(Prim::Line {
            a: e,
            b: [
                e[0] + along[0] * 4.0 * inward,
                e[1] + along[1] * 4.0 * inward,
            ],
            style: LineStyle::Visible,
        });
        let tip = [e[0] + dir[0] * 7.0, e[1] + dir[1] * 7.0];
        prims.push(Prim::Line {
            a: e,
            b: tip,
            style: LineStyle::Thin,
        });
        prims.push(arrow(tip, dir));
        prims.push(Prim::Text {
            at: [
                tip[0] + dir[0] * 2.0,
                tip[1] + dir[1] * 2.0 - if dir[1] < 0.0 { TEXT } else { 0.0 },
            ],
            size: TEXT,
            text: label.to_string(),
            anchor: Anchor::Middle,
            style: LineStyle::Thin,
        });
    }
    Group {
        id: format!("cut-{label}"),
        label: format!("Cutting line {label}-{label} on {parent}"),
        prims,
    }
}

fn flag_box(prims: &mut Vec<Prim>, c: [f64; 2], msg: &str) -> ([f64; 2], [f64; 2]) {
    let (w, h) = (60.0, 16.0);
    let lo = [c[0] - w / 2.0, c[1] - h / 2.0];
    let hi = [c[0] + w / 2.0, c[1] + h / 2.0];
    prims.push(Prim::Polygon {
        pts: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
        fill: false,
        style: LineStyle::Flag,
    });
    prims.push(Prim::Line {
        a: lo,
        b: hi,
        style: LineStyle::Flag,
    });
    prims.push(Prim::Text {
        at: [c[0], hi[1] + 1.5],
        size: 2.5,
        text: msg.to_string(),
        anchor: Anchor::Middle,
        style: LineStyle::Flag,
    });
    (lo, hi)
}

/// Render drawing `id` from the current evaluation (see the module docs).
pub fn render(doc: &Document, eval: &Evaluation, id: DrawingId) -> Result<DrawingRender> {
    let dr = doc.drawing(id)?;
    dr.validate()?;
    let (w, h) = dr.sheet.mm();
    // Bodies used anywhere in the drawing, with one shared HLR resolution.
    let mut used: Vec<BodyId> = Vec::new();
    for v in &dr.views {
        if let ViewKind::Base { bodies, .. } = &v.kind {
            for b in bodies {
                if !used.contains(b) {
                    used.push(*b);
                }
            }
        }
    }
    let mut diag: f64 = 0.0;
    let mut breps: BTreeMap<BodyId, &k::Body> = BTreeMap::new();
    let mut unsupported: Vec<BodyId> = Vec::new();
    for b in &used {
        let Some(eb) = eval.body_by_id(*b) else {
            continue;
        };
        match eb.geometry.brep() {
            Some(body) => {
                let bb =
                    k::bounds(body).map_err(|e| DocError::Validation(format!("drawing: {e}")))?;
                let d = (0..3)
                    .map(|i| (bb.max[i] - bb.min[i]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                diag = diag.max(d);
                breps.insert(*b, body);
            }
            None => unsupported.push(*b),
        }
    }
    let diag = diag.max(1.0);
    let chord = hlr_chord(diag);
    let spacing = sample_spacing(diag);
    let mut data: HashMap<BodyId, BodyData> = HashMap::new();
    for (b, body) in &breps {
        data.insert(*b, body_data(body, chord)?);
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    let mut reports: Vec<ViewReport> = Vec::new();
    let mut placed: BTreeMap<ViewId, Placed> = BTreeMap::new();
    let mut dimensioned = [false; 3];

    for v in &dr.views {
        let (frame, scale, base) = dr.resolve(v.id)?;
        let bodies = dr.bodies(v.id)?;
        let missing: Vec<BodyId> = bodies
            .iter()
            .copied()
            .filter(|b| eval.body_by_id(*b).is_none())
            .collect();
        let unsup: Vec<BodyId> = bodies
            .iter()
            .copied()
            .filter(|b| unsupported.contains(b))
            .collect();
        let mut shown: Vec<&BodyData> = bodies.iter().filter_map(|b| data.get(b)).collect();
        let label = match &v.kind {
            ViewKind::Base { orientation, .. } => {
                format!("{} {} (base)", v.id, orientation.label())
            }
            ViewKind::Projected { parent, from } => {
                format!("{} {} ({} {})", v.id, frame.name(), parent, from.label())
            }
            ViewKind::Section { label, .. } => format!("SECTION {label}-{label}"),
        };
        // Section views: the bodies cut by the section plane (part behind it).
        let mut section_data: Vec<BodyData> = Vec::new();
        let mut section_error: Option<String> = None;
        let mut cut_plane: Option<k::Frame> = None;
        if let ViewKind::Section { parent, offset, .. } = &v.kind
            && !shown.is_empty()
        {
            let (pf, _, _) = dr.resolve(*parent)?;
            let (_, from) = v.kind.parent().unwrap_or((*parent, Side::Below));
            let axis = match from {
                Side::Above | Side::Below => pf.up,
                Side::Left | Side::Right => pf.right,
            };
            let plane = k::Frame {
                origin: axis.map(|a| a * offset),
                x: frame.right,
                y: frame.up,
                z: frame.toward,
            };
            cut_plane = Some(plane);
            for b in &bodies {
                let Some(body) = breps.get(b) else { continue };
                match crate::regen::split_by_plane(body, &plane, FeatureId(0))
                    .and_then(|(behind, _)| body_data(&behind, chord))
                {
                    Ok(d) => section_data.push(d),
                    Err(e) => {
                        section_error = Some(format!("{b}: {e}"));
                        break;
                    }
                }
            }
            shown = if section_error.is_some() {
                Vec::new()
            } else {
                section_data.iter().collect()
            };
        }
        let geom = (!shown.is_empty()).then(|| view_geometry(&shown, &frame, chord, spacing, diag));
        // Placement.
        let (glo, ghi) = match &geom {
            Some(g) => (g.lo, g.hi),
            None => ([0.0; 2], [0.0; 2]),
        };
        let half = [
            (ghi[0] - glo[0]) * scale / 2.0,
            (ghi[1] - glo[1]) * scale / 2.0,
        ];
        let half = if geom.is_some() { half } else { [30.0, 8.0] };
        let center = match &v.kind {
            ViewKind::Base { position, .. } => *position,
            ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                let p = placed.get(parent).ok_or_else(|| {
                    DocError::Validation(format!("{}: {} is placed before its parent", dr.id, v.id))
                })?;
                let pc = [(p.lo[0] + p.hi[0]) / 2.0, (p.lo[1] + p.hi[1]) / 2.0];
                let ph = [(p.hi[0] - p.lo[0]) / 2.0, (p.hi[1] - p.lo[1]) / 2.0];
                let d = from.placement(dr.projection);
                let mut c = [
                    pc[0] + d[0] * (ph[0] + VIEW_GAP + half[0]),
                    pc[1] + d[1] * (ph[1] + VIEW_GAP + half[1]),
                ];
                // Aligned with the parent; moved further out past any view
                // already in that slot (deterministic: views in order).
                for _ in 0..dr.views.len() {
                    let hit = placed.values().find(|o| {
                        (0..2).all(|i| {
                            c[i] - half[i] < o.hi[i] + VIEW_GAP / 2.0
                                && c[i] + half[i] > o.lo[i] - VIEW_GAP / 2.0
                        })
                    });
                    let Some(o) = hit else { break };
                    let i = if d[0] != 0.0 { 0 } else { 1 };
                    c[i] = if d[i] > 0.0 {
                        o.hi[i] + VIEW_GAP + half[i]
                    } else {
                        o.lo[i] - VIEW_GAP - half[i]
                    };
                }
                c
            }
        };
        let gc = [(glo[0] + ghi[0]) / 2.0, (glo[1] + ghi[1]) / 2.0];
        let off = [center[0] - gc[0] * scale, center[1] - gc[1] * scale];
        let mut prims = Vec::new();
        let mut dims_prims = Vec::new();
        let mut dims = Vec::new();
        let mut status = ViewStatus::Ok;
        let (mut nvis, mut nhid) = (0, 0);
        let (lo, hi);
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|b| b.to_string()).collect();
            let msg = format!("{}: body {} missing", v.id, names.join(", "));
            issues.push(format!(
                "{} view {}: body {} no longer exists; it is not drawn",
                dr.id,
                v.id,
                names.join(", ")
            ));
            status = ViewStatus::MissingBodies {
                bodies: missing.clone(),
            };
            if geom.is_none() {
                let (a, b) = flag_box(&mut prims, center, &msg);
                (lo, hi) = (a, b);
            } else {
                lo = [off[0] + glo[0] * scale, off[1] + glo[1] * scale];
                hi = [off[0] + ghi[0] * scale, off[1] + ghi[1] * scale];
                prims.push(Prim::Text {
                    at: [lo[0], hi[1] + 2.0],
                    size: 2.5,
                    text: msg,
                    anchor: Anchor::Start,
                    style: LineStyle::Flag,
                });
            }
        } else if !unsup.is_empty() && geom.is_none() {
            let names: Vec<String> = unsup.iter().map(|b| b.to_string()).collect();
            let reason = "mesh bodies have no exact edges to draw".to_string();
            issues.push(format!(
                "{} view {}: body {} not drawn ({reason})",
                dr.id,
                v.id,
                names.join(", ")
            ));
            status = ViewStatus::Unsupported {
                bodies: unsup.clone(),
                reason,
            };
            let (a, b) = flag_box(
                &mut prims,
                center,
                &format!("{}: mesh body not drawn", v.id),
            );
            (lo, hi) = (a, b);
        } else if let Some(e) = &section_error {
            issues.push(format!("{} view {}: section failed: {e}", dr.id, v.id));
            status = ViewStatus::Unsupported {
                bodies: bodies.clone(),
                reason: format!("section failed: {e}"),
            };
            let (a, b) = flag_box(&mut prims, center, &format!("{}: section failed", v.id));
            (lo, hi) = (a, b);
        } else {
            lo = [off[0] + glo[0] * scale, off[1] + glo[1] * scale];
            hi = [off[0] + ghi[0] * scale, off[1] + ghi[1] * scale];
        }
        if let Some(g) = &geom {
            for (p, vis) in &g.prims {
                if !*vis && frame.name() == "Iso" {
                    // Pictorial views show visible edges only.
                    continue;
                }
                if *vis {
                    nvis += 1;
                } else {
                    nhid += 1;
                }
                prims.push(place(p, scale, off));
            }
            // Centre marks.
            for (c, r) in &g.circles {
                let c = [off[0] + c[0] * scale, off[1] + c[1] * scale];
                let e = r * scale + 2.0;
                prims.push(Prim::Line {
                    a: [c[0] - e, c[1]],
                    b: [c[0] + e, c[1]],
                    style: LineStyle::Center,
                });
                prims.push(Prim::Line {
                    a: [c[0], c[1] - e],
                    b: [c[0], c[1] + e],
                    style: LineStyle::Center,
                });
            }
            // Section hatching of the faces lying in the cutting plane.
            if let Some(plane) = &cut_plane {
                let tol = 1e-6 * diag;
                for (a, b) in hatch(&shown, plane, &frame, HATCH_SPACING / scale, tol) {
                    prims.push(Prim::Line {
                        a: [off[0] + a[0] * scale, off[1] + a[1] * scale],
                        b: [off[0] + b[0] * scale, off[1] + b[1] * scale],
                        style: LineStyle::Thin,
                    });
                }
            }
            // Hole callouts (semantic; resolved now).
            let mut called: Vec<usize> = Vec::new();
            for a in &v.annotations {
                let Annotation::HoleCallout { feature } = a;
                match resolve_hole(doc, eval, *feature, &frame, g, &bodies) {
                    Ok((ci, d)) => {
                        let (c, r) = g.circles[ci];
                        let text = format!("Ø{}", fmt_len(d));
                        diameter_callout(
                            &mut dims_prims,
                            [off[0] + c[0] * scale, off[1] + c[1] * scale],
                            r * scale,
                            &text,
                            LineStyle::Thin,
                        );
                        called.push(ci);
                        dims.push(DimensionReport {
                            kind: "hole_callout".into(),
                            value: Some(d),
                            text,
                            dangling: None,
                        });
                    }
                    Err(why) => {
                        let text = format!("Ø? {feature}: {why}");
                        dims_prims.push(Prim::Text {
                            at: [lo[0], lo[1] - 4.0 - TEXT * dims.len() as f64],
                            size: 2.5,
                            text: text.clone(),
                            anchor: Anchor::Start,
                            style: LineStyle::Flag,
                        });
                        issues.push(format!(
                            "{} view {}: hole callout of {feature} is dangling: {why}",
                            dr.id, v.id
                        ));
                        dims.push(DimensionReport {
                            kind: "hole_callout".into(),
                            value: None,
                            text,
                            dangling: Some(why),
                        });
                    }
                }
            }
            if dr.auto_dimensions && missing.is_empty() && cut_plane.is_none() {
                // Overall dimensions from the exact bounds, each world axis once.
                let mut bb = k::Bounds {
                    min: [f64::INFINITY; 3],
                    max: [f64::NEG_INFINITY; 3],
                };
                for d in &shown {
                    for i in 0..3 {
                        bb.min[i] = bb.min[i].min(d.bounds.min[i]);
                        bb.max[i] = bb.max[i].max(d.bounds.max[i]);
                    }
                }
                for (dir, horizontal) in [(frame.right, true), (frame.up, false)] {
                    let Some(ax) = ViewFrame::axis_of(dir) else {
                        continue;
                    };
                    if dimensioned[ax] {
                        continue;
                    }
                    dimensioned[ax] = true;
                    let value = bb.max[ax] - bb.min[ax];
                    let text = fmt_len(value);
                    // Sheet coordinate of the two extremes along this axis.
                    let ends = [bb.min[ax], bb.max[ax]].map(|x| {
                        let mut p = [0.0; 3];
                        p[ax] = x;
                        let t = dot(p, dir);
                        if horizontal {
                            off[0] + t * scale
                        } else {
                            off[1] + t * scale
                        }
                    });
                    let (e0, e1) = (ends[0].min(ends[1]), ends[0].max(ends[1]));
                    if horizontal {
                        linear_dim(
                            &mut dims_prims,
                            [e0, lo[1]],
                            [e1, lo[1]],
                            [0.0, -1.0],
                            8.0,
                            &text,
                        );
                    } else {
                        linear_dim(
                            &mut dims_prims,
                            [hi[0], e0],
                            [hi[0], e1],
                            [1.0, 0.0],
                            8.0,
                            &text,
                        );
                    }
                    dims.push(DimensionReport {
                        kind: if horizontal { "horizontal" } else { "vertical" }.into(),
                        value: Some(value),
                        text,
                        dangling: None,
                    });
                }
                // Diameters of visible end-on circles, grouped by size.
                let mut sizes: Vec<(f64, Vec<usize>)> = Vec::new();
                for (ci, (_, r)) in g.circles.iter().enumerate() {
                    if called.contains(&ci) {
                        continue;
                    }
                    match sizes.iter_mut().find(|(s, _)| (s - r).abs() <= 1e-6 * diag) {
                        Some((_, l)) => l.push(ci),
                        None => sizes.push((*r, vec![ci])),
                    }
                }
                for (r, list) in sizes {
                    let d = 2.0 * r;
                    let text = if list.len() > 1 {
                        format!("{}× Ø{}", list.len(), fmt_len(d))
                    } else {
                        format!("Ø{}", fmt_len(d))
                    };
                    // Attach to the right-most, then top-most circle.
                    let ci = *list
                        .iter()
                        .max_by(|a, b| {
                            let (pa, pb) = (g.circles[**a].0, g.circles[**b].0);
                            (pa[0], pa[1])
                                .partial_cmp(&(pb[0], pb[1]))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .unwrap_or(&list[0]);
                    let c = g.circles[ci].0;
                    diameter_callout(
                        &mut dims_prims,
                        [off[0] + c[0] * scale, off[1] + c[1] * scale],
                        r * scale,
                        &text,
                        LineStyle::Thin,
                    );
                    dims.push(DimensionReport {
                        kind: "diameter".into(),
                        value: Some(d),
                        text,
                        dangling: None,
                    });
                }
            }
        }
        if !unsup.is_empty() && geom.is_some() {
            issues.push(format!(
                "{} view {}: mesh bodies not drawn: {:?}",
                dr.id, v.id, unsup
            ));
        }
        // View label.
        prims.push(Prim::Text {
            at: [
                (lo[0] + hi[0]) / 2.0,
                lo[1]
                    - if dims.iter().any(|d| d.kind == "horizontal") {
                        18.0
                    } else {
                        6.0
                    },
            ],
            size: 2.5,
            text: label.clone(),
            anchor: Anchor::Middle,
            style: LineStyle::Thin,
        });
        prims.sort_by_cached_key(sort_key);
        groups.push(Group {
            id: format!("view-{}", v.id.0),
            label: label.clone(),
            prims,
        });
        groups.push(Group {
            id: format!("dims-{}", v.id.0),
            label: format!("{} dimensions", v.id),
            prims: dims_prims,
        });
        let _ = base;
        reports.push(ViewReport {
            id: v.id,
            label,
            direction: frame.name().to_string(),
            scale,
            center,
            status,
            visible_curves: nvis,
            hidden_curves: nhid,
            dimensions: dims,
        });
        placed.insert(
            v.id,
            Placed {
                lo,
                hi,
                off,
                scale,
                frame,
            },
        );
        if let (
            ViewKind::Section {
                parent,
                from,
                offset,
                label,
            },
            Some(p),
        ) = (
            &v.kind,
            placed.get(&v.kind.parent().map(|x| x.0).unwrap_or(v.id)),
        ) {
            groups.push(cutting_line(*parent, *from, *offset, label, p, &frame));
        }
    }
    // Clipping check: everything drawn must be on the sheet inside the border.
    for (vid, p) in &placed {
        if p.lo[0] < 10.0 || p.lo[1] < 10.0 || p.hi[0] > w - 10.0 || p.hi[1] > h - 10.0 {
            issues.push(format!(
                "{} view {vid} extends past the sheet border (reduce the scale or move it)",
                dr.id
            ));
        }
    }
    issues.sort();
    let mut all = vec![border(dr, w, h), title_block(doc, dr, w, &used)];
    all.extend(groups);
    Ok(DrawingRender {
        sheet: SheetPlot {
            width: w,
            height: h,
            title: dr.title.title.clone(),
            groups: all,
        },
        views: reports,
        issues,
        sample_spacing: spacing,
        chord,
    })
}

/// Find the end-on circle of Hole `feature` in a view: (circle index, diameter).
fn resolve_hole(
    doc: &Document,
    eval: &Evaluation,
    feature: FeatureId,
    frame: &ViewFrame,
    g: &ViewGeom,
    bodies: &[BodyId],
) -> std::result::Result<(usize, f64), String> {
    let f = doc
        .features
        .iter()
        .find(|f| f.id == feature)
        .ok_or_else(|| "the feature no longer exists".to_string())?;
    let FeatureKind::Hole {
        center,
        diameter,
        target,
        ..
    } = &f.kind
    else {
        return Err("the feature is not a hole".into());
    };
    if f.suppressed {
        return Err("the hole is suppressed".into());
    }
    if !bodies.contains(target) {
        return Err(format!(
            "the hole is in {target}, which this view does not show"
        ));
    }
    let fr = eval
        .features
        .iter()
        .find(|r| r.id == feature)
        .and_then(|r| r.frame)
        .ok_or_else(|| "the hole was not evaluated".to_string())?;
    if dot(fr.z, frame.toward).abs() < 1.0 - 1e-9 {
        return Err("the hole is not seen end-on in this view".into());
    }
    let c = frame.project(fr.to_world(*center));
    let r = diameter / 2.0;
    g.circles
        .iter()
        .position(|(cc, cr)| {
            (cc[0] - c[0]).hypot(cc[1] - c[1]) <= 1e-6 * r.max(1.0)
                && (cr - r).abs() <= 1e-6 * r.max(1.0)
        })
        .map(|i| (i, *diameter))
        .ok_or_else(|| "no visible circle of the hole's diameter at its centre in this view".into())
}

fn border(_dr: &Drawing, w: f64, h: f64) -> Group {
    let m = 10.0;
    Group {
        id: "border".into(),
        label: "Border".into(),
        prims: vec![Prim::Polygon {
            pts: vec![[m, m], [w - m, m], [w - m, h - m], [m, h - m]],
            fill: false,
            style: LineStyle::Frame,
        }],
    }
}

/// Part number and material from the model when the title block leaves them
/// empty.
pub fn title_fields(doc: &Document, dr: &Drawing, bodies: &[BodyId]) -> (String, String) {
    let pn = if !dr.title.part_number.is_empty() {
        dr.title.part_number.clone()
    } else {
        doc.active_configuration
            .as_ref()
            .and_then(|a| doc.configurations.iter().find(|c| &c.name == a))
            .map(|c| c.part_number.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "-".into())
    };
    let mat = if !dr.title.material.is_empty() {
        dr.title.material.clone()
    } else {
        let mut names: Vec<String> = bodies
            .iter()
            .filter_map(|b| doc.bodies.iter().find(|x| x.id == *b))
            .filter_map(|b| b.material.as_ref().map(|m| m.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        if names.is_empty() {
            "-".into()
        } else {
            names.join(", ")
        }
    };
    (pn, mat)
}

fn title_block(doc: &Document, dr: &Drawing, w: f64, bodies: &[BodyId]) -> Group {
    let (bw, bh) = (130.0, 32.0);
    let x0 = w - 10.0 - bw;
    let y0 = 10.0;
    let mut p = vec![Prim::Polygon {
        pts: vec![[x0, y0], [x0 + bw, y0], [x0 + bw, y0 + bh], [x0, y0 + bh]],
        fill: false,
        style: LineStyle::Thin,
    }];
    let row = |y: f64| Prim::Line {
        a: [x0, y],
        b: [x0 + bw, y],
        style: LineStyle::Thin,
    };
    p.push(row(y0 + 12.0));
    p.push(row(y0 + 22.0));
    p.push(Prim::Line {
        a: [x0 + 65.0, y0],
        b: [x0 + 65.0, y0 + 22.0],
        style: LineStyle::Thin,
    });
    let (pn, mat) = title_fields(doc, dr, bodies);
    let scales: Vec<f64> = dr
        .views
        .iter()
        .filter_map(|v| match v.kind {
            ViewKind::Base { scale, .. } => Some(scale),
            _ => None,
        })
        .collect();
    let scale = match scales.first() {
        Some(s) if scales.iter().all(|x| x == s) => fmt_scale(*s),
        Some(_) => "AS SHOWN".into(),
        None => "-".into(),
    };
    let label = |x: f64, y: f64, t: &str| Prim::Text {
        at: [x, y],
        size: 1.8,
        text: t.to_string(),
        anchor: Anchor::Start,
        style: LineStyle::Thin,
    };
    let value = |x: f64, y: f64, t: &str, s: f64| Prim::Text {
        at: [x, y],
        size: s,
        text: t.to_string(),
        anchor: Anchor::Start,
        style: LineStyle::Thin,
    };
    p.push(label(x0 + 1.5, y0 + bh - 3.0, "TITLE"));
    p.push(value(x0 + 1.5, y0 + 24.5, &dr.title.title, 5.0));
    p.push(label(x0 + 1.5, y0 + 19.5, "PART NUMBER"));
    p.push(value(x0 + 1.5, y0 + 14.0, &pn, 3.5));
    p.push(label(x0 + 66.5, y0 + 19.5, "MATERIAL"));
    p.push(value(x0 + 66.5, y0 + 14.0, &mat, 3.5));
    p.push(label(x0 + 1.5, y0 + 9.5, "SCALE"));
    p.push(value(x0 + 1.5, y0 + 3.0, &scale, 3.5));
    p.push(label(x0 + 32.0, y0 + 9.5, "SHEET"));
    p.push(value(
        x0 + 32.0,
        y0 + 3.0,
        &format!("{} mm", dr.sheet.label()),
        3.5,
    ));
    p.push(label(x0 + 66.5, y0 + 9.5, "PROJECTION"));
    p.push(value(x0 + 66.5, y0 + 3.0, dr.projection.label(), 2.8));
    Group {
        id: "title".into(),
        label: "Title block".into(),
        prims: p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_right_handed_and_projected_views_match_presets() {
        for o in Orientation::ALL {
            let f = o.frame();
            let c = cross(f.right, f.up);
            assert!(dot(c, f.toward) > 1.0 - 1e-12, "{o:?}");
        }
        let front = Orientation::Front.frame();
        assert_eq!(front.seen_from(Side::Above).name(), "Top");
        assert_eq!(front.seen_from(Side::Below).name(), "Bottom");
        assert_eq!(front.seen_from(Side::Right).name(), "Right");
        assert_eq!(front.seen_from(Side::Left).name(), "Left");
        let top = front.seen_from(Side::Above);
        let t = Orientation::Top.frame();
        assert!(dot(top.right, t.right) > 1.0 - 1e-12 && dot(top.up, t.up) > 1.0 - 1e-12);
        let right = front.seen_from(Side::Right);
        let r = Orientation::Right.frame();
        assert!(dot(right.right, r.right) > 1.0 - 1e-12 && dot(right.up, r.up) > 1.0 - 1e-12);
        assert_eq!(Side::Above.placement(Projection::ThirdAngle), [0.0, 1.0]);
        assert_eq!(Side::Above.placement(Projection::FirstAngle), [-0.0, -1.0]);
    }

    #[test]
    fn interval_logic() {
        let (v, h) = resolve_intervals(&[(0.0, 10.0, true), (5.0, 20.0, false)], 1e-9);
        assert_eq!(v, vec![(0.0, 10.0)]);
        assert_eq!(h, vec![(10.0, 20.0)]);
        assert_eq!(fmt_len(40.0), "40");
        assert_eq!(fmt_len(7.5), "7.5");
        assert_eq!(fmt_len(12.345), "12.35");
        assert_eq!(fmt_scale(0.5), "1:2");
        assert_eq!(fmt_scale(2.0), "2:1");
        assert_eq!(fit_scale(40.0, 90.0), 2.0);
    }
}
