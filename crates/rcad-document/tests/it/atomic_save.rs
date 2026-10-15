//! Atomic save over a read-only file, on every platform: the previous file is never
//! damaged and no temporary file is left behind. Unix replaces the file (its directory
//! is writable) and keeps its mode; on Windows the read-only attribute is never copied
//! onto the new file, so a later save is not blocked by an earlier one.
use rcad_document::{DocError, write_atomic};
use std::path::Path;

fn entries(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn set_readonly(path: &Path, ro: bool) {
    let mut p = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(ro);
    std::fs::set_permissions(path, p).unwrap();
}

#[test]
fn saving_over_a_read_only_file_keeps_the_previous_file_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("part.rcad.json");
    write_atomic(&path, b"first").unwrap();
    set_readonly(&path, true);
    let r = write_atomic(&path, b"second");
    if cfg!(windows) {
        // Whether Windows replaces a read-only file depends on the rename path std
        // takes; either way the file is whole: the old bytes with an error, or the new
        // bytes without the read-only attribute (it is not copied on Windows).
        match r {
            Err(DocError::File(_)) => assert_eq!(std::fs::read(&path).unwrap(), b"first"),
            Ok(()) => {
                assert_eq!(std::fs::read(&path).unwrap(), b"second");
                assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
            }
            other => panic!("{other:?}"),
        }
    } else {
        r.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    }
    assert_eq!(
        entries(dir.path()),
        vec!["part.rcad.json"],
        "temporary file left"
    );
    set_readonly(&path, false);
    write_atomic(&path, b"third").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"third");
    assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
    assert_eq!(entries(dir.path()), vec!["part.rcad.json"]);
}
