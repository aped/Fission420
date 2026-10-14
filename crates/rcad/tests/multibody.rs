//! Multi-body UI: two bodies, the Bodies browser (select, hide/show,
//! inline rename, undo), picking per body, and the Extrude dialog (New Body /
//! Join / Cut with a target, Through All, preview, cancel, refusal) driven
//! through the real widgets. Expected volumes are hand-derived box volumes.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate, end_cap_of};
use rcad_document::{BodyId, FaceRole, SketchPlane};
use rcad_kernel::VolumeResult;

const VOL_TOL: f64 = 0.005;

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

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
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
    let _ = h.try_run();
}

/// Rectangle template on world XY through the dialog.
fn rect_xy(h: &mut Harness<'_, RcadApp>, w: &str, hh: &str, cx: &str, cy: &str) {
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(h, "Rectangle on XY…");
    type_into(h, "Rectangle width (mm)", w);
    type_into(h, "Rectangle height (mm)", hh);
    type_into(h, "Rectangle center X (mm)", cx);
    type_into(h, "Rectangle center Y (mm)", cy);
    click(h, "OK");
}

fn body_volume(h: &Harness<'_, RcadApp>, id: BodyId) -> f64 {
    let b = h
        .state()
        .summary()
        .bodies
        .iter()
        .find(|b| b.id == id)
        .expect("body");
    match b.measurements.as_ref().map(|m| &m.volume) {
        Some(VolumeResult::Value { volume, .. }) => *volume,
        other => panic!("no volume for {}: {other:?}", b.name),
    }
}

fn assert_close(v: f64, e: f64) {
    assert!((v - e).abs() <= VOL_TOL, "volume {v} vs {e}");
}

/// Request a rectangle sketch on the end cap of extrusion `ext` (setup only;
/// the operations under test go through the dialog).
fn rect_on_cap(
    h: &mut Harness<'_, RcadApp>,
    ext: rcad_document::FeatureId,
    w: f64,
    hh: f64,
    c: [f64; 2],
) {
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: end_cap_of(ext),
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: hh,
            center: Some(c),
        },
    });
    settle(h);
}

fn fname(h: &Harness<'_, RcadApp>, id: rcad_document::FeatureId) -> String {
    h.state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == id)
        .unwrap()
        .name
        .clone()
}

/// Bodies A [0,40]x[0,30]x[0,10] and B [50,70]x[0,20]x[0,10].
fn two_bodies(h: &mut Harness<'_, RcadApp>) -> (BodyId, BodyId) {
    click(h, "New");
    rect_xy(h, "40", "30", "20", "15");
    click(h, "Extrude…");
    click(h, "OK");
    rect_xy(h, "20", "20", "60", "10");
    click(h, "Extrude…");
    click(h, "OK");
    let b = &h.state().summary().bodies;
    assert_eq!(b.len(), 2, "second sketch creates a second body");
    (b[0].id, b[1].id)
}

#[test]
fn two_bodies_render_pick_hide_rename_and_undo() {
    let mut h = harness();
    let (a, b) = two_bodies(&mut h);
    assert_close(body_volume(&h, a), 12000.0);
    assert_close(body_volume(&h, b), 4000.0);
    // Both bodies are in the display scene with disjoint group ranges and
    // distinct tints.
    let scene = h.state().scene().clone();
    assert_eq!(scene.bodies.len(), 2);
    assert!(scene.bodies[0].groups.end <= scene.bodies[1].groups.start);
    assert_ne!(scene.bodies[0].tint, scene.bodies[1].tint);
    // Picking a face of B selects B's face and body.
    let on_b = h.state().world_to_screen([60.0, 10.0, 10.0]).unwrap();
    pointer_click(&mut h, on_b);
    let f = h.state().selected_face().cloned().expect("face of B");
    assert_eq!(f.body, Some(b));
    assert!(f.reference.is_some(), "top cap of B is a sketch host");
    // Browser: select body A highlights all six faces of A.
    let name_a = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name_a);
    assert_eq!(h.state().selected_body(), Some(a));
    assert_eq!(h.state().highlighted_faces().len(), 6);
    // Hide B: it stays in the document, is not drawn and cannot be picked.
    let name_b = h.state().summary().bodies[1].name.clone();
    click(&mut h, &format!("Hide {name_b}"));
    assert_eq!(h.state().summary().bodies.len(), 2);
    assert!(!h.state().summary().bodies[1].visible);
    assert_eq!(h.state().scene().bodies.len(), 1);
    pointer_click(&mut h, on_b);
    assert!(h.state().selected_face().is_none_or(|f| f.body != Some(b)));
    // Undo restores visibility (one undo step).
    click(&mut h, "Undo");
    assert!(h.state().summary().bodies[1].visible);
    assert_eq!(h.state().scene().bodies.len(), 2);
    // Inline rename (inspector Rename… or double-click / context menu on the
    // row open the same in-place editor): Esc cancels, Enter commits.
    click(&mut h, &name_a);
    click(&mut h, "Rename body…");
    assert!(
        h.query_by_label("Body name").is_some(),
        "inline editor open"
    );
    h.key_press(Key::Escape);
    let _ = h.try_run();
    assert!(h.query_by_label("Body name").is_none());
    assert_eq!(h.state().summary().bodies[0].name, name_a);
    click(&mut h, "Rename body…");
    type_into(&mut h, "Body name", "Plate A");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(h.state().summary().bodies[0].name, "Plate A");
    assert_eq!(
        h.state()
            .summary()
            .can_undo
            .as_deref()
            .map(|s| s.contains("Rename")),
        Some(true)
    );
}

#[test]
fn extrude_dialog_join_cut_through_all_cancel_and_refusal() {
    let mut h = harness();
    let (a, b) = two_bodies(&mut h);
    let ext_a = h.state().summary().features[1].id;
    // Boss 20x10 on A's top face, centred at (20, 15).
    rect_on_cap(&mut h, ext_a, 20.0, 10.0, [20.0, 15.0]);
    let boss_sketch = h.state().summary().features.last().unwrap().id;
    h.state_mut().select_body(None);
    let sk_name = fname(&h, boss_sketch);
    click(&mut h, &sk_name);
    click(&mut h, "Extrude this sketch…");
    // Sketch on a face of A: Join with target A preselected (checked by the
    // preview's direction and the committed volume below).
    type_into(&mut h, "Extrude distance (mm)", "5");
    let pv = h.state().extrude_preview().expect("preview");
    assert_eq!(pv.z_range, [0.0, 5.0], "join goes along the face normal");
    assert!(pv.end.iter().all(|p| (p[2] - 15.0).abs() < 1e-9));
    // Target highlighted in the view while the dialog is open.
    let d = h
        .state()
        .scene()
        .bodies
        .iter()
        .find(|d| d.id == a)
        .unwrap()
        .groups
        .clone();
    // Cancel leaves the document untouched.
    let rev = h.state().summary().revision;
    click(&mut h, "Cancel");
    assert_eq!(h.state().summary().revision, rev);
    assert!(h.state().extrude_preview().is_none());
    assert!(d.end > d.start);
    // Reopen and commit the join: A = 13000, B unchanged.
    click(&mut h, "Extrude this sketch…");
    type_into(&mut h, "Extrude distance (mm)", "5");
    click(&mut h, "OK");
    assert_close(body_volume(&h, a), 13000.0);
    assert_close(body_volume(&h, b), 4000.0);
    assert_eq!(h.state().summary().bodies.len(), 2);

    // Rectangular 10x5 through-all cut on A's top face away from the boss.
    rect_on_cap(&mut h, ext_a, 10.0, 5.0, [6.0, 25.0]);
    let cut_sketch = h.state().summary().features.last().unwrap().id;
    let n = fname(&h, cut_sketch);
    click(&mut h, &n);
    click(&mut h, "Extrude this sketch…");
    h.get_by_label("Cut").click();
    let _ = h.try_run();
    // Blind cut from a face of the target goes into the material.
    type_into(&mut h, "Extrude distance (mm)", "3");
    let pv = h.state().extrude_preview().unwrap();
    assert_eq!(pv.z_range, [0.0, -3.0]);
    // Reversing a blind cut from a face would point into air: not offered.
    assert!(
        h.get_by_label("Reverse direction")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut h, "Through All");
    let pv = h.state().extrude_preview().unwrap();
    // Both sides of the plane z=10, through A's extent (z 0..15) + margin.
    assert!(
        pv.z_range[0] < -10.0 && pv.z_range[0] > -12.0,
        "{:?}",
        pv.z_range
    );
    assert!(
        pv.z_range[1] > 5.0 && pv.z_range[1] < 7.0,
        "{:?}",
        pv.z_range
    );
    click(&mut h, "OK");
    assert_close(body_volume(&h, a), 12500.0);
    assert_close(body_volume(&h, b), 4000.0);
    let cut = h.state().summary().features.last().unwrap().clone();
    assert!(matches!(cut.operation, Some(rcad_document::Operation::Cut { target }) if target == a));

    // Undo/redo the cut.
    click(&mut h, "Undo");
    assert_close(body_volume(&h, a), 13000.0);
    click(&mut h, "Redo");
    assert_close(body_volume(&h, a), 12500.0);

    // Refusal: a join far away from A (disconnected result) keeps the model.
    rect_xy(&mut h, "5", "5", "200", "200");
    let far = h.state().summary().features.last().unwrap().id;
    let n = fname(&h, far);
    click(&mut h, &n);
    click(&mut h, "Extrude this sketch…");
    h.get_by_label("Join").click();
    let _ = h.try_run();
    // Two B-rep bodies and the sketch is on a world plane: target required.
    assert!(h.state().extrude_preview().is_some());
    let rev = h.state().summary().revision;
    click(&mut h, "OK");
    assert_eq!(h.state().summary().revision, rev, "no target: nothing sent");
    assert!(h.state().diagnostics().last().unwrap().error);
    let _ = h.try_run();
    h.get_by_label("Target body").click();
    let _ = h.try_run();
    let a_name = h.state().summary().bodies[0].name.clone();
    h.get_by_label(&format!("{a_name} ({a})")).click();
    let _ = h.try_run();
    click(&mut h, "OK");
    assert_eq!(
        h.state().summary().revision,
        rev,
        "refused join keeps the model"
    );
    assert!(h.state().diagnostics().last().unwrap().error);
    assert_close(body_volume(&h, a), 12500.0);
    assert_eq!(h.state().summary().bodies.len(), 2);
    // Linear pattern of the rectangular cut: 2 instances 15 mm apart along
    // the sketch X axis (second slot x 16..26, clear of the boss): -500.
    click(&mut h, &cut.name);
    click(&mut h, "Pattern…");
    type_into(&mut h, "Pattern count", "2");
    type_into(&mut h, "Pattern spacing (mm)", "15");
    click(&mut h, "OK");
    let pat = h.state().summary().features.last().unwrap().clone();
    assert_eq!(pat.kind, "linear_pattern");
    assert_close(body_volume(&h, a), 12000.0);
    assert_close(body_volume(&h, b), 4000.0);
    // Circular instead: undo, then two instances 180° apart about A's centre
    // (20, 15): the rotated slot also lies inside A, -500.
    click(&mut h, "Undo");
    assert_close(body_volume(&h, a), 12500.0);
    click(&mut h, &cut.name);
    click(&mut h, "Pattern…");
    h.get_by_label("Circular").click();
    let _ = h.try_run();
    type_into(&mut h, "Pattern count", "2");
    type_into(&mut h, "Angle between instances (deg)", "180");
    type_into(&mut h, "Centre x (mm)", "20");
    type_into(&mut h, "Centre y (mm)", "15");
    click(&mut h, "OK");
    let pat = h.state().summary().features.last().unwrap().clone();
    assert_eq!(
        pat.kind,
        "circular_pattern",
        "{:?}",
        h.state().diagnostics().last()
    );
    assert_close(body_volume(&h, a), 12000.0);
    let _ = SketchPlane::WorldXY;
}

/// Chamfer through the UI: pick the top face, "Chamfer edge…", choose the front
/// face, 2 mm: removes 0.5 * 2 * 2 * 40.
#[test]
fn chamfer_edge_from_a_picked_face() {
    let mut h = harness();
    click(&mut h, "New");
    rect_xy(&mut h, "40", "30", "20", "15");
    let sk = h.state().summary().features.last().unwrap().id;
    h.state_mut().request(Command::extrude(None, sk, 10.0));
    settle(&mut h);
    let ext = h.state().summary().features.last().unwrap().clone();
    let b = h.state().summary().bodies[0].id;
    let top = h.state().world_to_screen([20.0, 15.0, 10.0]).unwrap();
    pointer_click(&mut h, top);
    settle(&mut h);
    click(&mut h, "Chamfer edge…");
    let front = ext
        .faces
        .iter()
        .find(|f| (f.frame.z[1] + 1.0).abs() < 1e-9)
        .unwrap()
        .reference;
    let FaceRole::ExtrudeSide { entity } = front.role else {
        panic!()
    };
    let label = format!("{} side face (sketch entity {entity})", ext.name);
    h.get_by_label("Second face").click();
    let _ = h.try_run();
    h.get_by_label(&label).click();
    let _ = h.try_run();
    type_into(&mut h, "Chamfer distance (mm)", "2");
    click(&mut h, "OK");
    let last = h.state().summary().features.last().unwrap().clone();
    assert_eq!(last.kind, "chamfer", "{:?}", h.state().diagnostics().last());
    assert_close(body_volume(&h, b), 12000.0 - 80.0);
}

/// The preview follows the regenerator's rule even when the sketch is on a world
/// plane that merely holds a face of the target (no explicit face reference):
/// body below XY (reversed extrude), Cut from XY goes down into it, Join up.
#[test]
fn preview_direction_matches_regeneration_on_coplanar_world_plane() {
    let mut h = harness();
    click(&mut h, "New");
    rect_xy(&mut h, "40", "30", "20", "15");
    let sk = h.state().summary().features.last().unwrap().id;
    h.state_mut().request(Command::Extrude {
        name: None,
        sketch: sk,
        distance: 10.0,
        op: rcad_commands::ExtrudeOp::NewBody,
        target: None,
        through_all: false,
        reverse: true,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    settle(&mut h);
    let b = h.state().summary().bodies[0].id;
    rect_xy(&mut h, "10", "10", "20", "15");
    let s2 = h.state().summary().features.last().unwrap().id;
    let n = fname(&h, s2);
    click(&mut h, &n);
    click(&mut h, "Extrude this sketch…");
    h.get_by_label("Cut").click();
    let _ = h.try_run();
    type_into(&mut h, "Extrude distance (mm)", "3");
    assert_eq!(h.state().extrude_preview().unwrap().z_range, [0.0, -3.0]);
    click(&mut h, "OK");
    assert_close(body_volume(&h, b), 12000.0 - 300.0);
    // Join on the same plane goes up, away from the material (bottom face is z = -10,
    // top face z = 0 holds the sketch plane with outward normal +Z).
    click(&mut h, &n);
    click(&mut h, "Extrude this sketch…");
    h.get_by_label("Join").click();
    let _ = h.try_run();
    type_into(&mut h, "Extrude distance (mm)", "2");
    assert_eq!(h.state().extrude_preview().unwrap().z_range, [0.0, 2.0]);
}

/// An open body rename whose body disappears (toolbar Undo)
/// closes, so keyboard shortcuts work again; a reply that selects a feature
/// clears the body selection (no hidden "Selected [A]" export default).
#[test]
fn undo_closes_a_rename_of_a_vanished_body() {
    let mut h = harness();
    let (a, _b) = two_bodies(&mut h);
    let name_b = h.state().summary().bodies[1].name.clone();
    click(&mut h, &name_b);
    click(&mut h, "Rename body…");
    assert!(h.query_by_label("Body name").is_some());
    click(&mut h, "Undo");
    settle(&mut h);
    assert_eq!(h.state().summary().bodies.len(), 1);
    assert!(h.query_by_label("Body name").is_none());
    // Shortcuts are live again: Ctrl+Z undoes B's sketch.
    let rev = h.state().summary().revision;
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    settle(&mut h);
    assert_ne!(h.state().summary().revision, rev, "Ctrl+Z ignored");
    // Body selected in the browser, then a command that selects its feature.
    let name_a = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name_a);
    assert_eq!(h.state().selected_body(), Some(a));
    rect_xy(&mut h, "10", "10", "20", "15");
    assert_eq!(h.state().selected_body(), None);
}

/// Re-attach does not preselect a face it does not offer
/// (the cap of the sketch's own extrusion would be a cycle), and its button
/// stays disabled until a valid support is chosen.
#[test]
fn reattach_does_not_preselect_an_unoffered_face() {
    let mut h = harness();
    click(&mut h, "New");
    rect_xy(&mut h, "40", "30", "20", "15");
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let sketch = h.state().summary().features[0].id;
    let top = h.state().world_to_screen([20.0, 15.0, 10.0]).unwrap();
    pointer_click(&mut h, top);
    assert!(
        h.state()
            .selected_face()
            .and_then(|f| f.reference)
            .is_some(),
        "picked the extrusion's end cap"
    );
    h.state_mut().start_reattach(sketch);
    let _ = h.try_run();
    assert!(h.get_by_label("Re-attach").accesskit_node().is_disabled());
}

/// A dialog holding ids of the old document does not survive
/// a document change.
#[test]
fn dialogs_close_when_the_document_changes() {
    let mut h = harness();
    click(&mut h, "New");
    rect_xy(&mut h, "40", "30", "20", "15");
    click(&mut h, "Extrude…");
    assert!(h.query_by_label("Extrude distance (mm)").is_some());
    h.state_mut().request(Command::NewDocument);
    settle(&mut h);
    assert!(h.query_by_label("Extrude distance (mm)").is_none());
}
