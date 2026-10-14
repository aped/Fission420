//! Transaction and identity behaviour through the
//! public command path — re-attachment ordering, ambiguous face references,
//! sketch-entity ids never reused, and body creation undo/redo with ids,
//! dependencies and geometry checked at every step.
use rcad_commands::{Command, DocSummary, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{BodyId, DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn sketch(s: &mut Session, plane: SketchPlane, t: SketchTemplate) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template: t,
        },
    )
    .feature
    .unwrap()
}

fn rect(w: f64, h: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: Some(c),
    }
}

fn op(sketch: FeatureId, d: f64, op: ExtrudeOp, target: BodyId) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: Some(target),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    }
}

fn face(feature: FeatureId, role: FaceRole) -> SketchPlane {
    SketchPlane::Face(FaceRef { feature, role })
}

fn vol(s: &Session, b: BodyId) -> f64 {
    let b = s.summary().bodies.into_iter().find(|x| x.id == b).unwrap();
    match b.measurements.unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Re-attaching a sketch to a face of a *later* feature of the same body
/// would reorder that body's modifiers silently. It is a dependency cycle,
/// refused with the document unchanged.
#[test]
fn reattach_to_a_later_feature_of_the_same_body_is_refused() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let s1 = sketch(&mut s, SketchPlane::WorldXY, rect(40.0, 40.0, [0.0, 0.0]));
    let e1 = ok(&mut s, Command::extrude(None, s1, 10.0))
        .feature
        .unwrap();
    // Pocket from the top, then a boss elsewhere on the top.
    let s2 = sketch(
        &mut s,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(6.0, 6.0, [-10.0, 0.0]),
    );
    ok(&mut s, op(s2, 3.0, ExtrudeOp::Cut, BodyId(1)));
    let s4 = sketch(
        &mut s,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(10.0, 10.0, [10.0, 0.0]),
    );
    let j5 = ok(&mut s, op(s4, 5.0, ExtrudeOp::Join, BodyId(1)))
        .feature
        .unwrap();
    let v = vol(&s, BodyId(1));
    assert!((v - (16000.0 - 108.0 + 500.0)).abs() < 1e-6, "{v}");
    let before: DocSummary = s.summary();
    // Move the pocket's sketch onto the boss top (a later feature).
    let r = s.execute(&Command::SetSketchPlane {
        feature: s2,
        plane: face(j5, FaceRole::ExtrudeEndCap),
    });
    assert!(!r.ok);
    match r.error {
        Some(DocError::Cycle(ids)) => assert!(ids.contains(&s2), "{ids:?}"),
        other => panic!("expected a cycle, got {other:?}"),
    }
    let after = s.summary();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.features, after.features);
    // Control: a cut whose sketch and consumer come after the boss may be
    // re-attached to the boss top. Cut 4x4x2 up from the bottom (world XY),
    // then move its sketch to the boss top: same volume, floor z 2 -> 13.
    let s6 = sketch(&mut s, SketchPlane::WorldXY, rect(4.0, 4.0, [10.0, 0.0]));
    let c7 = ok(&mut s, op(s6, 2.0, ExtrudeOp::Cut, BodyId(1)))
        .feature
        .unwrap();
    let floor_z = |s: &Session| {
        let f = s
            .summary()
            .features
            .into_iter()
            .find(|f| f.id == c7)
            .unwrap();
        let cap = f
            .faces
            .iter()
            .find(|x| x.reference.role == FaceRole::ExtrudeEndCap)
            .expect("floor face");
        cap.frame.origin[2]
    };
    assert!((floor_z(&s) - 2.0).abs() < 1e-9);
    assert!((vol(&s, BodyId(1)) - (v - 32.0)).abs() < 1e-6);
    ok(
        &mut s,
        Command::SetSketchPlane {
            feature: s6,
            plane: face(j5, FaceRole::ExtrudeEndCap),
        },
    );
    assert!((floor_z(&s) - 13.0).abs() < 1e-9, "{}", floor_z(&s));
    assert!((vol(&s, BodyId(1)) - (v - 32.0)).abs() < 1e-6);
}

/// Two coplanar faces with the same normal where a reference names one face:
/// "ambiguous", refused, never guessed.
#[test]
fn ambiguous_face_reference_is_refused() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let s1 = sketch(&mut s, SketchPlane::WorldXY, rect(40.0, 20.0, [0.0, 0.0]));
    let e1 = ok(&mut s, Command::extrude(None, s1, 10.0))
        .feature
        .unwrap();
    // A slot across the whole width, 4 deep from the top: the top face splits
    // into two coplanar faces.
    let s2 = sketch(
        &mut s,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(4.0, 30.0, [0.0, 0.0]),
    );
    let r = s.execute(&op(s2, 4.0, ExtrudeOp::Cut, BodyId(1)));
    assert!(r.ok, "{:?}", r.error);
    let before = s.summary().revision;
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: face(e1, FaceRole::ExtrudeEndCap),
        template: rect(2.0, 2.0, [10.0, 0.0]),
    });
    match r.error {
        Some(DocError::BrokenReference { detail, .. }) => {
            assert!(detail.contains("ambiguous"), "{detail}")
        }
        other => panic!("expected an ambiguous reference, got {other:?}"),
    }
    assert_eq!(s.summary().revision, before);
}

/// Sketch-entity ids are persistent face names: an edit that removes an
/// entity retires its id; giving that id to new geometry is refused.
#[test]
fn deleted_sketch_entity_ids_are_not_reused() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let s1 = sketch(&mut s, SketchPlane::WorldXY, rect(20.0, 10.0, [0.0, 0.0]));
    let sk = s.summary().features[0].sketch.clone().unwrap();
    // Replace the rectangle by a triangle reusing two of its points; drop line 4.
    let mut tri = sk.clone();
    tri.entities.retain(|e| e.id().0 != 3 && e.id().0 != 4);
    tri.constraints.clear();
    tri.constraints.push(rcad_sketch::Constraint::FixedPoint {
        p: rcad_sketch::PointId(0),
        x: 0.0,
        y: 0.0,
    });
    let (p1, p3) = (rcad_sketch::PointId(1), rcad_sketch::PointId(3));
    // Commit 1: lines 3 and 4 removed, a new closing line 5 added.
    let mut fresh = tri.clone();
    fresh.entities.push(rcad_sketch::Entity::Line {
        id: rcad_sketch::EntityId(5),
        a: p3,
        b: p1,
    });
    ok(
        &mut s,
        Command::EditSketch {
            feature: s1,
            sketch: fresh.clone(),
        },
    );
    let stored = s.summary().features[0].sketch.clone().unwrap();
    assert!(
        stored.retired_entity_ids >= 5,
        "{}",
        stored.retired_entity_ids
    );
    // Commit 2: giving retired id 4 to new geometry is refused, unchanged.
    let rev = s.summary().revision;
    let mut reuse = stored.clone();
    reuse.entities.retain(|e| e.id().0 != 5);
    reuse.entities.push(rcad_sketch::Entity::Line {
        id: rcad_sketch::EntityId(4),
        a: p3,
        b: p1,
    });
    let r = s.execute(&Command::EditSketch {
        feature: s1,
        sketch: reuse,
    });
    match r.error {
        Some(DocError::Validation(m)) => assert!(m.contains("reused"), "{m}"),
        other => panic!("expected a reuse refusal, got {other:?}"),
    }
    assert_eq!(s.summary().revision, rev);
}

/// Scenario A: body creation and modification undone and redone
/// one step at a time; ids, dependencies and volumes checked at each step.
#[test]
fn body_creation_undo_redo_each_step() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sa = sketch(&mut s, SketchPlane::WorldXY, rect(30.0, 20.0, [0.0, 0.0]));
    let ea = ok(&mut s, Command::extrude(None, sa, 10.0))
        .feature
        .unwrap();
    let sb = sketch(&mut s, SketchPlane::WorldXY, rect(10.0, 10.0, [40.0, 0.0]));
    let eb = ok(&mut s, Command::extrude(None, sb, 5.0)).feature.unwrap();
    let sj = sketch(
        &mut s,
        face(ea, FaceRole::ExtrudeEndCap),
        rect(10.0, 10.0, [0.0, 0.0]),
    );
    let ej = ok(&mut s, op(sj, 4.0, ExtrudeOp::Join, BodyId(1)))
        .feature
        .unwrap();
    let sc = sketch(
        &mut s,
        face(eb, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, [40.0, 0.0]),
    );
    let ec = ok(&mut s, op(sc, 2.0, ExtrudeOp::Cut, BodyId(2)))
        .feature
        .unwrap();
    // (feature count, [(body, volume)], dependencies of the last feature)
    type Step = (usize, Vec<(BodyId, f64)>, Vec<FeatureId>);
    let snap = |s: &Session| -> Step {
        let d = s.summary();
        let bodies = d
            .bodies
            .iter()
            .map(|b| (b.id, vol(s, b.id)))
            .collect::<Vec<_>>();
        let deps = d
            .features
            .last()
            .map(|f| f.depends_on.clone())
            .unwrap_or_default();
        (d.features.len(), bodies, deps)
    };
    let a = 6000.0;
    let b = 500.0;
    let expect: Vec<Step> = vec![
        (
            8,
            vec![(BodyId(1), a + 400.0), (BodyId(2), b - 32.0)],
            vec![sc, eb],
        ),
        (7, vec![(BodyId(1), a + 400.0), (BodyId(2), b)], vec![eb]),
        (
            6,
            vec![(BodyId(1), a + 400.0), (BodyId(2), b)],
            vec![sj, ea],
        ),
        (5, vec![(BodyId(1), a), (BodyId(2), b)], vec![ea]),
        (4, vec![(BodyId(1), a), (BodyId(2), b)], vec![sb]),
        (3, vec![(BodyId(1), a)], vec![]),
        (2, vec![(BodyId(1), a)], vec![sa]),
        (1, vec![], vec![]),
    ];
    let close = |got: &Step, want: &Step| {
        got.0 == want.0
            && got.2 == want.2
            && got.1.len() == want.1.len()
            && got
                .1
                .iter()
                .zip(&want.1)
                .all(|(g, w)| g.0 == w.0 && (g.1 - w.1).abs() < 1e-6)
    };
    assert!(close(&snap(&s), &expect[0]), "{:?}", snap(&s));
    for want in &expect[1..] {
        ok(&mut s, Command::Undo);
        let got = snap(&s);
        assert!(close(&got, want), "after undo: {got:?} want {want:?}");
    }
    for want in expect.iter().rev().skip(1) {
        ok(&mut s, Command::Redo);
        let got = snap(&s);
        assert!(close(&got, want), "after redo: {got:?} want {want:?}");
    }
    // Ids are the originals after redo.
    let d = s.summary();
    let ids: Vec<FeatureId> = d.features.iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![sa, ea, sb, eb, sj, ej, sc, ec]);
    assert_eq!(
        d.bodies.iter().map(|b| b.id).collect::<Vec<_>>(),
        vec![BodyId(1), BodyId(2)]
    );
}

/// Editing a sketch so that a dimension bound to an expression disappears
/// (e.g. Trim splitting a dimensioned line) drops that binding and says so,
/// instead of leaving a binding that would refuse every later edit.
#[test]
fn sketch_edit_removing_a_bound_dimension_drops_the_binding() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "w".into(),
            expression: "30".into(),
            unit: None,
            comment: None,
        },
    );
    let sk = sketch(&mut s, SketchPlane::WorldXY, rect(20.0, 10.0, [0.0, 0.0]));
    ok(
        &mut s,
        Command::SetExpression {
            feature: sk,
            name: "width".into(),
            expression: "w".into(),
        },
    );
    let mut edited = s.summary().features[0].sketch.clone().unwrap();
    edited
        .constraints
        .retain(|c| c.dimension().is_none_or(|d| d.name != "width"));
    let r = ok(
        &mut s,
        Command::EditSketch {
            feature: sk,
            sketch: edited,
        },
    );
    assert!(
        r.message.as_deref().unwrap_or("").contains("width = w"),
        "{:?}",
        r.message
    );
    assert!(s.summary().features[0].expressions.is_empty());
    // Later parameter edits still work.
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "w".into(),
            expression: "40".into(),
            unit: None,
            comment: None,
        },
    );
}

/// Parameters on a document with an embedded STEP import: bind the imported
/// box's hole diameter, change it, save, reopen without the source file.
/// Box 40 x 30 x 10 (12000) with a through hole: 12000 - pi r^2 10.
#[test]
fn parameters_drive_a_cut_in_an_imported_step_body_and_survive_reopen() {
    use std::f64::consts::PI;
    let dir = tempfile::tempdir().unwrap();
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/imported_step_hole.rcad.json");
    let mut s = Session::new();
    ok(&mut s, Command::Open { path: src });
    let hole = s.summary().features[1].id;
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "hole_d".into(),
            expression: "6".into(),
            unit: None,
            comment: None,
        },
    );
    ok(
        &mut s,
        Command::SetExpression {
            feature: hole,
            name: "diameter".into(),
            expression: "hole_d".into(),
        },
    );
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "hole_d".into(),
            expression: "8".into(),
            unit: None,
            comment: None,
        },
    );
    let want = 12000.0 - PI * 16.0 * 10.0;
    assert!(
        (vol(&s, BodyId(1)) - want).abs() < 1e-3,
        "{}",
        vol(&s, BodyId(1))
    );
    let out = dir.path().join("imported_param.rcad.json");
    ok(&mut s, Command::Save { path: out.clone() });
    let mut t = Session::new();
    ok(&mut t, Command::Open { path: out });
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((vol(&t, BodyId(1)) - want).abs() < 1e-3);
    ok(
        &mut t,
        Command::SetUserParameter {
            name: "hole_d".into(),
            expression: "5".into(),
            unit: None,
            comment: None,
        },
    );
    assert!((vol(&t, BodyId(1)) - (12000.0 - PI * 6.25 * 10.0)).abs() < 1e-3);
}

/// Editing a bound dimension in the sketch is refused with
/// the expression named (it would otherwise be reverted silently).
#[test]
fn editing_a_bound_dimension_in_the_sketch_is_refused() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "w".into(),
            expression: "30".into(),
            unit: None,
            comment: None,
        },
    );
    let sk = sketch(&mut s, SketchPlane::WorldXY, rect(20.0, 10.0, [0.0, 0.0]));
    ok(
        &mut s,
        Command::SetExpression {
            feature: sk,
            name: "width".into(),
            expression: "w".into(),
        },
    );
    let mut edited = s.summary().features[0].sketch.clone().unwrap();
    edited.set_dimension("width", 44.0).unwrap();
    let rev = s.summary().revision;
    let r = s.execute(&Command::EditSketch {
        feature: sk,
        sketch: edited,
    });
    match r.error {
        Some(DocError::Validation(m)) => assert!(m.contains("bound to the expression 'w'"), "{m}"),
        e => panic!("{e:?}"),
    }
    assert_eq!(s.summary().revision, rev);
}
