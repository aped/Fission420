//! A panic while a feature is evaluated (a kernel defect; injected here by
//! the test-only hook: a feature named `__panic__`) fails that feature like
//! any other error: the document opens with it flagged, unrelated bodies are
//! exact, an edit that would trigger it is refused with the model unchanged,
//! and the session keeps working.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{FeatureId, HealthState, Model, SketchPlane};

fn rect(w: f64, h: f64, center: [f64; 2]) -> Command {
    Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: Some(center),
        },
    }
}

/// F1/F2 a 10 x 10 x 10 block (Body1), F3/F4 a separate 20 x 10 x 10 block (Body2).
fn two_blocks() -> Session {
    let mut s = Session::new();
    for c in [
        Command::NewDocument,
        rect(10.0, 10.0, [0.0, 0.0]),
        Command::extrude(None, FeatureId(1), 10.0),
        rect(20.0, 10.0, [50.0, 0.0]),
        Command::extrude(None, FeatureId(3), 10.0),
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    s
}

#[test]
fn a_panicking_feature_fails_alone_and_the_document_opens() {
    let s = two_blocks();
    let mut doc = s.model().unwrap().document().clone();
    doc.feature_mut(FeatureId(2)).unwrap().name = "__panic__".into();
    let (m, report) = Model::open_document(doc, false).unwrap();
    let r = &m.evaluation().features[1];
    assert_eq!(r.health.state, HealthState::Error);
    let msg = r.health.message.clone().unwrap_or_default();
    assert!(msg.contains("internal error"), "{msg}");
    assert_eq!(report.failures.len(), 1);
    // The other block is evaluated and exact.
    let b2 = m
        .evaluation()
        .bodies
        .iter()
        .find(|b| b.id == rcad_document::BodyId(2))
        .unwrap();
    match b2.measurements.as_ref().unwrap().volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => {
            assert!((volume - 2000.0).abs() < 1e-6, "{volume}")
        }
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn an_edit_that_panics_is_refused_and_the_session_survives() {
    let mut s = two_blocks();
    let before = s.model().unwrap().document().clone();
    let r = s.execute(&Command::RenameFeature {
        feature: FeatureId(4),
        name: "__panic__".into(),
    });
    assert!(!r.ok, "a newly failing feature is refused");
    assert_eq!(s.model().unwrap().document(), &before);
    // Still usable.
    let r = s.execute(&Command::extrude(None, FeatureId(1), 5.0));
    assert!(r.ok, "{:?}", r.error);
}
