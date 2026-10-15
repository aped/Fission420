//! Autosave location and interval from the `RCAD_*` environment variables, through the
//! pure lookup functions (no process environment is modified).
use rcad::recovery::{Platform, autosave_interval, recovery_dir, recovery_dir_on};
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
    let unix = |pairs: &[(&str, &str)]| recovery_dir_on(Platform::Unix, env(pairs));
    let all = [
        ("RCAD_RECOVERY_DIR", "/r"),
        ("XDG_STATE_HOME", "/s"),
        ("HOME", "/h"),
        ("LOCALAPPDATA", "/l"),
    ];
    assert_eq!(unix(&all), Some(PathBuf::from("/r")));
    let xdg = [("XDG_STATE_HOME", "/s"), ("HOME", "/h")];
    assert_eq!(unix(&xdg), Some(PathBuf::from("/s/r-cad/recovery")));
    assert_eq!(
        unix(&[("HOME", "/h")]),
        Some(PathBuf::from("/h/.local/state/r-cad/recovery"))
    );
    // Empty values count as unset.
    assert_eq!(
        unix(&[
            ("RCAD_RECOVERY_DIR", ""),
            ("XDG_STATE_HOME", ""),
            ("HOME", "/h")
        ]),
        Some(PathBuf::from("/h/.local/state/r-cad/recovery"))
    );
    assert_eq!(unix(&[]), None);
    // The variable wins on every platform.
    for p in [Platform::Unix, Platform::MacOs, Platform::Windows] {
        assert_eq!(recovery_dir_on(p, env(&all)), Some(PathBuf::from("/r")));
    }
}

#[test]
fn recovery_dir_uses_the_macos_and_windows_user_folders() {
    let mac = |pairs: &[(&str, &str)]| recovery_dir_on(Platform::MacOs, env(pairs));
    // macOS: Application Support under HOME; XDG is a Linux convention.
    assert_eq!(
        mac(&[("XDG_STATE_HOME", "/s"), ("HOME", "/Users/u")]),
        Some(PathBuf::from(
            "/Users/u/Library/Application Support/R-CAD/recovery"
        ))
    );
    assert_eq!(mac(&[("XDG_STATE_HOME", "/s")]), None);
    let win = |pairs: &[(&str, &str)]| recovery_dir_on(Platform::Windows, env(pairs));
    // Windows: %LOCALAPPDATA%; a HOME (e.g. from Git Bash) is not used.
    let local = r"C:\Users\u\AppData\Local";
    assert_eq!(
        win(&[("LOCALAPPDATA", local), ("HOME", "/h")]),
        Some(PathBuf::from(local).join("R-CAD").join("recovery"))
    );
    assert_eq!(win(&[("HOME", "/h"), ("XDG_STATE_HOME", "/s")]), None);
}

#[test]
fn recovery_dir_follows_the_platform_it_was_built_for() {
    let pairs = [
        ("XDG_STATE_HOME", "/s"),
        ("HOME", "/h"),
        ("LOCALAPPDATA", "/l"),
    ];
    assert_eq!(
        recovery_dir(env(&pairs)),
        recovery_dir_on(Platform::CURRENT, env(&pairs))
    );
    let expected = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Unix
    };
    assert_eq!(Platform::CURRENT, expected);
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
