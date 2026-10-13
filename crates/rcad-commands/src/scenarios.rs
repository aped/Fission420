//! SESSION_2 core scenarios A and B as plain command data: the editor-style
//! sketches and edits they submit, and the replayable example workflows
//! (`examples/scenario_{a,b}_workflow.json`) generated from them.
//!
//! The headless acceptance checks for these scenarios live in
//! `acceptance::scenarios`; they use the same builders.

use crate::sketches::{self, RectIds};
use crate::{Command, SketchTemplate, end_cap_of};
use rcad_document::{FeatureId, SketchPlane};
use rcad_sketch::{Constraint, Dimension, DragOutcome, Entity, EntityId, PointId, Sketch};
use std::path::Path;

/// Scenario B profile vertices (mm), counter-clockwise; (50, 40) is the
/// chamfer's top end.
pub const CHAMFER_PLATE: [[f64; 2]; 5] = [
    [0.0, 0.0],
    [60.0, 0.0],
    [60.0, 30.0],
    [50.0, 40.0],
    [0.0, 40.0],
];
/// Index (into [`CHAMFER_PLATE`]) of the vertex the scenario moves.
pub const MOVED_VERTEX: usize = 3;
/// Where scenario B moves that vertex.
pub const MOVED_TO: [f64; 2] = [45.0, 40.0];
/// Value the scenario gives the chamfer-length dimension.
pub const CHAMFER_LENGTH: f64 = 12.0;

/// Error from building scenario data (a builder rejected its input).
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioError(pub String);

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScenarioError {}

fn e(x: impl std::fmt::Display) -> ScenarioError {
    ScenarioError(x.to_string())
}

/// Scenario A base sketch: editor rectangle centred on the origin.
pub fn base_rectangle(width: f64, height: f64) -> Result<(Sketch, RectIds), ScenarioError> {
    sketches::centered_rectangle(width, height).map_err(e)
}

/// Scenario A hole sketch: circle centred on the (end-cap) sketch origin.
pub fn hole_circle(diameter: f64) -> Result<Sketch, ScenarioError> {
    sketches::centered_circle(diameter)
        .map(|(s, _)| s)
        .map_err(e)
}

/// The line entity joining `a` and `b` (either direction).
pub fn line_between(s: &Sketch, a: PointId, b: PointId) -> Option<EntityId> {
    s.entities.iter().find_map(|en| match en {
        Entity::Line { id, a: p, b: q } if (*p == a && *q == b) || (*p == b && *q == a) => {
            Some(*id)
        }
        _ => None,
    })
}

/// The vertex point ids of a [`CHAMFER_PLATE`]-style polygon sketch, in
/// order: every non-origin point, by id.
pub fn polygon_vertices(s: &Sketch) -> Vec<PointId> {
    let mut v: Vec<PointId> = s
        .points
        .iter()
        .map(|p| p.id)
        .filter(|&id| Some(id) != s.origin)
        .collect();
    v.sort();
    v
}

/// Scenario B step 1: free (unconstrained) line loop through [`CHAMFER_PLATE`].
pub fn chamfer_plate() -> Result<Sketch, ScenarioError> {
    sketches::polygon(&CHAMFER_PLATE).map(|(s, _)| s).map_err(e)
}

/// Scenario B vertex edit, done like an editor drag of a free vertex.
pub fn move_vertex(mut s: Sketch, vertex: usize, to: [f64; 2]) -> Result<Sketch, ScenarioError> {
    let v = polygon_vertices(&s);
    let p = *v
        .get(vertex)
        .ok_or_else(|| e(format!("no vertex {vertex}")))?;
    let r = s.drag_point(p, to);
    if r.outcome != DragOutcome::Reached {
        return Err(e(format!("vertex drag did not reach target: {r:?}")));
    }
    Ok(s)
}

/// Scenario B driving dimensions on a 5-vertex chamfered plate (vertices
/// v0..v4 as in [`CHAMFER_PLATE`]): v0 on the origin, bottom/top horizontal,
/// right/left vertical, "width" (v0->v1), "height" (v0->v4), "side"
/// (v1->v2, the straight part of the right edge) and the aligned "chamfer"
/// length v2-v3 at its current value. Leaves the sketch fully constrained.
pub fn dimension_chamfer_plate(mut s: Sketch) -> Result<Sketch, ScenarioError> {
    let o = s.ensure_origin();
    let v = polygon_vertices(&s);
    if v.len() != 5 {
        return Err(e(format!("expected 5 vertices, found {}", v.len())));
    }
    let xy = |s: &Sketch, p: PointId| s.point_xy(p).ok_or_else(|| e("missing vertex"));
    let line = |s: &Sketch, a: usize, b: usize| {
        line_between(s, v[a], v[b]).ok_or_else(|| e(format!("no line v{a}-v{b}")))
    };
    let (p0, p1, p2, p3, p4) = (
        xy(&s, v[0])?,
        xy(&s, v[1])?,
        xy(&s, v[2])?,
        xy(&s, v[3])?,
        xy(&s, v[4])?,
    );
    let chamfer = (p3[0] - p2[0]).hypot(p3[1] - p2[1]);
    let geo = [
        Constraint::Coincident { a: o, b: v[0] },
        Constraint::Horizontal {
            line: line(&s, 0, 1)?,
        },
        Constraint::Vertical {
            line: line(&s, 1, 2)?,
        },
        Constraint::Horizontal {
            line: line(&s, 3, 4)?,
        },
        Constraint::Vertical {
            line: line(&s, 4, 0)?,
        },
    ];
    s.constraints.extend(geo);
    let dims = [
        ("width", v[0], v[1], p1[0] - p0[0], 0),
        ("height", v[0], v[4], p4[1] - p0[1], 1),
        ("side", v[1], v[2], p2[1] - p1[1], 1),
    ];
    for (name, a, b, value, axis) in dims {
        let dim = Dimension::new(s.unique_dimension_name(name), value);
        s.constraints.push(if axis == 0 {
            Constraint::HorizontalDistance { a, b, dim }
        } else {
            Constraint::VerticalDistance { a, b, dim }
        });
    }
    let dim = Dimension::new(s.unique_dimension_name("chamfer"), chamfer);
    s.constraints.push(Constraint::Distance {
        a: v[2],
        b: v[3],
        dim,
    });
    s.validate().map_err(e)?;
    Ok(s)
}

/// A self-intersecting ("bow-tie") replacement for the scenario B sketch.
pub fn bowtie() -> Result<Sketch, ScenarioError> {
    sketches::polygon(&[[0.0, 0.0], [60.0, 40.0], [60.0, 0.0], [0.0, 40.0]])
        .map(|(s, _)| s)
        .map_err(e)
}

/// The scenario B loop with its closing line deleted (open polyline).
pub fn open_loop() -> Result<Sketch, ScenarioError> {
    let mut s = chamfer_plate()?;
    let v = polygon_vertices(&s);
    let closing = line_between(&s, v[4], v[0]).ok_or_else(|| e("no closing line"))?;
    s.remove_entity(closing);
    Ok(s)
}

/// One step of a replayable workflow (`rcad-cli run`).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub cmd: Command,
    pub expect_ok: bool,
}

fn ok(cmd: Command) -> Step {
    Step {
        cmd,
        expect_ok: true,
    }
}

fn fails(cmd: Command) -> Step {
    Step {
        cmd,
        expect_ok: false,
    }
}

/// Scenario A as a command script. Saves `doc` and exports STL/STEP under
/// `out` (paths are written as given; relative paths resolve against the
/// working directory of the replaying process).
pub fn scenario_a_workflow(doc: &Path, out: &Path) -> Result<Vec<Step>, ScenarioError> {
    let (base, _) = base_rectangle(80.0, 50.0)?;
    let hole = hole_circle(10.0)?;
    let (s1, e1, s2) = (FeatureId(1), FeatureId(2), FeatureId(3));
    Ok(vec![
        ok(Command::NewDocument),
        ok(Command::CreateSketch {
            name: Some("Base sketch".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: base },
        }),
        ok(Command::extrude(Some("Plate".into()), s1, 10.0)),
        ok(Command::CreateSketch {
            name: Some("Hole sketch".into()),
            plane: end_cap_of(e1),
            template: SketchTemplate::Custom { sketch: hole },
        }),
        ok(Command::CutThroughAll {
            name: Some("Hole".into()),
            sketch: s2,
        }),
        ok(Command::SetParameter {
            feature: s1,
            name: "width".into(),
            value: 100.0,
        }),
        ok(Command::SetParameter {
            feature: e1,
            name: "distance".into(),
            value: 12.0,
        }),
        ok(Command::Undo),
        ok(Command::Undo),
        ok(Command::Redo),
        ok(Command::Redo),
        fails(Command::SetParameter {
            feature: s2,
            name: "diameter".into(),
            value: 50.0,
        }),
        ok(Command::Save { path: doc.into() }),
        ok(Command::Close),
        ok(Command::Open { path: doc.into() }),
        ok(Command::ExportStl {
            path: out.join("scenario_a.stl"),
            chord: Some(0.01),
            bodies: None,
            print: Default::default(),
        }),
        ok(Command::ExportStep {
            path: out.join("scenario_a.step"),
            bodies: None,
        }),
        ok(Command::Inspect),
    ])
}

/// Scenario B as a command script (see [`scenario_a_workflow`] for paths).
pub fn scenario_b_workflow(doc: &Path, out: &Path) -> Result<Vec<Step>, ScenarioError> {
    let s1 = FeatureId(1);
    let plate = chamfer_plate()?;
    let moved = move_vertex(plate.clone(), MOVED_VERTEX, MOVED_TO)?;
    let dimensioned = dimension_chamfer_plate(moved.clone())?;
    Ok(vec![
        ok(Command::NewDocument),
        ok(Command::CreateSketch {
            name: Some("Chamfered profile".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: plate },
        }),
        ok(Command::extrude(Some("Chamfered plate".into()), s1, 5.0)),
        ok(Command::EditSketch {
            feature: s1,
            sketch: moved,
        }),
        ok(Command::EditSketch {
            feature: s1,
            sketch: dimensioned,
        }),
        ok(Command::SetParameter {
            feature: s1,
            name: "chamfer".into(),
            value: CHAMFER_LENGTH,
        }),
        fails(Command::EditSketch {
            feature: s1,
            sketch: bowtie()?,
        }),
        fails(Command::EditSketch {
            feature: s1,
            sketch: open_loop()?,
        }),
        ok(Command::Save { path: doc.into() }),
        ok(Command::Close),
        ok(Command::Open { path: doc.into() }),
        ok(Command::ExportStl {
            path: out.join("scenario_b.stl"),
            chord: Some(0.01),
            bodies: None,
            print: Default::default(),
        }),
        ok(Command::ExportStep {
            path: out.join("scenario_b.step"),
            bodies: None,
        }),
        ok(Command::Inspect),
    ])
}

/// JSON form accepted by `rcad-cli run`: an array of command objects with an
/// optional `"expect_ok": false`.
pub fn workflow_json(steps: &[Step]) -> Result<serde_json::Value, ScenarioError> {
    steps
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(&s.cmd).map_err(e)?;
            if !s.expect_ok
                && let Some(o) = v.as_object_mut()
            {
                o.insert("expect_ok".into(), serde_json::Value::Bool(false));
            }
            Ok(v)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(serde_json::Value::Array)
}

// ---------------------------------------------------------------- scenario R

/// Scenario R rounded rectangle: width, height, initial corner radius,
/// thickness (mm).
pub const ROUNDED_PLATE: [f64; 4] = [80.0, 50.0, 8.0, 10.0];
/// Corner radius the scenario edits to.
pub const ROUNDED_RADIUS_EDIT: f64 = 12.0;
/// Hole centre near the top-right corner: a Ø10 circle there fits inside the
/// 80 x 50 bounding rectangle (5.5 mm to both sides > 5) but crosses the
/// rounded corner (corner arc centre (40 - r, 25 - r)).
pub const CORNER_HOLE_CENTER: [f64; 2] = [34.5, 19.5];
/// Scenario R slot: length between cap centres, radius, thickness (mm).
pub const SLOT_PLATE: [f64; 3] = [60.0, 10.0, 5.0];

/// Loop (0,0) -> (20,0) -> arc to (0,10) through (10,-4) -> (0,0): the arc
/// leaves (20,0) below the x axis and must cross the first line again to
/// reach (0,10).
pub fn self_intersecting_arc_loop() -> Result<Sketch, ScenarioError> {
    let mut s = Sketch::default();
    s.ensure_origin();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(20.0, 0.0);
    let c = s.add_point(0.0, 10.0);
    s.add_line(a, b);
    s.add_arc_3pt(b, c, [10.0, -4.0]).map_err(e)?;
    s.add_line(c, a);
    Ok(s)
}

/// Two chained arcs (0,0) -> (10,0) -> (20,0), not closed.
pub fn open_arc_chain() -> Result<Sketch, ScenarioError> {
    let mut s = Sketch::default();
    s.ensure_origin();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let c = s.add_point(20.0, 0.0);
    s.add_arc_3pt(a, b, [5.0, 5.0]).map_err(e)?;
    s.add_arc_3pt(b, c, [15.0, -5.0]).map_err(e)?;
    Ok(s)
}

/// Scenario R (three-point arcs) as a command script: rounded-rectangle plate,
/// radius edit, centred through hole, a corner hole refused by the containment
/// check, save/reopen/export; then a slot plate; then, in a new document, two
/// refused profiles (self-intersecting line+arc loop, open arc chain). Exports
/// `scenario_r.{stl,step}` and `scenario_r_slot.{stl,step}` under `out`.
pub fn scenario_r_workflow(doc: &Path, out: &Path) -> Result<Vec<Step>, ScenarioError> {
    let [w, h, r, t] = ROUNDED_PLATE;
    let (base, _) = sketches::rounded_rectangle(w, h, r).map_err(e)?;
    let hole = hole_circle(10.0)?;
    let [cx, cy] = CORNER_HOLE_CENTER;
    let (corner, _) = sketches::offset_circle(10.0, cx, cy).map_err(e)?;
    let [len, sr, st] = SLOT_PLATE;
    let (slot, _) = sketches::slot(len, sr).map_err(e)?;
    let (s1, e1, s2, s3) = (FeatureId(1), FeatureId(2), FeatureId(3), FeatureId(5));
    Ok(vec![
        ok(Command::NewDocument),
        ok(Command::CreateSketch {
            name: Some("Rounded base".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: base },
        }),
        ok(Command::extrude(Some("Rounded plate".into()), s1, t)),
        ok(Command::SetParameter {
            feature: s1,
            name: "radius".into(),
            value: ROUNDED_RADIUS_EDIT,
        }),
        ok(Command::CreateSketch {
            name: Some("Hole sketch".into()),
            plane: end_cap_of(e1),
            template: SketchTemplate::Custom { sketch: hole },
        }),
        ok(Command::CutThroughAll {
            name: Some("Hole".into()),
            sketch: s2,
        }),
        ok(Command::CreateSketch {
            name: Some("Corner hole sketch".into()),
            plane: end_cap_of(e1),
            template: SketchTemplate::Custom { sketch: corner },
        }),
        fails(Command::CutThroughAll {
            name: Some("Corner hole".into()),
            sketch: s3,
        }),
        ok(Command::Undo),
        ok(Command::Save { path: doc.into() }),
        ok(Command::Close),
        ok(Command::Open { path: doc.into() }),
        ok(Command::ExportStl {
            path: out.join("scenario_r.stl"),
            chord: Some(0.01),
            bodies: None,
            print: Default::default(),
        }),
        ok(Command::ExportStep {
            path: out.join("scenario_r.step"),
            bodies: None,
        }),
        ok(Command::Inspect),
        ok(Command::NewDocument),
        ok(Command::CreateSketch {
            name: Some("Slot".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: slot },
        }),
        ok(Command::extrude(
            Some("Slot plate".into()),
            FeatureId(1),
            st,
        )),
        ok(Command::ExportStl {
            path: out.join("scenario_r_slot.stl"),
            chord: Some(0.01),
            bodies: None,
            print: Default::default(),
        }),
        ok(Command::ExportStep {
            path: out.join("scenario_r_slot.step"),
            bodies: None,
        }),
        ok(Command::NewDocument),
        ok(Command::CreateSketch {
            name: Some("Crossing arc".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom {
                sketch: self_intersecting_arc_loop()?,
            },
        }),
        fails(Command::extrude(None, FeatureId(1), 5.0)),
        ok(Command::CreateSketch {
            name: Some("Open arcs".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom {
                sketch: open_arc_chain()?,
            },
        }),
        fails(Command::extrude(None, FeatureId(2), 5.0)),
        ok(Command::Inspect),
    ])
}
