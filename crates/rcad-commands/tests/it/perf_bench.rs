//! End-to-end timings of paths a user waits on (ignored; run explicitly):
//! `cargo test -p rcad-commands --release --test it perf_bench:: -- --ignored --nocapture`
//!
//! * open: every bundled example (`examples/*.rcad.json`) opened in a new session,
//!   then its summary (measurements) and the display tessellation of every body at
//!   the viewport's chord, slowest first;
//! * edit: a user-parameter edit on the enclosure example, regenerated, measured and
//!   re-tessellated, repeated.
//!
//! Prints one line per case; asserts only that the documents open and edit.
use rcad_commands::{Command, Session};
use rcad_document::BodyGeometry;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The viewport's display chord (`rcad::engine::DISPLAY_CHORD`).
const DISPLAY_CHORD: f64 = 0.05;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Seconds to (summary with measurements, display tessellation of every B-rep body).
fn measure_and_tessellate(s: &Session) -> (f64, f64) {
    let t = Instant::now();
    let summary = s.summary();
    assert!(summary.open);
    let measured = t.elapsed().as_secs_f64();
    let t = Instant::now();
    if let Some(m) = s.model() {
        for b in &m.evaluation().bodies {
            if let BodyGeometry::Brep(body) = &b.geometry {
                rcad_kernel::tessellate_display(body, DISPLAY_CHORD).unwrap();
            }
        }
    }
    (measured, t.elapsed().as_secs_f64())
}

#[test]
#[ignore = "timing report; run explicitly"]
fn open_every_example() {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".rcad.json"))
        .collect();
    paths.sort();
    let mut rows = Vec::new();
    for p in &paths {
        let mut s = Session::new();
        let t = Instant::now();
        let r = s.execute(&Command::Open {
            path: p.clone(),
            safe_mode: false,
        });
        let open = t.elapsed().as_secs_f64();
        assert!(r.ok, "{}: {:?}", p.display(), r.error);
        let (measure, mesh) = measure_and_tessellate(&s);
        rows.push((
            open + measure + mesh,
            open,
            measure,
            mesh,
            p.file_name().unwrap().to_string_lossy().to_string(),
        ));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total: f64 = rows.iter().map(|r| r.0).sum();
    for (all, open, measure, mesh, name) in &rows {
        eprintln!(
            "BENCH open {name:40} total {:8.1} ms  open {:8.1}  measure {:8.1}  display mesh {:8.1}",
            all * 1e3,
            open * 1e3,
            measure * 1e3,
            mesh * 1e3
        );
    }
    eprintln!(
        "BENCH open all {} examples: {:.1} ms",
        rows.len(),
        total * 1e3
    );
}

#[test]
#[ignore = "timing report; run explicitly"]
fn edit_enclosure_parameter() {
    edit_parameter(
        "enclosure.rcad.json",
        "outer_length",
        [84.0, 88.0, 80.0, 86.0],
    );
}

#[test]
#[ignore = "timing report; run explicitly"]
fn edit_rounded_enclosure_parameter() {
    edit_parameter(
        "rounded_enclosure.rcad.json",
        "length",
        [124.0, 130.0, 120.0, 126.0],
    );
}

/// Open `file`, then set user parameter `name` to each of `values` (mm) in turn.
fn edit_parameter(file: &str, name: &str, values: [f64; 4]) {
    let mut s = Session::new();
    let r = s.execute(&Command::Open {
        path: examples_dir().join(file),
        safe_mode: false,
    });
    assert!(r.ok, "{:?}", r.error);
    measure_and_tessellate(&s);
    for (i, len) in values.into_iter().enumerate() {
        let t = Instant::now();
        let r = s.execute(&Command::SetUserParameter {
            name: name.into(),
            expression: format!("{len} mm"),
            unit: None,
            comment: None,
        });
        let regen = t.elapsed().as_secs_f64();
        assert!(r.ok, "{:?}", r.error);
        let (measure, mesh) = measure_and_tessellate(&s);
        eprintln!(
            "BENCH edit {file} {i} {name}={len}: regenerate {:.1} ms  measure {:.1}  display mesh {:.1}  total {:.1}",
            regen * 1e3,
            measure * 1e3,
            mesh * 1e3,
            (regen + measure + mesh) * 1e3
        );
    }
}
