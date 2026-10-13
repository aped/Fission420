//! Component / occurrence / joint commands, BOM, interference and assembly
//! export (run 6 pull-forward of Run-7 §1-2; docs/ASSEMBLIES.md).
use crate::{Command, Response};
use rcad_document::assembly::{
    ComponentId, Joint, JointId, JointKind, OccurrenceId, Placement, Pose, inverse, parse_path,
    path_string,
};
use rcad_document::{BodyGeometry, BodyId, DocError, Model};
use rcad_kernel::{self as k, Body, Rigid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What an assembly command created or computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AssemblyOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<OccurrenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<JointId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bom: Option<Vec<BomRow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interference: Option<Vec<Interference>>,
    /// Run 7: indented BOM (quantity per parent; see [`structured_bom`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_bom: Option<Vec<StructuredBomRow>>,
}

/// One row of the structured (indented) BOM: a component used under one
/// parent (the top level or a sub-assembly definition), `quantity` =
/// non-suppressed occurrences of it in that parent's definition (per one
/// parent). Children of a sub-assembly row follow it with `level + 1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredBomRow {
    /// "1", "1.2", "1.2.1", ...
    pub item: String,
    pub level: usize,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyId>,
    pub part_number: String,
    pub quantity: usize,
    /// True when the component's definition holds child occurrences.
    pub assembly: bool,
    pub material: String,
}

/// One BOM line (flat): a component and how many occurrences of it the
/// document places, or a loose body (not in any component, quantity 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BomRow {
    pub item: usize,
    /// Component name, or the body name for a loose body.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyId>,
    pub part_number: String,
    pub quantity: usize,
    /// Occurrence names counted (empty for a loose body).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<String>,
    /// Material names of the definition bodies (distinct, "—" when unassigned).
    pub material: String,
}

/// Interference between two occurrences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interference {
    /// Leaf occurrences of the two instances.
    pub a: OccurrenceId,
    pub b: OccurrenceId,
    /// Instance path names ("Sub#1/Bolt#2").
    pub a_name: String,
    pub b_name: String,
    /// Run 7: stable instance paths ("O3/O7") for selection/highlighting.
    #[serde(default)]
    pub a_path: String,
    #[serde(default)]
    pub b_path: String,
    pub status: InterferenceStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InterferenceStatus {
    /// Overlap volume (mm³) of the placed bodies, from Truck's intersection
    /// checked against both inputs; `exact` when the volume method is exact.
    Overlap {
        volume: f64,
        error_estimate: f64,
        exact: bool,
    },
    /// Truck could not build the intersection, but sample points lie inside both
    /// bodies: they overlap; the volume is unknown.
    OverlapVolumeUnknown { reason: String },
    /// Bounding boxes are disjoint: certainly no interference.
    Clear,
    /// Bounds overlap but no intersection solid and no sample inside both: no
    /// interference found (touching or overlaps thinner than the sample spacing
    /// are not detected).
    NoneFound { note: String },
}

fn v(m: impl Into<String>) -> DocError {
    DocError::Validation(m.into())
}

fn valid_name(n: &str, what: &str) -> Result<String, DocError> {
    let t = n.trim();
    if t.is_empty() || t.chars().count() > 64 || t.chars().any(char::is_control) {
        return Err(v(format!("{what} name must be 1-64 printable characters")));
    }
    Ok(t.to_string())
}

/// Rotation matrix → (axis, angle) with `translate` = t (rotation about the origin).
pub fn placement_from_rigid(x: &Rigid) -> Placement {
    let r = x.r;
    let tr = r[0][0] + r[1][1] + r[2][2];
    let c = ((tr - 1.0) / 2.0).clamp(-1.0, 1.0);
    let angle = c.acos();
    let mut axis = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if angle < 1e-12 {
        return Placement {
            translate: x.t,
            ..Placement::default()
        };
    }
    if n < 1e-9 {
        // angle ≈ π: axis from the largest diagonal of (R + I) / 2.
        let i = (0..3)
            .max_by(|&a, &b| r[a][a].total_cmp(&r[b][b]))
            .unwrap_or(0);
        let mut a = [0.0; 3];
        for (j, x) in a.iter_mut().enumerate() {
            *x = (r[i][j] + if i == j { 1.0 } else { 0.0 }) / 2.0;
        }
        let m = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        axis = a.map(|x| x / m);
    } else {
        axis = axis.map(|x| x / n);
    }
    Placement {
        translate: x.t,
        axis,
        angle,
    }
}

fn degrees_placement(
    translate: [f64; 3],
    axis: Option<[f64; 3]>,
    angle_deg: f64,
) -> Result<Placement, DocError> {
    if !angle_deg.is_finite() || angle_deg.abs() > 360.0 {
        return Err(v(format!(
            "the rotation must be within ±360°, got {angle_deg}"
        )));
    }
    let p = Placement {
        translate,
        axis: axis.unwrap_or([0.0, 0.0, 1.0]),
        angle: angle_deg.to_radians(),
    };
    p.rigid()?;
    Ok(p)
}

/// `skip_serializing_if` helper for command indices.
pub(crate) fn is_zero_usize(x: &usize) -> bool {
    *x == 0
}

/// Local transforms (relative to each occurrence's owner) of the current
/// document.
fn locals(m: &Model) -> Result<BTreeMap<OccurrenceId, Rigid>, DocError> {
    m.document().assembly.local_transforms()
}

/// Remove exploded offsets whose path no longer exists.
fn prune_explode(a: &mut rcad_document::assembly::Assembly) {
    let keys: Vec<String> = a.explode.keys().cloned().collect();
    for k in keys {
        let ok = parse_path(&k).is_ok_and(|p| a.check_path(&p).is_ok());
        if !ok {
            a.explode.remove(&k);
        }
    }
}

fn free_to_move(
    a: &rcad_document::assembly::Assembly,
    o: OccurrenceId,
) -> Result<String, DocError> {
    let occ = a.occurrence(o)?;
    if occ.grounded {
        return Err(v(format!(
            "{} is grounded: unground it to move it",
            occ.name
        )));
    }
    if let Some(j) = a.joint_of_child(o) {
        return Err(v(format!(
            "{} is positioned by joint {} ({}): set the joint value instead",
            occ.name, j.id, j.name
        )));
    }
    Ok(occ.name.clone())
}

/// Execute an assembly command (one transaction for the editing ones).
pub(crate) fn run(m: &mut Model, cmd: &Command, r: &mut Response) -> Result<(), DocError> {
    let mut out = AssemblyOut::default();
    match cmd {
        Command::CreateComponent {
            name,
            bodies,
            part_number,
            occurrences,
        } => {
            let name = valid_name(name, "component")?;
            let (bodies, pn) = (bodies.clone(), part_number.clone().unwrap_or_default());
            let occs = occurrences.clone();
            let (c, o) = m.transact(&format!("Create component {name}"), |d| {
                for b in &bodies {
                    d.body(*b)?;
                    if let Some(c) = d.assembly.component_of(*b) {
                        return Err(v(format!(
                            "body {b} already belongs to component '{}'",
                            c.name
                        )));
                    }
                }
                if d.assembly.components.iter().any(|c| c.name == name) {
                    return Err(v(format!("another component is named '{name}'")));
                }
                // Grouped occurrences: siblings, distinct.
                let mut owner = None;
                for (i, oid) in occs.iter().enumerate() {
                    let x = d.assembly.occurrence(*oid)?;
                    if occs[..i].contains(oid) {
                        return Err(v(format!("{} is listed twice", x.name)));
                    }
                    if i == 0 {
                        owner = x.owner;
                    } else if x.owner != owner {
                        return Err(v(format!(
                            "{} is in a different (sub-)assembly: only sibling occurrences can \
                             be grouped",
                            x.name
                        )));
                    }
                }
                if let Some(j) = d
                    .assembly
                    .joints
                    .iter()
                    .find(|j| occs.contains(&j.parent) != occs.contains(&j.child))
                {
                    return Err(v(format!(
                        "joint {} ({}) connects a grouped occurrence with one outside the group: \
                         delete it first",
                        j.id, j.name
                    )));
                }
                // Instance paths change: exploded offsets of the moved
                // occurrences are dropped by `prune_explode`.
                let c = d
                    .assembly
                    .new_component(name.clone(), bodies.clone(), pn.clone());
                let on = d.assembly.default_occurrence_name(&name);
                let o = d.assembly.new_occurrence(c, on, Placement::default());
                d.assembly.occurrence_mut(o)?.owner = owner;
                for oid in &occs {
                    d.assembly.occurrence_mut(*oid)?.owner = Some(c);
                }
                prune_explode(&mut d.assembly);
                Ok((c, o))
            })?;
            out.component = Some(c);
            out.occurrence = Some(o);
        }
        Command::DeleteComponent { component } => {
            let c = *component;
            let name = m.document().assembly.component(c)?.name.clone();
            m.transact(&format!("Delete component {name}"), |d| {
                let a = &d.assembly;
                if let Some(x) = a
                    .components
                    .iter()
                    .find(|x| x.id != c && a.children_of(Some(x.id)).any(|o| o.component == c))
                {
                    return Err(v(format!(
                        "'{name}' is used inside component '{}': delete that occurrence first",
                        x.name
                    )));
                }
                // Its occurrences and its definition's child occurrences go.
                let occ: Vec<OccurrenceId> = a
                    .occurrences
                    .iter()
                    .filter(|o| o.component == c || o.owner == Some(c))
                    .map(|o| o.id)
                    .collect();
                if let Some(j) = a
                    .joints
                    .iter()
                    .find(|j| occ.contains(&j.parent) || occ.contains(&j.child))
                {
                    return Err(v(format!(
                        "joint {} ({}) uses an occurrence of '{name}': delete it first",
                        j.id, j.name
                    )));
                }
                d.assembly.occurrences.retain(|o| !occ.contains(&o.id));
                d.assembly.components.retain(|x| x.id != c);
                for x in &mut d.assembly.components {
                    if x.derived_from == Some(c) {
                        x.derived_from = None;
                    }
                }
                if d.assembly.active == Some(c) {
                    d.assembly.active = None;
                }
                prune_explode(&mut d.assembly);
                Ok(())
            })?;
        }
        Command::SetPartNumber {
            component,
            part_number,
        } => {
            let c = *component;
            let pn = part_number.trim().to_string();
            if pn.chars().count() > 64 || pn.chars().any(char::is_control) {
                return Err(v("part number must be at most 64 printable characters"));
            }
            m.transact("Set part number", |d| {
                d.assembly.component_mut(c)?.part_number = pn.clone();
                Ok(())
            })?;
        }
        Command::RenameComponent { component, name } => {
            let (c, n) = (*component, valid_name(name, "component")?);
            m.transact(&format!("Rename component to {n}"), |d| {
                if d.assembly
                    .components
                    .iter()
                    .any(|x| x.name == n && x.id != c)
                {
                    return Err(v(format!("another component is named '{n}'")));
                }
                d.assembly.component_mut(c)?.name = n.clone();
                Ok(())
            })?;
            out.component = Some(c);
        }
        Command::RenameOccurrence { occurrence, name } => {
            let (o, n) = (*occurrence, valid_name(name, "occurrence")?);
            m.transact(&format!("Rename occurrence to {n}"), |d| {
                if d.assembly
                    .occurrences
                    .iter()
                    .any(|x| x.name == n && x.id != o)
                {
                    return Err(v(format!("another occurrence is named '{n}'")));
                }
                d.assembly.occurrence_mut(o)?.name = n.clone();
                Ok(())
            })?;
            out.occurrence = Some(o);
        }
        Command::ActivateComponent { component } => {
            let c = *component;
            let label = match c {
                Some(c) => format!("Activate {}", m.document().assembly.component(c)?.name),
                None => "Activate top level".to_string(),
            };
            m.transact(&label, |d| {
                d.assembly.active = c;
                Ok(())
            })?;
            out.component = c;
        }
        Command::SetOccurrenceHidden { occurrence, hidden } => {
            let (o, h) = (*occurrence, *hidden);
            let name = m.document().assembly.occurrence(o)?.name.clone();
            m.transact(
                &format!("{} {name}", if h { "Hide" } else { "Show" }),
                |d| {
                    d.assembly.occurrence_mut(o)?.hidden = h;
                    Ok(())
                },
            )?;
            out.occurrence = Some(o);
        }
        Command::AddOccurrence {
            component,
            name,
            owner,
            translate,
            axis,
            angle_deg,
        } => {
            let (c, owner) = (*component, *owner);
            let p = degrees_placement(*translate, *axis, *angle_deg)?;
            let name = name
                .as_deref()
                .map(|n| valid_name(n, "occurrence"))
                .transpose()?;
            let a = &m.document().assembly;
            let cname = a.component(c)?.name.clone();
            if let Some(w) = owner {
                let wn = &a.component(w)?.name;
                if a.contains(c, w) {
                    return Err(v(format!(
                        "'{cname}' contains '{wn}' (or is it): placing it inside '{wn}' would be \
                         cyclic containment"
                    )));
                }
            }
            let o = m.transact(&format!("Add occurrence of {cname}"), |d| {
                let n = match &name {
                    Some(n) => n.clone(),
                    None => d.assembly.default_occurrence_name(&cname),
                };
                if d.assembly.occurrences.iter().any(|o| o.name == n) {
                    return Err(v(format!("another occurrence is named '{n}'")));
                }
                let o = d.assembly.new_occurrence(c, n, p);
                d.assembly.occurrence_mut(o)?.owner = owner;
                Ok(o)
            })?;
            out.occurrence = Some(o);
        }
        Command::SetOccurrenceTransform {
            occurrence,
            translate,
            axis,
            angle_deg,
        } => {
            let o = *occurrence;
            let p = degrees_placement(*translate, *axis, *angle_deg)?;
            let name = free_to_move(&m.document().assembly, o)?;
            m.transact(&format!("Move {name}"), |d| {
                d.assembly.occurrence_mut(o)?.placement = p;
                Ok(())
            })?;
            out.occurrence = Some(o);
        }
        Command::AlignOccurrence {
            occurrence,
            from,
            target,
            to,
            offset,
        } => {
            let (o, t) = (*occurrence, *target);
            let name = free_to_move(&m.document().assembly, o)?;
            let p = align_placement(m, o, *from, t, *to, *offset)?;
            m.transact(&format!("Align {name}"), |d| {
                d.assembly.occurrence_mut(o)?.placement = p;
                Ok(())
            })?;
            out.occurrence = Some(o);
        }
        Command::DeleteOccurrence { occurrence } => {
            let o = *occurrence;
            let a = &m.document().assembly;
            let name = a.occurrence(o)?.name.clone();
            if let Some(j) = a.joints.iter().find(|j| j.parent == o || j.child == o) {
                return Err(v(format!(
                    "joint {} ({}) uses {name}: delete the joint first",
                    j.id, j.name
                )));
            }
            m.transact(&format!("Delete {name}"), |d| {
                d.assembly.occurrences.retain(|x| x.id != o);
                prune_explode(&mut d.assembly);
                Ok(())
            })?;
        }
        Command::SetGrounded {
            occurrence,
            grounded,
        } => {
            let (o, g) = (*occurrence, *grounded);
            let a = &m.document().assembly;
            let name = a.occurrence(o)?.name.clone();
            if g && let Some(j) = a.joint_of_child(o) {
                return Err(v(format!(
                    "{name} is the child of joint {} ({}): a joint child cannot be grounded",
                    j.id, j.name
                )));
            }
            m.transact(
                &format!("{} {name}", if g { "Ground" } else { "Unground" }),
                |d| {
                    d.assembly.occurrence_mut(o)?.grounded = g;
                    Ok(())
                },
            )?;
        }
        Command::AddJoint {
            name,
            kind,
            parent,
            child,
            origin,
            axis,
            axis_from,
            value,
            min,
            max,
            extra,
            dir2,
        } => {
            let (kind, parent, child) = (*kind, *parent, *child);
            let (origin, axis) = match axis_from {
                Some(f) => {
                    let (o, a) = axis_from_faces(m, parent, f)?;
                    (o, Some(a))
                }
                None => (*origin, *axis),
            };
            let mut j = Joint {
                id: JointId(0),
                name: String::new(),
                kind,
                parent,
                child,
                origin,
                axis: axis.unwrap_or([0.0, 0.0, 1.0]),
                value: if kind == JointKind::Rigid {
                    0.0
                } else {
                    *value
                },
                min: *min,
                max: *max,
                rest: Pose::from(Rigid::IDENTITY),
                extra: if kind.dof() > 1 {
                    let mut e = extra.clone();
                    e.resize(kind.dof() as usize - 1, Default::default());
                    e
                } else {
                    vec![]
                },
                dir2: *dir2,
            };
            if kind.dof() <= 1 && !extra.is_empty() {
                return Err(v(format!(
                    "a {kind:?} joint has {} value(s): no extra values",
                    kind.dof()
                )));
            }
            let rest = check_new_joint(m, &j)?;
            j.rest = Pose::from(rest);
            let name = name.clone();
            let id = m.transact(&format!("Add {kind:?} joint"), |d| {
                let id = d.assembly.new_joint_id();
                j.id = id;
                j.name = name.clone().unwrap_or_else(|| format!("Joint{}", id.0));
                d.assembly.joints.push(j.clone());
                Ok(id)
            })?;
            out.joint = Some(id);
        }
        Command::CreateRigidGroup { occurrences } => {
            if occurrences.len() < 2 {
                return Err(v("a rigid group needs at least two occurrences"));
            }
            let first = occurrences[0];
            let mut joints = Vec::new();
            for (i, c) in occurrences.iter().enumerate().skip(1) {
                if occurrences[..i].contains(c) {
                    return Err(v(format!("{c} is listed twice")));
                }
                let j = Joint {
                    id: JointId(0),
                    name: String::new(),
                    kind: JointKind::Rigid,
                    parent: first,
                    child: *c,
                    origin: [0.0; 3],
                    axis: [0.0, 0.0, 1.0],
                    value: 0.0,
                    min: None,
                    max: None,
                    rest: Pose::from(Rigid::IDENTITY),
                    extra: vec![],
                    dir2: None,
                };
                let rest = check_new_joint(m, &j)?;
                joints.push(Joint {
                    rest: Pose::from(rest),
                    ..j
                });
            }
            let fname = m.document().assembly.occurrence(first)?.name.clone();
            let id = m.transact(&format!("Rigid group with {fname}"), |d| {
                let mut last = None;
                for mut j in joints {
                    let id = d.assembly.new_joint_id();
                    j.id = id;
                    j.name = format!("RigidGroup{}", id.0);
                    d.assembly.joints.push(j);
                    last = Some(id);
                }
                Ok(last)
            })?;
            out.joint = id;
        }
        Command::SetJointValue {
            joint,
            value,
            index,
        } => {
            let (id, value, i) = (*joint, *value, *index);
            let j = m.document().assembly.joint(id)?;
            let mut vals = j.values();
            if i >= vals.len() {
                return Err(v(format!(
                    "{} ({}) is {:?} with {} value(s); index {i} does not exist",
                    j.id,
                    j.name,
                    j.kind,
                    vals.len()
                )));
            }
            vals[i] = value;
            set_values(m, id, vals)?;
            out.joint = Some(id);
        }
        Command::SetJointValues { joint, values } => {
            set_values(m, *joint, values.clone())?;
            out.joint = Some(*joint);
        }
        Command::SetJointLimits {
            joint,
            min,
            max,
            index,
        } => {
            let (id, min, max, i) = (*joint, *min, *max, *index);
            let j = m.document().assembly.joint(id)?;
            if i >= (j.kind.dof() as usize).max(1) {
                return Err(v(format!(
                    "{} ({}) has no value {i} to limit",
                    j.id, j.name
                )));
            }
            let jn = j.name.clone();
            m.transact(&format!("Set limits of {jn}"), |d| {
                let j = d.assembly.joint_mut(id)?;
                j.set_limits(i, min, max);
                j.check_values(&j.values())?;
                Ok(())
            })?;
            out.joint = Some(id);
        }
        Command::DeleteJoint { joint } => {
            let id = *joint;
            let j = m.document().assembly.joint(id)?.clone();
            // The child keeps its current local pose as a free placement.
            let t = locals(m)?.get(&j.child).copied().unwrap_or(Rigid::IDENTITY);
            let p = placement_from_rigid(&t);
            m.transact(&format!("Delete joint {}", j.name), |d| {
                d.assembly.joints.retain(|x| x.id != id);
                d.assembly.occurrence_mut(j.child)?.placement = p;
                Ok(())
            })?;
        }
        Command::MakeIndependent { occurrence, name } => {
            let o = *occurrence;
            let a = &m.document().assembly;
            let occ = a.occurrence(o)?.clone();
            let src = a.component(occ.component)?.clone();
            if a.children_of(Some(src.id)).next().is_some() {
                return Err(DocError::ProductLimit(format!(
                    "'{}' is a sub-assembly: independent copies of sub-assemblies are not \
                     supported (copy its parts instead)",
                    src.name
                )));
            }
            if src.bodies.is_empty() {
                return Err(v(format!("'{}' has no bodies to copy", src.name)));
            }
            let cname = match name {
                Some(n) => valid_name(n, "component")?,
                None => (1..)
                    .map(|i| {
                        if i == 1 {
                            format!("{} copy", src.name)
                        } else {
                            format!("{} copy {i}", src.name)
                        }
                    })
                    .find(|n| !a.components.iter().any(|c| &c.name == n))
                    .unwrap_or_default(),
            };
            let c = m.transact(&format!("Make {} independent", occ.name), |d| {
                if d.assembly.components.iter().any(|c| c.name == cname) {
                    return Err(v(format!("another component is named '{cname}'")));
                }
                // Copies must not join the active component.
                let active = d.assembly.active.take();
                let mut copies = Vec::new();
                for b in &src.bodies {
                    let info = d.body(*b)?.clone();
                    let bn = (1..)
                        .map(|i| {
                            format!(
                                "{} ({cname}{})",
                                info.name,
                                if i == 1 {
                                    String::new()
                                } else {
                                    format!(" {i}")
                                }
                            )
                        })
                        .find(|n| !d.bodies.iter().any(|x| &x.name == n))
                        .unwrap_or_default();
                    let id = d.add_body(bn);
                    d.body_mut(id)?.material = info.material.clone();
                    let fname = d.default_name("Copy");
                    d.push(
                        fname,
                        rcad_document::FeatureKind::MoveBody {
                            body: *b,
                            translate: [0.0; 3],
                            origin: [0.0; 3],
                            axis: [0.0, 0.0, 1.0],
                            angle: 0.0,
                            copy: Some(id),
                        },
                    );
                    copies.push(id);
                }
                d.assembly.active = active;
                let c = d
                    .assembly
                    .new_component(cname.clone(), copies, src.part_number.clone());
                d.assembly.component_mut(c)?.derived_from = Some(src.id);
                d.assembly.occurrence_mut(o)?.component = c;
                Ok(c)
            })?;
            out.component = Some(c);
            out.occurrence = Some(o);
        }
        Command::SetExplodeOffset { path, offset } => {
            let p = parse_path(path)?;
            m.document().assembly.check_path(&p)?;
            if !offset.iter().all(|x| x.is_finite() && x.abs() <= 1.0e6) {
                return Err(v("exploded offsets must be finite and within ±1e6 mm"));
            }
            let (key, off) = (path_string(&p), *offset);
            m.transact(&format!("Explode {key}"), |d| {
                if off == [0.0; 3] {
                    d.assembly.explode.remove(&key);
                } else {
                    d.assembly.explode.insert(key.clone(), off);
                }
                Ok(())
            })?;
        }
        Command::SetExploded { exploded } => {
            let e = *exploded;
            m.transact(
                if e {
                    "Show exploded view"
                } else {
                    "Hide exploded view"
                },
                |d| {
                    d.assembly.exploded = e;
                    Ok(())
                },
            )?;
        }
        Command::Bom => {
            out.bom = Some(bom(m));
            out.structured_bom = Some(structured_bom(m)?);
        }
        Command::Interference { occurrences } => {
            out.interference = Some(interference(m, occurrences.as_deref())?);
        }
        Command::ExportAssembly { path, format } => {
            r.message = Some(export(m, path, *format)?);
        }
        _ => return Err(v("internal: not an assembly command")),
    }
    r.assembly = Some(out);
    Ok(())
}

/// Validate a new joint against the current document; return its rest pose
/// (the child keeps its current local pose: as-built).
fn check_new_joint(m: &Model, j: &Joint) -> Result<Rigid, DocError> {
    let a = &m.document().assembly;
    let pocc = a.occurrence(j.parent)?;
    let cocc = a.occurrence(j.child)?;
    let pn = pocc.name.clone();
    if j.parent == j.child {
        return Err(v("an occurrence cannot be jointed to itself"));
    }
    if pocc.owner != cocc.owner {
        return Err(DocError::ProductLimit(format!(
            "{pn} and {} are in different (sub-)assemblies: joints connect occurrences with \
             the same owner",
            cocc.name
        )));
    }
    if cocc.grounded {
        return Err(v(format!(
            "{} is grounded: a joint moves its child; make the grounded occurrence the parent",
            cocc.name
        )));
    }
    if let Some(x) = a.joint_of_child(j.child) {
        return Err(v(format!(
            "{} is already the child of joint {} ({}): overconstrained (only trees are solved)",
            cocc.name, x.id, x.name
        )));
    }
    // Loop: the child must not be an ancestor of the parent.
    let mut cur = j.parent;
    for _ in 0..=a.joints.len() {
        if cur == j.child {
            return Err(v(format!(
                "joint {pn} → {}: closes a loop; loops are not solved",
                cocc.name
            )));
        }
        match a.joint_of_child(cur) {
            Some(x) => cur = x.parent,
            None => break,
        }
    }
    if j.kind != JointKind::Rigid {
        j.frame()?;
    }
    j.check_values(&j.values())?;
    let l = locals(m)?;
    let get = |o: OccurrenceId| {
        l.get(&o)
            .copied()
            .ok_or_else(|| v(format!("occurrence {o} is not solved")))
    };
    let (tp, tc) = (get(j.parent)?, get(j.child)?);
    Ok(inverse(&tp.after(&j.current_motion()?)).after(&tc))
}

fn set_values(m: &mut Model, id: JointId, vals: Vec<f64>) -> Result<(), DocError> {
    let jn = {
        let j = m.document().assembly.joint(id)?;
        j.check_values(&vals)?;
        j.name.clone()
    };
    let label = format!(
        "Set {jn} to {}",
        vals.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    m.transact(&label, |d| {
        let j = d.assembly.joint_mut(id)?;
        for (i, x) in vals.iter().enumerate() {
            j.set_value(i, *x);
        }
        Ok(())
    })?;
    Ok(())
}

/// Placement putting planar face `from` of occurrence `o` onto planar face
/// `to` of sibling `t`, normals opposite, gap `offset` (owner coordinates).
fn align_placement(
    m: &Model,
    o: OccurrenceId,
    from: crate::measure_faces::FacePick,
    t: OccurrenceId,
    to: crate::measure_faces::FacePick,
    offset: f64,
) -> Result<Placement, DocError> {
    use rcad_kernel::DirectFace;
    if !offset.is_finite() {
        return Err(v("the offset must be finite"));
    }
    let a = &m.document().assembly;
    let (oo, to_occ) = (a.occurrence(o)?, a.occurrence(t)?);
    if o == t {
        return Err(v("align an occurrence to another one"));
    }
    if oo.owner != to_occ.owner {
        return Err(DocError::ProductLimit(
            "alignment works between occurrences with the same owner".into(),
        ));
    }
    let own = |occ: &rcad_document::assembly::Occurrence, p: &crate::measure_faces::FacePick| {
        let c = a.component(occ.component)?;
        if c.bodies.contains(&p.body) {
            Ok(())
        } else {
            Err(v(format!(
                "body {} is not in the definition of {} ('{}')",
                p.body, occ.name, c.name
            )))
        }
    };
    own(oo, &from)?;
    own(to_occ, &to)?;
    let plane = |p| match crate::measure_faces::classify(m, p)? {
        DirectFace::Plane { origin, normal } => Ok((origin, normal)),
        _ => Err(v(format!(
            "face {} of body {} is not planar",
            p.face, p.body
        ))),
    };
    let ((pf, nf), (pt, nt)) = (plane(from)?, plane(to)?);
    let l = locals(m)?;
    let (lo, lt) = (
        l.get(&o).copied().unwrap_or(Rigid::IDENTITY),
        l.get(&t).copied().unwrap_or(Rigid::IDENTITY),
    );
    let q = lt.point(pt);
    let mn = lt.vector(nt);
    let c = lo.point(pf);
    let n = lo.vector(nf);
    let want = mn.map(|x| -x);
    let dotp = n[0] * want[0] + n[1] * want[1] + n[2] * want[2];
    let ax = [
        n[1] * want[2] - n[2] * want[1],
        n[2] * want[0] - n[0] * want[2],
        n[0] * want[1] - n[1] * want[0],
    ];
    let s = (ax[0] * ax[0] + ax[1] * ax[1] + ax[2] * ax[2]).sqrt();
    let rot = if s < 1e-12 && dotp > 0.0 {
        Rigid::IDENTITY
    } else if s < 1e-12 {
        // Opposite: half turn about any axis ⊥ n.
        let seed = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let p = [
            n[1] * seed[2] - n[2] * seed[1],
            n[2] * seed[0] - n[0] * seed[2],
            n[0] * seed[1] - n[1] * seed[0],
        ];
        Rigid::new([0.0; 3], c, p, std::f64::consts::PI).map_err(|e| v(e.to_string()))?
    } else {
        Rigid::new([0.0; 3], c, ax, s.atan2(dotp)).map_err(|e| v(e.to_string()))?
    };
    let gap = (c[0] - q[0]) * mn[0] + (c[1] - q[1]) * mn[1] + (c[2] - q[2]) * mn[2];
    let d = offset - gap;
    let shift = Rigid::new(mn.map(|x| x * d), [0.0; 3], [0.0, 0.0, 1.0], 0.0)
        .map_err(|e| v(e.to_string()))?;
    let p = placement_from_rigid(&shift.after(&rot).after(&lo));
    p.rigid()?;
    Ok(p)
}

/// Where a joint axis comes from (faces of the parent's definition bodies,
/// definition coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisFrom {
    /// The axis of a circular cylinder face (hole or boss).
    Cylinder {
        face: crate::measure_faces::FacePick,
    },
    /// The line where two non-parallel planar faces meet (an edge line).
    Edge {
        a: crate::measure_faces::FacePick,
        b: crate::measure_faces::FacePick,
    },
}

fn axis_from_faces(
    m: &Model,
    parent: OccurrenceId,
    f: &AxisFrom,
) -> Result<([f64; 3], [f64; 3]), DocError> {
    use rcad_kernel::DirectFace;
    let a = &m.document().assembly;
    let comp = a.component(a.occurrence(parent)?.component)?;
    let own = |p: &crate::measure_faces::FacePick| {
        if comp.bodies.contains(&p.body) {
            Ok(())
        } else {
            Err(v(format!(
                "face {} of body {} is not on the parent's component '{}'",
                p.face, p.body, comp.name
            )))
        }
    };
    match f {
        AxisFrom::Cylinder { face } => {
            own(face)?;
            match crate::measure_faces::classify(m, *face)? {
                DirectFace::Cylinder { origin, axis, .. } => Ok((origin, axis)),
                _ => Err(v("the joint axis face is not a circular cylinder")),
            }
        }
        AxisFrom::Edge { a: fa, b: fb } => {
            own(fa)?;
            own(fb)?;
            let (
                DirectFace::Plane {
                    origin: p,
                    normal: n,
                },
                DirectFace::Plane {
                    origin: q,
                    normal: k,
                },
            ) = (
                crate::measure_faces::classify(m, *fa)?,
                crate::measure_faces::classify(m, *fb)?,
            )
            else {
                return Err(v("an edge axis needs two planar faces"));
            };
            let d = [
                n[1] * k[2] - n[2] * k[1],
                n[2] * k[0] - n[0] * k[2],
                n[0] * k[1] - n[1] * k[0],
            ];
            let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if dd < 1e-18 {
                return Err(v("the two planar faces are parallel: no edge line"));
            }
            // Point on both planes: x = ((h1 k − h2 n) × d) / |d|² ... solved as
            // x = (h1 (k × d) + h2 (d × n)) / |d|².
            let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
            let cross = |x: [f64; 3], y: [f64; 3]| {
                [
                    x[1] * y[2] - x[2] * y[1],
                    x[2] * y[0] - x[0] * y[2],
                    x[0] * y[1] - x[1] * y[0],
                ]
            };
            let (h1, h2) = (dot(n, p), dot(k, q));
            let (u, w) = (cross(k, d), cross(d, n));
            let o: [f64; 3] = std::array::from_fn(|i| (h1 * u[i] + h2 * w[i]) / dd);
            let len = dd.sqrt();
            Ok((o, d.map(|x| x / len)))
        }
    }
}

fn material_names(m: &Model, bodies: &[BodyId]) -> String {
    let d = m.document();
    let mut names: Vec<String> = bodies
        .iter()
        .filter_map(|b| d.body(*b).ok())
        .map(|b| {
            b.material
                .as_ref()
                .map_or("—".to_string(), |x| x.name.clone())
        })
        .collect();
    names.dedup();
    if names.is_empty() {
        "—".to_string()
    } else {
        names.join(", ")
    }
}

/// Flattened BOM: one row per component with bodies that has at least one
/// evaluated instance anywhere in the tree (quantity = its instances: a part
/// used twice in a sub-assembly that is itself used three times counts 6;
/// suppressed occurrences and everything below them are excluded; pure
/// sub-assemblies without bodies get no row), then one row per visible
/// evaluated body that belongs to no component (quantity 1).
pub fn bom(m: &Model) -> Vec<BomRow> {
    let d = m.document();
    let ev = m.evaluation();
    let mut rows = Vec::new();
    for c in &d.assembly.components {
        let occ: Vec<String> = ev
            .occurrences
            .iter()
            .filter(|o| o.component == c.id)
            .map(|o| o.path_name.clone())
            .collect();
        if occ.is_empty() {
            continue;
        }
        rows.push(BomRow {
            item: rows.len() + 1,
            name: c.name.clone(),
            component: Some(c.id),
            body: None,
            part_number: c.part_number.clone(),
            quantity: occ.len(),
            occurrences: occ,
            material: material_names(m, &c.bodies),
        });
    }
    for b in &ev.bodies {
        if !b.visible || d.assembly.component_of(b.id).is_some() {
            continue;
        }
        rows.push(BomRow {
            item: rows.len() + 1,
            name: b.name.clone(),
            component: None,
            body: Some(b.id),
            part_number: String::new(),
            quantity: 1,
            occurrences: vec![],
            material: material_names(m, &[b.id]),
        });
    }
    rows
}

/// Structured (indented) BOM: under each parent (the top level, then each
/// sub-assembly row's definition) one row per component used there, in first
/// use order, quantity = non-suppressed occurrences of it in that parent
/// (per one parent); loose visible bodies follow at level 0.
pub fn structured_bom(m: &Model) -> Result<Vec<StructuredBomRow>, DocError> {
    let d = m.document();
    let a = &d.assembly;
    fn level(
        m: &Model,
        owner: Option<ComponentId>,
        prefix: &str,
        depth: usize,
        out: &mut Vec<StructuredBomRow>,
    ) -> Result<(), DocError> {
        let a = &m.document().assembly;
        if depth > rcad_document::assembly::MAX_DEPTH {
            return Err(v("nesting too deep"));
        }
        let mut order: Vec<ComponentId> = Vec::new();
        let mut qty: BTreeMap<ComponentId, usize> = BTreeMap::new();
        for o in a.children_of(owner).filter(|o| !o.suppressed) {
            if !order.contains(&o.component) {
                order.push(o.component);
            }
            *qty.entry(o.component).or_default() += 1;
        }
        for (i, c) in order.iter().enumerate() {
            let comp = a.component(*c)?;
            let item = if prefix.is_empty() {
                format!("{}", i + 1)
            } else {
                format!("{prefix}.{}", i + 1)
            };
            let sub = a.children_of(Some(*c)).next().is_some();
            out.push(StructuredBomRow {
                item: item.clone(),
                level: depth,
                name: comp.name.clone(),
                component: Some(*c),
                body: None,
                part_number: comp.part_number.clone(),
                quantity: qty[c],
                assembly: sub,
                material: if comp.bodies.is_empty() {
                    "—".into()
                } else {
                    material_names(m, &comp.bodies)
                },
            });
            if sub {
                level(m, Some(*c), &item, depth + 1, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    level(m, None, "", 0, &mut out)?;
    let mut n = a.children_of(None).filter(|o| !o.suppressed).fold(
        Vec::<ComponentId>::new(),
        |mut v, o| {
            if !v.contains(&o.component) {
                v.push(o.component);
            }
            v
        },
    );
    for b in &m.evaluation().bodies {
        if !b.visible || a.component_of(b.id).is_some() {
            continue;
        }
        n.push(ComponentId(0));
        out.push(StructuredBomRow {
            item: n.len().to_string(),
            level: 0,
            name: b.name.clone(),
            component: None,
            body: Some(b.id),
            part_number: String::new(),
            quantity: 1,
            assembly: false,
            material: material_names(m, &[b.id]),
        });
    }
    Ok(out)
}

/// Placed B-rep bodies of instance `i` of the evaluation (evaluated
/// definition bodies; visibility is display only).
fn placed(m: &Model, i: usize) -> Result<Vec<Body>, DocError> {
    let ev = m.evaluation();
    let eo = ev
        .occurrences
        .get(i)
        .ok_or_else(|| v(format!("unknown instance {i}")))?;
    let mut out = Vec::new();
    for b in &eo.bodies {
        // Review run 6 #7: a definition body that is not evaluated (consumed
        // by a Combine, suppressed, rolled back) is reported, not skipped;
        // hidden bodies count (visibility is display only).
        let Some(eb) = ev.body_by_id(*b) else {
            return Err(v(format!(
                "{}: body {b} of its component is not evaluated (consumed, suppressed or \
                 rolled back)",
                eo.path_name
            )));
        };
        match &eb.geometry {
            BodyGeometry::Brep(g) => out.push(
                k::transform_body(g, &eo.transform)
                    .map_err(|e| v(format!("{}: {e}", eo.path_name)))?,
            ),
            BodyGeometry::Mesh(_) => {
                return Err(DocError::ProductLimit(format!(
                    "{}: mesh bodies are not supported in interference/assembly export",
                    eo.path_name
                )));
            }
        }
    }
    Ok(out)
}

/// Contact tolerance (mm) of the interference check: bounding boxes whose
/// common extent on some axis is at most this are touching, not overlapping.
pub const CONTACT_TOLERANCE: f64 = 1e-6;

/// Samples per axis of the interference membership fallback.
const INTERFERENCE_GRID: usize = 10;

fn pair_status(a: &Body, b: &Body) -> Result<InterferenceStatus, DocError> {
    let (ba, bb) = (
        k::bounds(a).map_err(|e| v(e.to_string()))?,
        k::bounds(b).map_err(|e| v(e.to_string()))?,
    );
    // Disjoint boxes, or boxes that only touch (common extent within the
    // contact tolerance on some axis): no volumetric overlap is possible.
    if (0..3).any(|i| ba.max[i].min(bb.max[i]) - ba.min[i].max(bb.min[i]) <= CONTACT_TOLERANCE) {
        return Ok(InterferenceStatus::Clear);
    }
    match k::boolean(a, b, k::CombineOp::Intersect) {
        Ok(x) => {
            let rep = k::verify_boolean_by_membership(a, b, &x, k::CombineOp::Intersect, 10)
                .map_err(|e| v(e.to_string()))?;
            if !rep.passed() {
                return Ok(InterferenceStatus::OverlapVolumeUnknown {
                    reason: format!(
                        "the kernel's intersection failed the independent check: {}",
                        rep.failure().unwrap_or_default()
                    ),
                });
            }
            match k::measure(&x).map_err(|e| v(e.to_string()))?.volume {
                k::VolumeResult::Value {
                    volume,
                    error_estimate,
                    method,
                } => Ok(InterferenceStatus::Overlap {
                    volume,
                    error_estimate,
                    exact: method != k::VolumeMethod::TessellationEstimate,
                }),
                k::VolumeResult::Unavailable { reason } => {
                    Ok(InterferenceStatus::OverlapVolumeUnknown { reason })
                }
            }
        }
        Err(e) => {
            // No intersection solid: sample the common bounding box.
            let lo: [f64; 3] = std::array::from_fn(|i| ba.min[i].max(bb.min[i]));
            let hi: [f64; 3] = std::array::from_fn(|i| ba.max[i].min(bb.max[i]));
            let n = INTERFERENCE_GRID;
            let pts: Vec<[f64; 3]> = (0..n * n * n)
                .map(|q| {
                    let ijk = [q % n, (q / n) % n, q / (n * n)];
                    std::array::from_fn(|i| {
                        lo[i] + (ijk[i] as f64 + 0.5) / n as f64 * (hi[i] - lo[i])
                    })
                })
                .collect();
            let (ma, mb) = (
                k::point_membership(a, &pts).map_err(|e| v(e.to_string()))?,
                k::point_membership(b, &pts).map_err(|e| v(e.to_string()))?,
            );
            let both = ma
                .iter()
                .zip(&mb)
                .filter(|(x, y)| **x == Some(true) && **y == Some(true))
                .count();
            if both > 0 {
                Ok(InterferenceStatus::OverlapVolumeUnknown {
                    reason: format!(
                        "{both} of {} sample points lie inside both bodies; Truck could not build the intersection ({e})",
                        pts.len()
                    ),
                })
            } else {
                Ok(InterferenceStatus::NoneFound {
                    note: format!(
                        "no intersection solid and none of {} samples inside both (touching contact and overlaps thinner than the sample spacing are not detected)",
                        pts.len()
                    ),
                })
            }
        }
    }
}

/// Pairwise interference between instances (default: all; a listed occurrence
/// selects every instance whose path passes through it). Pairs that are clear
/// or where none was found are omitted. An instance is not checked against
/// itself; a sub-assembly's own bodies are checked against its children.
pub fn interference(
    m: &Model,
    list: Option<&[OccurrenceId]>,
) -> Result<Vec<Interference>, DocError> {
    let ev = m.evaluation();
    if let Some(l) = list {
        for o in l {
            m.document().assembly.occurrence(*o)?;
        }
    }
    let ids: Vec<usize> = (0..ev.occurrences.len())
        .filter(|&i| list.is_none_or(|l| ev.occurrences[i].path.iter().any(|p| l.contains(p))))
        .collect();
    let mut cache: BTreeMap<usize, Vec<Body>> = BTreeMap::new();
    for i in &ids {
        cache.insert(*i, placed(m, *i)?);
    }
    let mut out = Vec::new();
    for (n, a) in ids.iter().enumerate() {
        for b in &ids[n + 1..] {
            let mut total: Option<(f64, f64, bool)> = None;
            let mut unknown: Option<String> = None;
            for x in &cache[a] {
                for y in &cache[b] {
                    match pair_status(x, y)? {
                        InterferenceStatus::Overlap {
                            volume,
                            error_estimate,
                            exact,
                        } => {
                            let t = total.get_or_insert((0.0, 0.0, true));
                            t.0 += volume;
                            t.1 += error_estimate;
                            t.2 &= exact;
                        }
                        InterferenceStatus::OverlapVolumeUnknown { reason } => {
                            unknown.get_or_insert(reason);
                        }
                        _ => {}
                    }
                }
            }
            let status = match (unknown, total) {
                (Some(reason), _) => InterferenceStatus::OverlapVolumeUnknown { reason },
                (None, Some((volume, error_estimate, exact))) => InterferenceStatus::Overlap {
                    volume,
                    error_estimate,
                    exact,
                },
                (None, None) => continue,
            };
            let (ea, eb) = (&ev.occurrences[*a], &ev.occurrences[*b]);
            out.push(Interference {
                a: ea.id,
                b: eb.id,
                a_name: ea.path_name.clone(),
                b_name: eb.path_name.clone(),
                a_path: ea.path_string(),
                b_path: eb.path_string(),
                status,
            });
        }
    }
    Ok(out)
}

/// Assembly export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyFormat {
    /// Flattened: one STEP solid per placed body, named "occurrence" or
    /// "occurrence/body", in a single product (no hierarchy).
    Step,
    /// Run 7: STEP with the product structure: one product per component
    /// (reused definitions written once), sub-assemblies, and every
    /// occurrence as a NEXT_ASSEMBLY_USAGE_OCCURRENCE with its placement.
    StepAssembly,
    /// One 3MF object per placed body, same names.
    #[serde(rename = "3mf")]
    ThreeMf,
}

/// Hierarchical STEP: products per component, usages per occurrence.
fn export_step_tree(m: &Model, path: &std::path::Path) -> Result<String, DocError> {
    let ev = m.evaluation();
    let d = m.document();
    let a = &d.assembly;
    let breps = |bodies: &[BodyId], what: &str| -> Result<Vec<(String, &Body)>, DocError> {
        let mut out = Vec::new();
        for b in bodies {
            let eb = ev.body_by_id(*b).ok_or_else(|| {
                v(format!(
                    "{what}: body {b} is not evaluated (consumed, suppressed or rolled back)"
                ))
            })?;
            match &eb.geometry {
                BodyGeometry::Brep(g) => out.push((eb.name.clone(), g)),
                BodyGeometry::Mesh(_) => {
                    return Err(DocError::ProductLimit(format!(
                        "{what}: mesh bodies cannot be exported as STEP solids"
                    )));
                }
            }
        }
        Ok(out)
    };
    let locals = a.local_transforms()?;
    let mut products: Vec<k::StepProduct> = vec![k::StepProduct {
        name: "Assembly".into(),
        solids: vec![],
        children: vec![],
    }];
    let mut index: BTreeMap<ComponentId, usize> = BTreeMap::new();
    // Components used in the (non-suppressed) tree, parents before children.
    let mut stack: Vec<Option<ComponentId>> = vec![None];
    let mut usages: Vec<(Option<ComponentId>, ComponentId, String, Rigid)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(owner) = stack.pop() {
        if !seen.insert(owner) {
            continue;
        }
        for o in a.children_of(owner).filter(|o| !o.suppressed) {
            let t = locals.get(&o.id).copied().unwrap_or(Rigid::IDENTITY);
            usages.push((owner, o.component, o.name.clone(), t));
            stack.push(Some(o.component));
        }
    }
    for (_, c, _, _) in &usages {
        if index.contains_key(c) {
            continue;
        }
        let comp = a.component(*c)?;
        index.insert(*c, products.len());
        products.push(k::StepProduct {
            name: comp.name.clone(),
            solids: breps(&comp.bodies, &comp.name)?,
            children: vec![],
        });
    }
    let mut skipped = Vec::new();
    for (owner, c, n, t) in usages {
        let ci = index[&c];
        if products[ci].solids.is_empty() && !a.children_of(Some(c)).any(|o| !o.suppressed) {
            skipped.push(n);
            continue;
        }
        let pi = owner.map_or(0, |o| index[&o]);
        products[pi].children.push((ci, n, t));
    }
    // Loose visible bodies: one part each at the top level.
    for b in &ev.bodies {
        if !b.visible || a.component_of(b.id).is_some() {
            continue;
        }
        if let BodyGeometry::Brep(g) = &b.geometry {
            products.push(k::StepProduct {
                name: b.name.clone(),
                solids: vec![(b.name.clone(), g)],
                children: vec![],
            });
            let i = products.len() - 1;
            products[0]
                .children
                .push((i, b.name.clone(), Rigid::IDENTITY));
        }
    }
    // Products only reachable through skipped empty usages are left out.
    let used: std::collections::BTreeSet<usize> = products
        .iter()
        .flat_map(|p| p.children.iter().map(|c| c.0))
        .collect();
    if products[0].children.is_empty() {
        return Err(v("nothing to export"));
    }
    let keep: Vec<usize> = (0..products.len())
        .filter(|i| *i == 0 || used.contains(i))
        .collect();
    let remap: BTreeMap<usize, usize> = keep.iter().enumerate().map(|(n, o)| (*o, n)).collect();
    let products: Vec<k::StepProduct> = keep
        .iter()
        .map(|i| {
            let mut p = products[*i].clone();
            for c in &mut p.children {
                c.0 = remap[&c.0];
            }
            p
        })
        .collect();
    let text = k::export_step_assembly(&products, 0, "R-CAD")
        .map_err(|e| v(format!("STEP export: {e}")))?;
    rcad_document::write_atomic(path, text.as_bytes())?;
    let usages: usize = products.iter().map(|p| p.children.len()).sum();
    Ok(format!(
        "STEP assembly (hierarchy preserved): {} products, {usages} usages{}",
        products.len(),
        if skipped.is_empty() {
            String::new()
        } else {
            format!("; empty occurrences left out: {}", skipped.join(", "))
        }
    ))
}

/// Every occurrence's placed bodies plus loose visible bodies, one object each.
fn export(m: &Model, path: &std::path::Path, format: AssemblyFormat) -> Result<String, DocError> {
    if format == AssemblyFormat::StepAssembly {
        return export_step_tree(m, path);
    }
    let ev = m.evaluation();
    let d = m.document();
    let mut items: Vec<(String, Body)> = Vec::new();
    for (i, o) in ev.occurrences.iter().enumerate() {
        let bodies = placed(m, i)?;
        let multi = bodies.len() > 1;
        for (b, id) in bodies.into_iter().zip(o.bodies.iter()) {
            let n = if multi {
                let bn = d.body(*id).map_or(String::new(), |x| x.name.clone());
                format!("{}/{bn}", o.path_name)
            } else {
                o.path_name.clone()
            };
            items.push((n, b));
        }
    }
    for b in &ev.bodies {
        if !b.visible || d.assembly.component_of(b.id).is_some() {
            continue;
        }
        if let BodyGeometry::Brep(g) = &b.geometry {
            items.push((b.name.clone(), g.clone()));
        }
    }
    if items.is_empty() {
        return Err(v("nothing to export"));
    }
    let bytes = match format {
        AssemblyFormat::StepAssembly => return Err(v("internal: hierarchical STEP")),
        AssemblyFormat::Step => {
            let refs: Vec<(&str, &Body)> = items.iter().map(|(n, b)| (n.as_str(), b)).collect();
            k::export_step_bodies(&refs, "R-CAD")
                .map_err(|e| v(format!("STEP export: {e}")))?
                .into_bytes()
        }
        AssemblyFormat::ThreeMf => {
            let mut objs = Vec::new();
            for (n, b) in &items {
                let tm = k::canonical_mesh(
                    &k::tessellate(b, k::tol::MESH_CHORD).map_err(|e| v(format!("{n}: {e}")))?,
                );
                let c = k::check_mesh(&tm);
                if !c.is_valid_solid() {
                    return Err(v(format!(
                        "{n}: tessellation is not a valid closed solid ({}); nothing written",
                        c.problems().join("; ")
                    )));
                }
                objs.push(rcad_exchange::threemf::MeshObject {
                    name: Some(n.clone()),
                    positions: tm.positions.clone(),
                    triangles: tm.triangles.clone(),
                    color: None,
                });
            }
            rcad_exchange::threemf::write(&objs, rcad_exchange::threemf::ThreeMfUnit::Millimeter)
                .map_err(|e| v(format!("3MF export: {e}")))?
        }
    };
    rcad_document::write_atomic(path, &bytes)?;
    Ok(format!(
        "{} objects: {}",
        items.len(),
        items
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Inspectable assembly state (`DocSummary::assembly`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AssemblySummary {
    pub components: Vec<ComponentSummary>,
    /// Every occurrence (definition level: top level and inside components).
    pub occurrences: Vec<OccurrenceSummary>,
    pub joints: Vec<Joint>,
    /// Sum of the non-suppressed occurrences' remaining degrees of freedom
    /// (per definition occurrence, not per instance).
    pub total_dof: u32,
    /// Run 7: every evaluated instance (tree order, sub-assemblies included).
    #[serde(default)]
    pub instances: Vec<InstanceSummary>,
    /// Run 7: the active component.
    #[serde(default)]
    pub active: Option<ComponentId>,
    /// Run 7: exploded view shown, and the offsets per instance path.
    #[serde(default)]
    pub exploded: bool,
    #[serde(default)]
    pub explode: BTreeMap<String, [f64; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSummary {
    pub id: ComponentId,
    pub name: String,
    pub part_number: String,
    pub bodies: Vec<BodyId>,
    /// Non-suppressed occurrences of it (definition level).
    pub occurrences: usize,
    /// Run 7: evaluated instances through the whole tree (flattened count).
    #[serde(default)]
    pub instances: usize,
    /// Run 7: occurrences inside its definition (sub-assembly).
    #[serde(default)]
    pub children: Vec<OccurrenceId>,
    /// Run 7: made as an independent copy of this component.
    #[serde(default)]
    pub derived_from: Option<ComponentId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceSummary {
    pub id: OccurrenceId,
    pub component: ComponentId,
    pub name: String,
    /// Stored placement relative to the owner (translate mm, axis, angle in
    /// degrees); a joint child is placed by its joint instead.
    pub translate: [f64; 3],
    pub axis: [f64; 3],
    pub angle_deg: f64,
    /// Solved world transform of its first instance (its local transform when
    /// it has none, e.g. suppressed).
    pub world: Pose,
    pub grounded: bool,
    pub dof: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<JointId>,
    /// Run 7: owner component (None = top level).
    #[serde(default)]
    pub owner: Option<ComponentId>,
    /// Run 7: solved transform relative to the owner.
    #[serde(default = "identity_pose")]
    pub local: Pose,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub suppressed: bool,
}

fn identity_pose() -> Pose {
    Pose::from(Rigid::IDENTITY)
}

/// One evaluated instance (run 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceSummary {
    /// "O3/O7".
    pub path: String,
    /// "Sub#1/Bolt#2".
    pub path_name: String,
    pub occurrence: OccurrenceId,
    pub component: ComponentId,
    pub level: usize,
    pub world: Pose,
    pub visible: bool,
    pub in_active: bool,
    /// Display offset of the exploded view (zero when off).
    pub explode: [f64; 3],
}

pub fn summary(m: &Model) -> AssemblySummary {
    let a = &m.document().assembly;
    let ev = m.evaluation();
    let locals = a.local_transforms().unwrap_or_default();
    let tree = a.evaluate_tree().unwrap_or_default();
    let occurrences: Vec<OccurrenceSummary> = a
        .occurrences
        .iter()
        .map(|o| {
            let e = tree.iter().find(|x| x.id == o.id);
            let j = a.joint_of_child(o.id);
            let l = locals.get(&o.id).copied().unwrap_or(Rigid::IDENTITY);
            OccurrenceSummary {
                id: o.id,
                component: o.component,
                name: o.name.clone(),
                translate: o.placement.translate,
                axis: o.placement.axis,
                angle_deg: o.placement.angle.to_degrees(),
                world: Pose::from(e.map_or(l, |e| e.transform)),
                grounded: o.grounded,
                dof: match (o.grounded, j) {
                    (true, _) => 0,
                    (false, Some(j)) => j.dof(),
                    (false, None) => 6,
                },
                driven_by: j.map(|j| j.id),
                owner: o.owner,
                local: Pose::from(l),
                hidden: o.hidden,
                suppressed: o.suppressed,
            }
        })
        .collect();
    let _ = ev;
    AssemblySummary {
        components: a
            .components
            .iter()
            .map(|c| ComponentSummary {
                id: c.id,
                name: c.name.clone(),
                part_number: c.part_number.clone(),
                bodies: c.bodies.clone(),
                occurrences: a
                    .occurrences
                    .iter()
                    .filter(|o| o.component == c.id && !o.suppressed)
                    .count(),
                instances: tree.iter().filter(|e| e.component == c.id).count(),
                children: a.children_of(Some(c.id)).map(|o| o.id).collect(),
                derived_from: c.derived_from,
            })
            .collect(),
        total_dof: occurrences
            .iter()
            .filter(|o| !o.suppressed)
            .map(|o| u32::from(o.dof))
            .sum(),
        occurrences,
        joints: a.joints.clone(),
        instances: tree
            .iter()
            .map(|e| InstanceSummary {
                path: e.path_string(),
                path_name: e.path_name.clone(),
                occurrence: e.id,
                component: e.component,
                level: e.level,
                world: Pose::from(e.transform),
                visible: e.visible,
                in_active: e.in_active,
                explode: e.explode,
            })
            .collect(),
        active: a.active,
        exploded: a.exploded,
        explode: a.explode.clone(),
    }
}
