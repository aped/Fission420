//! Minimal component / occurrence / joint model (run 6 pull-forward of Run-7
//! §1-2, docs/ASSEMBLIES.md; format v6).
//!
//! * A **component** is a named set of this document's bodies (its definition
//!   bodies, modeled by the ordinary feature history). A body belongs to at most
//!   one component.
//! * An **occurrence** places a component's definition bodies by a rigid
//!   transform. All occurrences of a component are linked: editing the
//!   definition's features changes every occurrence. (An independent copy is a
//!   `MoveBody` copy: a new body with its own history from that point.) While a
//!   body belongs to a component it is shown, measured and exported only through
//!   the component's occurrences.
//! * A **joint** (rigid, revolute, slider) makes its child occurrence follow its
//!   parent: `T_child = T_parent ∘ M(value) ∘ rest`, where `M` is the joint motion
//!   in the parent's definition coordinates and `rest` the child's pose relative to
//!   the parent captured when the joint was made (nothing jumps). Joints form a
//!   forest: every occurrence is the child of at most one joint, and a joint whose
//!   child is an ancestor of its parent (a loop) is refused. Grounded occurrences
//!   cannot be joint children or be moved.
//!
//! Flat only: no nested sub-assemblies, no external components.
use crate::{BodyId, DocError, Result};
use rcad_kernel::Rigid;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: ComponentId,
    pub name: String,
    pub bodies: Vec<BodyId>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub part_number: String,
}

/// Editable placement: rotate `angle` rad about the axis through the origin
/// along `axis`, then translate by `translate` (world, mm).
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
    /// World placement (ignored while the occurrence is a joint child).
    #[serde(default)]
    pub placement: Placement,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grounded: bool,
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
    /// Degrees (revolute) or mm (slider); 0 for rigid.
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Child pose relative to `T_parent ∘ M(value)` at creation.
    pub rest: Pose,
}

impl Joint {
    /// The joint motion `M(v)` in the parent's definition coordinates.
    pub fn motion(&self, v: f64) -> Result<Rigid> {
        let r = match self.kind {
            JointKind::Rigid => Ok(Rigid::IDENTITY),
            JointKind::Revolute => Rigid::new([0.0; 3], self.origin, self.axis, v.to_radians()),
            JointKind::Slider => {
                let n = norm(self.axis);
                Rigid::new(self.axis.map(|a| a / n * v), [0.0; 3], z_axis(), 0.0)
            }
        };
        r.map_err(|e| DocError::Validation(format!("{}: {e}", self.id)))
    }

    /// Whether `v` lies within the limits.
    pub fn check_value(&self, v: f64) -> Result<()> {
        if !v.is_finite() {
            return Err(DocError::Validation(format!(
                "{}: value must be finite",
                self.id
            )));
        }
        if self.kind == JointKind::Rigid && v != 0.0 {
            return Err(DocError::Validation(format!(
                "{}: a rigid joint has no value",
                self.id
            )));
        }
        let unit = if self.kind == JointKind::Revolute {
            "°"
        } else {
            " mm"
        };
        if let Some(m) = self.min
            && v < m
        {
            return Err(DocError::Validation(format!(
                "{} ({}): {v}{unit} is below the lower limit {m}{unit}",
                self.id, self.name
            )));
        }
        if let Some(m) = self.max
            && v > m
        {
            return Err(DocError::Validation(format!(
                "{} ({}): {v}{unit} is above the upper limit {m}{unit}",
                self.id, self.name
            )));
        }
        Ok(())
    }

    /// Degrees of freedom the joint leaves between parent and child.
    pub fn dof(&self) -> u8 {
        match self.kind {
            JointKind::Rigid => 0,
            JointKind::Revolute | JointKind::Slider => 1,
        }
    }
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
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
        }
    }
}

/// Evaluated placement of one occurrence.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalOccurrence {
    pub id: OccurrenceId,
    pub component: ComponentId,
    pub name: String,
    pub transform: Rigid,
    /// The component's definition bodies (placed by `transform`).
    pub bodies: Vec<BodyId>,
    pub grounded: bool,
    /// Joint driving this occurrence, if it is a joint child.
    pub driven_by: Option<JointId>,
    /// Remaining degrees of freedom (0 grounded / rigid child, 1 revolute or
    /// slider child, 6 free).
    pub dof: u8,
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

    /// Structural validity: unique ids below the allocators, unique names,
    /// bodies in at most one component and existing in `bodies`, occurrences of
    /// existing components, joints between distinct existing occurrences, at
    /// most one joint per child, no grounded child, no loops, finite values
    /// within limits.
    pub fn validate(&self, bodies: &[crate::BodyInfo]) -> Result<()> {
        let v = |m: String| Err(DocError::Validation(m));
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
            if c.bodies.is_empty() {
                return v(format!("component '{}' has no bodies", c.name));
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
            o.placement.rigid()?;
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
            self.occurrence(j.parent)?;
            if self.occurrence(j.child)?.grounded {
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
                let n = norm(j.axis);
                if !(n.is_finite() && n > 1e-12) {
                    return v(format!("{}: the joint axis has zero length", j.id));
                }
            }
            if let (Some(a), Some(b)) = (j.min, j.max)
                && (a.is_nan() || b.is_nan() || a > b)
            {
                return v(format!(
                    "{}: the lower limit {a} exceeds the upper limit {b}",
                    j.id
                ));
            }
            j.check_value(j.value)?;
            j.motion(j.value)?;
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
        Ok(())
    }

    /// World transform of every occurrence (tree solve), with DOF.
    pub fn evaluate(&self) -> Result<Vec<EvalOccurrence>> {
        let mut memo: BTreeMap<OccurrenceId, Rigid> = BTreeMap::new();
        let mut out = Vec::with_capacity(self.occurrences.len());
        for o in &self.occurrences {
            let t = self.world(o.id, &mut memo, 0)?;
            let j = self.joint_of_child(o.id);
            out.push(EvalOccurrence {
                id: o.id,
                component: o.component,
                name: o.name.clone(),
                transform: t,
                bodies: self.component(o.component)?.bodies.clone(),
                grounded: o.grounded,
                driven_by: j.map(|j| j.id),
                dof: match (o.grounded, j) {
                    (true, _) => 0,
                    (false, Some(j)) => j.dof(),
                    (false, None) => 6,
                },
            });
        }
        Ok(out)
    }

    fn world(
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
                let p = self.world(j.parent, memo, depth + 1)?;
                p.after(&j.motion(j.value)?).after(&Rigid::from(j.rest))
            }
            None => self.occurrence(id)?.placement.rigid()?,
        };
        memo.insert(id, t);
        Ok(t)
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
}
