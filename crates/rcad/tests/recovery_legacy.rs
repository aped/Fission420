//! Pre-rename (Fission420) autosave compatibility: old environment variables are still
//! honoured when the new ones are unset, the old recovery directory is still searched,
//! and copies with the old suffix are still offered. Uses the pure lookup functions, so
//! no process environment is modified.
use rcad::recovery::{autosave_interval, find_copies, recovery_dirs};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let v: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k| v.iter().find(|(n, _)| n == k).map(|(_, x)| x.into())
}

#[test]
fn recovery_dir_prefers_new_variable_then_old_then_xdg_home() {
    let both = env(&[("RCAD_RECOVERY_DIR", "/new"), ("F420_RECOVERY_DIR", "/old")]);
    assert_eq!(recovery_dirs(both), (Some(PathBuf::from("/new")), None));
    let old = env(&[("F420_RECOVERY_DIR", "/old"), ("HOME", "/h")]);
    assert_eq!(recovery_dirs(old), (Some(PathBuf::from("/old")), None));
    let xdg = env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")]);
    assert_eq!(
        recovery_dirs(xdg),
        (
            Some(PathBuf::from("/s/r-cad/recovery")),
            Some(PathBuf::from("/s/fission420/recovery"))
        )
    );
    let home = env(&[("HOME", "/h")]);
    assert_eq!(
        recovery_dirs(home),
        (
            Some(PathBuf::from("/h/.local/state/r-cad/recovery")),
            Some(PathBuf::from("/h/.local/state/fission420/recovery"))
        )
    );
    assert_eq!(recovery_dirs(env(&[])), (None, None));
}

#[test]
fn autosave_interval_prefers_new_variable_then_old() {
    let s = Duration::from_secs;
    assert_eq!(
        autosave_interval(env(&[
            ("RCAD_AUTOSAVE_SECS", "5"),
            ("F420_AUTOSAVE_SECS", "7")
        ])),
        s(5)
    );
    assert_eq!(autosave_interval(env(&[("F420_AUTOSAVE_SECS", "7")])), s(7));
    assert_eq!(autosave_interval(env(&[])), s(60));
    assert_eq!(
        autosave_interval(env(&[("RCAD_AUTOSAVE_SECS", "soon")])),
        s(60)
    );
}

#[test]
fn copies_with_the_old_suffix_are_found() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "a.autosave.rcad.json",
        "b.autosave.f420.json",
        "c.rcad.json",
        "d.f420.json",
    ] {
        std::fs::write(dir.path().join(name), "{}").unwrap();
    }
    let mut found: Vec<String> = find_copies(dir.path())
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    found.sort();
    assert_eq!(found, ["a.autosave.rcad.json", "b.autosave.f420.json"]);
}
