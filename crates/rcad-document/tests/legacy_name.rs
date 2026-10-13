//! Files written before the rename to R-CAD (`"format": "fission420"`, `*.f420.json`) still
//! open; a re-save writes the new name; unrelated format names are still refused.
use rcad_document::{DocError, FORMAT_NAME, Model, load_file, save_file};
use std::path::PathBuf;

fn frozen_v4() -> PathBuf {
    // A frozen run-4 document, written by a pre-rename build (contents never rewritten).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-commands/tests/fixtures/native_v4/enclosure.rcad.json")
}

#[test]
fn pre_rename_file_with_old_extension_opens_and_resaves_under_the_new_name() {
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(frozen_v4()).unwrap()).unwrap();
    assert_eq!(raw["format"], "fission420");
    assert_eq!(raw["version"], 4);
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("enclosure.f420.json");
    std::fs::copy(frozen_v4(), &old).unwrap();

    let nf = load_file(&old).unwrap();
    assert_eq!(nf.version, 4);
    let m = Model::from_document(nf.document.clone()).unwrap();
    let new = dir.path().join("enclosure.rcad.json");
    save_file(&new, m.document(), Some(m.evaluation())).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&new).unwrap()).unwrap();
    assert_eq!(saved["format"], FORMAT_NAME);
    assert_eq!(saved["format"], "r-cad");
    // No newer-version feature in this document: still written as v4 (readable by
    // pre-rename builds once the name check is the only difference).
    assert_eq!(saved["version"], 4);
    assert_eq!(load_file(&new).unwrap().document, nf.document);
}

#[test]
fn other_format_names_are_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(frozen_v4()).unwrap()).unwrap();
    for name in ["R-CAD", "rcad", "fission", "Fission420", ""] {
        v["format"] = name.into();
        let p = dir.path().join("x.rcad.json");
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        match load_file(&p) {
            Err(DocError::File(msg)) => {
                assert!(msg.contains("not an r-cad document"), "{name}: {msg}")
            }
            r => panic!("{name}: {:?}", r.map(|nf| nf.version)),
        }
    }
}
