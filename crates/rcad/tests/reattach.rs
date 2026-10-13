//! SESSION_3 scenario B through the UI: a sketch on a picked side face, a
//! blind cut into the material, an upstream edit that removes the support
//! face (refused with BrokenReference, model kept), then the explicit repair:
//! banner "Re-attach sketch…" → choose a face → preview → Re-attach (one undo
//! step), after which the edit is accepted. Volumes are hand-derived.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{DocError, FaceRole, SketchPlane};
use rcad_kernel::VolumeResult;

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

fn volume(h: &Harness<'_, RcadApp>) -> f64 {
    match h.state().summary().bodies[0]
        .measurements
        .as_ref()
        .map(|m| &m.volume)
    {
        Some(VolumeResult::Value { volume, .. }) => *volume,
        o => panic!("{o:?}"),
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[test]
fn side_face_cut_broken_reference_and_reattach() {
    let mut h = harness();
    click(&mut h, "New");
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(&mut h, "Rectangle on XY…");
    type_into(&mut h, "Rectangle width (mm)", "60");
    type_into(&mut h, "Rectangle height (mm)", "40");
    type_into(&mut h, "Rectangle center X (mm)", "30");
    type_into(&mut h, "Rectangle center Y (mm)", "20");
    click(&mut h, "OK");
    click(&mut h, "Extrude…");
    type_into(&mut h, "Extrude distance (mm)", "20");
    click(&mut h, "OK");
    assert!((volume(&h) - 48000.0).abs() < 0.005);
    let base_sketch = h.state().summary().features[0].id;
    let ext = h.state().summary().features[1].id;

    // Pick the x = 60 side face in the view: a semantic side-face reference.
    let p = h.state().world_to_screen([60.0, 20.0, 10.0]).unwrap();
    pointer_click(&mut h, p);
    let face = h.state().selected_face().cloned().expect("side face");
    let r = face.reference.expect("side face is a sketch host");
    let FaceRole::ExtrudeSide { entity } = r.role else {
        panic!("{r:?}")
    };
    let fr = h
        .state()
        .summary()
        .features
        .iter()
        .flat_map(|f| f.faces.iter())
        .find(|f| f.reference == r)
        .unwrap()
        .frame;
    assert!((fr.z[0] - 1.0).abs() < 1e-9 && (fr.origin[0] - 60.0).abs() < 1e-9);
    // 10x6 rectangle centred at world (60, 20, 10) in the face's local frame.
    let v = [
        60.0 - fr.origin[0],
        20.0 - fr.origin[1],
        10.0 - fr.origin[2],
    ];
    let (w, hh) = if fr.x[1].abs() > 0.5 {
        (10.0, 6.0)
    } else {
        (6.0, 10.0)
    };
    h.state_mut().request(Command::CreateSketch {
        name: Some("Slot".into()),
        plane: SketchPlane::Face(r),
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: hh,
            center: Some([dot(v, fr.x), dot(v, fr.y)]),
        },
    });
    settle(&mut h);
    // Blind cut 15 mm through the Extrude dialog: from a face of the target
    // it goes into the material.
    click(&mut h, "Slot");
    click(&mut h, "Extrude this sketch…");
    click(&mut h, "Cut");
    type_into(&mut h, "Extrude distance (mm)", "15");
    assert_eq!(h.state().extrude_preview().unwrap().z_range, [0.0, -15.0]);
    click(&mut h, "OK");
    assert!((volume(&h) - 47100.0).abs() < 0.005, "{}", volume(&h));
    let slot = h.state().feature_by_name("Slot").unwrap().id;

    // Upstream edit removing the support line: refused, model kept, repair offered.
    let rev = h.state().summary().revision;
    let edited = |h: &Harness<'_, RcadApp>| {
        let mut s = h.state().summary().features[0].sketch.clone().unwrap();
        let right = rcad_sketch::EntityId(entity);
        let ends = s.entity(right).map(|e| e.points()).unwrap_or_default();
        s.remove_entity(right);
        let apex = s.add_point(70.0, 20.0);
        s.add_line(ends[0], apex);
        s.add_line(apex, ends[1]);
        s
    };
    let e = edited(&h);
    h.state_mut().request(Command::EditSketch {
        feature: base_sketch,
        sketch: e.clone(),
    });
    settle(&mut h);
    assert_eq!(h.state().summary().revision, rev, "refused");
    let last = h.state().diagnostics().last().unwrap().clone();
    assert!(last.error && last.kind == "BrokenReference", "{last:?}");
    click(&mut h, "Re-attach sketch…");
    h.get_by_label("New support").click();
    let _ = h.try_run();
    let top = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == ext)
        .unwrap()
        .name
        .clone();
    let body = h.state().summary().bodies[0].name.clone();
    h.get_by_label(&format!("{top} end cap ({body})")).click();
    let _ = h.try_run();
    let pf = h.state().reattach_preview_frame().expect("preview frame");
    assert!((pf.origin[2] - 20.0).abs() < 1e-9 && (pf.z[2] - 1.0).abs() < 1e-9);
    click(&mut h, "Re-attach");
    let plane = h.state().feature_by_name("Slot").unwrap().plane;
    assert!(
        matches!(plane, Some(SketchPlane::Face(f)) if f.feature == ext && f.role == FaceRole::ExtrudeEndCap)
    );
    assert!((volume(&h) - 47100.0).abs() < 0.005, "{}", volume(&h));
    // The upstream edit is accepted now: + triangle 0.5*40*10*20 = 4000.
    h.state_mut().request(Command::EditSketch {
        feature: base_sketch,
        sketch: e,
    });
    settle(&mut h);
    assert!(!h.state().diagnostics().last().unwrap().error);
    assert!((volume(&h) - 51100.0).abs() < 0.005, "{}", volume(&h));
    // Undo the edit and the re-attach (one step each).
    click(&mut h, "Undo");
    click(&mut h, "Undo");
    let plane = h.state().feature_by_name("Slot").unwrap().plane;
    assert_eq!(plane, Some(SketchPlane::Face(r)));
    let _ = (slot, DocError::NothingTo(String::new()));
}
