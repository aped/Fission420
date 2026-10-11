//! Feature deletion through the shared command API.

use f420_commands::{Command, Session, SketchTemplate, end_cap_of};
use f420_document::{FeatureId, SketchPlane};

fn plate_with_hole() -> Session {
    let mut s = Session::new();
    for c in [
        Command::NewDocument,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 80.0,
                height: 50.0,
                center: None,
            },
        },
        Command::Extrude {
            name: None,
            sketch: FeatureId(1),
            distance: 10.0,
        },
        Command::CreateSketch {
            name: None,
            plane: end_cap_of(FeatureId(2)),
            template: SketchTemplate::CenteredCircle {
                diameter: 10.0,
                center: None,
            },
        },
        Command::CutThroughAll {
            name: None,
            sketch: FeatureId(3),
        },
    ] {
        assert!(s.execute(&c).ok, "{c:?}");
    }
    s
}

fn volume(s: &Session) -> f64 {
    match s
        .model()
        .unwrap()
        .evaluation()
        .measurements
        .as_ref()
        .unwrap()
        .volume
    {
        f420_kernel::VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn delete_refuses_used_features_and_is_one_undo_step() {
    let mut s = plate_with_hole();
    let rev = s.summary().revision;
    // The base sketch is used by the extrusion: refused, document unchanged.
    let r = s.execute(&Command::DeleteFeature {
        feature: FeatureId(1),
    });
    assert!(!r.ok);
    let msg = r.error.unwrap().to_string();
    assert!(msg.contains("used by F2"), "{msg}");
    assert_eq!(s.summary().revision, rev);
    // Deleting the cut, then its sketch, restores the plain plate.
    assert!(
        s.execute(&Command::DeleteFeature {
            feature: FeatureId(4)
        })
        .ok
    );
    assert!((volume(&s) - 40000.0).abs() < 1e-6);
    assert!(
        s.execute(&Command::DeleteFeature {
            feature: FeatureId(3)
        })
        .ok
    );
    assert_eq!(s.summary().features.len(), 2);
    // Each delete is one undo step; undo brings the hole back.
    assert_eq!(s.summary().can_undo.as_deref(), Some("Delete Sketch2"));
    assert!(s.execute(&Command::Undo).ok);
    assert!(s.execute(&Command::Undo).ok);
    let v = 40000.0 - 250.0 * std::f64::consts::PI;
    assert!((volume(&s) - v).abs() < 1e-6);
    // Unknown ids are reported, not panicked on.
    assert!(
        !s.execute(&Command::DeleteFeature {
            feature: FeatureId(99)
        })
        .ok
    );
}
