//! Exact face measurement through the face inspector widgets (run 6, IN01):
//! select a hole's cylinder → radius/diameter; measure from it to a second hole
//! → axis distance; a side plane to an axis. Expected values are the closed-form
//! hole layout. Face indices are per evaluation (display order), so the holes are
//! found by their readout.
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::FacePairMeasure;

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(180), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// 60 × 40 × 20 plate, Ø10 hole at (15, 10), Ø6 hole at (45, 30).
fn commands() -> Vec<Command> {
    let top = SketchPlane::Face(FaceRef {
        feature: FeatureId(2),
        role: FaceRole::ExtrudeEndCap,
    });
    let circle = |d: f64, c: [f64; 2]| Command::CreateSketch {
        name: None,
        plane: top,
        template: SketchTemplate::CenteredCircle {
            diameter: d,
            center: Some(c),
        },
    };
    let cut = |f: u32| Command::CutThroughAll {
        name: None,
        sketch: FeatureId(f),
    };
    vec![
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 60.0,
                height: 40.0,
                center: Some([30.0, 20.0]),
            },
        },
        Command::extrude(None, FeatureId(1), 20.0),
        circle(10.0, [15.0, 10.0]),
        cut(3),
        circle(6.0, [45.0, 30.0]),
        cut(5),
    ]
}

fn select(h: &mut Harness<'_, RcadApp>, g: u32) {
    h.state_mut().select_face(Some(g));
    settle(h);
}

/// Select display group `g` and request its exact geometry.
fn exact(h: &mut Harness<'_, RcadApp>, g: u32) {
    select(h, g);
    click(h, "Exact geometry");
}

#[test]
fn cylinder_radius_and_axis_distance_in_the_face_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    for c in commands() {
        h.state_mut().request(c);
        settle(&mut h);
    }
    let body = h.state().summary().bodies[0].id;
    let faces: Vec<(u32, bool)> = h
        .state()
        .faces()
        .iter()
        .filter(|f| f.body == Some(body))
        .map(|f| (f.group, f.planar))
        .collect();
    let (mut g10, mut g6) = (None, None);
    for &(g, planar) in &faces {
        if planar {
            continue;
        }
        exact(&mut h, g);
        if h.query_by_label_contains("radius 5 mm, diameter 10 mm")
            .is_some()
        {
            g10 = Some(g);
        }
        if h.query_by_label_contains("radius 3 mm, diameter 6 mm")
            .is_some()
        {
            g6 = Some(g);
        }
    }
    let (g10, g6) = (g10.expect("Ø10 hole face"), g6.expect("Ø6 hole face"));
    exact(&mut h, g10);
    h.get_by_label_contains("Cylinder (hole): radius 5 mm, diameter 10 mm");
    click(&mut h, "Exact measure from here");
    select(&mut h, g6);
    click(&mut h, "Measure to this face");
    let want = rcad::face_measure::pair_text(&FacePairMeasure::AxisDistance {
        distance: 30f64.hypot(20.0),
    });
    h.get_by_label(&want);
    // A planar face to the Ø10 axis: one of the side planes x = 0 / x = 60 /
    // y = 0 / y = 40 gives 15 mm (x = 0) and another 10 mm (y = 0).
    let mut found = vec![];
    for &(g, planar) in &faces {
        if !planar {
            continue;
        }
        select(&mut h, g);
        click(&mut h, "Measure to this face");
        for d in [15.0, 45.0, 10.0, 30.0] {
            let t =
                rcad::face_measure::pair_text(&FacePairMeasure::PlaneAxisDistance { distance: d });
            if h.query_by_label(&t).is_some() {
                found.push(d);
            }
        }
    }
    found.sort_by(f64::total_cmp);
    assert_eq!(found, vec![10.0, 15.0, 30.0, 45.0]);
}
