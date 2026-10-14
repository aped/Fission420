//! Mesh inspection and mesh edits in the body inspector through the real
//! widgets (egui_kittest; docs/MESH.md): import a 20 mm cube STL, check
//! self-intersections, preview and apply a two-sided plane cut (analytic
//! volumes), apply the inch → mm preset transform, undo.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::Command;

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

fn request(h: &mut Harness<'_, RcadApp>, c: Command) {
    h.state_mut().request(c);
    settle(h);
}

fn out(name: &str) -> PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/mesh_ui");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// Closed cube [0, 20]³ as ASCII STL.
fn cube() -> String {
    let p = [
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 20.0, 0.0],
        [0.0, 20.0, 0.0],
        [0.0, 0.0, 20.0],
        [20.0, 0.0, 20.0],
        [20.0, 20.0, 20.0],
        [0.0, 20.0, 20.0],
    ];
    let t = [
        [0, 2, 1],
        [0, 3, 2],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
        [4, 5, 6],
        [4, 6, 7],
    ];
    let mut s = String::from("solid c\n");
    for tr in t {
        s.push_str("facet normal 0 0 0\nouter loop\n");
        for i in tr {
            let v: [f64; 3] = p[i];
            s.push_str(&format!("vertex {} {} {}\n", v[0], v[1], v[2]));
        }
        s.push_str("endloop\nendfacet\n");
    }
    s.push_str("endsolid c\n");
    s
}

fn volume(h: &Harness<'_, RcadApp>, i: usize) -> f64 {
    let b = h.state().summary().bodies[i].clone();
    let d = b.mesh.expect("mesh body");
    assert!(d.closed, "{}: {d:?}", b.name);
    d.signed_volume.unwrap()
}

#[test]
fn inspect_cut_and_transform_a_mesh_body_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let stl = out("cube20.stl");
    std::fs::write(&stl, cube()).unwrap();
    request(
        &mut h,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    click(&mut h, "Mesh inspection");
    h.get_by_label_contains("0 duplicate");
    click(&mut h, "Check self-intersections");
    h.get_by_label_contains("no self-intersections found");

    click(&mut h, "Mesh edit");
    click(&mut h, "Plane cut");
    type_into(&mut h, "Mesh at (mm)", "5");
    click(&mut h, "Keep both");
    let rev = h.state().summary().revision;
    click(&mut h, "Preview edit");
    assert_eq!(
        h.state().summary().revision,
        rev,
        "preview changed the document"
    );
    assert_eq!(h.state().summary().bodies.len(), 1);
    h.get_by_label_contains("2 cut loops (2 capped");
    // Viewport preview: the cut plane at z = 5 across the body, and the two
    // result boxes split at z = 5.
    let pv = h.state().mesh_edit_preview().expect("viewport preview");
    let plane = pv.plane.expect("plane drawn");
    assert!(plane.iter().all(|p| p[2] == 5.0), "{plane:?}");
    assert!(pv.keep_dir.is_none(), "both sides kept");
    assert_eq!(pv.result_bounds.len(), 2);
    assert_eq!(pv.result_bounds[0].1[2], 5.0);
    assert_eq!(pv.result_bounds[1].0[2], 5.0);
    click(&mut h, "Apply as new body");
    assert_eq!(h.state().summary().bodies.len(), 3);
    assert!(
        !h.state().summary().bodies[0].visible,
        "source hidden, kept"
    );
    assert!((volume(&h, 1) - 2000.0).abs() < 1e-9);
    assert!((volume(&h, 2) - 6000.0).abs() < 1e-9);

    // The new lower half: inch -> mm preset.
    let lower = h.state().summary().bodies[1].name.clone();
    click(&mut h, &lower);
    click(&mut h, "Mesh edit");
    click(&mut h, "Transform");
    click(&mut h, "inch → mm (×25.4)");
    click(&mut h, "Apply as new body");
    assert_eq!(h.state().summary().bodies.len(), 4);
    let want = 2000.0 * 25.4f64.powi(3);
    assert!((volume(&h, 3) - want).abs() < 1e-6 * want);

    request(&mut h, Command::Undo);
    assert_eq!(h.state().summary().bodies.len(), 3);
}
