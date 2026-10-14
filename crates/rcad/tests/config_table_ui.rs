//! Configuration table editor and variant export through the real widgets
//! (egui_kittest; docs/CONFIGURATIONS.md).
use std::path::PathBuf;
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

fn out(name: &str) -> PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/config_ui");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn edit_the_configuration_table_and_export_variants() {
    let mut h = harness();
    click(&mut h, "New");
    request(
        &mut h,
        Command::SetUserParameter {
            name: "length".into(),
            expression: "100".into(),
            unit: None,
            comment: None,
        },
    );
    request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 100.0,
                height: 20.0,
                center: None,
            },
        },
    );
    request(
        &mut h,
        Command::SetExpression {
            feature: rcad_document::FeatureId(1),
            name: "width".into(),
            expression: "length".into(),
        },
    );
    request(
        &mut h,
        Command::extrude(None, rcad_document::FeatureId(1), 10.0),
    );
    request(
        &mut h,
        Command::SetConfiguration {
            name: "Short".into(),
            values: [("length".to_string(), "60".to_string())].into(),
            suppressed: vec![],
            part_number: "P-60".into(),
            excluded_occurrences: vec![],
        },
    );
    click(&mut h, "Configuration table");
    click(&mut h, "Add configuration column");
    type_into(&mut h, "Configuration 1 name", "Long");
    type_into(&mut h, "Configuration 1 length", "140");
    type_into(&mut h, "Configuration 1 part number", "P-140");
    type_into(&mut h, "Configuration 0 length", "70");
    click(&mut h, "Apply configuration table");
    let c = h.state().summary().configurations.clone();
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(c[0].values["length"], "70");
    assert_eq!(
        (c[1].name.as_str(), c[1].part_number.as_str()),
        ("Long", "P-140")
    );
    assert_eq!(c[1].values["length"], "140");
    // One undo step for the whole table.
    request(&mut h, Command::Undo);
    assert_eq!(h.state().summary().configurations.len(), 1);
    request(&mut h, Command::Redo);
    assert_eq!(h.state().summary().configurations.len(), 2);
    // An invalid table (duplicate names) is refused unchanged.
    type_into(&mut h, "Configuration 1 name", "Short");
    click(&mut h, "Apply configuration table");
    assert_eq!(h.state().summary().configurations[1].name, "Long");
    click(&mut h, "Revert configuration table");
    // Variant export through the form.
    let dir = out("variants");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    type_into(&mut h, "Variant export folder", dir.to_str().unwrap());
    type_into(&mut h, "Variant export stem", "bar");
    click(&mut h, "Variant STL");
    click(&mut h, "Export variants");
    for f in [
        "bar-Short-P-60.step",
        "bar-Short-P-60.stl",
        "bar-Long-P-140.step",
        "bar-Long-P-140.stl",
    ] {
        assert!(dir.join(f).is_file(), "{f}");
    }
}
