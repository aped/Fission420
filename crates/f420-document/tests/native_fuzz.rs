//! Native file robustness: damaged, truncated or rule-breaking `.f420.json` files are
//! refused with `DocError`, never a panic, and a file that does load also regenerates
//! without panicking (`Model::from_document` may refuse it). Inputs are the frozen
//! fixtures (`f420-commands/tests/fixtures/native_v3`, `native_v4`), so the cases are stable.
use f420_document::{DocError, Model, load_file};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

fn fixture(rel: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../f420-commands/tests/fixtures")
        .join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `load_file`, then regenerate when it loads. `Err` (string) on a panic.
fn load_and_regen(path: &Path) -> Result<Result<(), DocError>, String> {
    catch_unwind(AssertUnwindSafe(|| {
        let nf = load_file(path)?;
        Model::from_document(nf.document).map(|_| ())
    }))
    .map_err(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    })
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// xorshift64: deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn truncated_native_files_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    for rel in ["native_v3/plate.f420.json", "native_v4/enclosure.f420.json"] {
        let bytes = fixture(rel);
        // Trailing whitespace is not part of the JSON value: strip it so every strict
        // prefix really cuts the document.
        let end = bytes.trim_ascii_end().len();
        let step = (end / 400).max(1);
        for n in (0..end).step_by(step) {
            let p = write(dir.path(), "prefix.f420.json", &bytes[..n]);
            match load_and_regen(&p) {
                Ok(Err(DocError::File(_))) => {}
                other => panic!("{rel}: prefix {n}/{end}: {other:?}"),
            }
        }
    }
}

#[test]
fn mutated_native_files_never_panic() {
    const SUBST: &[u8] = b"0123456789-.eE\"{}[],: x\n";
    let dir = tempfile::tempdir().unwrap();
    let mut panics = Vec::new();
    let (mut loaded, mut regenerated) = (0, 0);
    for (rel, seed) in [
        ("native_v3/plate.f420.json", 0x9e37_79b9_7f4a_7c15u64),
        ("native_v4/enclosure.f420.json", 0x2545_f491_4f6c_dd1d),
    ] {
        let bytes = fixture(rel);
        let mut rng = Rng(seed);
        for i in 0..300 {
            let mut m = bytes.clone();
            let at = (rng.next() % m.len() as u64) as usize;
            m[at] = if i % 4 == 0 {
                rng.next() as u8
            } else {
                SUBST[(rng.next() % SUBST.len() as u64) as usize]
            };
            let p = write(dir.path(), "mutant.f420.json", &m);
            match load_and_regen(&p) {
                Ok(Ok(())) => {
                    loaded += 1;
                    regenerated += 1;
                }
                Ok(Err(DocError::File(_))) => {}
                Ok(Err(_)) => loaded += 1,
                Err(msg) => panics.push(format!("{rel} byte {at} = {:#04x}: {msg}", m[at])),
            }
        }
    }
    assert!(panics.is_empty(), "panics:\n{}", panics.join("\n"));
    // Guard against a vacuous sweep: some mutations (inside numbers, names,
    // whitespace) must survive parsing and reach regeneration.
    assert!(
        loaded > 20 && regenerated > 10,
        "{loaded} loaded, {regenerated} regenerated"
    );
}

/// docs/FORMAT_SUPPORT.md: parameters and expressions are v4 features; an older version
/// number on a file that uses them is refused instead of silently migrated.
#[test]
fn legacy_version_with_v4_features_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let v3: serde_json::Value =
        serde_json::from_slice(&fixture("native_v3/plate.f420.json")).unwrap();
    let v4: serde_json::Value =
        serde_json::from_slice(&fixture("native_v4/enclosure.f420.json")).unwrap();
    // The v3 control loads.
    let p = write(
        dir.path(),
        "v3.f420.json",
        &serde_json::to_vec(&v3).unwrap(),
    );
    assert!(load_file(&p).is_ok());

    let mut with_params = v3.clone();
    with_params["document"]["parameters"] = v4["document"]["parameters"].clone();
    assert!(
        !with_params["document"]["parameters"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut with_expr = v3.clone();
    with_expr["document"]["features"][0]["expressions"] = serde_json::json!({ "width": "100" });
    // A v4 document relabelled as v3 carries both.
    let mut relabelled = v4.clone();
    relabelled["version"] = 3.into();
    for (name, doc) in [
        ("params", with_params),
        ("expr", with_expr),
        ("relabelled", relabelled),
    ] {
        let p = write(dir.path(), name, &serde_json::to_vec(&doc).unwrap());
        match load_file(&p) {
            Err(DocError::File(msg)) => assert!(msg.contains("v4 feature"), "{name}: {msg}"),
            other => panic!("{name}: {:?}", other.map(|nf| nf.version)),
        }
    }
}

#[test]
fn malformed_version_headers_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let v4: serde_json::Value =
        serde_json::from_slice(&fixture("native_v4/enclosure.f420.json")).unwrap();
    let mut cases = Vec::new();
    for v in [
        serde_json::json!("4"),
        serde_json::json!(-1),
        serde_json::json!(4.5),
        serde_json::json!(4_294_967_296u64),
        serde_json::json!(0),
        serde_json::Value::Null,
    ] {
        let mut d = v4.clone();
        d["version"] = v.clone();
        cases.push((format!("version {v}"), d));
    }
    let mut no_version = v4.clone();
    no_version.as_object_mut().unwrap().remove("version");
    cases.push(("no version".into(), no_version));
    let mut no_document = v4.clone();
    no_document.as_object_mut().unwrap().remove("document");
    cases.push(("no document".into(), no_document));
    for (name, doc) in cases {
        let p = write(
            dir.path(),
            "h.f420.json",
            &serde_json::to_vec(&doc).unwrap(),
        );
        assert!(
            matches!(load_file(&p), Err(DocError::File(_))),
            "{name}: {:?}",
            load_file(&p).map(|nf| nf.version)
        );
    }
}
