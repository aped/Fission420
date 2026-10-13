//! Component / occurrence / joint commands, BOM, interference and assembly
//! export (run 6 pull-forward of Run-7 §1-2; docs/ASSEMBLIES.md).
use crate::{Command, Response};
use rcad_document::assembly::{
    ComponentId, Joint, JointId, JointKind, OccurrenceId, Placement, Pose, inverse,
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
    pub a: OccurrenceId,
    pub b: OccurrenceId,
    pub a_name: String,
    pub b_name: String,
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

/// Execute an assembly command (one transaction for the editing ones).
pub(crate) fn run(m: &mut Model, cmd: &Command, r: &mut Response) -> Result<(), DocError> {
    let mut out = AssemblyOut::default();
    match cmd {
        Command::CreateComponent {
            name,
            bodies,
            part_number,
        } => {
            let name = valid_name(name, "component")?;
            if bodies.is_empty() {
                return Err(v("a component needs at least one body"));
            }
            let (bodies, pn) = (bodies.clone(), part_number.clone().unwrap_or_default());
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
                let c = d
                    .assembly
                    .new_component(name.clone(), bodies.clone(), pn.clone());
                let on = d.assembly.default_occurrence_name(&name);
                let o = d.assembly.new_occurrence(c, on, Placement::default());
                Ok((c, o))
            })?;
            out.component = Some(c);
            out.occurrence = Some(o);
        }
        Command::DeleteComponent { component } => {
            let c = *component;
            let name = m.document().assembly.component(c)?.name.clone();
            m.transact(&format!("Delete component {name}"), |d| {
                let occ: Vec<OccurrenceId> = d
                    .assembly
                    .occurrences
                    .iter()
                    .filter(|o| o.component == c)
                    .map(|o| o.id)
                    .collect();
                if let Some(j) = d
                    .assembly
                    .joints
                    .iter()
                    .find(|j| occ.contains(&j.parent) || occ.contains(&j.child))
                {
                    return Err(v(format!(
                        "joint {} ({}) uses an occurrence of '{name}': delete it first",
                        j.id, j.name
                    )));
                }
                d.assembly.occurrences.retain(|o| o.component != c);
                d.assembly.components.retain(|x| x.id != c);
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
                d.assembly.component(c)?;
                if let Some(x) = d.assembly.components.iter_mut().find(|x| x.id == c) {
                    x.part_number = pn.clone();
                }
                Ok(())
            })?;
        }
        Command::AddOccurrence {
            component,
            name,
            translate,
            axis,
            angle_deg,
        } => {
            let c = *component;
            let p = degrees_placement(*translate, *axis, *angle_deg)?;
            let name = name
                .as_deref()
                .map(|n| valid_name(n, "occurrence"))
                .transpose()?;
            let cname = m.document().assembly.component(c)?.name.clone();
            let o = m.transact(&format!("Add occurrence of {cname}"), |d| {
                let n = match &name {
                    Some(n) => n.clone(),
                    None => d.assembly.default_occurrence_name(&cname),
                };
                if d.assembly.occurrences.iter().any(|o| o.name == n) {
                    return Err(v(format!("another occurrence is named '{n}'")));
                }
                Ok(d.assembly.new_occurrence(c, n, p))
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
            let a = &m.document().assembly;
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
            let name = occ.name.clone();
            m.transact(&format!("Move {name}"), |d| {
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
            // Ground at the current solved pose.
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
        } => {
            let (kind, parent, child) = (*kind, *parent, *child);
            let (origin, axis) = match axis_from {
                Some(f) => {
                    let (o, a) = axis_from_faces(m, parent, f)?;
                    (o, Some(a))
                }
                None => (*origin, *axis),
            };
            let (origin, axis) = (&origin, &axis);
            let a = &m.document().assembly;
            let pn = a.occurrence(parent)?.name.clone();
            let cocc = a.occurrence(child)?;
            if cocc.grounded {
                return Err(v(format!(
                    "{} is grounded: a joint moves its child; make the grounded occurrence the parent",
                    cocc.name
                )));
            }
            if let Some(j) = a.joint_of_child(child) {
                return Err(v(format!(
                    "{} is already the child of joint {} ({}): overconstrained (only trees are solved)",
                    cocc.name, j.id, j.name
                )));
            }
            // Loop: the child must not be an ancestor of the parent.
            let mut cur = parent;
            for _ in 0..=a.joints.len() {
                if cur == child {
                    return Err(v(format!(
                        "joint {pn} → {}: closes a loop; loops are not solved",
                        cocc.name
                    )));
                }
                match a.joint_of_child(cur) {
                    Some(j) => cur = j.parent,
                    None => break,
                }
            }
            let ev = m.evaluation();
            let world = |o: OccurrenceId| {
                ev.occurrences
                    .iter()
                    .find(|x| x.id == o)
                    .map(|x| x.transform)
                    .ok_or_else(|| v(format!("occurrence {o} is not evaluated")))
            };
            let (tp, tc) = (world(parent)?, world(child)?);
            let value = if kind == JointKind::Rigid {
                0.0
            } else {
                *value
            };
            let mut j = Joint {
                id: JointId(0),
                name: String::new(),
                kind,
                parent,
                child,
                origin: *origin,
                axis: axis.unwrap_or([0.0, 0.0, 1.0]),
                value,
                min: *min,
                max: *max,
                rest: Pose::from(Rigid::IDENTITY),
            };
            j.check_value(value)?;
            let rest = inverse(&tp.after(&j.motion(value)?)).after(&tc);
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
        Command::SetJointValue { joint, value } => {
            let (id, value) = (*joint, *value);
            let jn = {
                let j = m.document().assembly.joint(id)?;
                j.check_value(value)?;
                j.name.clone()
            };
            m.transact(&format!("Set {jn} to {value}"), |d| {
                d.assembly.joint_mut(id)?.value = value;
                Ok(())
            })?;
            out.joint = Some(id);
        }
        Command::SetJointLimits { joint, min, max } => {
            let (id, min, max) = (*joint, *min, *max);
            let jn = m.document().assembly.joint(id)?.name.clone();
            m.transact(&format!("Set limits of {jn}"), |d| {
                let j = d.assembly.joint_mut(id)?;
                j.min = min;
                j.max = max;
                j.check_value(j.value)?;
                Ok(())
            })?;
            out.joint = Some(id);
        }
        Command::DeleteJoint { joint } => {
            let id = *joint;
            let j = m.document().assembly.joint(id)?.clone();
            // The child keeps its current world pose as a free placement.
            let t = m
                .evaluation()
                .occurrences
                .iter()
                .find(|o| o.id == j.child)
                .map(|o| o.transform)
                .unwrap_or(Rigid::IDENTITY);
            let p = placement_from_rigid(&t);
            m.transact(&format!("Delete joint {}", j.name), |d| {
                d.assembly.joints.retain(|x| x.id != id);
                d.assembly.occurrence_mut(j.child)?.placement = p;
                Ok(())
            })?;
        }
        Command::Bom => {
            out.bom = Some(bom(m));
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

/// Flat BOM: one row per component with at least one occurrence (quantity =
/// its occurrences), then one row per visible evaluated body that belongs to no
/// component (quantity 1).
pub fn bom(m: &Model) -> Vec<BomRow> {
    let d = m.document();
    let mut rows = Vec::new();
    let material = |bodies: &[BodyId]| {
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
        names.join(", ")
    };
    for c in &d.assembly.components {
        let occ: Vec<String> = d
            .assembly
            .occurrences
            .iter()
            .filter(|o| o.component == c.id)
            .map(|o| o.name.clone())
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
            material: material(&c.bodies),
        });
    }
    for b in &m.evaluation().bodies {
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
            material: material(&[b.id]),
        });
    }
    rows
}

/// Placed B-rep bodies of an occurrence (evaluated, visible definition bodies).
fn placed(m: &Model, o: OccurrenceId) -> Result<Vec<Body>, DocError> {
    let ev = m.evaluation();
    let eo = ev
        .occurrences
        .iter()
        .find(|x| x.id == o)
        .ok_or_else(|| v(format!("unknown occurrence {o}")))?;
    let mut out = Vec::new();
    for b in &eo.bodies {
        let Some(eb) = ev.body_by_id(*b) else {
            continue;
        };
        if !eb.visible {
            continue;
        }
        match &eb.geometry {
            BodyGeometry::Brep(g) => out.push(
                k::transform_body(g, &eo.transform).map_err(|e| v(format!("{}: {e}", eo.name)))?,
            ),
            BodyGeometry::Mesh(_) => {
                return Err(DocError::ProductLimit(format!(
                    "{}: mesh bodies are not supported in interference/assembly export",
                    eo.name
                )));
            }
        }
    }
    Ok(out)
}

/// Samples per axis of the interference membership fallback.
const INTERFERENCE_GRID: usize = 10;

fn pair_status(a: &Body, b: &Body) -> Result<InterferenceStatus, DocError> {
    let (ba, bb) = (
        k::bounds(a).map_err(|e| v(e.to_string()))?,
        k::bounds(b).map_err(|e| v(e.to_string()))?,
    );
    if (0..3).any(|i| ba.max[i] < bb.min[i] || bb.max[i] < ba.min[i]) {
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

/// Pairwise interference between occurrences (default: all). Pairs that are
/// clear or where none was found are omitted.
pub fn interference(
    m: &Model,
    list: Option<&[OccurrenceId]>,
) -> Result<Vec<Interference>, DocError> {
    let ev = m.evaluation();
    let ids: Vec<OccurrenceId> = match list {
        Some(l) => {
            for o in l {
                m.document().assembly.occurrence(*o)?;
            }
            l.to_vec()
        }
        None => ev.occurrences.iter().map(|o| o.id).collect(),
    };
    let name = |o: OccurrenceId| {
        ev.occurrences
            .iter()
            .find(|x| x.id == o)
            .map_or(String::new(), |x| x.name.clone())
    };
    let mut cache: BTreeMap<OccurrenceId, Vec<Body>> = BTreeMap::new();
    for o in &ids {
        cache.insert(*o, placed(m, *o)?);
    }
    let mut out = Vec::new();
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
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
            out.push(Interference {
                a: *a,
                b: *b,
                a_name: name(*a),
                b_name: name(*b),
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
    /// One STEP solid per placed body, named "occurrence" or "occurrence/body".
    Step,
    /// One 3MF object per placed body, same names.
    #[serde(rename = "3mf")]
    ThreeMf,
}

/// Every occurrence's placed bodies plus loose visible bodies, one object each.
fn export(m: &Model, path: &std::path::Path, format: AssemblyFormat) -> Result<String, DocError> {
    let ev = m.evaluation();
    let d = m.document();
    let mut items: Vec<(String, Body)> = Vec::new();
    for o in &ev.occurrences {
        let bodies = placed(m, o.id)?;
        let multi = bodies.len() > 1;
        for (b, id) in bodies.into_iter().zip(o.bodies.iter()) {
            let n = if multi {
                let bn = d.body(*id).map_or(String::new(), |x| x.name.clone());
                format!("{}/{bn}", o.name)
            } else {
                o.name.clone()
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
    pub occurrences: Vec<OccurrenceSummary>,
    pub joints: Vec<Joint>,
    /// Sum of the occurrences' remaining degrees of freedom.
    pub total_dof: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSummary {
    pub id: ComponentId,
    pub name: String,
    pub part_number: String,
    pub bodies: Vec<BodyId>,
    pub occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceSummary {
    pub id: OccurrenceId,
    pub component: ComponentId,
    pub name: String,
    /// Stored placement (translate mm, axis, angle in degrees); a joint child
    /// is placed by its joint instead.
    pub translate: [f64; 3],
    pub axis: [f64; 3],
    pub angle_deg: f64,
    /// Solved world transform.
    pub world: Pose,
    pub grounded: bool,
    pub dof: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<JointId>,
}

pub fn summary(m: &Model) -> AssemblySummary {
    let a = &m.document().assembly;
    let ev = m.evaluation();
    let occurrences: Vec<OccurrenceSummary> = a
        .occurrences
        .iter()
        .map(|o| {
            let e = ev.occurrences.iter().find(|x| x.id == o.id);
            OccurrenceSummary {
                id: o.id,
                component: o.component,
                name: o.name.clone(),
                translate: o.placement.translate,
                axis: o.placement.axis,
                angle_deg: o.placement.angle.to_degrees(),
                world: Pose::from(e.map_or(Rigid::IDENTITY, |e| e.transform)),
                grounded: o.grounded,
                dof: e.map_or(6, |e| e.dof),
                driven_by: e.and_then(|e| e.driven_by),
            }
        })
        .collect();
    AssemblySummary {
        components: a
            .components
            .iter()
            .map(|c| ComponentSummary {
                id: c.id,
                name: c.name.clone(),
                part_number: c.part_number.clone(),
                bodies: c.bodies.clone(),
                occurrences: a.occurrences.iter().filter(|o| o.component == c.id).count(),
            })
            .collect(),
        total_dof: occurrences.iter().map(|o| u32::from(o.dof)).sum(),
        occurrences,
        joints: a.joints.clone(),
    }
}
