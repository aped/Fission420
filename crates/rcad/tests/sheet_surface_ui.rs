//! Run 7 sheet-metal and surface inspector through the real widgets
//! (egui_kittest; docs/SHEET_METAL.md, docs/SURFACES.md): base face from a
//! sketch, edge flange picked from the base lines, folded/flat toggle, rule
//! edit and a refused rule; surface body analysis, fill and thicken.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{BodyId, SketchPlane};
use rcad_kernel::VolumeResult;

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 1000.0])
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

fn feature_name(h: &Harness<'_, RcadApp>, kind: &str) -> String {
    h.state()
        .summary()
        .features
        .iter()
        .find(|f| f.kind == kind)
        .unwrap_or_else(|| panic!("no {kind}"))
        .name
        .clone()
}

fn volume(h: &Harness<'_, RcadApp>, b: u32) -> Option<f64> {
    let s = h.state().summary();
    let b = s.bodies.iter().find(|x| x.id == BodyId(b))?;
    match &b.measurements.as_ref()?.volume {
        VolumeResult::Value { volume, .. } => Some(*volume),
        _ => None,
    }
}

fn visible(h: &Harness<'_, RcadApp>, b: u32) -> bool {
    h.state()
        .summary()
        .bodies
        .iter()
        .find(|x| x.id == BodyId(b))
        .is_some_and(|x| x.visible)
}

#[test]
fn sheet_base_flange_flat_and_rule_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 60.0,
            height: 40.0,
            center: Some([30.0, 20.0]),
        },
    });
    settle(&mut h);
    let sk = feature_name(&h, "sketch");
    click(&mut h, &sk);
    click(&mut h, "Sheet metal base face");
    type_into(&mut h, "Thickness (mm)", "1");
    type_into(&mut h, "Inside bend radius (mm)", "2");
    type_into(&mut h, "K-factor", "0.4");
    click(&mut h, "Create sheet base");
    let base = feature_name(&h, "sheet_base");
    assert!((volume(&h, 1).unwrap() - 2400.0).abs() < 1e-6);
    // Select the base: rule editor, display toggle, flange form.
    click(&mut h, &base);
    click(&mut h, "Add edge flange");
    h.get_by_role_and_label(Role::ComboBox, "Flange edge")
        .click();
    settle(&mut h);
    let label = "(0.0, 40.0) → (0.0, 0.0)"; // the left edge
    h.query_all_by_label_contains(label).last().unwrap().click();
    settle(&mut h);
    type_into(&mut h, "Length beyond bend (mm)", "20");
    click(&mut h, "Add flange");
    assert_eq!(
        h.state().summary().features.last().unwrap().kind,
        "sheet_flange",
        "{:?}",
        h.state().last_reply()
    );
    let want = 40.0 * 80.0 + 40.0 * (PI / 4.0) * 5.0;
    assert!((volume(&h, 1).unwrap() - want).abs() < 1e-6);
    // Flat pattern display state and back.
    click(&mut h, &base);
    click(&mut h, "Flat pattern");
    assert!(!visible(&h, 1) && visible(&h, 2));
    let flat_len = 60.0 + (PI / 2.0) * 2.4 + 20.0;
    assert!((volume(&h, 2).unwrap() - flat_len * 40.0).abs() < 1e-6);
    click(&mut h, &base);
    click(&mut h, "Folded");
    assert!(visible(&h, 1) && !visible(&h, 2));
    // Rule edit: thickness 2 regenerates the flange.
    click(&mut h, &base);
    type_into(&mut h, "Thickness (mm)", "2");
    click(&mut h, "Apply rule");
    let want2 = 40.0 * 2.0 * 80.0 + 40.0 * (PI / 4.0) * (16.0 - 4.0);
    assert!((volume(&h, 1).unwrap() - want2).abs() < 1e-6);
    // A refused rule (K > 1) leaves everything as it was.
    let n = h.state().summary().revision;
    type_into(&mut h, "K-factor", "1.5");
    click(&mut h, "Apply rule");
    assert_eq!(h.state().summary().revision, n);
    assert_eq!(h.state().last_reply().map(|r| r.1), Some(false));
    assert!((volume(&h, 1).unwrap() - want2).abs() < 1e-6);
}

#[test]
fn surface_body_analyse_fill_and_thicken_from_the_inspector() {
    let fx = |n: &str| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step")
            .join(n)
    };
    let mut h = harness();
    click(&mut h, "New");
    // Through the Import dialog with the surfaces option.
    click(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fx("open_box_mm.step").display().to_string(),
    );
    click(
        &mut h,
        "Import surface models (open shells) as a surface body",
    );
    click(&mut h, "OK");
    let imp = feature_name(&h, "import_surfaces");
    assert_eq!(volume(&h, 1), None, "open shell: no volume");
    click(&mut h, &imp);
    assert!(
        h.query_by_label_contains("Open shell (surface body): no volume")
            .is_some()
    );
    click(&mut h, "Analyse");
    assert!(
        h.query_by_label_contains("4 free edge(s)").is_some(),
        "analysis shown"
    );
    click(&mut h, "Fill planar holes");
    assert!((volume(&h, 2).unwrap() - 24000.0).abs() < 1e-6);
    assert!(!visible(&h, 1), "source kept, hidden");
    // Thicken a planar surface.
    h.state_mut().request(Command::ImportSurfaces(
        rcad_commands::sheet_surface::ImportSurfacesArgs {
            path: fx("plate_mm.step"),
            units_to_mm: None,
        },
    ));
    settle(&mut h);
    let plate = h.state().summary().features.last().unwrap().name.clone();
    click(&mut h, &plate);
    type_into(&mut h, "Thickness (mm)", "3");
    click(&mut h, "Thicken");
    let b = h.state().summary().bodies.last().unwrap().id.0;
    assert!((volume(&h, b).unwrap() - 3000.0).abs() < 1e-6);
}
