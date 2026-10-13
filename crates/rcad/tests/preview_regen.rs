//! SESSION_4 §1: the extrude preview resolves the direction from the same face
//! data as regeneration. Regression: a top face split in two by a slot is no
//! longer a unique semantic face, so the run-3 preview (semantic faces only)
//! drew a Cut sketched in that plane going into air while regeneration cut
//! into the material.
use rcad::extrude::ExtrudeForm;
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, SketchPlane};
use rcad_kernel::VolumeResult;

fn ok(s: &mut Session, c: Command) -> rcad_commands::Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn rect(s: &mut Session, w: f64, h: f64, c: [f64; 2]) -> rcad_document::FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: h,
                center: Some(c),
            },
        },
    )
    .feature
    .unwrap()
}

fn cut(sketch: rcad_document::FeatureId, d: f64) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    }
}

fn volume(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

#[test]
fn preview_direction_matches_regeneration_on_a_split_face() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    // Box below the XY plane: its top face lies in world XY.
    let b = rect(&mut s, 40.0, 20.0, [0.0, 0.0]);
    ok(
        &mut s,
        Command::Extrude {
            name: None,
            sketch: b,
            distance: 10.0,
            op: ExtrudeOp::NewBody,
            target: None,
            through_all: false,
            reverse: true,
            body_name: None,
            region: None,
            options: Default::default(),
        },
    );
    // A slot across the whole width splits the top face in two.
    let slot = rect(&mut s, 4.0, 30.0, [0.0, 0.0]);
    ok(&mut s, cut(slot, 2.0));
    let v0 = volume(&s);
    // A pocket sketched on world XY (the plane of the split top face).
    let p = rect(&mut s, 6.0, 6.0, [10.0, 0.0]);
    let sum = s.summary();
    // The split face is not a referenceable semantic face any more...
    assert!(
        !sum.features
            .iter()
            .flat_map(|f| f.faces.iter())
            .any(|f| f.frame.origin[2].abs() < 1e-9 && f.frame.z[2] > 0.5),
        "top face unexpectedly referenceable"
    );
    // ...but the preview still finds it in the body's planes: into the material.
    let form = ExtrudeForm {
        sketch: Some(p),
        distance: "3".into(),
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        region: None,
        more: Default::default(),
    };
    assert_eq!(form.side(&sum.features, &sum.bodies), Some(1.0));
    assert_eq!(form.direction(&sum.features, &sum.bodies), -1.0);
    // Regeneration agrees: the cut removed 6 x 6 x 3 below the plane.
    ok(&mut s, cut(p, 3.0));
    assert!(
        (v0 - volume(&s) - 108.0).abs() < 1e-6,
        "{}",
        v0 - volume(&s)
    );
    let m = s.summary().bodies[0].measurements.clone().unwrap();
    assert!(m.bounds.max[2].abs() < 1e-9, "nothing above the plane");
}
