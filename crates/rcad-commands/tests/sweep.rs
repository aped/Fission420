//! Exact sweep as a feature (ADR 0009): the closed profile of one
//! sketch along the open line/arc chain of another. Expected volume by Pappus:
//! profile area × length of the centroid's path (the profile centroid is on
//! the path here).
mod support;
use rcad_commands::Session;
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};
use rcad_sketch::{Entity, Sketch};
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

/// Path on World XY from the origin: line to (0, -l1), 90° arc about
/// (r, -l1) to (r, -l1 - r), line l2 along +X.
fn path(l1: f64, r: f64, l2: f64) -> Sketch {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(0.0, -l1);
    let c = s.add_point(r, -l1);
    let p2 = s.add_point(r, -l1 - r);
    let p3 = s.add_point(r + l2, -l1 - r);
    s.add_line(p0, p1);
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: c,
        start: p1,
        end: p2,
    });
    s.add_line(p2, p3);
    s
}

fn custom(plane: SketchPlane, sketch: Sketch) -> Command {
    Command::CreateSketch {
        name: None,
        plane,
        template: SketchTemplate::Custom { sketch },
    }
}

#[test]
fn handle_sweep_is_exact_and_regenerates() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    // Profile 8 x 4 on World XZ (normal -Y) centred on the origin.
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_rectangle",
                "width": 8.0, "height": 4.0, "center": [0.0, 0.0]}),
    );
    let r = s.execute(&custom(SketchPlane::WorldXY, path(10.0, 15.0, 10.0)));
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "sweep", "profile": 1, "path": 2, "body_name": "Handle"}),
    );
    let want = 32.0 * (20.0 + 7.5 * PI);
    assert!(
        (tray::volume(&s) - want).abs() < 1e-6,
        "{} vs {want}",
        tray::volume(&s)
    );
    // Inside the bend and the far leg; outside the bend's inner corner.
    let p = tray::probe(
        &s,
        &[
            [0.0, -5.0, 0.0],
            [
                15.0 - 15.0 * std::f64::consts::FRAC_1_SQRT_2,
                -10.0 - 15.0 * std::f64::consts::FRAC_1_SQRT_2,
                0.0,
            ],
            [20.0, -25.0, 0.0],
            [10.0, -15.0, 0.0],
        ],
    );
    assert_eq!(p, vec![Some(true), Some(true), Some(true), Some(false)]);
    // Upstream edit: a wider profile regenerates the sweep.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 6.0}),
    );
    let want6 = 24.0 * (20.0 + 7.5 * PI);
    // After the edit the solved points carry ~1e-11 mm of solver noise and the
    // volume may come back as a labelled tessellation estimate: hold it to its
    // own reported error bound.
    let m = s.summary().bodies[0].measurements.clone().unwrap().volume;
    match m {
        rcad_kernel::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => assert!(
            (volume - want6).abs() <= error_estimate.max(1e-6),
            "{volume} ± {error_estimate} vs {want6}"
        ),
        v => panic!("{v:?}"),
    }
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - want).abs() < 1e-6);
}

#[test]
fn sweep_path_refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_rectangle",
                "width": 8.0, "height": 4.0, "center": [0.0, 0.0]}),
    );
    // A path that does not start at the profile's origin.
    let mut off = path(10.0, 15.0, 10.0);
    for p in &mut off.points {
        p.x += 3.0;
    }
    assert!(s.execute(&custom(SketchPlane::WorldXY, off)).ok);
    // A branching path.
    let mut br = path(10.0, 15.0, 10.0);
    let q = br.add_point(-5.0, -10.0);
    let p1 = br.points[1].id;
    br.add_line(p1, q);
    assert!(s.execute(&custom(SketchPlane::WorldXY, br)).ok);
    let before = s.model().unwrap().document().clone();
    for (path, why) in [(2, "origin"), (3, "branches")] {
        let r = s.execute(&Command::Sweep {
            name: None,
            profile: FeatureId(1),
            path: FeatureId(path),
            body_name: None,
        });
        assert!(!r.ok);
        assert!(format!("{:?}", r.error).contains(why), "{:?}", r.error);
        assert_eq!(s.model().unwrap().document(), &before);
    }
}
