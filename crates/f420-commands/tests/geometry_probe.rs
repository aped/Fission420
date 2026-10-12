//! Before/after probe (run 4 evidence): run on the pre-fix and fixed trees;
//! prints the outcome of each known wrong-geometry case. No assertions.
use f420_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use f420_kernel::VolumeResult;

fn sk(s: &mut Session, plane: SketchPlane, t: SketchTemplate) -> FeatureId {
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane,
        template: t,
    });
    assert!(r.ok, "{:?}", r.error);
    r.feature.unwrap()
}
fn rect(w: f64, h: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: Some(c),
    }
}
fn circ(d: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredCircle {
        diameter: d,
        center: Some(c),
    }
}
fn ext(sketch: FeatureId, d: f64, op: ExtrudeOp, through_all: bool) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: Some(BodyId(1)),
        through_all,
        reverse: false,
        body_name: None,
        region: None,
    }
}
fn face(f: FeatureId, role: FaceRole) -> SketchPlane {
    SketchPlane::Face(FaceRef { feature: f, role })
}
fn vol(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}
fn report(case: &str, s: &mut Session, c: Command, want: &str) {
    let before = vol(s);
    let r = s.execute(&c);
    if r.ok {
        println!(
            "PROBE {case}: ok, volume {:.4} (was {:.4}); correct: {want}",
            vol(s),
            before
        );
    } else {
        let e = r.error.map(|e| format!("{e:?}")).unwrap_or_default();
        println!(
            "PROBE {case}: refused {}; correct: {want}",
            &e[..e.len().min(160)]
        );
    }
}

#[test]
#[ignore = "before/after evidence probe"]
fn geometry_probe() {
    use std::f64::consts::PI;
    // Engulfed hole.
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let a = sk(&mut s, SketchPlane::WorldXY, rect(80.0, 50.0, [40.0, 25.0]));
    let e = s.execute(&Command::extrude(None, a, 10.0)).feature.unwrap();
    let h = sk(
        &mut s,
        face(e, FaceRole::ExtrudeEndCap),
        circ(10.0, [20.0, 25.0]),
    );
    s.execute(&ext(h, 0.0, ExtrudeOp::Cut, true));
    let big = sk(
        &mut s,
        face(e, FaceRole::ExtrudeEndCap),
        circ(16.0, [20.0, 25.0]),
    );
    report(
        "engulfed-hole cut",
        &mut s,
        ext(big, 0.0, ExtrudeOp::Cut, true),
        &format!("volume {:.4} or a refusal", 40000.0 - PI * 64.0 * 10.0),
    );
    // Thin skin / flange joins (pocket inset from the walls).
    let base = 4000.0 - 19.0 * 9.0 * 9.7;
    for (name, x, pattern) in [
        ("join over 0.3 mm flange", 10.0, None),
        ("linear pattern of a join onto the flange", -10.0, Some(3)),
        ("mirror of a join onto the flange", -10.0, Some(0)),
    ] {
        let mut s = Session::new();
        s.execute(&Command::NewDocument);
        let a = sk(&mut s, SketchPlane::WorldXY, rect(40.0, 10.0, [0.0, 0.0]));
        let e = s.execute(&Command::extrude(None, a, 10.0)).feature.unwrap();
        let p = sk(
            &mut s,
            face(e, FaceRole::ExtrudeStartCap),
            rect(19.0, 9.0, [9.5, 0.0]),
        );
        let r = s.execute(&ext(p, 9.7, ExtrudeOp::Cut, false));
        assert!(r.ok, "{:?}", r.error);
        let b = sk(
            &mut s,
            face(e, FaceRole::ExtrudeEndCap),
            rect(4.0, 4.0, [x, 0.0]),
        );
        match pattern {
            None => report(
                name,
                &mut s,
                ext(b, 5.0, ExtrudeOp::Join, false),
                &format!("volume {:.4}", base + 80.0),
            ),
            Some(n) => {
                let j = s
                    .execute(&ext(b, 5.0, ExtrudeOp::Join, false))
                    .feature
                    .unwrap();
                let c = if n > 0 {
                    Command::LinearPattern {
                        name: None,
                        feature: j,
                        count: n,
                        spacing: 10.0,
                        direction: [1.0, 0.0],
                    }
                } else {
                    Command::Mirror {
                        name: None,
                        feature: j,
                        plane: SketchPlane::WorldYZ,
                    }
                };
                let k = if n > 0 { n as f64 } else { 2.0 };
                report(name, &mut s, c, &format!("volume {:.4}", base + 80.0 * k));
            }
        }
    }
    // Skin over a cavity.
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let a = sk(&mut s, SketchPlane::WorldXY, rect(20.0, 20.0, [0.0, 0.0]));
    let e = s.execute(&Command::extrude(None, a, 10.0)).feature.unwrap();
    let p = sk(
        &mut s,
        face(e, FaceRole::ExtrudeStartCap),
        rect(10.0, 10.0, [0.0, 0.0]),
    );
    s.execute(&ext(p, 9.7, ExtrudeOp::Cut, false));
    let b = sk(
        &mut s,
        face(e, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, [0.0, 0.0]),
    );
    report(
        "join over 0.3 mm skin above a cavity",
        &mut s,
        ext(b, 5.0, ExtrudeOp::Join, false),
        "volume 3110.0000",
    );
}
