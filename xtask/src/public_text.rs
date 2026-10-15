//! Tracked files describe the product, not how it was built: no development-run
//! numbers ("run 7", `run6_parts`), session-spec citations or paths into the
//! local, git-ignored working-notes directory. Checks every tracked path and
//! the text of every tracked UTF-8 file.
//!
//! The same pass refuses the project's former name (see [`find_old_name`]): the
//! rename to R-CAD kept no compatibility code, so any mention is a leftover.
use std::path::Path;
use std::process::Command;

/// Files that must name the markers themselves: this check, the root-layout
/// check and the ignore rules for the local notes directory.
const EXEMPT: &[&str] = &[
    ".gitignore",
    "xtask/src/main.rs",
    "xtask/src/public_text.rs",
];

/// Case-insensitive substrings that are always process vocabulary.
const MARKERS: &[&str] = &["morning_report", "goal.md", ".meta/", "claude.local.md"];

/// Prefixes that are process vocabulary when a digit follows (`SESSION_5`).
const NUMBERED: &[&str] = &["session_", "night_"];

/// The project's former name, in both spellings: the product name and the
/// four-character crate, file-extension and environment-variable prefix. Built
/// from parts so this file does not trip a plain-text search for it.
const OLD_PRODUCT: &str = concat!("fiss", "ion", "420");
const OLD_PREFIX: &str = concat!("f", "420");

const OLD_NAME_HINT: &str = "former project name; the project is R-CAD";

/// The first mention of the former project name in `text`, if any
/// (case-insensitive). The product name matches anywhere. The short prefix
/// matches only where it starts a word (`_`, `.` and `-` also start one, as in
/// an extension or `X_` variable) and does not begin a longer hexadecimal run,
/// so checksums and hex constants (`…d7f42…`, `0xf42…`) pass.
pub fn find_old_name(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let b = lower.as_bytes();
    if let Some(i) = lower.find(OLD_PRODUCT) {
        return Some(snippet(text, i, OLD_PRODUCT.len()));
    }
    for (i, _) in lower.match_indices(OLD_PREFIX) {
        if i > 0 && b[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = &b[i + OLD_PREFIX.len()..];
        let word = rest
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if word > 0 && rest[..word].iter().all(u8::is_ascii_hexdigit) {
            continue;
        }
        return Some(snippet(text, i, OLD_PREFIX.len()));
    }
    None
}

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
    // CI containers check the repository out as another user than the one running git, which
    // git refuses ("dubious ownership"); trust this one repository for this one call.
    let root = root
        .canonicalize()
        .map_err(|e| format!("repository root {}: {e}", root.display()))?;
    let out = Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", root.display()))
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("cannot run git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut found = Vec::new();
    for path in out.stdout.split(|&c| c == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path).into_owned();
        if path == "xtask/src/public_text.rs" {
            continue;
        }
        let markers = !EXEMPT.contains(&path.as_str());
        if let Some(m) = find_old_name(&path) {
            found.push(format!("{path}: name contains \"{m}\" ({OLD_NAME_HINT})"));
        }
        if let Some(m) = find_marker(&path).filter(|_| markers) {
            found.push(format!("{path}: name contains \"{m}\""));
        }
        // Deleted-but-unstaged files and binaries are skipped.
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if let Some(m) = find_old_name(line) {
                found.push(format!("{path}:{}: \"{m}\" ({OLD_NAME_HINT})", n + 1));
            }
            if let Some(m) = find_marker(line).filter(|_| markers) {
                found.push(format!("{path}:{}: \"{m}\"", n + 1));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::{find_marker, find_old_name};

    #[test]
    fn former_project_name_is_found() {
        // `{P}`, `{q}`, `{R}`, `{p}` stand for the spellings, substituted at run
        // time so the literal names never appear in the tree.
        let (big, small) = (concat!("Fiss", "ion", "420"), concat!("f", "420"));
        for s in [
            "Until 2026 the project was called {P}.",
            r#"  "format": "{q}","#,
            "{P}",
            "crates/{p}-commands/tests",
            "use {p}_document::Model;",
            "saved as `*.{p}.json`",
            "{R}_RECOVERY_DIR=/x",
            "struct {R}App;",
            "{p}",
            "tools/rename_{p}.py",
        ] {
            let s = s
                .replace("{P}", big)
                .replace("{q}", &big.to_ascii_lowercase())
                .replace("{R}", &small.to_ascii_uppercase())
                .replace("{p}", small);
            assert!(find_old_name(&s).is_some(), "{s}");
        }
    }

    #[test]
    fn hex_and_other_words_pass() {
        for s in [
            concat!(
                "checksum = \"67ec1fc3ad980a24e4761b763bf5bfbf58c1d88be1bffb14654d369d7f",
                "420258\""
            ),
            concat!("const SEED: u64 = 0xf", "420a;"),
            concat!("commit f", "420abc1"),
            concat!("fis", "sion products"),
            "R-CAD",
            "rcad_document",
        ] {
            assert!(find_old_name(s).is_none(), "{s}");
        }
    }

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
