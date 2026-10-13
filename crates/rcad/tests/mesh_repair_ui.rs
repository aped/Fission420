//! Mesh check / repair in the body inspector through the real widgets
//! (egui_kittest, run 7; docs/MESH.md): import a cube STL missing its top,
//! preview (nothing changes), repair as a new body (closed), undo.
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
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/run7/mesh_ui");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// Cube [0, 20]³ without its two top triangles, as ASCII STL.
fn open_cube() -> String {
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

#[test]
fn preview_and_repair_a_mesh_body_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    let stl = out("open_cube.stl");
    std::fs::write(&stl, open_cube()).unwrap();
    request(
        &mut h,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    click(&mut h, "Mesh check / repair");
    type_into(&mut h, "Mesh hole fill limit", "8");
    let rev = h.state().summary().revision;
    click(&mut h, "Preview repair");
    assert_eq!(
        h.state().summary().revision,
        rev,
        "preview changed the document"
    );
    assert_eq!(h.state().summary().bodies.len(), 1);
    // The preview report is shown (one 4-edge hole, closed afterwards).
    h.get_by_label_contains("1 holes filled, 0 left open");
    click(&mut h, "Repair as new body");
    let bodies = h.state().summary().bodies.clone();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].mesh.as_ref().unwrap().closed);
    assert!(!bodies[0].visible, "source hidden, kept");
    request(&mut h, Command::Undo);
    assert_eq!(h.state().summary().bodies.len(), 1);
}
