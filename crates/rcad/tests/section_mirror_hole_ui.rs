//! Section, mirror and hole controls through the real widgets (egui_kittest):
//! the display-only section view (picking ignores hidden material, flip, selected-face plane;
//! the model never changes), Mirror across a picked face with a plane offset,
//! and a Hole started from an offset plane. Volumes are analytic.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, PlaneBase, SketchPlane};

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

fn request(h: &mut Harness<'_, RcadApp>, c: Command) -> FeatureId {
    h.state_mut().request(c);
    settle(h);
    h.state().summary().features.last().expect("feature").id
}

fn rect(
    h: &mut Harness<'_, RcadApp>,
    plane: SketchPlane,
    w: f64,
    hh: f64,
    c: [f64; 2],
) -> FeatureId {
    request(
        h,
        Command::CreateSketch {
            name: None,
            plane,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: hh,
                center: Some(c),
            },
        },
    )
}

fn op(sketch: FeatureId, d: f64, op: ExtrudeOp) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    }
}

fn open_view_menu(h: &mut Harness<'_, RcadApp>) {
    h.get_by_label("View").click();
    for _ in 0..3 {
        let _ = h.try_run();
    }
}

fn group_of(h: &Harness<'_, RcadApp>, r: FaceRef) -> u32 {
    h.state()
        .faces()
        .iter()
        .find(|f| f.reference == Some(r))
        .map(|f| f.group)
        .unwrap_or_else(|| panic!("no display face for {r:?}"))
}

fn pick_world(h: &Harness<'_, RcadApp>, p: [f64; 3]) -> Option<u32> {
    let s = h.state().world_to_screen(p).expect("on screen");
    h.state().face_at(s)
}

#[test]
fn section_view_hides_and_unpicks_material_without_changing_the_model() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 40.0, 30.0, [20.0, 15.0]);
    let ext = request(&mut h, Command::extrude(None, sk, 10.0));
    let top = FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeEndCap,
    };
    let top_g = group_of(&h, top);
    let p = [10.0, 10.0, 10.0];
    assert_eq!(pick_world(&h, p), Some(top_g), "top face pickable");
    let (rev, vol) = (h.state().summary().revision, h.state().volume());

    open_view_menu(&mut h);
    click(&mut h, "Section view…");
    assert!(h.state().section.is_some());
    type_into(&mut h, "Section offset (mm)", "5");
    settle(&mut h);
    let plane = h.state().section_plane().expect("plane");
    assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
    assert!((plane.offset - 5.0).abs() < 1e-6);
    // The top (z = 10) is hidden: the pick goes through it.
    assert_ne!(
        pick_world(&h, p),
        Some(top_g),
        "hidden top face not pickable"
    );
    // Flip: the lower half is hidden instead; the top is back.
    h.get_by_label("Flip (hide the other side)").click();
    settle(&mut h);
    assert_eq!(pick_world(&h, p), Some(top_g));
    h.get_by_label("Flip (hide the other side)").click();
    settle(&mut h);
    // Section on the selected face's plane, 2 mm into the body: hides z > 8.
    h.state_mut().select_face(Some(top_g));
    let _ = h.try_run();
    click(&mut h, "Use selected face");
    type_into(&mut h, "Section offset (mm)", "-2");
    settle(&mut h);
    let plane = h.state().section_plane().expect("plane");
    assert!((plane.offset - 8.0).abs() < 1e-5, "{plane:?}");
    assert_ne!(pick_world(&h, p), Some(top_g));
    // A non-number disables the plane (nothing hidden) and says so.
    type_into(&mut h, "Section offset (mm)", "abc");
    settle(&mut h);
    assert!(h.state().section_plane().is_none());
    assert!(h.query_by_label("Offset must be a number (mm).").is_some());
    // Display only: no revision, same volume.
    assert_eq!(h.state().summary().revision, rev);
    assert_eq!(h.state().volume(), vol);
    // Off again from the menu.
    open_view_menu(&mut h);
    click(&mut h, "Section view off");
    assert!(h.state().section.is_none());
    assert_eq!(pick_world(&h, p), Some(top_g));
}

#[test]
fn mirror_across_a_picked_face_with_an_offset() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 60.0, 30.0, [0.0, 0.0]);
    let base = request(&mut h, Command::extrude(None, sk, 8.0));
    let top = SketchPlane::Face(FaceRef {
        feature: base,
        role: FaceRole::ExtrudeEndCap,
    });
    let boss_sk = rect(&mut h, top, 10.0, 10.0, [15.0, 0.0]);
    let boss = request(&mut h, op(boss_sk, 5.0, ExtrudeOp::Join));
    assert!((h.state().volume().unwrap() - 14900.0).abs() < 1e-6);
    let name = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == boss)
        .unwrap()
        .name
        .clone();
    click(&mut h, &name);
    click(&mut h, "Pattern…");
    h.get_by_label("Mirror").click();
    let _ = h.try_run();
    // The base's +X side (x = 30), offset -30: the plane x = 0.
    h.get_by_label("Mirror face").click();
    let _ = h.try_run();
    h.get_by_label(&format!("{} side face (sketch entity 2)", fname(&h, base)))
        .click();
    let _ = h.try_run();
    type_into(&mut h, "Mirror plane offset (mm)", "-30");
    click(&mut h, "OK");
    let f = h.state().summary().features.last().unwrap().clone();
    assert_eq!(f.kind, "mirror", "{:?}", h.state().diagnostics().last());
    assert!((h.state().volume().unwrap() - 15400.0).abs() < 1e-6);
    assert!(f.parameters.contains(&("plane_offset".to_string(), -30.0)));
    assert!(matches!(
        f.plane,
        Some(SketchPlane::Offset {
            base: PlaneBase::Face(_),
            distance,
            ..
        }) if distance == -30.0
    ));
}

fn fname(h: &Harness<'_, RcadApp>, id: FeatureId) -> String {
    h.state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == id)
        .unwrap()
        .name
        .clone()
}

#[test]
fn hole_from_an_offset_plane_on_a_stepped_block() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 40.0, 30.0, [20.0, 15.0]);
    let base = request(&mut h, Command::extrude(None, sk, 10.0));
    let cap = FaceRef {
        feature: base,
        role: FaceRole::ExtrudeEndCap,
    };
    let step_sk = rect(&mut h, SketchPlane::Face(cap), 20.0, 30.0, [30.0, 15.0]);
    request(&mut h, op(step_sk, 5.0, ExtrudeOp::Join));
    let v0 = 40.0 * 30.0 * 10.0 + 20.0 * 30.0 * 5.0;
    assert!((h.state().volume().unwrap() - v0).abs() < 1e-6);
    // The lower level (z = 10), Hole…, start plane offset 5 = the upper level.
    let g = group_of(&h, cap);
    h.state_mut().select_face(Some(g));
    let _ = h.try_run();
    click(&mut h, "Hole…");
    type_into(&mut h, "Hole centre X (face mm)", "30");
    type_into(&mut h, "Hole centre Y (face mm)", "15");
    type_into(&mut h, "Start plane offset (mm)", "5");
    click(&mut h, "OK");
    let f = h.state().summary().features.last().unwrap().clone();
    assert_eq!(f.kind, "hole", "{:?}", h.state().diagnostics().last());
    let want = v0 - PI * 2.75 * 2.75 * 15.0;
    let v = h.state().volume().unwrap();
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    assert!(f.parameters.contains(&("plane_offset".to_string(), 5.0)));
}

/// Apply with a new name and a refused expression: the
/// rename lands, the typed expression stays in the open edit with the error.
#[test]
fn parameter_rename_then_refused_value_keeps_the_typed_edit() {
    let mut h = harness();
    click(&mut h, "New");
    type_into(&mut h, "New parameter name", "a");
    type_into(&mut h, "New parameter expression", "5");
    click(&mut h, "Add parameter");
    assert_eq!(h.state().summary().parameters.len(), 1);
    click(&mut h, "param a");
    type_into(&mut h, "Parameter name", "b");
    type_into(&mut h, "Parameter expression", "1 / 0");
    click(&mut h, "Apply parameter");
    let p = &h.state().summary().parameters;
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].name, "b", "rename applied");
    assert_eq!(p[0].expression, "5", "refused value not applied");
    // The edit is still open on the renamed parameter, with the typed text.
    let expr = format!("{:?}", h.get_by_label("Parameter expression"));
    assert!(expr.contains("1 / 0"), "{expr}");
}

/// A Cut sketched on a plane holding target faces facing
/// both ways gets no guessed preview direction, and the dialog says why.
#[test]
fn ambiguous_face_side_has_no_preview() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 40.0, 30.0, [20.0, 15.0]);
    let base = request(&mut h, Command::extrude(None, sk, 10.0));
    let cap = SketchPlane::Face(FaceRef {
        feature: base,
        role: FaceRole::ExtrudeEndCap,
    });
    let over = rect(&mut h, cap, 30.0, 30.0, [45.0, 15.0]);
    request(&mut h, op(over, 10.0, ExtrudeOp::Join));
    // A sketch on the base top (z = 10), which shares its plane with the
    // overhang's underside (x 40..60, facing -Z).
    let cut = rect(&mut h, cap, 4.0, 4.0, [15.0, 15.0]);
    let name = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == cut)
        .unwrap()
        .name
        .clone();
    click(&mut h, &name);
    click(&mut h, "Extrude this sketch…");
    h.get_by_label("Cut").click();
    settle(&mut h);
    assert!(h.state().extrude_preview().is_none());
    assert!(
        h.query_by_label_contains("material side is ambiguous")
            .is_some(),
        "warning shown"
    );
}

/// Suppression from the inspector: a leaf cut is suppressed and restored; a
/// consumed join offers only "Suppress with dependents" (the pattern goes too).
#[test]
fn suppress_and_restore_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 60.0, 30.0, [0.0, 0.0]);
    let base = request(&mut h, Command::extrude(None, sk, 8.0));
    let top = SketchPlane::Face(FaceRef {
        feature: base,
        role: FaceRole::ExtrudeEndCap,
    });
    let boss_sk = rect(&mut h, top, 10.0, 10.0, [15.0, 0.0]);
    let boss = request(&mut h, op(boss_sk, 5.0, ExtrudeOp::Join));
    request(
        &mut h,
        Command::LinearPattern {
            name: None,
            feature: boss,
            count: 2,
            spacing: 30.0,
            direction: [-1.0, 0.0],
        },
    );
    let slot_sk = rect(&mut h, top, 4.0, 20.0, [0.0, 0.0]);
    let slot = request(&mut h, op(slot_sk, 3.0, ExtrudeOp::Cut));
    let v = 14400.0 + 1000.0 - 240.0;
    assert!((h.state().volume().unwrap() - v).abs() < 1e-6);
    let n = fname(&h, slot);
    click(&mut h, &n);
    click(&mut h, "Suppress");
    assert!((h.state().volume().unwrap() - (v + 240.0)).abs() < 1e-6);
    assert!(
        h.state()
            .summary()
            .features
            .iter()
            .any(|f| f.id == slot && f.suppressed)
    );
    click(&mut h, "Restore");
    assert!((h.state().volume().unwrap() - v).abs() < 1e-6);
    // The boss is used by the pattern: plain Suppress is disabled.
    let n = fname(&h, boss);
    click(&mut h, &n);
    click(&mut h, "Suppress");
    assert!(h.state().summary().features.iter().all(|f| !f.suppressed));
    click(&mut h, "Suppress with dependents");
    assert!((h.state().volume().unwrap() - (14400.0 - 240.0)).abs() < 1e-6);
    let k = h
        .state()
        .summary()
        .features
        .iter()
        .filter(|f| f.suppressed)
        .count();
    assert_eq!(k, 2);
    click(&mut h, "Undo");
    assert!((h.state().volume().unwrap() - v).abs() < 1e-6);
}

/// A refused rename must not let the value change through
/// under the new name (it used to create a second parameter).
#[test]
fn refused_rename_sends_no_value_change() {
    let mut h = harness();
    click(&mut h, "New");
    type_into(&mut h, "New parameter name", "a");
    type_into(&mut h, "New parameter expression", "5");
    click(&mut h, "Add parameter");
    click(&mut h, "param a");
    type_into(&mut h, "Parameter name", "1x");
    type_into(&mut h, "Parameter expression", "9");
    click(&mut h, "Apply parameter");
    let p = &h.state().summary().parameters;
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!((p[0].name.as_str(), p[0].expression.as_str()), ("a", "5"));
    // The edit stays open with what was typed.
    let expr = format!("{:?}", h.get_by_label("Parameter expression"));
    assert!(expr.contains('9'), "{expr}");
}

/// Exact solid fillet through the UI: top face, "Fillet edge…", the front
/// face as the second face, radius 3: removes (r² − πr²/4) × 40.
#[test]
fn fillet_edge_from_a_picked_face() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = rect(&mut h, SketchPlane::WorldXY, 40.0, 30.0, [20.0, 15.0]);
    let ext = request(&mut h, Command::extrude(None, sk, 10.0));
    let top = FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeEndCap,
    };
    let g = group_of(&h, top);
    h.state_mut().select_face(Some(g));
    let _ = h.try_run();
    click(&mut h, "Fillet edge…");
    let label = format!("{} side face (sketch entity 1)", fname(&h, ext));
    h.get_by_label("Second face").click();
    let _ = h.try_run();
    h.get_by_label(&label).click();
    let _ = h.try_run();
    type_into(&mut h, "Fillet radius (mm)", "3");
    click(&mut h, "OK");
    let last = h.state().summary().features.last().unwrap().clone();
    assert_eq!(last.kind, "fillet", "{:?}", h.state().diagnostics().last());
    let want = 12000.0 - (9.0 - PI * 9.0 / 4.0) * 40.0;
    assert!((h.state().volume().unwrap() - want).abs() < 1e-6);
    assert!(last.parameters.contains(&("radius".to_string(), 3.0)));
}

/// Angled construction plane from the dialog: World XY tilted 30° about X.
#[test]
fn sketch_on_a_tilted_plane_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Sketch on offset plane…");
    type_into(&mut h, "Plane offset (mm)", "0");
    type_into(&mut h, "Plane angle (deg)", "30");
    click(&mut h, "OK");
    let f = h.state().sketch_session().expect("sketch session").frame;
    let (s, c) = 30f64.to_radians().sin_cos();
    assert!(
        (f.z[1] + s).abs() < 1e-12 && (f.z[2] - c).abs() < 1e-12,
        "{f:?}"
    );
    assert!(
        (f.y[1] - c).abs() < 1e-12 && (f.y[2] - s).abs() < 1e-12,
        "{f:?}"
    );
}

/// Loft through the UI: two rectangles on parallel planes, Loft / Sweep ›
/// Loft…, choose both sketches, OK: the prismatoid volume.
#[test]
fn loft_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    let a = rect(&mut h, SketchPlane::WorldXY, 60.0, 40.0, [0.0, 0.0]);
    let off = SketchPlane::Offset {
        base: PlaneBase::WorldXY,
        distance: 30.0,
        angle: 0.0,
    };
    let b = rect(&mut h, off, 70.0, 50.0, [0.0, 0.0]);
    h.get_by_label("Loft / Sweep").click();
    for _ in 0..3 {
        let _ = h.try_run();
    }
    click(&mut h, "Loft…");
    for (combo, id) in [("First profile sketch", a), ("Second profile sketch", b)] {
        h.get_by_label(combo).click();
        let _ = h.try_run();
        let item = format!("{}: {}", id, fname(&h, id));
        h.get_by_label(&item).click();
        let _ = h.try_run();
    }
    click(&mut h, "OK");
    let last = h.state().summary().features.last().unwrap().clone();
    assert_eq!(last.kind, "loft", "{:?}", h.state().diagnostics().last());
    let v = h
        .state()
        .summary()
        .bodies
        .last()
        .unwrap()
        .measurements
        .clone()
        .unwrap()
        .volume;
    match v {
        rcad_kernel::VolumeResult::Value { volume, .. } => {
            assert!((volume - 88000.0).abs() < 1e-6, "{volume}")
        }
        v => panic!("{v:?}"),
    }
}
