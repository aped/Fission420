//! Body material and mass properties through the real widgets (egui_kittest,
//! run 6): pick a catalog material, then a custom density, in the body
//! inspector, and read the mass properties back. Expected values are closed-form
//! (20 × 30 × 40 mm box: 24 cm³; 1.24 g/cm³ → 29.76 g; 1.2 g/cm³ → 28.8 g).
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
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

fn mass(h: &Harness<'_, RcadApp>) -> Option<f64> {
    h.state().summary().bodies[0].mass
}

#[test]
fn set_material_and_read_mass_properties_in_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 20.0,
                height: 30.0,
                center: Some([10.0, 15.0]),
            },
        },
    );
    let s1 = h.state().summary().features.last().unwrap().id;
    request(&mut h, Command::extrude(None, s1, 40.0));
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    h.get_by_label("Material: none (no mass)");
    // Catalog material from the combo box.
    click(&mut h, "Set material");
    click(&mut h, "Material");
    click(&mut h, "PLA (1.24 g/cm³)");
    click(&mut h, "Apply material");
    let m = h.state().summary().bodies[0]
        .material
        .clone()
        .expect("material");
    assert_eq!(
        (m.name.as_str(), m.catalog.as_deref()),
        ("PLA", Some("pla"))
    );
    assert!((mass(&h).unwrap() - 29.76).abs() < 1e-9, "{:?}", mass(&h));
    h.get_by_label("Mass 29.7600 g");
    // A custom density in kg/m³.
    click(&mut h, "Material");
    click(&mut h, "Custom density");
    type_into(&mut h, "Material name", "Resin");
    type_into(&mut h, "Density", "1200");
    click(&mut h, "kg/m³");
    click(&mut h, "Apply material");
    let m = h.state().summary().bodies[0]
        .material
        .clone()
        .expect("material");
    assert_eq!((m.name.as_str(), m.catalog), ("Resin", None));
    assert!((m.density - 1.2).abs() < 1e-12);
    assert!((mass(&h).unwrap() - 28.8).abs() < 1e-9, "{:?}", mass(&h));
    // Mass properties readout (world frame).
    click(&mut h, "Mass properties");
    h.get_by_label("Mass properties (world frame; mm, g, g·mm²)");
    // Volume and mass appear in the measurements above and in the readout.
    assert_eq!(h.get_all_by_label("Volume 24000.0000 mm³").count(), 2);
    assert_eq!(h.get_all_by_label("Mass 28.8000 g").count(), 2);
    h.get_by_label("Exact boundary integral (BrepBoundaryIntegralZ)");
    h.get_by_label("Centre of mass (10.0000, 15.0000, 20.0000) mm");
    // One undo step per material change.
    click(&mut h, "Undo");
    assert!((mass(&h).unwrap() - 29.76).abs() < 1e-9);
}
