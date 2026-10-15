//! Sketch patterns through the command API: a 80 x 60 plate with a
//! bolt circle made by a circular sketch pattern of one Ø6 hole (6 × 60°),
//! extruded as one New Body: volume = (4800 − 6 · 9π) · 5. The pattern angle is
//! a feature parameter (edit: holes move, volume unchanged; an angle that makes
//! holes overlap is refused and rolled back). Saved as format v6, reloaded.

use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, Sketch};
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn volume(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn plate_with_bolt_circle() -> Sketch {
    let mut s = Sketch::centered_rectangle(80.0, 60.0).unwrap();
    let o = rcad_sketch::PointId(0);
    let c = s.add_point(20.0, 0.0);
    let hole = s.add_circle(c, 3.0);
    s.constraints.extend([
        Constraint::HorizontalDistance {
            a: o,
            b: c,
            dim: Dimension::new("bolt_r", 20.0),
        },
        Constraint::Diameter {
            circle: hole,
            dim: Dimension::new("hole_d", 6.0),
        },
    ]);
    // y of the hole centre = 0: a horizontal construction line from the origin.
    let l = s.add_line(o, c);
    s.set_construction(l, true);
    s.constraints.push(Constraint::Horizontal { line: l });
    s.pattern_circular(&[hole], Some(o), [0.0, 0.0], 6, 60.0)
        .unwrap();
    assert!(s.solve().is_success());
    s
}

#[test]
fn bolt_circle_pattern_extrudes_saves_and_edits() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom {
                sketch: plate_with_bolt_circle(),
            },
        },
    )
    .feature
    .unwrap();
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let expect = (4800.0 - 6.0 * 9.0 * PI) * 5.0;
    let v = volume(&s);
    assert!((v - expect).abs() < 1e-6 * expect, "{v} vs {expect}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bolt.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(j["version"], 6, "sketch patterns need format v6");
    let mut s2 = Session::new();
    ok(
        &mut s2,
        Command::Open {
            path,
            safe_mode: false,
        },
    );
    assert!((volume(&s2) - expect).abs() < 1e-6 * expect);

    // The shared angle is one parameter: 45° moves the holes, same volume.
    ok(
        &mut s2,
        Command::SetParameter {
            feature: FeatureId(1),
            name: "pattern_angle".into(),
            value: 45.0,
        },
    );
    assert!((volume(&s2) - expect).abs() < 1e-6 * expect);
    // 5° packs the Ø6 holes at r 20 into overlap (chord 1.7 mm): refused,
    // the 45° model stays.
    let r = s2.execute(&Command::SetParameter {
        feature: FeatureId(1),
        name: "pattern_angle".into(),
        value: 5.0,
    });
    assert!(!r.ok, "{r:?}");
    assert!((volume(&s2) - expect).abs() < 1e-6 * expect);
    let f = &s2.summary().features[0];
    assert!(
        f.parameters
            .iter()
            .any(|(n, v)| n == "pattern_angle" && *v == 45.0)
    );
}
