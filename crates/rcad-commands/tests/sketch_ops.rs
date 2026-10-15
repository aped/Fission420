//! Granular sketch ops (`Command::SketchOp`): a dimensioned plate with a hole
//! built only from JSON ops, refusals that leave the document unchanged, and
//! trim/break through the command layer. Expected values come from the
//! construction (rectangle 40 x 20, hole at (10, 10)).
use rcad_commands::sketch_ops::{PointRef, SketchOp};
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{FeatureId, FeatureKind, SketchPlane};
use rcad_sketch::{Constraint, Dimension, Entity, EntityId, PointId, Sketch};

fn empty_sketch(s: &mut Session) -> FeatureId {
    assert!(s.execute(&Command::NewDocument).ok);
    let mut sk = Sketch::default();
    sk.ensure_origin();
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok, "{r:?}");
    r.feature.unwrap()
}

/// Run an op given as JSON (the scripting form) and return the response.
fn op(s: &mut Session, f: FeatureId, json: &str) -> rcad_commands::Response {
    let op: SketchOp = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    let c = Command::SketchOp { feature: f, op };
    // The command itself round-trips through JSON.
    let back: Command = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(back, c);
    s.execute(&c)
}

fn stored(s: &Session, f: FeatureId) -> Sketch {
    match &s.model().unwrap().document().feature(f).unwrap().kind {
        FeatureKind::Sketch { sketch, .. } => sketch.clone(),
        _ => panic!("not a sketch"),
    }
}

fn revision(s: &Session) -> u64 {
    s.summary().revision
}

#[test]
fn a_dimensioned_plate_with_a_hole_from_granular_ops() {
    let mut s = Session::new();
    let f = empty_sketch(&mut s);
    let origin = stored(&s, f).origin.unwrap().0;
    // Rectangle: corners shared by id.
    let r = op(
        &mut s,
        f,
        &format!(r#"{{"op":"add_line","a":{origin},"b":[38,1]}}"#),
    );
    assert!(r.ok, "{r:?}");
    let sk = stored(&s, f);
    let p1 = sk.points.last().unwrap().id.0;
    for json in [
        format!(r#"{{"op":"add_line","a":{p1},"b":[39,21]}}"#),
        format!(r#"{{"op":"add_line","a":{},"b":[1,19]}}"#, p1 + 1),
        format!(r#"{{"op":"add_line","a":{},"b":{origin}}}"#, p1 + 2),
    ] {
        let r = op(&mut s, f, &json);
        assert!(r.ok, "{json}: {r:?}");
    }
    let sk = stored(&s, f);
    let lines: Vec<u32> = sk.entities.iter().map(|e| e.id().0).collect();
    assert_eq!(lines.len(), 4);
    for (i, l) in lines.iter().enumerate() {
        let c = if i % 2 == 0 { "Horizontal" } else { "Vertical" };
        let r = op(
            &mut s,
            f,
            &format!(r#"{{"op":"add_constraint","constraint":{{"{c}":{{"line":{l}}}}}}}"#),
        );
        assert!(r.ok, "{r:?}");
    }
    // Width bound to a user parameter, height a number.
    assert!(
        s.execute(&Command::SetUserParameter {
            name: "w".into(),
            expression: "40 mm".into(),
            unit: None,
            comment: None,
        })
        .ok
    );
    let r = op(
        &mut s,
        f,
        &format!(
            r#"{{"op":"add_dimension","constraint":{{"HorizontalDistance":{{"a":{origin},"b":{p1},"dim":{{"name":"width","value":38}}}}}},"expression":"w"}}"#
        ),
    );
    assert!(r.ok, "{r:?}");
    let r = op(
        &mut s,
        f,
        &format!(
            r#"{{"op":"add_dimension","constraint":{{"VerticalDistance":{{"a":{p1},"b":{},"dim":{{"name":"","value":20}}}}}}}}"#,
            p1 + 1
        ),
    );
    assert!(r.ok, "{r:?}");
    assert!(
        r.message.as_deref().unwrap().contains("dimension 'd'"),
        "{r:?}"
    );
    assert!(
        r.message.as_deref().unwrap().contains(" 0 DOF left"),
        "{r:?}"
    );
    // Hole: a circle with a fixed centre (via dimensions to the origin).
    let r = op(
        &mut s,
        f,
        r#"{"op":"add_circle","center":[9,11],"radius":3}"#,
    );
    assert!(r.ok, "{r:?}");
    let sk = stored(&s, f);
    let (cid, cpt) = sk
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Circle { id, center, .. } => Some((id.0, center.0)),
            _ => None,
        })
        .unwrap();
    for (kind, name, v) in [
        ("HorizontalDistance", "hx", 10.0),
        ("VerticalDistance", "hy", 10.0),
    ] {
        let json = format!(
            r#"{{"op":"add_dimension","constraint":{{"{kind}":{{"a":{origin},"b":{cpt},"dim":{{"name":"{name}","value":{v}}}}}}}}}"#
        );
        assert!(op(&mut s, f, &json).ok);
    }
    let r = op(
        &mut s,
        f,
        &format!(
            r#"{{"op":"add_dimension","constraint":{{"Diameter":{{"circle":{cid},"dim":{{"name":"hole","value":6}}}}}},"expression":"w / 8"}}"#
        ),
    );
    assert!(r.ok, "{r:?}");
    let sk = stored(&s, f);
    let dims: std::collections::HashMap<String, f64> = sk.dimensions().into_iter().collect();
    assert_eq!(dims["width"], 40.0);
    assert_eq!(dims["hole"], 5.0);
    // Changing the parameter moves the bound dimensions in one step.
    assert!(
        s.execute(&Command::SetUserParameter {
            name: "w".into(),
            expression: "60 mm".into(),
            unit: None,
            comment: None,
        })
        .ok
    );
    let sk = stored(&s, f);
    let dims: std::collections::HashMap<String, f64> = sk.dimensions().into_iter().collect();
    assert_eq!((dims["width"], dims["hole"]), (60.0, 7.5));
    let region = sk.profile_region().unwrap();
    assert_eq!(region.holes.len(), 1);
}

#[test]
fn refused_ops_leave_the_revision_unchanged() {
    let mut s = Session::new();
    let f = empty_sketch(&mut s);
    assert!(op(&mut s, f, r#"{"op":"add_line","a":[0,0],"b":[10,0]}"#).ok);
    let line = stored(&s, f).entities[0].id().0;
    assert!(
        op(
            &mut s,
            f,
            &format!(
                r#"{{"op":"add_constraint","constraint":{{"Horizontal":{{"line":{line}}}}}}}"#
            )
        )
        .ok
    );
    let rev = revision(&s);
    let before = stored(&s, f);
    for json in [
        // Redundant: a second Horizontal.
        format!(r#"{{"op":"add_constraint","constraint":{{"Horizontal":{{"line":{line}}}}}}}"#),
        // Conflicting: Vertical on a horizontal line.
        format!(r#"{{"op":"add_constraint","constraint":{{"Vertical":{{"line":{line}}}}}}}"#),
        // Nothing crosses it.
        format!(r#"{{"op":"break_at","entity":{line},"at":[5,0]}}"#),
        r#"{"op":"delete_entity","entity":99}"#.to_string(),
        r#"{"op":"add_circle","center":[0,0],"radius":-1}"#.to_string(),
        r#"{"op":"add_line","a":77,"b":[1,1]}"#.to_string(),
    ] {
        let r = op(&mut s, f, &json);
        assert!(!r.ok, "{json} should be refused");
        assert!(r.error.is_some());
        assert_eq!(revision(&s), rev, "{json}");
        assert_eq!(stored(&s, f), before, "{json}");
    }
    let r = op(
        &mut s,
        f,
        &format!(r#"{{"op":"add_constraint","constraint":{{"Horizontal":{{"line":{line}}}}}}}"#),
    );
    assert!(
        r.error.unwrap().to_string().contains("over-constrain"),
        "redundancy is named"
    );
}

#[test]
fn trim_and_break_ops_keep_ids_and_undo_in_one_step() {
    let mut s = Session::new();
    let f = empty_sketch(&mut s);
    for json in [
        r#"{"op":"add_line","a":[0,0],"b":[30,0]}"#,
        r#"{"op":"add_line","a":[10,-5],"b":[10,5]}"#,
        r#"{"op":"add_line","a":[20,-5],"b":[20,5]}"#,
    ] {
        assert!(op(&mut s, f, json).ok);
    }
    let h = stored(&s, f).entities[0].id();
    let r = op(
        &mut s,
        f,
        &format!(r#"{{"op":"break_at","entity":{},"at":[15,0]}}"#, h.0),
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(stored(&s, f).entities.len(), 5);
    // Trim the middle piece away: it has no crossing left inside it.
    let mid = EntityId(4);
    let r = op(
        &mut s,
        f,
        &format!(r#"{{"op":"trim","entity":{},"at":[15,0]}}"#, mid.0),
    );
    assert!(r.ok, "{r:?}");
    let sk = stored(&s, f);
    assert!(sk.entity(mid).is_none());
    assert!(sk.entity(h).is_some());
    assert!(r.message.unwrap().contains("removed entities [4]"));
    // A re-added curve never reuses the removed id.
    assert!(op(&mut s, f, r#"{"op":"add_line","a":[0,9],"b":[5,9]}"#).ok);
    assert!(stored(&s, f).entity(mid).is_none());
    assert!(s.execute(&Command::Undo).ok);
    assert!(s.execute(&Command::Undo).ok);
    assert_eq!(stored(&s, f).entities.len(), 5, "undo restores the piece");
    let _ = (
        PointRef::Id(0),
        PointId(0),
        Dimension::new("x", 1.0),
        Constraint::Horizontal { line: h },
    );
}

#[test]
fn replace_commits_sketch_parameters_and_bindings_in_one_step() {
    use rcad_commands::sketch_ops::NewParameter;
    let mut s = Session::new();
    assert!(s.execute(&Command::NewDocument).ok);
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 30.0,
            height: 10.0,
            center: None,
        },
    });
    let f = r.feature.unwrap();
    let mut sk = stored(&s, f);
    sk.set_dimension("width", 40.0).unwrap();
    sk.set_dimension("height", 20.0).unwrap();
    let rev = revision(&s);
    let op = SketchOp::Replace {
        sketch: sk.clone(),
        parameters: vec![NewParameter {
            name: "w".into(),
            expression: "40".into(),
            unit: None,
        }],
        expressions: [
            ("width".to_string(), "w".to_string()),
            ("height".to_string(), "w/2".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let r = s.execute(&Command::SketchOp { feature: f, op });
    assert!(r.ok, "{r:?}");
    assert_eq!(revision(&s), rev + 1, "one step");
    assert!(
        s.execute(&Command::SetUserParameter {
            name: "w".into(),
            expression: "50".into(),
            unit: None,
            comment: None,
        })
        .ok
    );
    let d: std::collections::HashMap<String, f64> =
        stored(&s, f).dimensions().into_iter().collect();
    assert_eq!((d["width"], d["height"]), (50.0, 25.0));
    // Changing a bound dimension without rebinding it is refused.
    let mut sk = stored(&s, f);
    sk.set_dimension("width", 70.0).unwrap();
    let rev = revision(&s);
    let r = s.execute(&Command::SketchOp {
        feature: f,
        op: SketchOp::Replace {
            sketch: sk,
            parameters: vec![],
            expressions: Default::default(),
        },
    });
    assert!(
        !r.ok
            && r.error
                .unwrap()
                .to_string()
                .contains("bound to the expression")
    );
    assert_eq!(revision(&s), rev);
    // One undo removes the parameter and the bindings together.
    assert!(s.execute(&Command::Undo).ok);
    assert!(s.execute(&Command::Undo).ok);
    assert!(s.summary().parameters.is_empty());
}

#[test]
fn fix_and_driven_dimensions_through_ops() {
    let mut s = Session::new();
    let f = empty_sketch(&mut s);
    assert!(op(&mut s, f, r#"{"op":"add_line","a":[0,0],"b":[30,0]}"#).ok);
    let sk = stored(&s, f);
    let Some(Entity::Line { id, a, b }) = sk.entities.first().cloned() else {
        panic!()
    };
    // Fix both ends: fully constrained.
    let r = op(
        &mut s,
        f,
        &format!(r#"{{"op":"fix","points":[{},{}]}}"#, a.0, b.0),
    );
    assert!(
        r.ok && r.message.as_deref().unwrap().contains(" 0 DOF"),
        "{r:?}"
    );
    // A length dimension is now redundant: added driven, measuring 30.
    let r = op(
        &mut s,
        f,
        &format!(
            r#"{{"op":"add_dimension","constraint":{{"Distance":{{"a":{},"b":{},"dim":{{"name":"len","value":30}}}}}}}}"#,
            a.0, b.0
        ),
    );
    assert!(
        r.ok && r.message.as_deref().unwrap().contains("driven"),
        "{r:?}"
    );
    let d = stored(&s, f)
        .constraints
        .iter()
        .filter_map(|c| c.dimension().cloned())
        .find(|d| d.name == "len")
        .unwrap();
    assert!(d.driven && (d.value - 30.0).abs() < 1e-9);
    // Driving while redundant: refused; after unfixing b: allowed.
    let rev = revision(&s);
    assert!(
        !op(
            &mut s,
            f,
            r#"{"op":"set_driven","name":"len","driven":false}"#
        )
        .ok
    );
    assert_eq!(revision(&s), rev);
    assert!(
        op(
            &mut s,
            f,
            &format!(r#"{{"op":"fix","points":[{}],"fix":false}}"#, b.0)
        )
        .ok
    );
    assert!(
        op(
            &mut s,
            f,
            r#"{"op":"set_driven","name":"len","driven":false}"#
        )
        .ok
    );
    let _ = id;
}

/// A parameter edit of a sketch dimension goes through the branch-flip
/// guard: a radius that would push one fillet past the next is refused with
/// the document unchanged; a tenfold width edit keeps every fillet a quarter
/// arc (closed-form area w·h − (4 − π)·r²).
#[test]
fn parameter_edits_are_guarded_against_branch_flips() {
    let mut sk = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]]
        .iter()
        .map(|q| sk.add_point(q[0], q[1]))
        .collect();
    sk.origin = Some(p[0]);
    let l: Vec<EntityId> = (0..4).map(|i| sk.add_line(p[i], p[(i + 1) % 4])).collect();
    sk.constraints.extend([
        Constraint::FixedPoint {
            p: p[0],
            x: 0.0,
            y: 0.0,
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("w", 20.0),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", 10.0),
        },
    ]);
    assert!(sk.solve().is_success());
    for (i, c) in p.iter().enumerate() {
        sk.fillet_corner(*c, 2.0, Some(if i == 0 { "r" } else { "rr" }))
            .unwrap();
    }
    let mut s = Session::new();
    assert!(s.execute(&Command::NewDocument).ok);
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok, "{r:?}");
    let f = r.feature.unwrap();
    let rev = revision(&s);
    let r = s.execute(&Command::SetParameter {
        feature: f,
        name: "r".into(),
        value: 9.0,
    });
    assert!(!r.ok, "{r:?}");
    assert!(
        r.error.unwrap().to_string().contains("would flip"),
        "refused by name"
    );
    assert_eq!(revision(&s), rev);
    let r = s.execute(&Command::SetParameter {
        feature: f,
        name: "w".into(),
        value: 200.0,
    });
    assert!(r.ok, "{r:?}");
    let mut sk = stored(&s, f);
    assert!(sk.solve().is_success());
    let area = sk.regions().unwrap()[0].area;
    let want = 200.0 * 10.0 - (4.0 - std::f64::consts::PI) * 4.0;
    assert!((area - want).abs() < 1e-6, "{area} vs {want}");
}

/// Two lines mirrored about the vertical axis line x = 0 through the
/// `symmetric_entities` op: the second line's ends land on the images of the
/// first's (which is fixed), paired as drawn.
#[test]
fn symmetric_entities_op_mirrors_lines() {
    let mut s = Session::new();
    let f = empty_sketch(&mut s);
    for json in [
        r#"{"op":"add_line","a":[0,-20],"b":[0,20]}"#,
        r#"{"op":"add_line","a":[5,2],"b":[12,9]}"#,
        r#"{"op":"add_line","a":[-6,3],"b":[-11,10]}"#,
    ] {
        assert!(op(&mut s, f, json).ok, "{json}");
    }
    let sk = stored(&s, f);
    let ids: Vec<u32> = sk.entities.iter().map(|e| e.id().0).collect();
    let pts: Vec<u32> = sk
        .points
        .iter()
        .filter(|p| Some(p.id) != sk.origin)
        .map(|p| p.id.0)
        .collect();
    assert!(
        op(
            &mut s,
            f,
            &format!(r#"{{"op":"fix","points":{:?}}}"#, &pts[..4])
        )
        .ok
    );
    let r = op(
        &mut s,
        f,
        &format!(
            r#"{{"op":"symmetric_entities","a":{},"b":{},"line":{}}}"#,
            ids[1], ids[2], ids[0]
        ),
    );
    assert!(r.ok, "{r:?}");
    let sk = stored(&s, f);
    let Some(Entity::Line { a, b, .. }) = sk.entity(EntityId(ids[2])).cloned() else {
        panic!()
    };
    let (pa, pb) = (sk.point_xy(a).unwrap(), sk.point_xy(b).unwrap());
    for (got, want) in [(pa, [-5.0, 2.0]), (pb, [-12.0, 9.0])] {
        assert!(
            (got[0] - want[0]).abs() < 1e-8 && (got[1] - want[1]).abs() < 1e-8,
            "{got:?} vs {want:?}"
        );
    }
}
