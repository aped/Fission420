//! Deferred measurement of imported bodies through the threaded engine: every
//! B-rep body ends up measured (no "No measurements available" for a solid).
use rcad::{ExecMode, engine::Engine};
use rcad_commands::Command;
use std::time::{Duration, Instant};

fn fixture(f: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-kernel/tests/fixtures/step")
        .join(f)
}

#[test]
fn threaded_engine_measures_every_imported_solid() {
    let mut e = Engine::new(ExecMode::Threaded, None);
    e.submit(Command::NewDocument);
    e.submit(Command::ImportStep {
        path: fixture("two_boxes_compound_mm.step"),
        units_to_mm: None,
    });
    e.submit(Command::ImportStep {
        path: fixture("plate_hole_mm.step"),
        units_to_mm: None,
    });
    let t0 = Instant::now();
    let mut last = None;
    let mut log = Vec::new();
    while t0.elapsed() < Duration::from_secs(30) {
        if let Some(r) = e.poll() {
            log.push(format!(
                "{} ok={} followup={} measuring={} err={:?}",
                r.response.command,
                r.response.ok,
                r.followup,
                r.summary.measuring,
                r.response.error
            ));
            last = Some(r.summary);
        } else if !e.busy()
            && last
                .as_ref()
                .is_some_and(|s| !s.measuring && s.bodies.len() == 3)
        {
            break;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let s = last.expect("replies");
    for b in &s.bodies {
        assert!(
            b.measurements.is_some(),
            "{} unmeasured; log: {log:#?}",
            b.name
        );
    }
}
