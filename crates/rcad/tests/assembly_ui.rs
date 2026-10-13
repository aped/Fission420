//! Browser "Components" section through the real widgets (egui_kittest, run 6):
//! create a component from the selected body, add and move an occurrence, ground,
//! BOM, interference, a joint value within/outside its limits; plus the display
//! scene drawing one placed copy per occurrence.
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::SketchPlane;
use rcad_document::assembly::{JointKind, OccurrenceId};

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

fn block() -> Vec<Command> {
    vec![
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 20.0,
                height: 30.0,
                center: Some([10.0, 15.0]),
            },
        },
        Command::extrude(None, rcad_document::FeatureId(1), 40.0),
    ]
}

#[test]
fn components_section_create_move_ground_bom_interference_joint() {
    let mut h = harness();
    click(&mut h, "New");
    for c in block() {
        h.state_mut().request(c);
        settle(&mut h);
    }
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    type_into(&mut h, "Component name", "Block");
    type_into(&mut h, "Part number", "P-7");
    click(&mut h, "Create component from selected body");
    let a = h.state().summary().assembly.clone();
    assert_eq!(
        a.components.len(),
        1,
        "{:?}",
        h.state().diagnostics().last()
    );
    assert_eq!(a.components[0].part_number, "P-7");
    assert_eq!(a.occurrences.len(), 1);
    click(&mut h, "Add occurrence of Block");
    assert_eq!(h.state().summary().assembly.occurrences.len(), 2);
    // Move Block#2 numerically so it overlaps Block#1 by 10 × 27 × 35.
    click(&mut h, "Move Block#2");
    type_into(&mut h, "X (mm)", "10");
    type_into(&mut h, "Y (mm)", "3");
    type_into(&mut h, "Z (mm)", "5");
    click(&mut h, "Apply placement");
    let o2 = h.state().summary().assembly.occurrences[1].clone();
    assert_eq!(o2.translate, [10.0, 3.0, 5.0]);
    click(&mut h, "Ground Block#1");
    assert!(h.state().summary().assembly.occurrences[0].grounded);
    click(&mut h, "Bill of materials");
    h.get_by_label("1. Block ×2 · P/N P-7");
    click(&mut h, "Check interference");
    let l = h.get_by_label_contains("Block#1 ∩ Block#2");
    let _ = l;
    // A slider joint driven from the browser: inside the limits, then outside.
    h.state_mut().request(Command::AddJoint {
        name: None,
        kind: JointKind::Slider,
        parent: OccurrenceId(1),
        child: OccurrenceId(2),
        origin: [0.0; 3],
        axis: Some([1.0, 0.0, 0.0]),
        axis_from: None,
        value: 0.0,
        min: Some(0.0),
        max: Some(40.0),
    });
    settle(&mut h);
    assert_eq!(h.state().summary().assembly.joints.len(), 1);
    click(&mut h, "Edit Joint1");
    type_into(&mut h, "Joint value", "30");
    click(&mut h, "Set value");
    assert_eq!(h.state().summary().assembly.joints[0].value, 30.0);
    let t = h.state().summary().assembly.occurrences[1].world.t;
    assert!((t[0] - 40.0).abs() < 1e-12, "{t:?}");
    type_into(&mut h, "Joint value", "50");
    click(&mut h, "Set value");
    assert_eq!(h.state().summary().assembly.joints[0].value, 30.0);
    assert!(h.state().diagnostics().last().unwrap().error);
}

#[test]
fn display_scene_draws_each_occurrence_placed() {
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    for c in block() {
        assert!(s.execute(&c).ok);
    }
    let body = s.summary().bodies[0].id;
    for c in [
        Command::CreateComponent {
            name: "Block".into(),
            bodies: vec![body],
            part_number: None,
        },
        Command::AddOccurrence {
            component: rcad_document::assembly::ComponentId(1),
            name: None,
            translate: [100.0, 0.0, 0.0],
            axis: None,
            angle_deg: 0.0,
        },
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{:?}", r.error);
    }
    let scene = rcad::engine::display_scene(s.model().unwrap().evaluation()).unwrap();
    assert_eq!(scene.bodies.len(), 2);
    let xs: Vec<f32> = scene.mesh.positions.iter().map(|p| p[0]).collect();
    let max = xs.iter().copied().fold(f32::MIN, f32::max);
    let min = xs.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        (min - 0.0).abs() < 1e-3 && (max - 120.0).abs() < 1e-3,
        "{min} {max}"
    );
}
