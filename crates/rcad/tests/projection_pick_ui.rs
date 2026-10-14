//! Project edge and Intersect from the sketch inspector through the real
//! widgets: "Project edge (pick in view)" then a click on the top face next
//! to a hole projects that hole's rim (a Ø8 circle); "Intersect Body…" adds
//! the block's section (outline + hole) to another sketch. Geometry is closed-form.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, PlaneBase, SketchPlane};
use rcad_sketch::Entity;

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

fn pointer_click(h: &mut Harness<'_, RcadApp>, s: Pos2) {
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: s,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        let _ = h.try_run();
    }
    settle(h);
}

fn request(h: &mut Harness<'_, RcadApp>, c: Command) -> FeatureId {
    h.state_mut().request(c);
    settle(h);
    h.state().summary().features.last().expect("feature").id
}

fn sketch_of(h: &Harness<'_, RcadApp>, id: FeatureId) -> rcad_sketch::Sketch {
    h.state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == id)
        .and_then(|f| f.sketch.clone())
        .expect("sketch")
}

#[test]
fn project_edge_by_pick_and_intersect_body() {
    let mut h = harness();
    click(&mut h, "New");
    let s1 = request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: None,
            },
        },
    );
    request(&mut h, Command::extrude(None, s1, 10.0));
    let s3 = request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredCircle {
                diameter: 8.0,
                center: Some([10.0, 0.0]),
            },
        },
    );
    request(
        &mut h,
        Command::CutThroughAll {
            name: None,
            sketch: s3,
        },
    );
    let mid = request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Offset {
                base: PlaneBase::WorldXY,
                distance: 4.0,
                angle: 0.0,
            },
            template: SketchTemplate::Polygon {
                vertices: vec![[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]],
            },
        },
    );
    let name = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == mid)
        .unwrap()
        .name
        .clone();
    click(&mut h, &name);
    click(&mut h, "Project geometry into this sketch");
    click(&mut h, "Project edge (pick in view)");
    // A click on empty space ends the pick with the reason, nothing changes.
    let corner = h
        .state()
        .view_rect()
        .map(|r| r.min + egui::vec2(5.0, 5.0))
        .unwrap();
    pointer_click(&mut h, corner);
    assert!(
        h.state()
            .diagnostics()
            .iter()
            .any(|d| d.text.contains("Project edge: no face under the cursor")),
        "{:?}",
        h.state().diagnostics()
    );
    click(&mut h, "Project edge (pick in view)");
    // Click the top face just outside the hole rim.
    let p = h.state().world_to_screen([14.6, 0.0, 10.0]).unwrap();
    pointer_click(&mut h, p);
    let sk = sketch_of(&h, mid);
    let c: Vec<_> = sk
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Circle { center, radius, .. } => Some((sk.point_xy(*center)?, *radius)),
            _ => None,
        })
        .collect();
    assert_eq!(c.len(), 1, "{:?}", sk.entities);
    assert!((c[0].0[0] - 10.0).abs() < 1e-9 && c[0].0[1].abs() < 1e-9);
    assert!((c[0].1 - 4.0).abs() < 1e-9);
    // Intersect into a second sketch at z = 6: the block's section.
    let mid = request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Offset {
                base: PlaneBase::WorldXY,
                distance: 6.0,
                angle: 0.0,
            },
            template: SketchTemplate::Polygon {
                vertices: vec![[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]],
            },
        },
    );
    let name = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == mid)
        .unwrap()
        .name
        .clone();
    click(&mut h, &name);
    if h.query_by_label("Project edge (pick in view)").is_none() {
        click(&mut h, "Project geometry into this sketch");
    }
    let body = h.state().summary().bodies[0].name.clone();
    click(&mut h, &format!("Intersect {body}"));
    let f = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == mid)
        .unwrap()
        .clone();
    assert_eq!(f.projections.len(), 1);
    let big = f
        .sketch
        .unwrap()
        .regions()
        .unwrap()
        .into_iter()
        .map(|r| r.area)
        .fold(0.0, f64::max);
    assert!((big - (1200.0 - 16.0 * PI)).abs() < 1e-6, "{big}");
}
