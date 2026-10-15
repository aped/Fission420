//! Persistence failure cases, all in
//! task-owned temporary directories: a failed save keeps the previous file
//! byte-for-byte and leaves no temporary file; the document stays dirty and
//! editable; truncated, corrupt, mislabelled and future-version files are refused
//! with a structured error and do not replace the open document; stale
//! temporary files from an interrupted save do not affect open or save;
//! exports into an unwritable directory write nothing.
//!
//! Permission cases need a non-root user (root ignores directory permissions);
//! they are skipped with a message when the probe write succeeds anyway. They use
//! Unix mode bits, so they run on Unix only (Windows has no read-only directories).
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::SketchPlane;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn block(s: &mut Session, w: f64) {
    ok(s, Command::NewDocument);
    let sk = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: 10.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    ok(s, Command::extrude(None, sk, 5.0));
}

fn entries(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

/// Whether directory permissions are enforced for this user.
#[cfg(unix)]
fn perms_enforced(d: &Path) -> bool {
    let probe = d.join("probe");
    let w = std::fs::write(&probe, b"x").is_err();
    let _ = std::fs::remove_file(&probe);
    w
}

#[cfg(unix)]
#[test]
fn failed_save_keeps_the_previous_file_and_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join("part.rcad.json");
    let mut s = Session::new();
    block(&mut s, 20.0);
    ok(&mut s, Command::Save { path: path.clone() });
    let before = std::fs::read(&path).unwrap();
    // Edit, then make the directory read-only: the rename cannot happen.
    let f = s.summary().features[0].id;
    ok(
        &mut s,
        Command::SetParameter {
            feature: f,
            name: "width".into(),
            value: 30.0,
        },
    );
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    if !perms_enforced(&sub) {
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("SKIP: directory permissions are not enforced for this user (root?)");
        return;
    }
    let r = s.execute(&Command::Save { path: path.clone() });
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!r.ok, "save into a read-only directory succeeded");
    assert!(
        matches!(r.error, Some(rcad_document::DocError::File(_))),
        "{:?}",
        r.error
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "previous save changed"
    );
    assert_eq!(entries(&sub), vec!["part.rcad.json"], "temporary file left");
    // The document is intact, still dirty, and saves once writable again.
    assert!(s.model().unwrap().dirty);
    ok(&mut s, Command::Save { path: path.clone() });
    assert_ne!(std::fs::read(&path).unwrap(), before);
    assert!(!s.model().unwrap().dirty);
}

#[test]
fn save_to_a_missing_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    block(&mut s, 20.0);
    let r = s.execute(&Command::Save {
        path: dir.path().join("no/such/dir/part.rcad.json"),
    });
    assert!(!r.ok);
    assert!(s.model().unwrap().dirty);
    assert!(entries(dir.path()).is_empty());
}

#[test]
fn damaged_files_are_refused_without_replacing_the_open_document() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.rcad.json");
    let mut s = Session::new();
    block(&mut s, 20.0);
    ok(&mut s, Command::Save { path: good.clone() });
    let text = std::fs::read_to_string(&good).unwrap();
    let mut cases: Vec<(&str, String)> = vec![
        ("truncated", text[..text.len() / 2].to_string()),
        ("empty", String::new()),
        ("not json", "this is not a document".into()),
        (
            "wrong type",
            text.replacen("\"features\": [", "\"features\": 7, \"x\": [", 1),
        ),
        (
            "future version",
            text.replacen(
                &format!("\"version\": {}", version(&text)),
                "\"version\": 999",
                1,
            ),
        ),
    ];
    // A v7-only feature (a drawing note) in a file labelled v6.
    let mut t = Session::new();
    block(&mut t, 20.0);
    ok(
        &mut t,
        serde_json::from_value(serde_json::json!({"cmd": "create_drawing", "bodies": [1]}))
            .unwrap(),
    );
    ok(
        &mut t,
        serde_json::from_value(serde_json::json!({"cmd": "add_drawing_note", "drawing": 1,
            "text": "N", "position": [20.0, 20.0]}))
        .unwrap(),
    );
    let v7 = dir.path().join("v7.rcad.json");
    ok(&mut t, Command::Save { path: v7.clone() });
    let v7text = std::fs::read_to_string(&v7).unwrap();
    assert_eq!(version(&v7text), 7);
    cases.push((
        "v7 content labelled v6",
        v7text.replacen("\"version\": 7", "\"version\": 6", 1),
    ));
    // The open document: 30 wide, unsaved edit.
    let f = s.summary().features[0].id;
    ok(
        &mut s,
        Command::SetParameter {
            feature: f,
            name: "width".into(),
            value: 30.0,
        },
    );
    let doc = s.model().unwrap().document().clone();
    for (what, body) in cases {
        let p = dir
            .path()
            .join(format!("{}.rcad.json", what.replace(' ', "_")));
        std::fs::write(&p, body).unwrap();
        let r = s.execute(&Command::Open {
            path: p,
            safe_mode: false,
        });
        assert!(!r.ok, "{what}: opened");
        assert!(r.error.is_some(), "{what}");
        assert_eq!(
            s.model().unwrap().document(),
            &doc,
            "{what}: the open document changed"
        );
    }
    assert!(s.model().unwrap().dirty);
}

fn version(text: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(text).unwrap()["version"]
        .as_u64()
        .unwrap()
}

#[test]
fn stale_temporary_files_do_not_matter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("part.rcad.json");
    let mut s = Session::new();
    block(&mut s, 20.0);
    ok(&mut s, Command::Save { path: path.clone() });
    // An interrupted earlier save left a partial temporary file next to it.
    let stale = dir.path().join(".part.rcad.json.tmp-1-2-3");
    std::fs::write(&stale, b"{\"format\": \"r-cad\", \"vers").unwrap();
    let mut t = Session::new();
    ok(
        &mut t,
        Command::Open {
            path: path.clone(),
            safe_mode: false,
        },
    );
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    ok(&mut t, Command::Save { path: path.clone() });
    // The stale file is not ours to delete; the save still works.
    assert!(stale.exists());
    ok(
        &mut t,
        Command::Open {
            path,
            safe_mode: false,
        },
    );
}

#[cfg(unix)]
#[test]
fn export_into_an_unwritable_directory_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).unwrap();
    let mut s = Session::new();
    block(&mut s, 20.0);
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    if !perms_enforced(&sub) {
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("SKIP: directory permissions are not enforced for this user (root?)");
        return;
    }
    let mut results = Vec::new();
    for c in [
        Command::ExportStep {
            path: sub.join("p.step"),
            bodies: None,
        },
        serde_json::from_value(serde_json::json!({"cmd": "export_stl",
            "path": sub.join("p.stl")}))
        .unwrap(),
    ] {
        results.push(s.execute(&c).ok);
    }
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(results, vec![false, false]);
    assert!(entries(&sub).is_empty(), "{:?}", entries(&sub));
}
