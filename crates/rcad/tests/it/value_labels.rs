//! Value fields of the command dialogs name their unit the same way:
//! "(mm)" (or "(face mm)") for lengths and "(deg)" for angles, never "(°)"
//! in one dialog and "(deg)" in the next. Scans the `num_field` labels in
//! the app's sources.
use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Labels of `num_field(ui, "…", …)` calls (the label may be on the next
/// line after a formatter break).
fn num_field_labels(text: &str) -> Vec<String> {
    let mut out = vec![];
    for (i, _) in text.match_indices("num_field(") {
        let rest = &text[i + "num_field(".len()..];
        let Some(rest) = rest.trim_start().strip_prefix("ui,") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue; // a computed label
        };
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}

#[test]
fn dialog_value_fields_name_their_unit_consistently() {
    // Unitless values: counts, and density (its unit is a choice beside it).
    const UNITLESS: &[&str] = &["Pattern count", "Second direction count", "Density"];
    let mut files = vec![];
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut bad = vec![];
    let mut seen = 0;
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for l in num_field_labels(&text) {
            seen += 1;
            let ok = UNITLESS.contains(&l.as_str())
                || l.ends_with("(mm)")
                || l.ends_with("(face mm)")
                || l.ends_with("(deg)");
            if !ok {
                bad.push(format!("{}: {l:?}", f.display()));
            }
        }
    }
    assert!(seen > 40, "only {seen} labels found: the scan is broken");
    assert!(bad.is_empty(), "labels without a consistent unit: {bad:#?}");
}
