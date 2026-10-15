//! Project inside the sketch editor, through real widgets and pointer events:
//! auto-projected cap edges (construction, read-only, Cancel undoes them),
//! the Project tool on a cap edge, a hole dimensioned to the projected edge
//! that follows an upstream width edit, Unlink, and the Projection Link
//! option. Base: a 40 x 20 centred rectangle extruded 10 mm (cap at z = 10,
//! right edge at x = 20).

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp, SketchTarget};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};
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

fn button(h: &mut Harness<'_, RcadApp>, pos: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    let _ = h.try_run();
}

fn screen_click(h: &mut Harness<'_, RcadApp>, s: Pos2) {
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
    button(h, s, true);
    button(h, s, false);
    settle(h);
}

fn sk_click(h: &mut Harness<'_, RcadApp>, p: [f64; 2]) {
    let s = h.state().sketch_to_screen(p).expect("visible");
    screen_click(h, s);
}

fn editor<'a>(h: &'a Harness<'_, RcadApp>) -> &'a rcad::sketch_edit::SketchEditor {
    &h.state().sketch_session().expect("sketch session").editor
}

fn feedback(h: &Harness<'_, RcadApp>) -> String {
    editor(h)
        .feedback
        .as_ref()
        .map(|f| f.text.clone())
        .unwrap_or_default()
}

/// New document with the extruded plate; returns (base sketch, extrude).
fn plate(h: &mut Harness<'_, RcadApp>) -> (FeatureId, FeatureId) {
    click(h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 20.0,
            center: None,
        },
    });
    settle(h);
    let base = h.state().summary().features[0].id;
    h.state_mut().request(Command::extrude(None, base, 10.0));
    settle(h);
    let ext = h.state().summary().features[1].id;
    (base, ext)
}

fn sketch_on_cap(h: &mut Harness<'_, RcadApp>, ext: FeatureId) {
    h.state_mut().begin_sketch(SketchTarget::New {
        plane: rcad_commands::end_cap_of(ext),
    });
    settle(h);
}

/// Sketch-plane coordinates of a world point on the cap (z = 10).
fn on_cap(h: &Harness<'_, RcadApp>, w: [f64; 3]) -> [f64; 2] {
    let f = h.state().sketch_session().unwrap().frame;
    let d = [w[0] - f.origin[0], w[1] - f.origin[1], w[2] - f.origin[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    [dot(d, f.x), dot(d, f.y)]
}

#[test]
fn cap_edges_are_auto_projected_read_only_and_cancel_undoes_them() {
    let mut h = harness();
    let (_, ext) = plate(&mut h);
    let before = h.state().summary().features.len();
    sketch_on_cap(&mut h, ext);
    let ed = editor(&h);
    assert_eq!(ed.projected.len(), 4, "{}", feedback(&h));
    assert!(
        ed.projected.keys().all(|e| ed.sketch.is_construction(*e)),
        "auto-projected edges are construction"
    );
    assert_eq!(ed.drawn_entity_count(), 0);
    assert_eq!(h.state().summary().features.len(), before + 1);
    // Select the right edge (x = 20) and delete: refused, nothing changes.
    click(&mut h, "Select");
    let p = on_cap(&h, [20.0, 3.0, 10.0]);
    sk_click(&mut h, p);
    assert!(
        matches!(
            editor(&h).selection.as_slice(),
            [rcad::sketch_edit::Item::Entity(_)]
        ),
        "{:?}",
        editor(&h).selection
    );
    let sk = editor(&h).sketch.clone();
    h.key_press(Key::Delete);
    settle(&mut h);
    assert_eq!(editor(&h).sketch, sk);
    assert!(feedback(&h).contains("read-only"), "{}", feedback(&h));
    // Dragging a projected corner is refused with the reason too.
    // Cancel: the document is back where it was.
    click(&mut h, "Cancel Sketch");
    assert!(h.state().sketch_session().is_none());
    assert_eq!(h.state().summary().features.len(), before);
}

#[test]
fn projected_edge_drives_a_dimensioned_hole_through_an_upstream_edit() {
    let mut h = harness();
    let (base, ext) = plate(&mut h);
    // Without auto-projection: only what the Project tool picks.
    h.state_mut().set_auto_project_faces(false);
    sketch_on_cap(&mut h, ext);
    assert!(editor(&h).projected.is_empty());
    h.key_press(Key::P);
    settle(&mut h);
    assert_eq!(
        editor(&h).tool,
        rcad::sketch_edit::SketchTool::Project,
        "P starts Project"
    );
    // Click on the cap just inside its right edge.
    let s = h.state().world_to_screen([19.0, 2.0, 10.0]).unwrap();
    screen_click(&mut h, s);
    let ed = editor(&h);
    assert_eq!(ed.projected.len(), 1, "{}", feedback(&h));
    let edge = *ed.projected.keys().next().unwrap();
    assert!(
        ed.sketch.is_construction(edge),
        "a projected edge is reference geometry (open curves break profiles)"
    );
    let Some(Entity::Line { a, b, .. }) = ed.sketch.entity(edge).cloned() else {
        panic!("projected a line")
    };
    let (pa, pb) = (
        ed.sketch.point_xy(a).unwrap(),
        ed.sketch.point_xy(b).unwrap(),
    );
    // The lower end of the edge (world y = -10).
    let low = if on_cap(&h, [20.0, -10.0, 10.0])
        .iter()
        .zip(pa)
        .all(|(x, y)| (x - y).abs() < 1e-6)
    {
        pa
    } else {
        pb
    };
    // A circle 8 mm left of and 4 mm above that end, dimensioned to it.
    let c = on_cap(&h, [12.0, -6.0, 10.0]);
    click(&mut h, "Circle");
    sk_click(&mut h, c);
    let rim = on_cap(&h, [14.0, -6.0, 10.0]);
    sk_click(&mut h, rim);
    click(&mut h, "Dimension");
    for (name, v) in [("dx", "8"), ("dy", "4")] {
        sk_click(&mut h, low);
        sk_click(&mut h, c);
        let label = format!("Edit {name} (mm)");
        assert!(
            h.query_by_label(&label).is_some(),
            "{label}: {}",
            feedback(&h)
        );
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.get_by_label(&label).type_text(v);
        h.key_press(Key::Enter);
        settle(&mut h);
    }
    click(&mut h, "Finish Sketch");
    assert!(h.state().sketch_session().is_none(), "finished");
    let hole = h.state().summary().features[2].clone();
    let centre = |h: &Harness<'_, RcadApp>| {
        let sk = h.state().summary().features[2].sketch.clone().unwrap();
        sk.entities
            .iter()
            .find_map(|e| match e {
                Entity::Circle { center, .. } => sk.point_xy(*center),
                _ => None,
            })
            .unwrap()
    };
    let c0 = centre(&h);
    assert_eq!(hole.projections.len(), 1);
    // Base width 40 -> 60: the right edge moves 10 mm, the hole follows.
    h.state_mut().request(Command::SetParameter {
        feature: base,
        name: "width".into(),
        value: 60.0,
    });
    settle(&mut h);
    let c1 = centre(&h);
    let moved = (c1[0] - c0[0]).hypot(c1[1] - c0[1]);
    assert!((moved - 10.0).abs() < 1e-6, "{c0:?} -> {c1:?}");
}

#[test]
fn unlink_breaks_the_link_and_link_off_copies() {
    let mut h = harness();
    let (_, ext) = plate(&mut h);
    sketch_on_cap(&mut h, ext);
    assert_eq!(editor(&h).projected.len(), 4);
    click(&mut h, "Select");
    let p = on_cap(&h, [20.0, 3.0, 10.0]);
    sk_click(&mut h, p);
    click(&mut h, "Unlink projection");
    assert!(editor(&h).projected.is_empty(), "{}", feedback(&h));
    let id = match &h.state().sketch_session().unwrap().target {
        SketchTarget::Existing(id) => *id,
        t => panic!("{t:?}"),
    };
    let f = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == id)
        .unwrap();
    assert!(f.projections.is_empty());
    assert_eq!(editor(&h).sketch.entities.len(), 4, "geometry kept");
    // Projection Link off: the Project tool copies the edge unlinked.
    click(&mut h, "Project");
    click(&mut h, "Projection Link");
    assert!(!editor(&h).project_linked);
    let n = editor(&h).sketch.entities.len();
    let s = h.state().world_to_screen([-19.0, 2.0, 10.0]).unwrap();
    screen_click(&mut h, s);
    assert!(feedback(&h).contains("unlinked"), "{}", feedback(&h));
    assert!(editor(&h).projected.is_empty());
    assert_eq!(editor(&h).sketch.entities.len(), n + 1);
}
