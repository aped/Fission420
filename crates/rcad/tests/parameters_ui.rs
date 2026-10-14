//! User parameters through the real widgets: add parameters in the Parameters
//! panel, bind a sketch dimension and an extrusion distance to expressions in
//! the inspector, edit a parameter (one Undo step), see a refused edit with the
//! model unchanged, rename (dependents follow), and the enclosure example's
//! fit clearance edited from the panel.
use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

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
        assert!(start.elapsed() < Duration::from_secs(120), "not idle");
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

fn add_param(h: &mut Harness<'_, RcadApp>, name: &str, expr: &str) {
    type_into(h, "New parameter name", name);
    type_into(h, "New parameter expression", expr);
    click(h, "Add parameter");
}

fn volume(h: &Harness<'_, RcadApp>) -> f64 {
    h.state().volume().expect("volume")
}

#[test]
fn parameters_panel_and_expression_bindings() {
    let mut h = harness();
    click(&mut h, "New");
    add_param(&mut h, "plate", "40");
    add_param(&mut h, "wall", "2");
    add_param(&mut h, "inner", "plate - 2 * wall");
    let p = h.state().summary().parameters.clone();
    assert_eq!(p.len(), 3);
    assert_eq!(p[2].value, Some(36.0));
    h.get_by_label("= 36 mm"); // value shown in the panel
    // A rectangle sketch; bind its width to an expression in the inspector.
    h.state_mut().request(Command::CreateSketch {
        name: Some("Plate outline".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 20.0,
            center: None,
        },
    });
    settle(&mut h);
    click(&mut h, "Plate outline");
    type_into(&mut h, "width (mm)", "inner + wall");
    click(&mut h, "Apply");
    let f = h.state().feature_by_name("Plate outline").unwrap().clone();
    assert_eq!(
        f.expressions.get("width").map(String::as_str),
        Some("inner + wall")
    );
    let w = f.parameters.iter().find(|(n, _)| n == "width").unwrap().1;
    assert_eq!(w, 38.0);
    h.get_by_label("= 38"); // value beside the expression
    // Extrude and bind the distance.
    click(&mut h, "Extrude this sketch…");
    click(&mut h, "OK");
    assert!((volume(&h) - 38.0 * 20.0 * 10.0).abs() < 1e-6);
    let ext = h.state().summary().features[1].name.clone();
    click(&mut h, &ext);
    type_into(&mut h, "distance (mm)", "wall * 3");
    click(&mut h, "Apply");
    assert!((volume(&h) - 38.0 * 20.0 * 6.0).abs() < 1e-6);
    // Edit a parameter in the panel: one transaction, one Undo step.
    let rev = h.state().summary().revision;
    click(&mut h, "param wall");
    type_into(&mut h, "Parameter expression", "3");
    click(&mut h, "Apply parameter");
    assert_eq!(h.state().summary().revision, rev + 1);
    // width = 40 - 6 + 3 = 37, distance = 9
    assert!(
        (volume(&h) - 37.0 * 20.0 * 9.0).abs() < 1e-6,
        "{}",
        volume(&h)
    );
    click(&mut h, "Undo");
    assert!((volume(&h) - 38.0 * 20.0 * 6.0).abs() < 1e-6);
    click(&mut h, "Redo");
    // A refused edit: division by zero; shown in the panel, model unchanged.
    let rev = h.state().summary().revision;
    click(&mut h, "param plate");
    type_into(&mut h, "Parameter expression", "40 / (wall - 3)");
    click(&mut h, "Apply parameter");
    assert_eq!(h.state().summary().revision, rev);
    let refused = h.get_by_label_contains("Refused, the model is unchanged");
    assert!(format!("{:?}", refused.accesskit_node()).contains("division by zero"));
    click(&mut h, "Cancel parameter edit");
    // Rename: dependents' expressions follow.
    click(&mut h, "param wall");
    type_into(&mut h, "Parameter name", "t");
    click(&mut h, "Apply parameter");
    let p = h.state().summary().parameters.clone();
    assert!(p.iter().any(|x| x.name == "t"));
    assert_eq!(
        p.iter().find(|x| x.name == "inner").unwrap().expression,
        "plate - 2 * t"
    );
    let f = h.state().feature_by_name("Plate outline").unwrap().clone();
    assert_eq!(f.expressions["width"], "inner + t");
    // A used parameter cannot be deleted (button disabled with the reason).
    click(&mut h, "param t");
    assert!(
        h.get_by_label("Delete parameter")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut h, "Cancel parameter edit");
    // Typing a plain number in a bound field removes the binding.
    click(&mut h, &ext);
    type_into(&mut h, "distance (mm)", "5");
    click(&mut h, "Apply");
    let e = h.state().feature_by_name(&ext).unwrap().clone();
    assert!(e.expressions.is_empty());
    assert!((volume(&h) - 37.0 * 20.0 * 5.0).abs() < 1e-6);
    let _ = PI;
}

#[test]
fn enclosure_fit_clearance_from_the_panel() {
    let mut h = harness();
    let doc = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/enclosure.rcad.json");
    h.state_mut().request(Command::Open { path: doc });
    settle(&mut h);
    let lid = |h: &Harness<'_, RcadApp>| {
        let b = h.state().summary().bodies[1].clone();
        match b.measurements.unwrap().volume {
            rcad_kernel::VolumeResult::Value { volume, .. } => volume,
            v => panic!("{v:?}"),
        }
    };
    assert!((lid(&h) - 9404.0).abs() < 1e-6);
    click(&mut h, "param fit_clearance");
    type_into(&mut h, "Parameter expression", "0.35");
    click(&mut h, "Apply parameter");
    // Lip outer 75.3 x 45.3, inner 71.3 x 41.3.
    let want = 80.0 * 50.0 * 2.0 + (75.3 * 45.3 - 71.3 * 41.3) * 3.0;
    assert!((lid(&h) - want).abs() < 1e-6, "{} vs {want}", lid(&h));
    h.get_by_label("= 75.3 mm"); // dependent parameter updated in the panel
    let p = h.state().summary().parameters.clone();
    let ll = p.iter().find(|x| x.name == "lip_length").unwrap();
    assert!((ll.value.unwrap() - 75.3).abs() < 1e-9);
}

/// Selection-based measurement of the enclosure: wall thickness
/// (base outer -X face to the cavity's -X wall) and the lip's per-side
/// clearance (lip outer -X face of the lid to the cavity wall of the base),
/// exact from the B-rep planes.
#[test]
fn measure_wall_and_lip_clearance_between_faces() {
    let mut h = harness();
    let doc = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/enclosure.rcad.json");
    h.state_mut().request(Command::Open { path: doc });
    settle(&mut h);
    let find = |h: &Harness<'_, RcadApp>, body: u32, nx: f64, x: f64| {
        h.state()
            .faces()
            .iter()
            .find(|f| {
                f.body.map(|b| b.0) == Some(body)
                    && f.plane.is_some_and(|p| {
                        (p.normal[0] - nx).abs() < 1e-9 && (p.point[0] - x).abs() < 1e-9
                    })
            })
            .map(|f| f.group)
            .unwrap_or_else(|| panic!("no face body {body} n {nx} x {x}"))
    };
    let outer = find(&h, 1, -1.0, 0.0);
    let cavity_wall = find(&h, 1, 1.0, 2.0);
    let lip_outer = find(&h, 2, -1.0, 2.25);
    h.state_mut().select_face(Some(outer));
    let _ = h.try_run();
    click(&mut h, "Measure from this face");
    h.state_mut().select_face(Some(cavity_wall));
    let _ = h.try_run();
    h.get_by_label_contains("Distance between parallel faces: 2 mm");
    click(&mut h, "Clear measurement");
    click(&mut h, "Measure from this face");
    h.state_mut().select_face(Some(lip_outer));
    let _ = h.try_run();
    h.get_by_label_contains("Distance between parallel faces: 0.25 mm");
}

/// Templates > Parametric enclosure + lid builds the editable
/// enclosure in a new document (same parameters and volumes as the example).
#[test]
fn enclosure_template_from_the_menu() {
    let mut h = harness();
    click(&mut h, "New");
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(&mut h, "Parametric enclosure + lid");
    let s = h.state().summary().clone();
    assert_eq!(s.parameters.len(), 15);
    assert_eq!(s.bodies.len(), 2);
    let v: Vec<f64> = s
        .bodies
        .iter()
        .map(|b| match b.measurements.clone().unwrap().volume {
            rcad_kernel::VolumeResult::Value { volume, .. } => volume,
            v => panic!("{v:?}"),
        })
        .collect();
    assert!(
        (v[0] - 19592.0).abs() < 1e-6 && (v[1] - 9404.0).abs() < 1e-6,
        "{v:?}"
    );
    assert!(s.path.is_none(), "a new, unsaved document");
}
