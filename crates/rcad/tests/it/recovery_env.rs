//! Autosave location and interval from the `RCAD_*` environment variables, through the
//! pure lookup functions (no process environment is modified).
use rcad::recovery::{autosave_interval, recovery_dir};
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
fn recovery_dir_prefers_the_variable_then_xdg_then_home() {
    let all = env(&[
        ("RCAD_RECOVERY_DIR", "/r"),
        ("XDG_STATE_HOME", "/s"),
        ("HOME", "/h"),
    ]);
    assert_eq!(recovery_dir(all), Some(PathBuf::from("/r")));
    let xdg = env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")]);
    assert_eq!(recovery_dir(xdg), Some(PathBuf::from("/s/r-cad/recovery")));
    let home = env(&[("HOME", "/h")]);
    assert_eq!(
        recovery_dir(home),
        Some(PathBuf::from("/h/.local/state/r-cad/recovery"))
    );
    assert_eq!(recovery_dir(env(&[])), None);
}

#[test]
fn autosave_interval_reads_the_variable_or_defaults_to_a_minute() {
    let s = Duration::from_secs;
    assert_eq!(autosave_interval(env(&[("RCAD_AUTOSAVE_SECS", "5")])), s(5));
    assert_eq!(autosave_interval(env(&[("RCAD_AUTOSAVE_SECS", "0")])), s(0));
    assert_eq!(autosave_interval(env(&[])), s(60));
    assert_eq!(
        autosave_interval(env(&[("RCAD_AUTOSAVE_SECS", "soon")])),
        s(60)
    );
}
