//! Cancellable imports through the command API (`rcad_cancel`, the flag the
//! UI's Cancel sets): with cancellation requested every import command fails
//! with `DocError::ImportCancelled` and the document (and revision) is
//! unchanged; cleared, the same imports succeed. A large generated STL
//! (2 000 000 triangles) is cancelled from another thread while it is read.
//! Own test binary: the flag is process-wide.
use rcad_commands::{Command, Session};
use rcad_document::DocError;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn corpus(f: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/fixtures/corpus")
        .join(f)
}

fn exec(s: &mut Session, v: &Value) -> rcad_commands::Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap();
    s.execute(&c)
}

fn with_body() -> Session {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 10.0, "height": 10.0}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
    ] {
        assert!(exec(&mut s, &v).ok);
    }
    s
}

/// Binary STL of `n` separate small triangles (a strip), written fast.
fn big_stl(path: &std::path::Path, n: u32) {
    let mut b = Vec::with_capacity(84 + 50 * n as usize);
    b.extend_from_slice(&[b' '; 80]);
    b.extend_from_slice(&n.to_le_bytes());
    for i in 0..n {
        let x = (i % 1000) as f32;
        let y = (i / 1000) as f32;
        for v in [
            [0.0f32, 0.0, 1.0],
            [x, y, 0.0],
            [x + 1.0, y, 0.0],
            [x, y + 1.0, 0.0],
        ] {
            for c in v {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 0]);
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn imports_are_cancellable_and_commit_nothing() {
    let imports = [
        json!({"cmd": "import_step", "path": corpus("p8_fillet_block_mm.step")}),
        json!({"cmd": "import_step_assembly", "path": corpus("a5_nested_transforms_mm.step")}),
        json!({"cmd": "import_stl", "path": corpus("m1_cylinder_binary.stl"), "units_to_mm": 1.0}),
        json!({"cmd": "import_3mf", "path": corpus("h6_three_items_cm.3mf")}),
        json!({"cmd": "import_dxf", "path": corpus("h7_counters_mm.dxf"), "plane": "WorldXY"}),
        json!({"cmd": "import_svg", "path": corpus("h8_counters_mm.svg"), "plane": "WorldXY"}),
    ];
    for v in &imports {
        let mut s = with_body();
        let before = s.model().unwrap().document().clone();
        let rev = s.summary().revision;
        rcad_document::request_cancel();
        let r = exec(&mut s, v);
        rcad_document::clear_cancel();
        assert!(
            !r.ok && matches!(r.error, Some(DocError::ImportCancelled(_))),
            "{v}: {:?}",
            r.error
        );
        assert_eq!(s.summary().revision, rev, "{v}");
        assert_eq!(s.model().unwrap().document(), &before, "{v}");
        let r = exec(&mut s, v);
        assert!(r.ok, "{v} after clearing: {:?}", r.error);
    }

    // A long read cancelled from another thread (the UI thread's role).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.stl");
    big_stl(&path, 2_000_000);
    let cmd = json!({"cmd": "import_stl", "path": path, "units_to_mm": 1.0});
    // Reference: the full import time.
    let mut full = with_body();
    let t = Instant::now();
    assert!(exec(&mut full, &cmd).ok);
    let full_time = t.elapsed();
    let mut s = with_body();
    let before = s.model().unwrap().document().clone();
    let delay = (full_time / 10).max(Duration::from_millis(5));
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(delay);
        rcad_document::request_cancel();
    });
    let t = Instant::now();
    let r = exec(&mut s, &cmd);
    let took = t.elapsed();
    canceller.join().unwrap();
    rcad_document::clear_cancel();
    assert!(
        !r.ok && matches!(r.error, Some(DocError::ImportCancelled(_))),
        "{:?} (full import {full_time:?}, cancel after {delay:?})",
        r.error
    );
    assert_eq!(s.model().unwrap().document(), &before);
    assert!(
        took < full_time,
        "cancelled import took {took:?}, the full import {full_time:?}"
    );
}
