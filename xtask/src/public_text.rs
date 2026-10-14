//! Tracked files describe the product, not how it was built: no development-run
//! numbers ("run 7", `run6_parts`), session-spec citations or paths into the
//! local, git-ignored working-notes directory. Checks every tracked path and
//! the text of every tracked UTF-8 file.
use std::path::Path;
use std::process::Command;

/// Files that must name the markers themselves: this check, the root-layout
/// check and the ignore rules for the local notes directory.
const EXEMPT: &[&str] = &[".gitignore", "xtask/src/main.rs", "xtask/src/public_text.rs"];

/// Case-insensitive substrings that are always process vocabulary.
const MARKERS: &[&str] = &["morning_report", "goal.md", ".meta/", "claude.local.md"];

/// Prefixes that are process vocabulary when a digit follows (`SESSION_5`).
const NUMBERED: &[&str] = &["session_", "night_"];

/// The first process marker in `text`, if any. "run" followed by an optional
/// `-`, `_` or space and a digit counts when "run" starts a word (an
/// underscore before it also starts one, as in `v6_run6`).
pub fn find_marker(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let b = lower.as_bytes();
    for m in MARKERS {
        if lower.contains(m) {
            return Some((*m).to_string());
        }
    }
    let digit_at = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    for p in NUMBERED {
        for (i, _) in lower.match_indices(p) {
            if digit_at(i + p.len()) {
                return Some(snippet(text, i, p.len() + 1));
            }
        }
    }
    for (i, _) in lower.match_indices("run") {
        if i > 0 && b[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let mut j = i + 3;
        if matches!(b.get(j), Some(b'-' | b'_' | b' ')) {
            j += 1;
        }
        if digit_at(j) {
            return Some(snippet(text, i, j + 1 - i));
        }
    }
    None
}

fn snippet(text: &str, start: usize, len: usize) -> String {
    text.get(start..start + len).unwrap_or("?").to_string()
}

/// Offending `path: marker` (or `path:line: marker`) entries across the
/// tracked files of the repository at `root`.
pub fn offenders(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files failed".into());
    }
    let mut found = Vec::new();
    for path in out.stdout.split(|&c| c == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path).into_owned();
        if EXEMPT.contains(&path.as_str()) {
            continue;
        }
        if let Some(m) = find_marker(&path) {
            found.push(format!("{path}: name contains \"{m}\""));
        }
        // Deleted-but-unstaged files and binaries are skipped.
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if let Some(m) = find_marker(line) {
                found.push(format!("{path}:{}: \"{m}\"", n + 1));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::find_marker;

    #[test]
    fn process_vocabulary_is_found() {
        for s in [
            "/// Exact loft (run 5, ADR 0008)",
            "// Review run 7 #40: one rule",
            "fn v6_run6_documents() {",
            "Run-7 drawing annotations",
            "artifacts/run3/acceptance",
            "(SESSION_4 §7 item 1)",
            "see MORNING_REPORT_RUN_7.md",
            "records live in .meta/runs/",
        ] {
            assert!(find_marker(s).is_some(), "{s}");
        }
    }

    #[test]
    fn ordinary_text_passes() {
        for s in [
            "cargo run --release -p rcad-cli",
            "the run of edges",
            "rerun 3 times",
            "fn run(stage: &str)",
            "truncated runs",
            "session_state",
            "a 3 mm runout",
        ] {
            assert!(find_marker(s).is_none(), "{s}");
        }
    }
}
