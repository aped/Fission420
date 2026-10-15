//! v1/v2 -> v3 migration against frozen copies of the v1/v2 example
//! documents (crates/rcad-commands/tests/fixtures/native_v2; the live examples/
//! are re-saved as v3 by `cargo xtask check`):
//! every file loads, gets exactly one body (the base extrusion/revolve, "Body1"),
//! keeps its feature ids, and regenerates to the volume recorded in its own
//! save-time cache (written by the build that saved it).
use rcad_document::*;
use rcad_kernel::VolumeResult;
use std::path::PathBuf;

fn examples() -> Vec<PathBuf> {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rcad-commands/tests/fixtures/native_v2");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".rcad.json"))
        .collect();
    v.sort();
    v
}

#[test]
fn committed_v2_examples_migrate_and_regenerate_identically() {
    let files = examples();
    assert!(files.len() >= 5, "{files:?}");
    for p in files {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            raw["version"],
            2,
            "{}: frozen fixture must stay v2",
            p.display()
        );
        let ids_before: Vec<u64> = raw["document"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_u64().unwrap())
            .collect();
        let nf = load_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(nf.version, 2);
        let doc = &nf.document;
        let ids: Vec<u64> = doc.features.iter().map(|f| f.id.0 as u64).collect();
        assert_eq!(ids, ids_before, "{}: feature ids changed", p.display());
        assert_eq!(doc.bodies.len(), 1, "{}", p.display());
        assert_eq!(doc.bodies[0].name, "Body1");
        let b = doc.bodies[0].id;
        for f in &doc.features {
            if let Some(t) = f.target_body() {
                assert_eq!(t, b, "{}: {} target", p.display(), f.id);
            }
        }
        let m = Model::from_document(nf.document.clone()).unwrap();
        let v = match m.evaluation().measurements.as_ref().unwrap().volume {
            VolumeResult::Value { volume, .. } => volume,
            ref o => panic!("{}: {o:?}", p.display()),
        };
        let cached = nf.cache.as_ref().and_then(|c| c.volume).unwrap();
        assert!(
            (v - cached).abs() <= 1e-7 * cached.abs().max(1.0),
            "{}: regenerated {v} vs saved {cached}",
            p.display()
        );
        // Saving writes v3 and reloads to the same document.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("v3.rcad.json");
        save_file(&out, m.document(), Some(m.evaluation())).unwrap();
        let nf3 = load_file(&out).unwrap();
        // Plain documents are written as v4 (v5 only for v5 features).
        assert_eq!(nf3.version, 4);
        assert_eq!(&nf3.document, m.document());
    }
}

#[test]
fn v3_document_with_dangling_body_target_is_refused_on_load() {
    let mut d = Document::new();
    let s = d.push(
        "S",
        FeatureKind::Sketch {
            plane: SketchPlane::WorldXY,
            sketch: rcad_sketch::Sketch::centered_rectangle(10.0, 10.0).unwrap(),
            links: Vec::new(),
        },
    );
    d.push_extrude("E", s, 1.0);
    d.push(
        "Cut",
        FeatureKind::extrude(s, 1.0, Operation::Cut { target: BodyId(7) }),
    );
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bad.rcad.json");
    std::fs::write(&p, serde_json::to_vec(&NativeFile::new(&d, None)).unwrap()).unwrap();
    match load_file(&p) {
        Err(DocError::BrokenReference { detail, .. }) => assert!(detail.contains("B7"), "{detail}"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn embedded_blob_corruption_is_detected() {
    let mut d = Document::new();
    let id = d.add_blob("x.stl", b"solid x\nendsolid x\n");
    assert_eq!(
        d.blob(id).unwrap().bytes().unwrap(),
        b"solid x\nendsolid x\n"
    );
    d.blobs[0].data.push('!');
    assert!(matches!(
        d.blob(id).unwrap().bytes(),
        Err(DocError::File(_))
    ));
    let bin = [0u8, 159, 146, 150, 255];
    let id2 = d.add_blob("b.bin", &bin);
    assert_eq!(d.blob(id2).unwrap().encoding, BlobEncoding::Base64);
    assert_eq!(d.blob(id2).unwrap().bytes().unwrap(), bin);
}
