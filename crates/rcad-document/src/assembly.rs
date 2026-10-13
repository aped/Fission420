//! Component / occurrence / joint model (run 6 flat model, run 7 nesting and
//! more joint types; docs/ASSEMBLIES.md; formats v6/v7).
//!
//! * A **component** is a definition: a named set of this document's bodies
//!   (modeled by the ordinary feature history) plus, since run 7, *child
//!   occurrences* of other components (a sub-assembly). A body belongs to at
//!   most one component. A component may be empty (a new component that is
//!   about to receive bodies through activation).
//! * An **occurrence** places a component by a rigid transform relative to its
//!   **owner**: `None` = the document's top-level assembly, `Some(C)` = inside
//!   component C's definition (a child occurrence). All occurrences of a
//!   component are linked: editing the definition (its features, its child
//!   occurrences, joints between them) changes every occurrence. Containment is
//!   acyclic (validated). An **instance** is a path of occurrences from the top
//!   level down (`O3/O7`): its world transform is the composition of the local
//!   transforms along the path. Paths are the stable identifiers used by BOM
//!   rows, interference, export names and exploded offsets.
//! * A **joint** makes its child occurrence follow its parent:
//!   `L_child = L_parent ∘ M(values) ∘ rest` in the coordinates of their common
//!   owner (parent and child must be siblings). `M` is the joint motion in the
//!   parent's definition coordinates and `rest` the child's pose relative to the
//!   parent captured when the joint was made (as-built: nothing jumps). Joints
//!   form a forest: every occurrence is the child of at most one joint, loops
//!   are refused. Grounded occurrences cannot be joint children or be moved.
use crate::{BodyId, DocError, Result};
use rcad_kernel::Rigid;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct ComponentId(pub u32);
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct OccurrenceId(pub u32);
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct JointId(pub u32);

impl std::fmt::Display for ComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "C{}", self.0)
    }
}
impl std::fmt::Display for OccurrenceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "O{}", self.0)
    }
}
impl std::fmt::Display for JointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "J{}", self.0)
    }
}

/// Maximum nesting depth of sub-assemblies (levels below the top).
pub const MAX_DEPTH: usize = 16;
/// Maximum number of evaluated instances (guards exponential reuse).
pub const MAX_INSTANCES: usize = 20_000;

/// Stable instance path "O3/O7" (top-level occurrence first).
pub fn path_string(path: &[OccurrenceId]) -> String {
    path.iter()
        .map(|o| o.to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Parse "O3/O7" (syntax only; [`Assembly::check_path`] validates it).
pub fn parse_path(s: &str) -> Result<Vec<OccurrenceId>> {
    let bad = || DocError::Validation(format!("'{s}' is not an occurrence path like O3/O7"));
    if s.trim().is_empty() {
        return Err(bad());
    }
    s.split('/')
        .map(|p| {
            p.trim()
                .strip_prefix('O')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n > 0)
                .map(OccurrenceId)
                .ok_or_else(bad)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: ComponentId,
    pub name: String,
    #[serde(default)]
    pub bodies: Vec<BodyId>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub part_number: String,
    /// Run 7: this component was made as an independent copy of that one
    /// (its bodies are copies with their own history; it does not follow the
    /// source's later edits). Provenance only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<ComponentId>,
    /// Run 7: the component was inserted from another native document (its
    /// bodies are an embedded STEP copy of that document's evaluated solids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalSource>,
}

/// Provenance of an external component (run 7). The geometry is always the
/// embedded copy; the source file is only read by an explicit update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalSource {
    /// Source file name (provenance).
    pub file_name: String,
    /// Path given at insertion when the component is *linked* (updatable on
    /// request); `None` = embedded only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// FNV-1a 64 (hex) and size of the source file bytes the embedded copy was
    /// made from: the pinned content identity (change detection, not security).
    pub fnv64: String,
    pub len: u64,
}

/// Editable placement: rotate `angle` rad about the axis through the origin
/// along `axis`, then translate by `translate` (owner coordinates, mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub translate: [f64; 3],
    #[serde(default = "z_axis")]
    pub axis: [f64; 3],
    #[serde(default)]
    pub angle: f64,
}

fn z_axis() -> [f64; 3] {
    [0.0, 0.0, 1.0]
}

impl Default for Placement {
    fn default() -> Self {
        Placement {
            translate: [0.0; 3],
            axis: z_axis(),
            angle: 0.0,
        }
    }
}

impl Placement {
    pub fn rigid(&self) -> Result<Rigid> {
        Rigid::new(self.translate, [0.0; 3], self.axis, self.angle)
            .map_err(|e| DocError::Validation(format!("placement: {e}")))
    }
}

/// A raw rigid pose (rotation matrix rows, translation), stored exactly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub r: [[f64; 3]; 3],
    pub t: [f64; 3],
}

impl From<Rigid> for Pose {
    fn from(x: Rigid) -> Self {
        Pose { r: x.r, t: x.t }
    }
}
impl From<Pose> for Rigid {
    fn from(p: Pose) -> Self {
        Rigid { r: p.r, t: p.t }
    }
}

/// Inverse of a rigid transform: x ↦ Rᵀ (x − t).
pub fn inverse(x: &Rigid) -> Rigid {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = x.r[j][i];
        }
    }
    let rt = Rigid { r, t: [0.0; 3] }.vector(x.t);
    Rigid {
        r,
        t: [-rt[0], -rt[1], -rt[2]],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: OccurrenceId,
    pub component: ComponentId,
    pub name: String,
    /// Placement in the owner's coordinates (ignored while the occurrence is a
    /// joint child).
    #[serde(default)]
    pub placement: Placement,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grounded: bool,
    /// Run 6: excluded from the evaluated assembly (display, BOM, mass,
    /// interference, export) with everything below it; a configuration can
    /// control it. The definition is not deleted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppressed: bool,
    /// Run 7: the component whose definition contains this occurrence
    /// (`None` = the top-level assembly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ComponentId>,
    /// Run 7: not displayed (with everything below it). Still counted in BOM,
    /// mass properties, interference and export.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointKind {
    /// No relative motion (0 DOF).
    Rigid,
    /// Rotation about `axis` through `origin`; value in degrees.
    Revolute,
    /// Translation along `axis`; value in mm.
    Slider,
    /// Run 7: rotation about and translation along `axis` (2 DOF: degrees, mm).
    Cylindrical,
    /// Run 7: rotation about `axis` through `origin` and translation along the
    /// slot direction `dir2` (⊥ axis) (2 DOF: degrees, mm).
    PinSlot,
    /// Run 7: translation in the plane with normal `axis` along `u` = `dir2`
    /// and `w` = axis × u, and rotation about the normal (3 DOF: mm, mm, deg).
    Planar,
    /// Run 7: rotation about `origin`: about `axis`, then `u`, then `w`, all
    /// fixed in the parent (3 DOF: degrees).
    Ball,
}

impl JointKind {
    /// Units of each value ("°" or " mm").
    pub fn units(self) -> &'static [&'static str] {
        match self {
            JointKind::Rigid => &[],
            JointKind::Revolute => &["°"],
            JointKind::Slider => &[" mm"],
            JointKind::Cylindrical | JointKind::PinSlot => &["°", " mm"],
            JointKind::Planar => &[" mm", " mm", "°"],
            JointKind::Ball => &["°", "°", "°"],
        }
    }

    /// Names of each value.
    pub fn value_names(self) -> &'static [&'static str] {
        match self {
            JointKind::Rigid => &[],
            JointKind::Revolute => &["angle"],
            JointKind::Slider => &["distance"],
            JointKind::Cylindrical => &["angle", "distance"],
            JointKind::PinSlot => &["angle", "slot distance"],
            JointKind::Planar => &["u", "w", "angle"],
            JointKind::Ball => &["angle about axis", "angle about u", "angle about w"],
        }
    }

    pub fn dof(self) -> u8 {
        self.units().len() as u8
    }

    pub const ALL: [JointKind; 7] = [
        JointKind::Rigid,
        JointKind::Revolute,
        JointKind::Slider,
        JointKind::Cylindrical,
        JointKind::PinSlot,
        JointKind::Planar,
        JointKind::Ball,
    ];

    /// Run 7 kinds (need format v7).
    pub fn is_v7(self) -> bool {
        !matches!(
            self,
            JointKind::Rigid | JointKind::Revolute | JointKind::Slider
        )
    }
}

/// A further joint value with its own limits (run 7, multi-DOF joints).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct DofValue {
    #[serde(default)]
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub id: JointId,
    pub name: String,
    pub kind: JointKind,
    pub parent: OccurrenceId,
    pub child: OccurrenceId,
    /// Joint axis in the parent's definition coordinates (mm, unit vector).
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    /// First value: degrees (revolute) or mm (slider); 0 for rigid.
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Child pose relative to `L_parent ∘ M(values)` at creation.
    pub rest: Pose,
    /// Run 7: second and third values of multi-DOF joints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<DofValue>,
    /// Run 7: slot direction (pin-slot) or in-plane `u` direction (planar,
    /// ball); projected perpendicular to `axis`. Default: a fixed
    /// perpendicular (see [`Joint::frame`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir2: Option<[f64; 3]>,
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
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

impl Joint {
    fn err(&self, m: impl std::fmt::Display) -> DocError {
        DocError::Validation(format!("{} ({}): {m}", self.id, self.name))
    }

    /// All values (first, then `extra`).
    pub fn values(&self) -> Vec<f64> {
        std::iter::once(self.value)
            .chain(self.extra.iter().map(|d| d.value))
            .take(self.kind.dof() as usize)
            .collect()
    }

    /// Limits of value `i`.
    pub fn limits(&self, i: usize) -> (Option<f64>, Option<f64>) {
        if i == 0 {
            (self.min, self.max)
        } else {
            self.extra
                .get(i - 1)
                .map_or((None, None), |d| (d.min, d.max))
        }
    }

    /// Unit frame (a, u, w) of the joint: a = axis, u = `dir2` projected ⊥ a
    /// (or a fixed perpendicular), w = a × u.
    pub fn frame(&self) -> Result<([f64; 3], [f64; 3], [f64; 3])> {
        let n = norm(self.axis);
        if !(n.is_finite() && n > 1e-12) {
            return Err(self.err("the joint axis has zero length"));
        }
        let a = self.axis.map(|x| x / n);
        let seed = match self.dir2 {
            Some(d) => d,
            None if a[0].abs() < 0.9 => [1.0, 0.0, 0.0],
            None => [0.0, 1.0, 0.0],
        };
        let k = dot(seed, a);
        let u0 = [seed[0] - k * a[0], seed[1] - k * a[1], seed[2] - k * a[2]];
        let nu = norm(u0);
        if !(nu.is_finite() && nu > 1e-9 * norm(seed).max(1e-300)) {
            return Err(
                self.err("the second direction is parallel to the axis (singular joint frame)")
            );
        }
        let u = u0.map(|x| x / nu);
        Ok((a, u, cross(a, u)))
    }

    /// The joint motion `M(values)` in the parent's definition coordinates.
    pub fn motion(&self, vals: &[f64]) -> Result<Rigid> {
        let need = self.kind.dof() as usize;
        if vals.len() != need {
            return Err(self.err(format!(
                "a {:?} joint takes {need} value(s), got {}",
                self.kind,
                vals.len()
            )));
        }
        if self.kind == JointKind::Rigid {
            return Ok(Rigid::IDENTITY);
        }
        let (a, u, w) = self.frame()?;
        let o = self.origin;
        let e = |x: rcad_kernel::KernelError| self.err(x);
        let rot = |axis: [f64; 3], deg: f64| Rigid::new([0.0; 3], o, axis, deg.to_radians());
        let tr = |d: [f64; 3]| Rigid::new(d, [0.0; 3], z_axis(), 0.0);
        let s = |d: [f64; 3], k: f64| d.map(|x| x * k);
        let m = match self.kind {
            JointKind::Rigid => Ok(Rigid::IDENTITY),
            JointKind::Revolute => rot(a, vals[0]),
            JointKind::Slider => tr(s(a, vals[0])),
            JointKind::Cylindrical => Ok(tr(s(a, vals[1]))
                .map_err(e)?
                .after(&rot(a, vals[0]).map_err(e)?)),
            JointKind::PinSlot => Ok(tr(s(u, vals[1]))
                .map_err(e)?
                .after(&rot(a, vals[0]).map_err(e)?)),
            JointKind::Planar => {
                let d: [f64; 3] = std::array::from_fn(|i| u[i] * vals[0] + w[i] * vals[1]);
                Ok(tr(d).map_err(e)?.after(&rot(a, vals[2]).map_err(e)?))
            }
            JointKind::Ball => Ok(rot(a, vals[0])
                .map_err(e)?
                .after(&rot(u, vals[1]).map_err(e)?)
                .after(&rot(w, vals[2]).map_err(e)?)),
        };
        m.map_err(e)
    }

    /// The motion at the stored values.
    pub fn current_motion(&self) -> Result<Rigid> {
        self.motion(&self.values())
    }

    /// Whether `vals` (all of them) are finite and within the limits.
    pub fn check_values(&self, vals: &[f64]) -> Result<()> {
        let need = self.kind.dof() as usize;
        if self.kind == JointKind::Rigid {
            if vals.iter().any(|v| *v != 0.0) {
                return Err(self.err("a rigid joint has no value"));
            }
            return Ok(());
        }
        if vals.len() != need {
            return Err(self.err(format!(
                "a {:?} joint takes {need} value(s), got {}",
                self.kind,
                vals.len()
            )));
        }
        for (i, v) in vals.iter().enumerate() {
            let unit = self.kind.units()[i];
            let what = self.kind.value_names()[i];
            if !v.is_finite() {
                return Err(self.err(format!("{what} must be finite")));
            }
            let (lo, hi) = self.limits(i);
            if let (Some(a), Some(b)) = (lo, hi)
                && (a.is_nan() || b.is_nan() || a > b)
            {
                return Err(self.err(format!(
                    "the lower limit {a} of {what} exceeds the upper limit {b}"
                )));
            }
            if let Some(m) = lo
                && *v < m
            {
                return Err(self.err(format!(
                    "{what} {v}{unit} is below the lower limit {m}{unit}"
                )));
            }
            if let Some(m) = hi
                && *v > m
            {
                return Err(self.err(format!(
                    "{what} {v}{unit} is above the upper limit {m}{unit}"
                )));
            }
        }
        Ok(())
    }

    /// Run-6 single-value check (the first value; others unchanged).
    pub fn check_value(&self, v: f64) -> Result<()> {
        let mut vals = self.values();
        if vals.is_empty() {
            return self.check_values(&[v]);
        }
        vals[0] = v;
        self.check_values(&vals)
    }

    /// Degrees of freedom the joint leaves between parent and child.
    pub fn dof(&self) -> u8 {
        self.kind.dof()
    }

    /// Set value `i` (unchecked; callers check limits).
    pub fn set_value(&mut self, i: usize, v: f64) {
        if i == 0 {
            self.value = v;
        } else {
            while self.extra.len() < i {
                self.extra.push(DofValue::default());
            }
            self.extra[i - 1].value = v;
        }
    }

    /// Set the limits of value `i` (unchecked).
    pub fn set_limits(&mut self, i: usize, min: Option<f64>, max: Option<f64>) {
        if i == 0 {
            self.min = min;
            self.max = max;
        } else {
            while self.extra.len() < i {
                self.extra.push(DofValue::default());
            }
            self.extra[i - 1].min = min;
            self.extra[i - 1].max = max;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<Occurrence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joints: Vec<Joint>,
    #[serde(default = "one")]
    pub next_component: u32,
    #[serde(default = "one")]
    pub next_occurrence: u32,
    #[serde(default = "one")]
    pub next_joint: u32,
    /// Run 7: the component being edited: bodies created while it is active
    /// become its definition bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ComponentId>,
    /// Run 7: exploded-view offsets (world mm) per instance path ("O3/O7");
    /// display state only (no geometry, joint, BOM or mass effect).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub explode: BTreeMap<String, [f64; 3]>,
    /// Run 7: whether the exploded offsets are displayed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exploded: bool,
}

fn one() -> u32 {
    1
}

impl Default for Assembly {
    fn default() -> Self {
        Assembly {
            components: vec![],
            occurrences: vec![],
            joints: vec![],
            next_component: 1,
            next_occurrence: 1,
            next_joint: 1,
            active: None,
            explode: BTreeMap::new(),
            exploded: false,
        }
    }
}

/// Evaluated placement of one instance (an occurrence reached through a path
/// of parent occurrences).
#[derive(Debug, Clone, PartialEq)]
pub struct EvalOccurrence {
    /// The leaf occurrence (not unique when a sub-assembly is reused; the
    /// `path` is).
    pub id: OccurrenceId,
    pub component: ComponentId,
    /// The leaf occurrence's name.
    pub name: String,
    /// World transform (composition along the path).
    pub transform: Rigid,
    /// The component's definition bodies (placed by `transform`).
    pub bodies: Vec<BodyId>,
    pub grounded: bool,
    /// Joint driving the leaf occurrence, if it is a joint child.
    pub driven_by: Option<JointId>,
    /// Remaining degrees of freedom of the leaf occurrence relative to its
    /// owner (0 grounded / rigid child, joint DOF, 6 free).
    pub dof: u8,
    /// Run 7: top-level occurrence first.
    pub path: Vec<OccurrenceId>,
    /// Run 7: names along the path joined by "/".
    pub path_name: String,
    /// Run 7: nesting level (0 = top level).
    pub level: usize,
    /// Run 7: false when this occurrence or one above it is hidden.
    pub visible: bool,
    /// Run 7: exploded display offset (world mm; zero unless the exploded view
    /// is on). Never applied to geometry, mass or interference.
    pub explode: [f64; 3],
    /// Run 7: inside the active component (or no component is active).
    pub in_active: bool,
}

impl EvalOccurrence {
    pub fn path_string(&self) -> String {
        path_string(&self.path)
    }
}

impl Assembly {
    pub fn is_empty(&self) -> bool {
        self.components.is_empty() && self.occurrences.is_empty() && self.joints.is_empty()
    }

    pub fn component(&self, id: ComponentId) -> Result<&Component> {
        self.components
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown component {id}")))
    }

    pub fn component_mut(&mut self, id: ComponentId) -> Result<&mut Component> {
        self.components
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown component {id}")))
    }

    pub fn occurrence(&self, id: OccurrenceId) -> Result<&Occurrence> {
        self.occurrences
            .iter()
            .find(|o| o.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown occurrence {id}")))
    }

    pub fn occurrence_mut(&mut self, id: OccurrenceId) -> Result<&mut Occurrence> {
        self.occurrences
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown occurrence {id}")))
    }

    pub fn joint(&self, id: JointId) -> Result<&Joint> {
        self.joints
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown joint {id}")))
    }

    pub fn joint_mut(&mut self, id: JointId) -> Result<&mut Joint> {
        self.joints
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown joint {id}")))
    }

    /// Component owning body `b`, if any.
    pub fn component_of(&self, b: BodyId) -> Option<&Component> {
        self.components.iter().find(|c| c.bodies.contains(&b))
    }

    /// Joint whose child is `o`.
    pub fn joint_of_child(&self, o: OccurrenceId) -> Option<&Joint> {
        self.joints.iter().find(|j| j.child == o)
    }

    /// Child occurrences of component `c`'s definition (`None` = top level).
    pub fn children_of(&self, c: Option<ComponentId>) -> impl Iterator<Item = &Occurrence> {
        self.occurrences.iter().filter(move |o| o.owner == c)
    }

    /// Whether component `inner` is `outer` or (transitively) contained in
    /// `outer`'s definition.
    pub fn contains(&self, outer: ComponentId, inner: ComponentId) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![outer];
        while let Some(c) = stack.pop() {
            if c == inner {
                return true;
            }
            if !seen.insert(c) {
                continue;
            }
            stack.extend(self.children_of(Some(c)).map(|o| o.component));
        }
        false
    }

    pub fn new_component(
        &mut self,
        name: String,
        bodies: Vec<BodyId>,
        part_number: String,
    ) -> ComponentId {
        let id = ComponentId(self.next_component.max(1));
        self.next_component = id.0.saturating_add(1);
        self.components.push(Component {
            id,
            name,
            bodies,
            part_number,
            derived_from: None,
            external: None,
        });
        id
    }

    pub fn new_occurrence(
        &mut self,
        component: ComponentId,
        name: String,
        placement: Placement,
    ) -> OccurrenceId {
        let id = OccurrenceId(self.next_occurrence.max(1));
        self.next_occurrence = id.0.saturating_add(1);
        self.occurrences.push(Occurrence {
            id,
            component,
            name,
            placement,
            grounded: false,
            suppressed: false,
            owner: None,
            hidden: false,
        });
        id
    }

    pub fn new_joint_id(&mut self) -> JointId {
        let id = JointId(self.next_joint.max(1));
        self.next_joint = id.0.saturating_add(1);
        id
    }

    /// "Name#n" not used by another occurrence.
    pub fn default_occurrence_name(&self, component: &str) -> String {
        (1..)
            .map(|i| format!("{component}#{i}"))
            .find(|n| !self.occurrences.iter().any(|o| &o.name == n))
            .unwrap_or_else(|| component.to_string())
    }

    /// A path is valid when it starts at the top level and every next
    /// occurrence is a child of the previous one's component.
    pub fn check_path(&self, path: &[OccurrenceId]) -> Result<()> {
        let mut owner = None;
        if path.is_empty() {
            return Err(DocError::Validation("empty occurrence path".into()));
        }
        for o in path {
            let x = self.occurrence(*o)?;
            if x.owner != owner {
                return Err(DocError::Validation(format!(
                    "{} is not an occurrence path: {o} is not a child of the previous occurrence",
                    path_string(path)
                )));
            }
            owner = Some(x.component);
        }
        Ok(())
    }

    /// Structural validity: unique ids below the allocators, unique names,
    /// bodies in at most one component and existing in `bodies`, occurrences of
    /// existing components with existing owners, acyclic containment within
    /// [`MAX_DEPTH`], joints between distinct sibling occurrences, at most one
    /// joint per child, no grounded child, no loops, finite values within
    /// limits, valid exploded-view paths.
    pub fn validate(&self, bodies: &[crate::BodyInfo]) -> Result<()> {
        let v = |m: String| Err(DocError::Validation(m));
        // Review run 6 #6: stored poses must be proper rigid motions.
        for j in &self.joints {
            let p = &j.rest;
            let finite =
                p.r.iter()
                    .flatten()
                    .chain(p.t.iter())
                    .all(|x| x.is_finite());
            let mut ok = finite;
            for a in 0..3 {
                for b in 0..3 {
                    let d: f64 = (0..3).map(|k| p.r[k][a] * p.r[k][b]).sum();
                    let want = if a == b { 1.0 } else { 0.0 };
                    ok &= (d - want).abs() <= 1e-9;
                }
            }
            let det = p.r[0][0] * (p.r[1][1] * p.r[2][2] - p.r[1][2] * p.r[2][1])
                - p.r[0][1] * (p.r[1][0] * p.r[2][2] - p.r[1][2] * p.r[2][0])
                + p.r[0][2] * (p.r[1][0] * p.r[2][1] - p.r[1][1] * p.r[2][0]);
            ok &= det > 0.0;
            if !ok {
                return v(format!(
                    "joint {} has a rest pose that is not a rigid motion (finite orthonormal, \
                     det +1)",
                    j.id
                ));
            }
        }
        let mut owner: BTreeMap<BodyId, ComponentId> = BTreeMap::new();
        for (i, c) in self.components.iter().enumerate() {
            if c.id.0 == 0 || c.id.0 >= self.next_component {
                return v(format!("invalid component id {}", c.id));
            }
            if self.components[..i]
                .iter()
                .any(|d| d.id == c.id || d.name == c.name)
            {
                return v(format!("duplicate component {} '{}'", c.id, c.name));
            }
            if c.name.trim().is_empty() {
                return v(format!("component {} has an empty name", c.id));
            }
            if let Some(d) = c.derived_from
                && (d == c.id || self.component(d).is_err())
            {
                return v(format!(
                    "component '{}' is a copy of a missing component {d}",
                    c.name
                ));
            }
            for b in &c.bodies {
                if !bodies.iter().any(|x| x.id == *b) {
                    return v(format!("component '{}': body {b} does not exist", c.name));
                }
                if let Some(o) = owner.insert(*b, c.id) {
                    return v(format!(
                        "body {b} belongs to two components ({o} and {})",
                        c.id
                    ));
                }
            }
        }
        if let Some(a) = self.active {
            self.component(a)?;
        }
        for (i, o) in self.occurrences.iter().enumerate() {
            if o.id.0 == 0 || o.id.0 >= self.next_occurrence {
                return v(format!("invalid occurrence id {}", o.id));
            }
            if self.occurrences[..i]
                .iter()
                .any(|p| p.id == o.id || p.name == o.name)
            {
                return v(format!("duplicate occurrence {} '{}'", o.id, o.name));
            }
            self.component(o.component)?;
            if let Some(w) = o.owner {
                self.component(w)?;
            }
            o.placement.rigid()?;
        }
        // Containment must be acyclic: no component contains itself.
        for c in &self.components {
            for o in self.children_of(Some(c.id)) {
                if self.contains(o.component, c.id) {
                    return v(format!(
                        "cyclic containment: '{}' contains {} ({}) whose component contains '{}'",
                        c.name, o.id, o.name, c.name
                    ));
                }
            }
        }
        // Depth bound.
        for c in &self.components {
            if self.depth_below(c.id, 0)? > MAX_DEPTH {
                return v(format!(
                    "component '{}' nests deeper than {MAX_DEPTH} levels",
                    c.name
                ));
            }
        }
        for (i, j) in self.joints.iter().enumerate() {
            if j.id.0 == 0 || j.id.0 >= self.next_joint {
                return v(format!("invalid joint id {}", j.id));
            }
            if self.joints[..i].iter().any(|k| k.id == j.id) {
                return v(format!("duplicate joint {}", j.id));
            }
            if j.parent == j.child {
                return v(format!(
                    "{}: an occurrence cannot be jointed to itself",
                    j.id
                ));
            }
            let p = self.occurrence(j.parent)?;
            let c = self.occurrence(j.child)?;
            if p.owner != c.owner {
                return v(format!(
                    "{}: {} and {} are in different (sub-)assemblies; joints connect occurrences \
                     with the same owner",
                    j.id, p.name, c.name
                ));
            }
            if c.grounded {
                return v(format!(
                    "{}: the child {} is grounded (a grounded occurrence cannot be moved by a joint)",
                    j.id, j.child
                ));
            }
            if self.joints[..i].iter().any(|k| k.child == j.child) {
                return v(format!(
                    "{}: {} is already the child of another joint (overconstrained; only tree-structured assemblies are solved)",
                    j.id, j.child
                ));
            }
            if j.kind != JointKind::Rigid {
                j.frame()?;
            }
            if j.extra.len() + 1 > (j.kind.dof() as usize).max(1) {
                return v(format!(
                    "{}: a {:?} joint has {} value(s), {} stored",
                    j.id,
                    j.kind,
                    j.kind.dof(),
                    j.extra.len() + 1
                ));
            }
            j.check_values(&j.values())?;
            j.current_motion()?;
        }
        // Loops: following parents from any occurrence must terminate.
        for o in &self.occurrences {
            let mut cur = o.id;
            for _ in 0..=self.joints.len() {
                match self.joint_of_child(cur) {
                    Some(j) => cur = j.parent,
                    None => break,
                }
                if cur == o.id {
                    return v(format!(
                        "joint loop through {} ({}): only tree-structured assemblies are solved",
                        o.id, o.name
                    ));
                }
            }
        }
        for (k, off) in &self.explode {
            self.check_path(&parse_path(k)?)?;
            if !off.iter().all(|x| x.is_finite()) {
                return v(format!("exploded offset of {k} is not finite"));
            }
        }
        Ok(())
    }

    /// Nesting levels below component `c` (0 when it has no children).
    fn depth_below(&self, c: ComponentId, depth: usize) -> Result<usize> {
        if depth > MAX_DEPTH {
            return Ok(depth);
        }
        let mut best = 0;
        for o in self.children_of(Some(c)) {
            best = best.max(1 + self.depth_below(o.component, depth + 1)?);
        }
        Ok(best)
    }

    /// Local transform of every occurrence relative to its owner (joint
    /// tree solve among siblings).
    pub fn local_transforms(&self) -> Result<BTreeMap<OccurrenceId, Rigid>> {
        let mut memo = BTreeMap::new();
        for o in &self.occurrences {
            self.local(o.id, &mut memo, 0)?;
        }
        Ok(memo)
    }

    fn local(
        &self,
        id: OccurrenceId,
        memo: &mut BTreeMap<OccurrenceId, Rigid>,
        depth: usize,
    ) -> Result<Rigid> {
        if let Some(t) = memo.get(&id) {
            return Ok(*t);
        }
        if depth > self.joints.len() {
            return Err(DocError::Validation(format!("joint loop through {id}")));
        }
        let t = match self.joint_of_child(id) {
            Some(j) => {
                let p = self.local(j.parent, memo, depth + 1)?;
                p.after(&j.current_motion()?).after(&Rigid::from(j.rest))
            }
            None => self.occurrence(id)?.placement.rigid()?,
        };
        memo.insert(id, t);
        Ok(t)
    }

    /// Every instance (including pure sub-assembly instances without bodies),
    /// depth first, top level in document order; suppressed subtrees
    /// excluded.
    pub fn evaluate_tree(&self) -> Result<Vec<EvalOccurrence>> {
        let local = self.local_transforms()?;
        let mut out = Vec::new();
        let mut path = Vec::new();
        let mut names = Vec::new();
        self.expand(
            None,
            Rigid::IDENTITY,
            [0.0; 3],
            true,
            self.active.is_none(),
            &local,
            &mut path,
            &mut names,
            &mut out,
        )?;
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(
        &self,
        owner: Option<ComponentId>,
        t: Rigid,
        off: [f64; 3],
        visible: bool,
        in_active: bool,
        local: &BTreeMap<OccurrenceId, Rigid>,
        path: &mut Vec<OccurrenceId>,
        names: &mut Vec<String>,
        out: &mut Vec<EvalOccurrence>,
    ) -> Result<()> {
        if path.len() > MAX_DEPTH {
            return Err(DocError::Validation(format!(
                "nesting deeper than {MAX_DEPTH} levels at {}",
                path_string(path)
            )));
        }
        for o in self.children_of(owner).filter(|o| !o.suppressed) {
            if out.len() >= MAX_INSTANCES {
                return Err(DocError::ProductLimit(format!(
                    "more than {MAX_INSTANCES} occurrence instances"
                )));
            }
            let lt = local
                .get(&o.id)
                .copied()
                .ok_or_else(|| DocError::Validation(format!("{} is not solved", o.id)))?;
            let w = t.after(&lt);
            path.push(o.id);
            names.push(o.name.clone());
            let key = path_string(path);
            let mine = if self.exploded {
                self.explode.get(&key).copied().unwrap_or([0.0; 3])
            } else {
                [0.0; 3]
            };
            let off2: [f64; 3] = std::array::from_fn(|i| off[i] + mine[i]);
            let vis = visible && !o.hidden;
            let act = in_active || self.active == Some(o.component);
            let j = self.joint_of_child(o.id);
            out.push(EvalOccurrence {
                id: o.id,
                component: o.component,
                name: o.name.clone(),
                transform: w,
                bodies: self.component(o.component)?.bodies.clone(),
                grounded: o.grounded,
                driven_by: j.map(|j| j.id),
                dof: match (o.grounded, j) {
                    (true, _) => 0,
                    (false, Some(j)) => j.dof(),
                    (false, None) => 6,
                },
                path: path.clone(),
                path_name: names.join("/"),
                level: path.len() - 1,
                visible: vis,
                explode: off2,
                in_active: act,
            });
            self.expand(
                Some(o.component),
                w,
                off2,
                vis,
                act,
                local,
                path,
                names,
                out,
            )?;
            path.pop();
            names.pop();
        }
        Ok(())
    }

    /// World placement of every instance whose component has bodies (the
    /// "leaf" placements used by display, mass, interference, export and
    /// drawings), nested ones included, suppressed subtrees excluded.
    pub fn evaluate(&self) -> Result<Vec<EvalOccurrence>> {
        Ok(self
            .evaluate_tree()?
            .into_iter()
            .filter(|e| !e.bodies.is_empty())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_composes_to_identity() {
        let a = Rigid::new([1.0, -2.0, 3.0], [0.5, 0.0, 0.0], [1.0, 1.0, 0.2], 0.7).unwrap();
        let i = a.after(&inverse(&a));
        for r in 0..3 {
            for c in 0..3 {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((i.r[r][c] - want).abs() < 1e-15);
            }
            assert!(i.t[r].abs() < 1e-14);
        }
    }

    #[test]
    fn paths_round_trip_and_reject_garbage() {
        let p = vec![OccurrenceId(3), OccurrenceId(7)];
        assert_eq!(path_string(&p), "O3/O7");
        assert_eq!(parse_path("O3/O7").unwrap(), p);
        for bad in ["", "3/7", "O0", "O3//O7", "Ox"] {
            assert!(parse_path(bad).is_err(), "{bad}");
        }
    }

    fn joint(kind: JointKind) -> Joint {
        Joint {
            id: JointId(1),
            name: "J".into(),
            kind,
            parent: OccurrenceId(1),
            child: OccurrenceId(2),
            origin: [1.0, 2.0, 0.0],
            axis: [0.0, 0.0, 2.0],
            value: 0.0,
            min: None,
            max: None,
            rest: Pose::from(Rigid::IDENTITY),
            extra: vec![],
            dir2: Some([1.0, 0.0, 0.5]),
        }
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-12)
    }

    #[test]
    fn multi_dof_motions_are_closed_form() {
        // Cylindrical: 90° about z through (1,2,0), then +5 along z.
        let m = joint(JointKind::Cylindrical).motion(&[90.0, 5.0]).unwrap();
        // (2,2,0) is 1 from the axis along +x → (1,3,5).
        assert!(near(m.point([2.0, 2.0, 0.0]), [1.0, 3.0, 5.0]));
        // Pin-slot: u = x (dir2 projected ⊥ z): rotate then slide 3 along x.
        let m = joint(JointKind::PinSlot).motion(&[90.0, 3.0]).unwrap();
        assert!(near(m.point([2.0, 2.0, 0.0]), [4.0, 3.0, 0.0]));
        // Planar: u = x, w = z × x = y; slide (2, -1), rotate 180° about z at origin.
        let m = joint(JointKind::Planar)
            .motion(&[2.0, -1.0, 180.0])
            .unwrap();
        assert!(near(m.point([2.0, 2.0, 0.0]), [2.0, 1.0, 0.0]));
        // Ball: 90° about u = x through (1,2,0): (1,3,0) → (1,2,1).
        let m = joint(JointKind::Ball).motion(&[0.0, 90.0, 0.0]).unwrap();
        assert!(near(m.point([1.0, 3.0, 0.0]), [1.0, 2.0, 1.0]));
        // Wrong value count refused.
        assert!(joint(JointKind::Ball).motion(&[1.0]).is_err());
    }

    #[test]
    fn singular_frames_and_limits_are_refused() {
        let mut j = joint(JointKind::PinSlot);
        j.dir2 = Some([0.0, 0.0, 3.0]);
        assert!(j.motion(&[0.0, 0.0]).is_err());
        let mut j = joint(JointKind::Cylindrical);
        j.set_limits(1, Some(0.0), Some(10.0));
        assert!(j.check_values(&[0.0, 5.0]).is_ok());
        assert!(j.check_values(&[0.0, 11.0]).is_err());
        assert!(j.check_values(&[f64::NAN, 1.0]).is_err());
    }
}
