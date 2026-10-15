//! Controlled external components: another native
//! document inserted as a component from an embedded STEP copy of its solids,
//! with a pinned content identity. The model changes only through an explicit
//! update; a modified source is reported, a missing source is diagnosed and the
//! embedded copy is kept; embedded-only components cannot be updated; a
//! structural change of the source is refused; undo/redo and save/reopen
//! without the source file. Volumes are closed-form.
use crate::support;
use rcad_commands::assembly::AssemblyOut;
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn out(r: Response) -> AssemblyOut {
    r.assembly.expect("assembly output")
}

fn refused(s: &mut Session, v: Value) -> String {
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(s, &v);
    assert!(!r.ok, "{v} should be refused");
    assert_eq!(
        s.model().unwrap().document(),
        &before,
        "{v} changed the document"
    );
    format!("{:?}", r.error)
}

/// A source document: a 10 × 10 × `h` box (plus a second 2 × 2 × 2 box when
/// `two`), saved to `path`.
fn source(path: &std::path::Path, h: f64, two: bool) {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 10.0, "height": 10.0, "center": [0.0, 0.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": h}),
    );
    if two {
        ok(
            &mut s,
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 2.0, "height": 2.0, "center": [20.0, 0.0]}),
        );
        ok(
            &mut s,
            json!({"cmd": "extrude", "sketch": 3, "distance": 2.0}),
        );
    }
    ok(&mut s, json!({"cmd": "save", "path": path}));
}

fn volumes(s: &mut Session) -> Vec<f64> {
    ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap()
        .occurrences
        .iter()
        .map(|o| o.volume.unwrap())
        .collect()
}

fn status(s: &mut Session) -> Vec<String> {
    out(ok(s, json!({"cmd": "check_external_components"})))
        .external
        .unwrap()
        .into_iter()
        .map(|x| x.status)
        .collect()
}

#[test]
fn linked_external_component_updates_only_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("cube.rcad.json");
    source(&src, 10.0, false);
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let o = out(ok(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src, "linked": true}),
    ));
    let c = o.component.unwrap();
    let comp = s.summary().assembly.components[0].clone();
    assert_eq!(comp.name, "cube");
    assert_eq!(comp.id, c);
    ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [30.0, 0.0, 0.0]}),
    );
    let v = volumes(&mut s);
    assert_eq!(v.len(), 2);
    for v in v {
        assert!((v - 1000.0).abs() < 1e-6, "{v}");
    }
    assert_eq!(status(&mut s), vec!["up_to_date"]);
    // Unchanged source: update is a no-op (no history entry).
    let before = s.model().unwrap().document().clone();
    let r = ok(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    assert!(r.message.unwrap().contains("up to date"));
    assert_eq!(s.model().unwrap().document(), &before);
    // The source changes (height 10 → 20): reported, the model keeps the pin.
    source(&src, 20.0, false);
    assert_eq!(status(&mut s), vec!["modified"]);
    for v in volumes(&mut s) {
        assert!((v - 1000.0).abs() < 1e-6);
    }
    // Explicit update: both linked occurrences follow (2000 mm³).
    ok(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    for v in volumes(&mut s) {
        assert!((v - 2000.0).abs() < 1e-6, "{v}");
    }
    assert_eq!(status(&mut s), vec!["up_to_date"]);
    ok(&mut s, json!({"cmd": "undo"}));
    for v in volumes(&mut s) {
        assert!((v - 1000.0).abs() < 1e-6);
    }
    ok(&mut s, json!({"cmd": "redo"}));
    // A structural change (a second solid) is refused, document unchanged.
    source(&src, 20.0, true);
    let e = refused(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    assert!(e.contains("2 solids instead of 1"), "{e}");
    // Missing source: diagnosed, refused, the embedded copy stays.
    std::fs::remove_file(&src).unwrap();
    assert_eq!(status(&mut s), vec!["missing"]);
    let e = refused(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    assert!(e.contains("missing"), "{e}");
    for v in volumes(&mut s) {
        assert!((v - 2000.0).abs() < 1e-6);
    }
    // Save / reopen without the source: the embedded copy regenerates.
    let doc = dir.path().join("asm.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": doc}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&doc).unwrap()).unwrap();
    assert_eq!(v["version"], 7);
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": doc}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    for v in volumes(&mut t) {
        assert!((v - 2000.0).abs() < 1e-6);
    }
}

#[test]
fn embedded_external_component_and_bad_sources() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("pair.rcad.json");
    source(&src, 5.0, true);
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let c = out(ok(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src, "name": "Pair"}),
    ))
    .component
    .unwrap();
    // Two solids: 500 + 8.
    let v = volumes(&mut s);
    assert_eq!(v.len(), 1);
    assert!((v[0] - 508.0).abs() < 1e-6, "{v:?}");
    assert_eq!(status(&mut s), vec!["embedded"]);
    let e = refused(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    assert!(e.contains("embedded copy"), "{e}");
    // Duplicate name, missing file, not a native document: refused.
    refused(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src, "name": "Pair"}),
    );
    refused(
        &mut s,
        json!({"cmd": "insert_external_component", "path": dir.path().join("nope.rcad.json")}),
    );
    let junk = dir.path().join("junk.rcad.json");
    std::fs::write(&junk, "{\"hello\": 1}").unwrap();
    refused(
        &mut s,
        json!({"cmd": "insert_external_component", "path": junk}),
    );
}

/// An update whose solids changed names/order is refused
/// (bodies would silently swap geometry).
#[test]
fn reordered_source_solids_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("pair.rcad.json");
    source(&src, 5.0, true);
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let c = out(ok(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src, "linked": true}),
    ))
    .component
    .unwrap();
    // Swap the two body names in the source.
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": src}));
    let b = t.summary().bodies.clone();
    ok(
        &mut t,
        json!({"cmd": "rename_body", "body": b[0].id, "name": "tmp"}),
    );
    ok(
        &mut t,
        json!({"cmd": "rename_body", "body": b[1].id, "name": b[0].name}),
    );
    ok(
        &mut t,
        json!({"cmd": "rename_body", "body": b[0].id, "name": b[1].name}),
    );
    ok(&mut t, json!({"cmd": "save", "path": src}));
    let e = refused(
        &mut s,
        json!({"cmd": "update_external_component", "component": c}),
    );
    assert!(e.contains("changed names or order"), "{e}");
    // A FIFO / directory as the source is diagnosed, not read forever.
    let e = refused(
        &mut s,
        json!({"cmd": "insert_external_component", "path": dir.path()}),
    );
    assert!(e.contains("not a regular file"), "{e}");
}
