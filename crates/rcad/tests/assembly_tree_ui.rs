//! Assembly UI through the real widgets (egui_kittest): the occurrence
//! tree with a sub-assembly, component activation (new bodies join the active
//! component, the rest is dimmed), the joint-creation dialog (kind, child,
//! explicit axis, limits, DOF feedback) and joint editing with a refused
//! out-of-limit value, selectable interference results highlighting both
//! occurrences, the structured BOM and the exploded-view toggle.
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::assembly::{ComponentId, JointKind, OccurrenceId};
use rcad_document::{BodyId, FeatureId, SketchPlane};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1600.0, 1000.0])
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

fn send(h: &mut Harness<'_, RcadApp>, c: Command) {
    h.state_mut().request(c);
    settle(h);
    let d = h.state().diagnostics().last().cloned();
    assert!(d.as_ref().is_none_or(|d| !d.error), "{d:?}");
}

/// Box `w` × `h` × `d` with its base centred at `c`.
fn block(h: &mut Harness<'_, RcadApp>, w: f64, ht: f64, d: f64, c: [f64; 2]) -> BodyId {
    let f = h.state().summary().features.len() as u32 + 1;
    send(
        h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: ht,
                center: Some(c),
            },
        },
    );
    send(h, Command::extrude(None, FeatureId(f), d));
    h.state().summary().bodies.last().unwrap().id
}

fn component(h: &mut Harness<'_, RcadApp>, name: &str, b: BodyId) {
    send(
        h,
        Command::CreateComponent {
            name: name.into(),
            bodies: vec![b],
            part_number: None,
            occurrences: vec![],
        },
    );
}

#[test]
fn tree_activation_and_joint_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    let base = block(&mut h, 40.0, 20.0, 4.0, [0.0, 0.0]);
    let pin = block(&mut h, 2.0, 2.0, 10.0, [30.0, 0.0]);
    component(&mut h, "Base", base);
    component(&mut h, "Pin", pin);
    // Group both into a sub-assembly through the tree.
    click(&mut h, "Occurrence Base#1");
    click(&mut h, "Group Base#1");
    h.get_by_label("Include Pin#1").click();
    settle(&mut h);
    type_into(&mut h, "Sub-assembly name", "Station");
    click(&mut h, "Create sub-assembly");
    let a = h.state().summary().assembly.clone();
    assert_eq!(
        a.components.len(),
        3,
        "{:?}",
        h.state().diagnostics().last()
    );
    // The tree shows the nested rows by path.
    h.get_by_label("Occurrence Station#1");
    h.get_by_label("Occurrence Station#1/Pin#1");
    // Activate Pin from its nested row: a new body joins Pin; Base is dimmed.
    click(&mut h, "Occurrence Station#1/Pin#1");
    click(&mut h, "Activate Pin");
    assert_eq!(h.state().summary().assembly.active, Some(ComponentId(2)));
    let boss = block(&mut h, 2.0, 2.0, 2.0, [30.0, 5.0]);
    let pin_c = h.state().summary().assembly.components[1].clone();
    assert_eq!(pin_c.bodies, vec![pin, boss]);
    let tints = h.state().display_tints();
    let scene = h.state().scene().clone();
    let base_tint = scene
        .bodies
        .iter()
        .zip(&tints)
        .find(|(b, _)| b.id == base)
        .map(|(_, t)| t.1)
        .unwrap();
    assert_eq!(base_tint, rcad_viewport::TINT_DIMMED);
    // The selected Pin row is highlighted.
    let pin_tint = scene
        .bodies
        .iter()
        .zip(&tints)
        .find(|(b, _)| b.id == pin)
        .map(|(_, t)| t.1)
        .unwrap();
    assert_eq!(pin_tint, rcad_viewport::TINT_HIGHLIGHT);
    click(&mut h, "Return to top level");
    assert_eq!(h.state().summary().assembly.active, None);
    // Joint dialog from the Base row: slider Base → Pin along +x, 0–40.
    click(&mut h, "Occurrence Station#1/Base#1");
    click(&mut h, "Joint from Base#1");
    h.get_by_label("Joint type").click();
    settle(&mut h);
    h.get_by_label("Slider").click();
    settle(&mut h);
    h.get_by_label("Joint child").click();
    settle(&mut h);
    h.get_by_label("Pin#1").click();
    settle(&mut h);
    type_into(&mut h, "Axis X", "1");
    type_into(&mut h, "Axis Z", "0");
    type_into(&mut h, "Lower limit", "0");
    type_into(&mut h, "Upper limit", "40");
    h.get_by_label_contains("Leaves 1 DOF");
    click(&mut h, "Create joint");
    let a = h.state().summary().assembly.clone();
    assert_eq!(a.joints.len(), 1, "{:?}", h.state().diagnostics().last());
    let j = &a.joints[0];
    assert_eq!(j.kind, JointKind::Slider);
    assert_eq!((j.min, j.max), (Some(0.0), Some(40.0)));
    assert_eq!(j.axis, [1.0, 0.0, 0.0]);
    // Edit: 25 accepted, 50 refused (value unchanged, no drift).
    click(&mut h, "Edit Joint1");
    type_into(&mut h, "Joint value", "25");
    click(&mut h, "Set value");
    assert_eq!(h.state().summary().assembly.joints[0].value, 25.0);
    let pin_world = |h: &Harness<'_, RcadApp>| {
        h.state()
            .summary()
            .assembly
            .occurrences
            .iter()
            .find(|o| o.id == OccurrenceId(2))
            .unwrap()
            .world
            .t
    };
    assert!((pin_world(&h)[0] - 25.0).abs() < 1e-12);
    // The drive slider (keyboard step) commits a new value within the limits
    // and the child follows it exactly.
    h.get_by_label("Drive Joint1").focus();
    let _ = h.try_run();
    h.key_press(Key::ArrowRight);
    settle(&mut h);
    let v = h.state().summary().assembly.joints[0].value;
    assert!(v > 25.0 && v <= 40.0, "{v}");
    assert!((pin_world(&h)[0] - v).abs() < 1e-12);
    type_into(&mut h, "Joint value", "25");
    click(&mut h, "Set value");
    type_into(&mut h, "Joint value", "50");
    click(&mut h, "Set value");
    assert_eq!(h.state().summary().assembly.joints[0].value, 25.0);
    assert!(h.state().diagnostics().last().unwrap().error);
    assert!((pin_world(&h)[0] - 25.0).abs() < 1e-12);
}

#[test]
fn interference_highlight_structured_bom_and_explode() {
    let mut h = harness();
    click(&mut h, "New");
    let b = block(&mut h, 20.0, 30.0, 40.0, [10.0, 15.0]);
    component(&mut h, "Block", b);
    send(
        &mut h,
        Command::AddOccurrence {
            component: ComponentId(1),
            name: None,
            owner: None,
            translate: [10.0, 3.0, 5.0],
            axis: None,
            angle_deg: 0.0,
        },
    );
    click(&mut h, "Check interference");
    h.get_by_label_contains("Block#1 × Block#2").click();
    settle(&mut h);
    let hl = h.state().assembly_highlight();
    assert_eq!(hl, vec!["O1".to_string(), "O2".to_string()]);
    let tints = h.state().display_tints();
    assert_eq!(tints.len(), 2);
    assert!(tints.iter().all(|t| t.1 == rcad_viewport::TINT_HIGHLIGHT));
    // Structured BOM (no nesting: one level).
    click(&mut h, "Structured BOM");
    h.get_by_label("1 Block ×2");
    // Exploded view: offset Block#2 by +100 z, toggle on: display moves,
    // the document geometry does not.
    click(&mut h, "Occurrence Block#2");
    click(&mut h, "Explode Block#2");
    type_into(&mut h, "Explode Z", "100");
    click(&mut h, "Set explode offset");
    h.get_by_label("Exploded view").click();
    settle(&mut h);
    let a = h.state().summary().assembly.clone();
    assert!(a.exploded);
    assert_eq!(a.occurrences[1].world.t, [10.0, 3.0, 5.0]);
    let zmax = h
        .state()
        .scene()
        .mesh
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::MIN, f32::max);
    assert!((zmax - 145.0).abs() < 1e-3, "{zmax}");
    // Hide Block#2: display drops it, BOM keeps it.
    click(&mut h, "Hide Block#2");
    assert_eq!(h.state().scene().bodies.len(), 1);
    click(&mut h, "Bill of materials");
    h.get_by_label("1. Block ×2");
}

/// Save a 10 × 10 × `h` box document to `path` (headless session).
fn cube_source(path: &std::path::Path, h: f64) {
    let mut s = rcad_commands::Session::new();
    s.execute(&Command::NewDocument);
    for c in [
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 10.0,
                height: 10.0,
                center: Some([0.0, 0.0]),
            },
        },
        Command::extrude(None, FeatureId(1), h),
        Command::Save {
            path: path.to_path_buf(),
        },
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{:?}", r.error);
    }
}

#[test]
fn external_component_insert_check_and_update() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("cube.rcad.json");
    cube_source(&src, 10.0);
    let mut h = harness();
    click(&mut h, "New");
    type_into(&mut h, "External document", src.to_str().unwrap());
    h.get_by_label("Linked").click();
    settle(&mut h);
    click(&mut h, "Insert external component");
    let a = h.state().summary().assembly.clone();
    assert_eq!(
        a.components.len(),
        1,
        "{:?}",
        h.state().diagnostics().last()
    );
    assert!(a.components[0].external.as_ref().unwrap().path.is_some());
    let vol = |h: &Harness<'_, RcadApp>| match h.state().summary().bodies[0]
        .measurements
        .as_ref()
        .unwrap()
        .volume
    {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    };
    assert!((vol(&h) - 1000.0).abs() < 1e-6);
    cube_source(&src, 30.0);
    click(&mut h, "Check external files");
    h.get_by_label_contains("cube: modified");
    assert!((vol(&h) - 1000.0).abs() < 1e-6, "pinned until updated");
    click(&mut h, "Occurrence cube#1");
    click(&mut h, "Update cube from file");
    assert!((vol(&h) - 3000.0).abs() < 1e-6);
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

/// Clicking a placed copy in the 3D view selects that instance's tree row
/// (and highlights it); the face pick is the definition face.
#[test]
fn viewport_pick_selects_the_occurrence() {
    let mut h = harness();
    click(&mut h, "New");
    let b = block(&mut h, 20.0, 20.0, 10.0, [0.0, 0.0]);
    component(&mut h, "Block", b);
    send(
        &mut h,
        Command::AddOccurrence {
            component: ComponentId(1),
            name: None,
            owner: None,
            translate: [40.0, 0.0, 0.0],
            axis: None,
            angle_deg: 0.0,
        },
    );
    // Top face centre of the second copy: (40, 0, 10).
    let p = h.state().world_to_screen([40.0, 0.0, 10.0]).unwrap();
    pointer_click(&mut h, p);
    settle(&mut h);
    assert!(h.state().selected_face().is_some(), "a face is picked");
    assert_eq!(h.state().assembly_highlight(), vec!["O2".to_string()]);
    h.get_by_label("Selected: Block#2");
    // The first copy.
    let p = h.state().world_to_screen([0.0, 0.0, 10.0]).unwrap();
    pointer_click(&mut h, p);
    settle(&mut h);
    assert_eq!(h.state().assembly_highlight(), vec!["O1".to_string()]);
}

/// Part number and component deletion from the rename panel; joint axis
/// redefinition from the joint editor (the child does not move).
#[test]
fn part_number_delete_component_and_joint_axis() {
    let mut h = harness();
    click(&mut h, "New");
    let a = block(&mut h, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut h, 2.0, 2.0, 2.0, [20.0, 0.0]);
    let c = block(&mut h, 2.0, 2.0, 2.0, [-20.0, 0.0]);
    component(&mut h, "A", a);
    component(&mut h, "B", b);
    component(&mut h, "C", c);
    click(&mut h, "Occurrence A#1");
    click(&mut h, "Rename A#1");
    type_into(&mut h, "Part number ", "PN-A");
    click(&mut h, "Set part number");
    assert_eq!(
        h.state().summary().assembly.components[0].part_number,
        "PN-A"
    );
    click(&mut h, "Occurrence C#1");
    click(&mut h, "Rename C#1");
    click(&mut h, "Delete component C");
    assert_eq!(h.state().summary().assembly.components.len(), 2);
    send(
        &mut h,
        Command::AddJoint {
            name: None,
            kind: JointKind::Revolute,
            parent: OccurrenceId(1),
            child: OccurrenceId(2),
            origin: [0.0; 3],
            axis: Some([0.0, 0.0, 1.0]),
            axis_from: None,
            value: 90.0,
            min: None,
            max: None,
            extra: vec![],
            dir2: None,
            child_axis_from: None,
            flip: false,
        },
    );
    let world = |h: &Harness<'_, RcadApp>| {
        h.state()
            .summary()
            .assembly
            .occurrences
            .iter()
            .find(|o| o.id == OccurrenceId(2))
            .unwrap()
            .world
    };
    let before = world(&h);
    click(&mut h, "Edit Joint1");
    type_into(&mut h, "Joint axis X", "1");
    type_into(&mut h, "Joint axis Z", "0");
    click(&mut h, "Set axis");
    assert_eq!(h.state().summary().assembly.joints[0].axis, [1.0, 0.0, 0.0]);
    assert_eq!(world(&h), before, "nothing moves");
}

/// A nudge keeps a non-Z rotation axis; the joint dialog stays
/// open (inputs kept) when the engine refuses the joint, and closes on success.
#[test]
fn nudge_keeps_axis_and_dialog_survives_refusal() {
    let mut h = harness();
    click(&mut h, "New");
    let a = block(&mut h, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut h, 2.0, 2.0, 2.0, [20.0, 0.0]);
    component(&mut h, "A", a);
    component(&mut h, "B", b);
    send(
        &mut h,
        Command::SetOccurrenceTransform {
            occurrence: OccurrenceId(2),
            translate: [0.0, 0.0, 0.0],
            axis: Some([1.0, 0.0, 0.0]),
            angle_deg: 90.0,
        },
    );
    click(&mut h, "Occurrence B#1");
    click(&mut h, "Move B#1");
    click(&mut h, "+X");
    let o = h.state().summary().assembly.occurrences[1].clone();
    assert_eq!(o.translate, [5.0, 0.0, 0.0]);
    assert_eq!(o.axis, [1.0, 0.0, 0.0]);
    assert!((o.angle_deg - 90.0).abs() < 1e-12);
    // Joint dialog: initial value outside its own limits → refused, kept open.
    click(&mut h, "Occurrence A#1");
    click(&mut h, "Joint from A#1");
    h.get_by_label("Joint child").click();
    settle(&mut h);
    h.get_by_label("B#1").click();
    settle(&mut h);
    type_into(&mut h, "Initial value", "50");
    type_into(&mut h, "Upper limit", "40");
    click(&mut h, "Create joint");
    assert!(h.state().summary().assembly.joints.is_empty());
    assert!(h.state().diagnostics().last().unwrap().error);
    h.get_by_label("Create joint");
    type_into(&mut h, "Initial value", "30");
    click(&mut h, "Create joint");
    assert_eq!(h.state().summary().assembly.joints.len(), 1);
    assert!(
        h.query_by_label("Create joint").is_none(),
        "closed on success"
    );
}

/// The joint editor's sweep check reports the sampled values that interfere.
#[test]
fn joint_sweep_check_in_the_editor() {
    let mut h = harness();
    click(&mut h, "New");
    let a = block(&mut h, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut h, 4.0, 4.0, 4.0, [20.0, 0.0]);
    component(&mut h, "A", a);
    component(&mut h, "B", b);
    // B slides along −x from x 18..22 towards A (x −5..5); lifted 1 mm so no
    // faces are coplanar. Overlap for travel > 13.
    send(
        &mut h,
        Command::SetOccurrenceTransform {
            occurrence: OccurrenceId(2),
            translate: [0.0, 0.0, 1.0],
            axis: None,
            angle_deg: 0.0,
        },
    );
    send(
        &mut h,
        Command::AddJoint {
            name: None,
            kind: JointKind::Slider,
            parent: OccurrenceId(1),
            child: OccurrenceId(2),
            origin: [0.0; 3],
            axis: Some([-1.0, 0.0, 0.0]),
            axis_from: None,
            value: 0.0,
            min: Some(0.0),
            max: Some(20.0),
            extra: vec![],
            dir2: None,
            child_axis_from: None,
            flip: false,
        },
    );
    click(&mut h, "Edit Joint1");
    click(&mut h, "Sweep check Joint1");
    // Samples 0, 1, …, 20: interference for values 14..20 (7 samples).
    h.get_by_label_contains("interference at 7 of 21 samples");
    assert_eq!(h.state().summary().assembly.joints[0].value, 0.0);
}

/// Import STEP assembly from the browser: the OpenCascade two-box assembly
/// arrives as components with occurrences under the root product.
#[test]
fn import_step_assembly_from_the_browser() {
    let mut h = harness();
    click(&mut h, "New");
    let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-kernel/tests/fixtures/step/two_boxes_assembly_mm.step");
    type_into(&mut h, "External document", f.to_str().unwrap());
    click(&mut h, "Import STEP assembly");
    let a = h.state().summary().assembly.clone();
    assert_eq!(
        a.components.len(),
        3,
        "{:?}",
        h.state().diagnostics().last()
    );
    // The tree shows the nested occurrence row.
    assert!(
        h.state()
            .summary()
            .assembly
            .instances
            .iter()
            .any(|i| i.path_name.ends_with("/BoxB"))
    );
}

/// Assembly mass (missing material reported) and BOM CSV export from the
/// browser.
#[test]
fn assembly_mass_and_bom_csv() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness();
    click(&mut h, "New");
    let b = block(&mut h, 10.0, 10.0, 10.0, [0.0, 0.0]);
    component(&mut h, "Cube", b);
    click(&mut h, "Assembly mass");
    h.get_by_label_contains("No assembly total");
    send(
        &mut h,
        Command::SetBodyMaterial {
            body: b,
            material: None,
            density: Some(2.0),
            density_unit: rcad_document::DensityUnit::GPerCm3,
            name: None,
        },
    );
    click(&mut h, "Assembly mass");
    // 1000 mm³ at 2 g/cm³ = 2 g, centre (0, 0, 5).
    h.get_by_label_contains("Assembly: 2 g, 1000 mm³, centre of mass (0, 0, 5) mm");
    let p = dir.path().join("bom.csv");
    type_into(&mut h, "BOM CSV path", p.to_str().unwrap());
    click(&mut h, "Export BOM CSV");
    let t = std::fs::read_to_string(&p).unwrap();
    assert!(t.lines().nth(1).unwrap().starts_with("1,Cube,,1,"), "{t}");
}

fn two_blocks_with_slider(h: &mut Harness<'_, RcadApp>, limits: Option<(f64, f64)>) {
    click(h, "New");
    let a = block(h, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(h, 4.0, 4.0, 4.0, [20.0, 0.0]);
    component(h, "A", a);
    component(h, "B", b);
    send(
        h,
        Command::AddJoint {
            name: None,
            kind: JointKind::Slider,
            parent: OccurrenceId(1),
            child: OccurrenceId(2),
            origin: [0.0; 3],
            axis: Some([1.0, 0.0, 0.0]),
            axis_from: None,
            value: 0.0,
            min: limits.map(|l| l.0),
            max: limits.map(|l| l.1),
            extra: vec![],
            dir2: None,
            child_axis_from: None,
            flip: false,
        },
    );
    click(h, "Edit Joint1");
}

/// One pointer drag on the drive slider commits one value
/// (one undo step), at release.
#[test]
fn drive_slider_drag_commits_once() {
    let mut h = harness();
    two_blocks_with_slider(&mut h, Some((0.0, 40.0)));
    let r = h.get_by_label("Drive Joint1").rect();
    let rev = h.state().summary().revision;
    let (a, b) = (
        Pos2::new(r.left() + 6.0, r.center().y),
        Pos2::new(r.left() + 60.0, r.center().y),
    );
    h.event(egui::Event::PointerMoved(a));
    let _ = h.try_run();
    h.event(egui::Event::PointerButton {
        pos: a,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let _ = h.try_run();
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(a + (b - a) * (k as f32 / 8.0)));
        let _ = h.try_run();
    }
    assert_eq!(
        h.state().summary().revision,
        rev,
        "nothing committed mid-drag"
    );
    h.event(egui::Event::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    settle(&mut h);
    settle(&mut h);
    let v = h.state().summary().assembly.joints[0].value;
    assert_eq!(h.state().summary().revision, rev + 1, "one commit");
    assert!(v > 0.0, "{v}");
}

/// A joint without limits says why drive and sweep are
/// unavailable; a bad axis field names itself; a refused sweep clears the
/// previous result.
#[test]
fn joint_editor_explains_unavailable_controls() {
    let mut h = harness();
    two_blocks_with_slider(&mut h, None);
    h.get_by_label_contains("set both limits");
    type_into(&mut h, "Joint axis Y", "x");
    h.get_by_label_contains("Not a number: axis Y");
    type_into(&mut h, "Joint axis Y", "0");
    send(
        &mut h,
        Command::SetJointLimits {
            joint: rcad_document::assembly::JointId(1),
            min: Some(0.0),
            max: Some(20.0),
            index: 0,
        },
    );
    click(&mut h, "Sweep check Joint1");
    h.get_by_label_contains("Joint1 sweep:");
    h.state_mut().request(Command::JointSweep {
        joint: rcad_document::assembly::JointId(1),
        index: 0,
        from: 0.0,
        to: 20.0,
        steps: 1,
        occurrences: None,
    });
    settle(&mut h);
    assert!(h.query_by_label_contains("Joint1 sweep:").is_none());
}

/// A STEP assembly without a length unit imports once a
/// unit is chosen in the browser.
#[test]
fn import_step_assembly_with_chosen_units() {
    let dir = tempfile::tempdir().unwrap();
    let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-kernel/tests/fixtures/step/two_boxes_assembly_mm.step");
    let text = std::fs::read_to_string(f)
        .unwrap()
        .replace("(#24,#25,#26)", "(#25,#26)")
        .replace("(#188,#189,#190)", "(#189,#190)")
        .replace("(#357,#358,#359)", "(#358,#359)");
    let p = dir.path().join("unitless.step");
    std::fs::write(&p, text).unwrap();
    let mut h = harness();
    click(&mut h, "New");
    type_into(&mut h, "External document", p.to_str().unwrap());
    click(&mut h, "Import STEP assembly");
    assert!(h.state().summary().assembly.components.is_empty());
    assert!(h.state().diagnostics().last().unwrap().error);
    click(&mut h, "STEP assembly units");
    click(&mut h, "cm");
    click(&mut h, "Import STEP assembly");
    let a = h.state().summary().assembly.clone();
    assert_eq!(
        a.components.len(),
        3,
        "{:?}",
        h.state().diagnostics().last()
    );
}
