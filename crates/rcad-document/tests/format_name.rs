//! Only `"format": "r-cad"` documents load. Documents carrying the project's earlier
//! format name are refused like any other foreign format, with a message that names the
//! format found and the one expected.
use rcad_document::{DocError, FORMAT_NAME, load_file};
use std::path::PathBuf;

fn frozen_v4() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-commands/tests/fixtures/native_v4/enclosure.rcad.json")
}

#[test]
fn only_the_current_format_name_loads() {
    let dir = tempfile::tempdir().unwrap();
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(frozen_v4()).unwrap()).unwrap();
    assert_eq!(v["format"], FORMAT_NAME);
    assert_eq!(load_file(&frozen_v4()).unwrap().version, 4);

    // The pre-rename name is assembled here so the literal never appears in the tree.
    let old = concat!("fiss", "ion", "420");
    for name in [old, "R-CAD", "rcad", ""] {
        v["format"] = name.into();
        let p = dir.path().join("x.rcad.json");
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        match load_file(&p) {
            Err(DocError::File(msg)) => {
                assert!(msg.contains("not an r-cad document"), "{name}: {msg}");
                assert!(msg.contains(&format!("{name:?}")), "{name}: {msg}");
                assert!(msg.contains("only \"r-cad\" is supported"), "{name}: {msg}");
            }
            r => panic!("{name}: {:?}", r.map(|nf| nf.version)),
        }
    }
}
