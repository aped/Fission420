//! Run-5 controls through the real widgets (egui_kittest): the display-only
//! section view (picking ignores hidden material, flip, selected-face plane;
//! the model never changes), Mirror across a picked face with a plane offset,
//! and a Hole started from an offset plane. Volumes are analytic.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use f420_app::{ExecMode, F420App};
use f420_commands::{Command, ExtrudeOp, SketchTemplate};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId, PlaneBase, SketchPlane};

fn harness() -> Harness<'static, F420App> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut F420App| app.ui(ui),
            F420App::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, F420App>) {
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

fn click(h: &mut Harness<'_, F420App>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn type_into(h: &mut Harness<'_, F420App>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn request(h: &mut Harness<'_, F420App>, c: Command) -> FeatureId {
    h.state_mut().request(c);
    settle(h);
    h.state().summary().features.last().expect("feature").id
}

fn rect(
    h: &mut Harness<'_, F420App>,
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
    }
}

fn open_view_menu(h: &mut Harness<'_, F420App>) {
    h.get_by_label("View").click();
    for _ in 0..3 {
        let _ = h.try_run();
    }
}

fn group_of(h: &Harness<'_, F420App>, r: FaceRef) -> u32 {
    h.state()
        .faces()
        .iter()
        .find(|f| f.reference == Some(r))
        .map(|f| f.group)
        .unwrap_or_else(|| panic!("no display face for {r:?}"))
}

fn pick_world(h: &Harness<'_, F420App>, p: [f64; 3]) -> Option<u32> {
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
            distance
        }) if distance == -30.0
    ));
}

fn fname(h: &Harness<'_, F420App>, id: FeatureId) -> String {
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
