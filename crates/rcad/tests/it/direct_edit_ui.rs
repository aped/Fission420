//! Direct editing of an imported STEP body through the real widgets
//! (egui_kittest): pick the top face → "Offset face…", pick the
//! through-hole wall → "Resize hole…", pick the blind-hole wall → "Remove
//! hole", undo. Fixture and analytic values as in
//! crates/rcad-commands/tests/it/direct_edit.rs.
use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::Command;
use rcad_kernel::DirectFace;

const V0: f64 = 48000.0 - 267.0 * PI;
const TOP_AREA: f64 = 2400.0 - 50.0 * PI;

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

/// Display group of the imported face matching `pick`.
fn group(h: &Harness<'_, RcadApp>, pick: impl Fn(&DirectFace) -> bool) -> u32 {
    let s = h.state().summary();
    let b = s
        .bodies
        .iter()
        .find(|b| !b.imported_faces.is_empty())
        .expect("imported body");
    let pos = b
        .imported_faces
        .iter()
        .position(|f| pick(&f.kind))
        .expect("face");
    let start = h
        .state()
        .faces()
        .iter()
        .filter(|f| f.body == Some(b.id))
        .map(|f| f.group)
        .min()
        .unwrap();
    start + pos as u32
}

fn select(h: &mut Harness<'_, RcadApp>, g: u32) {
    h.state_mut().select_face(Some(g));
    let _ = h.try_run();
    let _ = h.try_run();
}

fn volume(h: &Harness<'_, RcadApp>) -> f64 {
    h.state().volume().unwrap()
}

#[test]
fn tilt_a_picked_side_face() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::ImportStep {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step/direct_edit_block_mm.step"),
        units_to_mm: None,
    });
    settle(&mut h);
    let side = group(
        &h,
        |k| matches!(k, DirectFace::Plane { origin, normal } if normal[0] > 0.5 && (origin[0] - 60.0).abs() < 1e-9),
    );
    select(&mut h, side);
    click(&mut h, "Tilt face…");
    type_into(&mut h, "Tilt angle (deg)", "10");
    type_into(&mut h, "Offset after tilt (mm)", "2");
    click(&mut h, "OK");
    let want = V0 + 1600.0 / 10f64.to_radians().cos();
    assert!(
        (volume(&h) - want).abs() < 1e-6 * want,
        "{} vs {want}: {:?}",
        volume(&h),
        h.state().diagnostics().last()
    );
}

#[test]
fn offset_resize_remove_from_picked_faces() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::ImportStep {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step/direct_edit_block_mm.step"),
        units_to_mm: None,
    });
    settle(&mut h);
    assert!((volume(&h) - V0).abs() < 1e-6 * V0);

    let top = group(
        &h,
        |k| matches!(k, DirectFace::Plane { origin, normal } if normal[2] > 0.5 && (origin[2] - 20.0).abs() < 1e-9),
    );
    select(&mut h, top);
    click(&mut h, "Offset face…");
    type_into(&mut h, "Distance (mm)", "3");
    click(&mut h, "OK");
    let last = h.state().summary().features.last().unwrap().clone();
    assert_eq!(
        last.kind,
        "direct_edit",
        "{:?}",
        h.state().diagnostics().last()
    );
    let v1 = V0 + 3.0 * TOP_AREA;
    assert!((volume(&h) - v1).abs() < 1e-6 * v1, "{}", volume(&h));

    let hole = group(
        &h,
        |k| matches!(k, DirectFace::Cylinder { origin, hole: true, .. } if (origin[0] - 15.0).abs() < 1e-6),
    );
    select(&mut h, hole);
    click(&mut h, "Resize hole…");
    type_into(&mut h, "New diameter (mm)", "12");
    click(&mut h, "OK");
    let v2 = v1 - PI * 20.0 * 23.0;
    assert!((volume(&h) - v2).abs() < 1e-6 * v2, "{}", volume(&h));

    // A refused edit reports the kernel's reason; the model is unchanged.
    let blind = group(
        &h,
        |k| matches!(k, DirectFace::Cylinder { origin, hole: true, .. } if (origin[0] - 30.0).abs() < 1e-6),
    );
    select(&mut h, blind);
    click(&mut h, "Resize hole…");
    type_into(&mut h, "New diameter (mm)", "40");
    click(&mut h, "OK");
    assert!((volume(&h) - v2).abs() < 1e-6 * v2);
    let d = format!("{:?}", h.state().diagnostics().last());
    assert!(d.contains("would reach another edge"), "{d}");
    if h.query_by_label("Cancel").is_some() {
        click(&mut h, "Cancel");
    }

    select(&mut h, blind);
    click(&mut h, "Remove hole");
    click(&mut h, "OK");
    let v3 = v2 + PI * 9.0 * 11.0;
    assert!((volume(&h) - v3).abs() < 1e-6 * v3, "{}", volume(&h));

    click(&mut h, "Undo");
    assert!((volume(&h) - v2).abs() < 1e-6 * v2);
}

/// Countersink from the Hole feature's inspector: plate 60x40x10, Ø6 hole,
/// Ø12 x 90° countersink removes 36π (frustum h = 3 minus the Ø6 cylinder).
#[test]
fn countersink_from_the_hole_inspector() {
    use rcad_commands::SketchTemplate;
    use rcad_document::{FaceRef, FaceRole, SketchPlane};
    let mut h = harness();
    click(&mut h, "New");
    let req = |h: &mut Harness<'_, RcadApp>, c: Command| {
        h.state_mut().request(c);
        settle(h);
        h.state().summary().features.last().unwrap().id
    };
    let sk = req(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 60.0,
                height: 40.0,
                center: Some([30.0, 20.0]),
            },
        },
    );
    let ext = req(&mut h, Command::extrude(None, sk, 10.0));
    let hole = req(
        &mut h,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: ext,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [30.0, 20.0],
            diameter: 6.0,
            depth: None,
            counterbore: None,
            target: None,
        },
    );
    let name = h
        .state()
        .summary()
        .features
        .iter()
        .find(|f| f.id == hole)
        .unwrap()
        .name
        .clone();
    click(&mut h, &name);
    click(&mut h, "Countersink…");
    type_into(&mut h, "Countersink diameter (mm)", "12");
    click(&mut h, "OK");
    let s = h.state().summary();
    let last = s.features.last().unwrap();
    assert_eq!(
        last.kind,
        "countersink",
        "{:?}",
        h.state().diagnostics().last()
    );
    let want = 24000.0 - 90.0 * PI - 36.0 * PI;
    match s.bodies[0].measurements.as_ref().unwrap().volume {
        rcad_kernel::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => assert!(
            (volume - want).abs() <= error_estimate.max(1e-6 * want),
            "{volume} ± {error_estimate} vs {want}"
        ),
        ref v => panic!("{v:?}"),
    }
}
