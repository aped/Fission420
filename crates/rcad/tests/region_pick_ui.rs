//! Region picking in the Extrude dialog through the real UI: a sketch with two
//! concentric rectangles and a circle island (three regions) can be extruded
//! although its whole profile is refused; a click on the sketch plane in the
//! 3D view picks the region under the cursor (shown in the Region list and
//! shaded in the preview); OK extrudes exactly that region. Expected volumes
//! are analytic.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};

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

fn view_click(h: &mut Harness<'_, RcadApp>, world: [f64; 3]) {
    let s: Pos2 = h.state().world_to_screen(world).expect("visible");
    let r = h.state().view_rect().expect("view");
    assert!(r.contains(s), "{world:?} -> {s:?} outside {r:?}");
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
    let _ = h.try_run();
}

fn rect(s: &mut Sketch, w: f64, h: f64) -> Vec<EntityId> {
    let p: Vec<PointId> = [
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect()
}

/// Outer 60x40, inner 40x20, circle Ø10 (fixed geometry).
fn nested() -> Sketch {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    rect(&mut s, 60.0, 40.0);
    rect(&mut s, 40.0, 20.0);
    let c = s.add_point(0.0, 0.0);
    let circle = s.add_circle(c, 5.0);
    s.constraints.extend([
        Constraint::Coincident { a: c, b: o },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("hole", 10.0),
        },
    ]);
    s
}

#[test]
fn click_a_region_in_the_view_and_extrude_only_it() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: nested() },
    });
    settle(&mut h);
    let f = h.state().summary().features[0].clone();
    assert!(f.profile_error.is_some(), "whole profile refused (island)");
    click(&mut h, "Extrude…");
    assert!(h.query_by_label("Region").is_some(), "region chooser shown");
    assert!(
        h.query_by_label_contains("pick the region to extrude")
            .is_some()
    );
    // Run 4: OK stays disabled until a region is picked (it could only be refused).
    assert!(h.get_by_label("OK").accesskit_node().is_disabled());
    // Click inside the island first, then inside the ring: the last pick wins.
    view_click(&mut h, [1.0, 1.0, 0.0]);
    let pv = h.state().extrude_preview().expect("preview of the island");
    assert!(pv.picked && pv.holes.is_empty() && pv.loops_2d.len() == 1);
    view_click(&mut h, [25.0, 0.0, 0.0]);
    let pv = h.state().extrude_preview().expect("preview of the ring");
    assert!(pv.picked);
    assert_eq!(pv.holes.len(), 1, "ring = outer minus inner rectangle");
    let sk = h.state().summary().features[0].sketch.clone().unwrap();
    let ring = sk.region_at([25.0, 0.0]).unwrap().unwrap();
    assert!(
        h.query_by_label_contains(&ring.label()).is_some(),
        "picked region named in the Region list: {}",
        ring.label()
    );
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    assert!((v - (2400.0 - 800.0) * 10.0).abs() < 1e-6, "{v}");
    let ext = h.state().summary().features[1].clone();
    let r = ext.region.expect("region stored");
    assert_eq!(r.outer.len(), 4);
    assert_eq!(r.holes.len(), 1);
    // The island's area is not part of the body (sanity on the analytic value).
    assert!((v - 25.0 * PI * 10.0).abs() > 1.0);
}
