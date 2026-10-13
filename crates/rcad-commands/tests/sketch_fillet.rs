//! SKETCH corner fillet through the command path: a rectangle with all four
//! corners rounded (2D sketch fillets, not solid-edge fillets) extrudes to
//! (w·h − (4 − π) r²)·d, and the width / radius dimensions regenerate the body.
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::SketchPlane;
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn vol(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn rounded_plate(w: f64, h: f64, r: f64) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    s.origin = Some(p[0]);
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
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
            dim: Dimension::new("w", w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", h),
        },
    ]);
    assert!(s.solve().is_success());
    s.fillet_corner(p[0], r, Some("r")).unwrap();
    for c in &p[1..] {
        s.fillet_corner(*c, r, Some("rx")).unwrap();
    }
    s
}

#[test]
fn filleted_plate_extrudes_to_the_analytic_volume_and_regenerates() {
    let area = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom {
                sketch: rounded_plate(60.0, 40.0, 6.0),
            },
        },
    )
    .feature
    .unwrap();
    let f = s.summary().features[0].clone();
    assert!(f.profile_error.is_none(), "{:?}", f.profile_error);
    assert_eq!(f.solve_dof, Some(0), "fully constrained after filleting");
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let v = vol(&s);
    assert!((v - area(60.0, 40.0, 6.0) * 5.0).abs() < 1e-3, "{v}");
    ok(
        &mut s,
        Command::SetParameter {
            feature: sk,
            name: "w".into(),
            value: 80.0,
        },
    );
    ok(
        &mut s,
        Command::SetParameter {
            feature: sk,
            name: "r".into(),
            value: 4.0,
        },
    );
    // Corner "r" is now 4, the other three stay 6.
    let want = 80.0 * 40.0 - (4.0 - PI) / 4.0 * (16.0 + 3.0 * 36.0);
    let v = vol(&s);
    assert!((v - want * 5.0).abs() < 1e-3, "{v} vs {}", want * 5.0);
}
