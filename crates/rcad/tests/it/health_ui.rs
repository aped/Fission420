//! Feature health in the UI: a document whose history no longer fully
//! regenerates opens with its failing features tinted and badged in History
//! (reason and repair hint on hover) under an "errors" banner; File ▸ Open
//! (safe mode) opens it rolled back before the first failure. Fixture: the
//! frozen six-feature document of the open-with-errors command tests (F3
//! sketches on a face that does not exist, F4 cuts with it).
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_document::HealthState;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-commands/tests/fixtures/open_with_errors.rcad.json")
}

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
    use eframe::egui::{Key, Modifiers};
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn name_of(h: &Harness<'_, RcadApp>, id: u32) -> String {
    h.state()
        .summary()
        .features
        .iter()
        .find(|f| f.id.0 == id)
        .unwrap()
        .name
        .clone()
}

#[test]
fn failed_features_are_badged_and_the_banner_offers_review() {
    let mut h = harness();
    click(&mut h, "Open…");
    type_into(&mut h, "File path", fixture().to_str().unwrap());
    click(&mut h, "OK");
    let s = h.state().summary();
    assert!(s.open);
    let errors: Vec<u32> = s
        .features
        .iter()
        .filter(|f| f.health.state == HealthState::Error)
        .map(|f| f.id.0)
        .collect();
    assert_eq!(errors, [3, 4]);
    // Badges on exactly the failing rows; healthy rows have none.
    let (f3, f4, f2) = (name_of(&h, 3), name_of(&h, 4), name_of(&h, 2));
    h.get_by_label(&format!("Health {f3}"));
    h.get_by_label(&format!("Health {f4}"));
    assert!(h.query_by_label(&format!("Health {f2}")).is_none());
    // Banner: two features, Review selects the first failure.
    h.get_by_label_contains("2 features with errors");
    click(&mut h, "Review first error");
    assert_eq!(h.state().selected().map(|f| f.0), Some(3));
    // Roll back before it: the marker moves, the document is healthy.
    click(&mut h, "Roll back before the first error");
    assert!(
        h.state()
            .summary()
            .features
            .iter()
            .all(|f| f.health.state != HealthState::Error),
        "{:?}",
        h.state().diagnostics().last()
    );
    assert!(h.query_by_label("Document has errors").is_none());
}

#[test]
fn open_safe_mode_from_the_file_menu() {
    let mut h = harness();
    click(&mut h, "File menu");
    click(&mut h, "Open (safe mode)…");
    h.get_by_label("Open document (safe mode)");
    type_into(&mut h, "File path", fixture().to_str().unwrap());
    click(&mut h, "OK");
    let s = h.state().summary();
    assert!(s.open, "{:?}", h.state().diagnostics().last());
    // Rolled back to just before F3: no feature is in error.
    assert!(
        s.features
            .iter()
            .all(|f| f.health.state != HealthState::Error)
    );
    assert!(s.features.iter().any(|f| f.rolled_back));
    assert!(h.query_by_label("Document has errors").is_none());
}
