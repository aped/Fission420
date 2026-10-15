//! JSON shapes of the Edit Feature, sketch-op and open-with-errors commands
//! (fixed so the UI, CLI and scripts agree on them).
use rcad_commands::sketch_ops::SketchOp;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{FeatureHealth, FeatureId, HealthState, SketchPlane};

fn round_trip(c: &Command) -> Command {
    let j = serde_json::to_string(c).unwrap();
    serde_json::from_str(&j).unwrap()
}

#[test]
fn open_defaults_to_normal_mode() {
    let c: Command = serde_json::from_str(r#"{"cmd":"open","path":"a.rcad.json"}"#).unwrap();
    assert_eq!(
        c,
        Command::Open {
            path: "a.rcad.json".into(),
            safe_mode: false
        }
    );
    let s = Command::Open {
        path: "b".into(),
        safe_mode: true,
    };
    assert_eq!(round_trip(&s), s);
}

#[test]
fn sketch_op_round_trips() {
    let c = Command::SketchOp {
        feature: FeatureId(3),
        op: SketchOp::Trim {
            entity: 2,
            at: [1.5, -2.0],
        },
    };
    let j = serde_json::to_string(&c).unwrap();
    assert!(
        j.contains(r#""cmd":"sketch_op""#) && j.contains(r#""op":"trim""#),
        "{j}"
    );
    assert_eq!(round_trip(&c), c);
}

#[test]
fn redefine_feature_round_trips() {
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 10.0,
            center: None,
        },
    });
    assert!(r.ok, "{r:?}");
    let doc = s.model().unwrap().document();
    let f = doc.features[0].clone();
    let c = Command::RedefineFeature {
        feature: f.id,
        kind: f.kind.clone(),
        name: Some("Profile".into()),
    };
    assert_eq!(round_trip(&c), c);
}

#[test]
fn health_defaults_to_healthy() {
    let h: FeatureHealth = serde_json::from_str("{}").unwrap_or_default();
    assert_eq!(h.state, HealthState::Healthy);
    let e = FeatureHealth::upstream(FeatureId(2));
    let back: FeatureHealth = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back, e);
}

#[test]
fn granular_sketch_ops_have_snake_case_tags_and_round_trip() {
    use rcad_commands::sketch_ops::PointRef;
    let ops = [
        SketchOp::AddLine {
            a: PointRef::Id(0),
            b: PointRef::At([10.0, 0.0]),
            construction: false,
        },
        SketchOp::AddDimension {
            constraint: rcad_sketch::Constraint::Diameter {
                circle: rcad_sketch::EntityId(3),
                dim: rcad_sketch::Dimension::new("hole", 6.0),
            },
            expression: Some("w / 8".into()),
        },
        SketchOp::BreakAt {
            entity: 1,
            at: [5.0, 0.0],
        },
        SketchOp::DeleteConstraint { index: 2 },
    ];
    for op in ops {
        let c = Command::SketchOp {
            feature: FeatureId(1),
            op,
        };
        assert_eq!(round_trip(&c), c);
    }
    let j = r#"{"cmd":"sketch_op","feature":1,"op":{"op":"add_line","a":0,"b":[1.5,2]}}"#;
    let c: Command = serde_json::from_str(j).unwrap();
    assert!(matches!(
        c,
        Command::SketchOp {
            op: SketchOp::AddLine {
                a: PointRef::Id(0),
                b: PointRef::At([1.5, 2.0]),
                construction: false
            },
            ..
        }
    ));
}
