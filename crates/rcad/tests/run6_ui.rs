//! Run-6 controls through the real widgets (egui_kittest): Extrude dialog
//! extent options (symmetric + thin, two sides, to plane, Intersect) with the
//! preview span agreeing with the committed solid, and the History timeline
//! marker / reorder buttons. Volumes are closed-form.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};

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

fn sketch(h: &mut Harness<'_, RcadApp>, plane: SketchPlane, t: SketchTemplate) -> FeatureId {
    request(
        h,
        Command::CreateSketch {
            name: None,
            plane,
            template: t,
        },
    )
}

fn rect(w: f64, hh: f64) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: hh,
        center: None,
    }
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

fn bounds_z(h: &Harness<'_, RcadApp>) -> (f64, f64) {
    let b = h.state().summary().bodies[0].bounds.unwrap();
    (b.min[2], b.max[2])
}

#[test]
fn symmetric_thin_extrusion_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    click(&mut h, "Extrude…");
    click(&mut h, "Symmetric");
    type_into(&mut h, "Whole length (mm)", "10");
    click(&mut h, "Thin");
    type_into(&mut h, "Wall (mm)", "2");
    settle(&mut h);
    // The preview spans what OK will build.
    let pv = h.state().extrude_preview().expect("preview");
    assert!((pv.z_range[0] + 5.0).abs() < 1e-9 && (pv.z_range[1] - 5.0).abs() < 1e-9);
    click(&mut h, "OK");
    assert!((h.state().volume().unwrap() - (1200.0 - 36.0 * 26.0) * 10.0).abs() < 1e-6);
    let (lo, hi) = bounds_z(&h);
    assert!((lo + 5.0).abs() < 1e-9 && (hi - 5.0).abs() < 1e-9);
    click(&mut h, "Undo");
    assert!(h.state().summary().bodies.is_empty());
}

#[test]
fn two_sides_and_intersect_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    click(&mut h, "Extrude…");
    click(&mut h, "Two sides");
    type_into(&mut h, "Extrude distance (mm)", "8");
    type_into(&mut h, "Second side (mm)", "3");
    settle(&mut h);
    let pv = h.state().extrude_preview().expect("preview");
    assert!((pv.z_range[0] + 3.0).abs() < 1e-9 && (pv.z_range[1] - 8.0).abs() < 1e-9);
    click(&mut h, "OK");
    let (lo, hi) = bounds_z(&h);
    assert!((lo + 3.0).abs() < 1e-9 && (hi - 8.0).abs() < 1e-9);
    assert!((h.state().volume().unwrap() - 1200.0 * 11.0).abs() < 1e-6);
    let _ = sk;
    // Intersect with a Ø20 circle from World XY, 5 deep (into z 0..5).
    sketch(
        &mut h,
        SketchPlane::WorldXY,
        SketchTemplate::CenteredCircle {
            diameter: 20.0,
            center: None,
        },
    );
    click(&mut h, "Extrude…");
    click(&mut h, "Intersect");
    type_into(&mut h, "Extrude distance (mm)", "5");
    click(&mut h, "OK");
    assert!(
        (h.state().volume().unwrap() - 500.0 * PI).abs() < 1e-6,
        "{:?}",
        h.state().volume()
    );
}

#[test]
fn timeline_marker_and_reorder_from_the_history() {
    let mut h = harness();
    click(&mut h, "New");
    let s1 = sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    let block = request(&mut h, Command::extrude(None, s1, 10.0));
    let s2 = sketch(
        &mut h,
        SketchPlane::WorldXY,
        SketchTemplate::CenteredCircle {
            diameter: 6.0,
            center: Some([-10.0, 0.0]),
        },
    );
    let hole = request(
        &mut h,
        Command::Extrude {
            name: None,
            sketch: s2,
            distance: 0.0,
            op: ExtrudeOp::Cut,
            target: None,
            through_all: true,
            reverse: false,
            body_name: None,
            region: None,
            options: Default::default(),
        },
    );
    let full = 12000.0 - 90.0 * PI;
    assert!((h.state().volume().unwrap() - full).abs() < 1e-6);
    // Roll back to after the block: the hole is rolled back.
    let n = fname(&h, block);
    click(&mut h, &n);
    click(&mut h, "Roll back here");
    assert!((h.state().volume().unwrap() - 12000.0).abs() < 1e-6);
    assert!(h.query_by_label_contains("timeline marker").is_some());
    click(&mut h, "Roll to end");
    assert!((h.state().volume().unwrap() - full).abs() < 1e-6);
    // Move the hole's cut up above its own sketch: refused, unchanged.
    let before = h.state().summary().features.clone();
    let n = fname(&h, hole);
    click(&mut h, &n);
    click(&mut h, "Move up");
    assert_eq!(
        h.state()
            .summary()
            .features
            .iter()
            .map(|f| f.id)
            .collect::<Vec<_>>(),
        before.iter().map(|f| f.id).collect::<Vec<_>>()
    );
    // Moving the hole's sketch up above the block is allowed (it depends on
    // nothing); the result is the same solid.
    let n = fname(&h, s2);
    click(&mut h, &n);
    click(&mut h, "Move up");
    let order: Vec<FeatureId> = h.state().summary().features.iter().map(|f| f.id).collect();
    assert_eq!(order, vec![s1, s2, block, hole]);
    assert!((h.state().volume().unwrap() - full).abs() < 1e-6);
}

#[test]
fn project_a_face_into_a_sketch_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let s1 = sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    let block = request(&mut h, Command::extrude(None, s1, 10.0));
    let above = sketch(
        &mut h,
        SketchPlane::Offset {
            base: rcad_document::PlaneBase::WorldXY,
            distance: 20.0,
            angle: 0.0,
        },
        SketchTemplate::Polygon {
            vertices: vec![[30.0, 30.0], [31.0, 30.0], [30.0, 31.0]],
        },
    );
    let n = fname(&h, above);
    click(&mut h, &n);
    click(&mut h, "Project geometry into this sketch");
    let label = format!("Project {} ExtrudeEndCap", fname(&h, block));
    click(&mut h, &label);
    let f = |h: &Harness<'_, RcadApp>| {
        h.state()
            .summary()
            .features
            .iter()
            .find(|f| f.id == above)
            .unwrap()
            .clone()
    };
    assert_eq!(f(&h).projections.len(), 1);
    // 3 own lines + 4 projected.
    assert_eq!(f(&h).sketch.unwrap().entities.len(), 7);
    click(&mut h, "Unlink 1");
    assert!(f(&h).projections.is_empty());
    assert_eq!(f(&h).sketch.unwrap().entities.len(), 7);
    click(&mut h, "Undo");
    assert_eq!(f(&h).projections.len(), 1);
}

#[test]
fn move_and_copy_a_body_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let s1 = sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    request(&mut h, Command::extrude(None, s1, 10.0));
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    click(&mut h, "Move / Copy");
    type_into(&mut h, "ΔX (mm)", "100");
    click(&mut h, "Move body");
    let b = h.state().summary().bodies[0].bounds.unwrap();
    assert!(
        (b.min[0] - 80.0).abs() < 1e-9 && (b.max[0] - 120.0).abs() < 1e-9,
        "{b:?}"
    );
    assert!((h.state().volume().unwrap() - 12000.0).abs() < 1e-6);
    // The new Move feature is selected after OK: back to the body.
    click(&mut h, &name);
    if h.query_by_label("Copy (keep the original; the copy is independent)")
        .is_none()
    {
        click(&mut h, "Move / Copy");
    }
    click(&mut h, "Copy (keep the original; the copy is independent)");
    click(&mut h, "Copy body");
    assert_eq!(h.state().summary().bodies.len(), 2);
    click(&mut h, "Undo");
    assert_eq!(h.state().summary().bodies.len(), 1);
}

#[test]
fn save_and_activate_configurations_from_the_browser() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::SetUserParameter {
        name: "length".into(),
        expression: "100".into(),
        unit: None,
        comment: None,
    });
    settle(&mut h);
    let s1 = sketch(&mut h, SketchPlane::WorldXY, rect(100.0, 30.0));
    h.state_mut().request(Command::SetExpression {
        feature: s1,
        name: "width".into(),
        expression: "length".into(),
    });
    settle(&mut h);
    request(&mut h, Command::extrude(None, s1, 10.0));
    click(&mut h, "Configurations (0)");
    type_into(&mut h, "Configuration name", "Long");
    type_into(&mut h, "Part number", "P-100");
    click(&mut h, "Save current as configuration");
    assert_eq!(h.state().summary().configurations.len(), 1);
    h.state_mut().request(Command::SetUserParameter {
        name: "length".into(),
        expression: "60".into(),
        unit: None,
        comment: None,
    });
    settle(&mut h);
    type_into(&mut h, "Configuration name", "Short");
    type_into(&mut h, "Part number", "P-60");
    click(&mut h, "Save current as configuration");
    assert!((h.state().volume().unwrap() - 18000.0).abs() < 1e-6);
    click(&mut h, "Activate Long");
    assert!((h.state().volume().unwrap() - 30000.0).abs() < 1e-6);
    assert_eq!(
        h.state().summary().active_configuration.as_deref(),
        Some("Long")
    );
    click(&mut h, "Activate Short");
    assert!((h.state().volume().unwrap() - 18000.0).abs() < 1e-6);
}

#[test]
fn revolve_about_the_sketch_x_axis_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    sketch(
        &mut h,
        SketchPlane::WorldXY,
        SketchTemplate::Polygon {
            vertices: vec![[0.0, 5.0], [30.0, 5.0], [30.0, 15.0], [0.0, 15.0]],
        },
    );
    click(&mut h, "Revolve…");
    type_into(&mut h, "Revolve angle (deg)", "360");
    h.get_by_label("Revolve axis").click();
    let _ = h.try_run();
    click(&mut h, "Sketch X axis");
    click(&mut h, "OK");
    assert!(
        (h.state().volume().unwrap() - PI * 200.0 * 30.0).abs() < 1e-6,
        "{:?}",
        h.state().volume()
    );
}

#[test]
fn split_a_body_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let s1 = sketch(&mut h, SketchPlane::WorldXY, rect(40.0, 30.0));
    request(&mut h, Command::extrude(None, s1, 10.0));
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    click(&mut h, "Split by plane");
    type_into(&mut h, "Split plane offset (mm)", "4");
    click(&mut h, "Split body");
    let bodies = h.state().summary().bodies.clone();
    assert_eq!(bodies.len(), 2);
    let v = |i: usize| match bodies[i].measurements.as_ref().unwrap().volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        ref x => panic!("{x:?}"),
    };
    assert!((v(0) - 4800.0).abs() < 1e-6 && (v(1) - 7200.0).abs() < 1e-6);
}

#[test]
fn sheet_metal_flange_and_flat_pattern_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let mut sk = rcad_sketch::Sketch::default();
    let p: Vec<_> = [[0.0, 22.0], [0.0, 0.0], [64.0, 0.0], [64.0, 22.0]]
        .iter()
        .map(|q| sk.add_point(q[0], q[1]))
        .collect();
    for i in 0..3 {
        sk.add_line(p[i], p[i + 1]);
    }
    let s1 = sketch(
        &mut h,
        SketchPlane::WorldXY,
        SketchTemplate::Custom { sketch: sk },
    );
    let n = fname(&h, s1);
    click(&mut h, &n);
    click(&mut h, "Sheet metal flange");
    type_into(&mut h, "K-factor", "0.4");
    click(&mut h, "Create flange");
    let want = 40.0 * 100.0 + 40.0 * 2.0 * (PI / 4.0) * (9.0 - 4.0);
    assert!(
        (h.state().volume().unwrap() - want).abs() < 1e-6,
        "{:?}",
        h.state().volume()
    );
    let f = h.state().summary().features.last().unwrap().clone();
    click(&mut h, &f.name);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flat.dxf");
    type_into(&mut h, "Flat pattern DXF path", path.to_str().unwrap());
    click(&mut h, "Export flat pattern");
    assert!(path.is_file());
}

#[test]
fn thread_rod_and_nut_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Thread…");
    click(&mut h, "OK");
    assert!(
        (h.state().volume().unwrap() - 3215.09586).abs() < 1e-3,
        "{:?}",
        h.state().volume()
    );
    click(&mut h, "Thread…");
    click(
        &mut h,
        "Square nut (internal thread, 0.1 mm radial clearance)",
    );
    click(&mut h, "OK");
    assert_eq!(h.state().summary().bodies.len(), 2);
}
