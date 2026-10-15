//! Edit Feature (`Command::RedefineFeature`): an extrusion's operation and
//! extent change in one undoable step, checked against closed-form volumes
//! and bounds; downstream features that break are committed flagged; a
//! feature that fails itself is refused with nothing changed.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{
    BodyId, Extent, ExtrudeOptions, FeatureId, FeatureKind, HealthState, Operation, SketchPlane,
};
use rcad_kernel::VolumeResult;

fn rect(w: f64, h: f64) -> Command {
    Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: None,
        },
    }
}

fn kind_of(s: &Session, id: u32) -> FeatureKind {
    s.model()
        .unwrap()
        .document()
        .feature(FeatureId(id))
        .unwrap()
        .kind
        .clone()
}

fn body(s: &Session, id: u32) -> (f64, f64) {
    let ev = s.model().unwrap().evaluation();
    let b = ev.bodies.iter().find(|b| b.id == BodyId(id)).unwrap();
    let m = b.measurements.as_ref().unwrap();
    let VolumeResult::Value { volume, .. } = m.volume else {
        panic!("{:?}", m.volume)
    };
    (volume, m.bounds.min[2])
}

/// F1 40x20 sketch, F2 extrude 10 (Body1), F3 10x10 sketch, F4 extrude 30
/// joined to Body1.
fn base() -> Session {
    let mut s = Session::new();
    for c in [
        Command::NewDocument,
        rect(40.0, 20.0),
        Command::extrude(None, FeatureId(1), 10.0),
        rect(10.0, 10.0),
        Command::extrude(None, FeatureId(3), 30.0),
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    redefine(&mut s, 4, |k| {
        if let FeatureKind::Extrude { operation, .. } = k {
            *operation = Operation::Join { target: BodyId(1) };
        }
    })
    .unwrap();
    s
}

fn redefine(
    s: &mut Session,
    id: u32,
    f: impl FnOnce(&mut FeatureKind),
) -> Result<rcad_commands::Response, Box<rcad_commands::Response>> {
    let mut k = kind_of(s, id);
    f(&mut k);
    let r = s.execute(&Command::RedefineFeature {
        feature: FeatureId(id),
        kind: Box::new(k),
        name: None,
        expressions: None,
    });
    if r.ok { Ok(r) } else { Err(Box::new(r)) }
}

#[test]
fn new_body_to_join_removes_the_body() {
    let s = base();
    let ev = s.model().unwrap().evaluation();
    assert_eq!(
        ev.bodies.len(),
        1,
        "the joined extrusion no longer makes Body2"
    );
    assert_eq!(s.model().unwrap().document().bodies.len(), 1);
    // The sketch plane holds the plate's bottom face, so a Join extrudes away
    // from the material (ADR 0003): 40*20*10 + 10*10*30 below z = 0.
    let (v, z0) = body(&s, 1);
    assert!((v - 11000.0).abs() < 1e-6, "{v}");
    assert!((z0 + 30.0).abs() < 1e-9, "{z0}");
}

#[test]
fn join_to_cut_and_one_side_to_symmetric() {
    let mut s = base();
    let rev = s.model().unwrap().revision;
    redefine(&mut s, 4, |k| {
        if let FeatureKind::Extrude { operation, .. } = k {
            *operation = Operation::Cut { target: BodyId(1) };
        }
    })
    .unwrap();
    // The 10x10 window through the 10 mm plate.
    let (v, _) = body(&s, 1);
    assert!((v - 7000.0).abs() < 1e-6, "{v}");
    assert_eq!(s.model().unwrap().revision, rev + 1, "one undo step");

    // Back to Join, now symmetric about the sketch plane: [-15, 15].
    redefine(&mut s, 4, |k| {
        if let FeatureKind::Extrude {
            operation, options, ..
        } = k
        {
            *operation = Operation::Join { target: BodyId(1) };
            *options = ExtrudeOptions {
                symmetric: true,
                ..Default::default()
            };
        }
    })
    .unwrap();
    let (v, z0) = body(&s, 1);
    // 8000 + 10*10*5 above + 10*10*15 below.
    assert!((v - 10000.0).abs() < 1e-6, "{v}");
    assert!((z0 + 15.0).abs() < 1e-9, "{z0}");

    // Undo restores the previous definition exactly, then the Cut.
    assert!(s.execute(&Command::Undo).ok);
    assert!(matches!(
        kind_of(&s, 4),
        FeatureKind::Extrude {
            operation: Operation::Cut { .. },
            ..
        }
    ));
    assert!((body(&s, 1).0 - 7000.0).abs() < 1e-6);
    assert!(s.execute(&Command::Redo).ok);
    assert!((body(&s, 1).1 + 15.0).abs() < 1e-9);
}

#[test]
fn a_definition_that_fails_itself_is_refused() {
    let mut s = base();
    let before = s.model().unwrap().document().clone();
    let rev = s.model().unwrap().revision;
    let r = redefine(&mut s, 4, |k| {
        if let FeatureKind::Extrude { extent, .. } = k {
            // Through all is cut-only.
            *extent = Extent::ThroughAll;
        }
    });
    assert!(r.is_err());
    assert_eq!(s.model().unwrap().revision, rev);
    assert_eq!(s.model().unwrap().document(), &before);
    // Changing the feature type is refused.
    let r = s.execute(&Command::RedefineFeature {
        feature: FeatureId(4),
        kind: Box::new(kind_of(&s, 1)),
        name: None,
        expressions: None,
    });
    assert!(!r.ok);
}

#[test]
fn dependents_that_break_are_committed_flagged() {
    let mut s = Session::new();
    for c in [
        Command::NewDocument,
        rect(40.0, 20.0),
        Command::extrude(None, FeatureId(1), 10.0),
        rect(10.0, 10.0),
        Command::extrude(None, FeatureId(3), 30.0),
        // A cut into Body2, the 10x10 block.
        Command::CreateSketch {
            name: None,
            plane: rcad_commands::end_cap_of(FeatureId(4)),
            template: SketchTemplate::CenteredCircle {
                diameter: 4.0,
                center: None,
            },
        },
        Command::CutThroughAll {
            name: None,
            sketch: FeatureId(5),
        },
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    // Joining the block into Body1 removes Body2, which F6 cuts.
    let r = redefine(&mut s, 4, |k| {
        if let FeatureKind::Extrude { operation, .. } = k {
            *operation = Operation::Join { target: BodyId(1) };
        }
    })
    .unwrap();
    assert!(r.message.unwrap().contains("now fail"));
    let sum = s.summary();
    let h = |id: u32| {
        sum.features
            .iter()
            .find(|f| f.id == FeatureId(id))
            .unwrap()
            .health
            .state
    };
    assert_eq!(h(4), HealthState::Healthy);
    assert_eq!(h(6), HealthState::Error);
    // Undo always restores the healthy definition.
    assert!(s.execute(&Command::Undo).ok);
    assert!(s.model().unwrap().evaluation().is_healthy());
}
