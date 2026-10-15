//! Edit Feature through the real widgets: right-click ▸ Edit Feature on a
//! History row (double-click does the same), the docked dialog reopens prefilled, OK
//! replaces the definition in place as one undo step. Volumes are checked
//! against closed forms: a 40 × 30 × 10 plate with a Ø10 boss sketched on its
//! top face, 5 mm (Join, one side = +125π; Symmetric = half inside the plate,
//! +62.5π; Cut one side = −125π).
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate, end_cap_of};
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

fn volume(h: &Harness<'_, RcadApp>) -> f64 {
    h.state().volume().expect("volume")
}

fn assert_volume(h: &Harness<'_, RcadApp>, want: f64) {
    let v = volume(h);
    assert!((v - want).abs() < 1e-6 * want.max(1.0), "{v} vs {want}");
}

/// Plate (Extrude1) and a Ø10 × 5 boss joined on its top face (Extrude2).
fn plate_with_boss(h: &mut Harness<'_, RcadApp>) {
    click(h, "New");
    let s = h.state_mut();
    s.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: None,
        },
    });
    s.request(Command::extrude(None, FeatureId(1), 10.0));
    settle(h);
    let body = h.state().summary().bodies[0].id;
    let plate = h.state().feature_by_name("Extrude1").expect("Extrude1").id;
    let s = h.state_mut();
    s.request(Command::CreateSketch {
        name: None,
        plane: end_cap_of(plate),
        template: SketchTemplate::CenteredCircle {
            diameter: 10.0,
            center: None,
        },
    });
    settle(h);
    let circle = h.state().summary().features.last().unwrap().id;
    h.state_mut().request(Command::Extrude {
        name: None,
        sketch: circle,
        distance: 5.0,
        op: ExtrudeOp::Join,
        target: Some(body),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    settle(h);
    assert_volume(h, 12000.0 + 125.0 * PI);
}

/// Right-click ▸ Edit Feature on the History row (the double-click path
/// is exercised with real input by the live walkthrough).
fn edit(h: &mut Harness<'_, RcadApp>, name: &str) {
    h.get_by_label(name).click_secondary();
    settle(h);
    click(h, &format!("Edit Feature {name}"));
    let id = h.state().feature_by_name(name).unwrap().id;
    assert_eq!(
        h.state().editing_feature(),
        Some(id),
        "double-click did not open Edit Feature: {:?}",
        h.state().diagnostics().last()
    );
    h.get_by_label(&format!("Edit {name}"));
}

#[test]
fn join_to_cut_and_one_side_to_symmetric_match_the_closed_form() {
    let mut h = harness();
    plate_with_boss(&mut h);
    let n = h.state().summary().features.len();
    // Join → Cut (one side, into the plate).
    edit(&mut h, "Extrude2");
    // Prefilled from the stored definition.
    let field = format!(
        "{:?}",
        h.get_by_label("Extrude distance (mm)").accesskit_node()
    );
    assert!(field.contains("\"5\""), "{field}");
    click(&mut h, "Cut");
    click(&mut h, "OK");
    assert_volume(&h, 12000.0 - 125.0 * PI);
    // Edited in place: same feature count, same name and position.
    assert_eq!(h.state().summary().features.len(), n);
    assert_eq!(h.state().summary().features[n - 1].name, "Extrude2");
    // Back to Join, then Symmetric: half the boss is inside the plate.
    edit(&mut h, "Extrude2");
    click(&mut h, "Join");
    click(&mut h, "Symmetric");
    click(&mut h, "OK");
    assert_volume(&h, 12000.0 + 62.5 * PI);
    // Undo restores the previous definition (Cut, one side).
    h.state_mut().request(Command::Undo);
    settle(&mut h);
    assert_volume(&h, 12000.0 - 125.0 * PI);
    let f = h.state().feature_by_name("Extrude2").unwrap();
    assert!(matches!(
        f.operation,
        Some(rcad_document::Operation::Cut { .. })
    ));
}

#[test]
fn cancel_and_escape_leave_the_document_unchanged() {
    let mut h = harness();
    plate_with_boss(&mut h);
    let rev = h.state().summary().revision;
    edit(&mut h, "Extrude2");
    click(&mut h, "Cut");
    type_into(&mut h, "Extrude distance (mm)", "8");
    click(&mut h, "Cancel");
    assert_eq!(h.state().summary().revision, rev);
    assert_volume(&h, 12000.0 + 125.0 * PI);
    // Edit Feature again, then Esc.
    edit(&mut h, "Extrude2");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("Edit Extrude2").is_none());
    assert_eq!(h.state().summary().revision, rev);
}

#[test]
fn distance_expression_binds_to_a_parameter() {
    let mut h = harness();
    plate_with_boss(&mut h);
    h.state_mut().request(Command::SetUserParameter {
        name: "boss".into(),
        expression: "8".into(),
        unit: None,
        comment: None,
    });
    settle(&mut h);
    edit(&mut h, "Extrude2");
    // An invalid expression turns the field red and disables OK.
    type_into(&mut h, "Extrude distance (mm)", "nosuch / 2");
    assert!(
        h.query_by_label_contains("unknown").is_some()
            || h.query_by_label_contains("nosuch").is_some()
    );
    type_into(&mut h, "Extrude distance (mm)", "boss / 2");
    click(&mut h, "OK");
    assert_volume(&h, 12000.0 + 100.0 * PI);
    let f = h.state().feature_by_name("Extrude2").unwrap();
    assert_eq!(
        f.expressions.get("distance").map(String::as_str),
        Some("boss / 2")
    );
}

/// Revolve as Cut with a target from the dialog: a ring (r 5..10, z 4..12)
/// revolved as its own body, then Edit Feature ▸ Cut with the block as
/// target cuts a groove into the top of the 40 × 30 × 10 block:
/// 12000 − π(10² − 5²)·6.
#[test]
fn revolve_new_body_to_cut_with_a_target_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    let s = h.state_mut();
    s.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: None,
        },
    });
    s.request(Command::extrude(None, FeatureId(1), 10.0));
    s.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXZ,
        template: SketchTemplate::CenteredRectangle {
            width: 5.0,
            height: 8.0,
            center: Some([7.5, 8.0]),
        },
    });
    settle(&mut h);
    let profile = h.state().summary().features.last().unwrap().id;
    h.state_mut().request(Command::Revolve {
        name: None,
        sketch: profile,
        angle_deg: 360.0,
        axis: Default::default(),
        reverse: false,
        op: ExtrudeOp::NewBody,
        target: None,
    });
    settle(&mut h);
    assert_eq!(
        h.state().summary().bodies.len(),
        2,
        "{:?}",
        h.state().diagnostics().last()
    );
    let block = h.state().summary().bodies[0].name.clone();
    edit(&mut h, "Revolve1");
    click(&mut h, "Revolve Cut");
    h.get_by_label("Revolve target body").click();
    settle(&mut h);
    click(&mut h, &format!("Revolve target {block}"));
    click(&mut h, "OK");
    assert_eq!(
        h.state().summary().bodies.len(),
        1,
        "{:?}",
        h.state().diagnostics().last()
    );
    assert_volume(&h, 12000.0 - PI * (100.0 - 25.0) * 6.0);
    let f = h.state().feature_by_name("Revolve1").unwrap();
    assert_eq!(f.kind, "revolve");
    // Undo: the ring is its own body again.
    h.state_mut().request(Command::Undo);
    settle(&mut h);
    assert_eq!(h.state().summary().bodies.len(), 2);
}
