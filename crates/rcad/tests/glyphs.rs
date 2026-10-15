//! Every character the UI draws from a string literal has a glyph in the
//! UI font: a missing one is drawn as an empty box ("Export ☐", "☐ unsaved
//! changes"). Scans the string literals of the app's sources (comments are
//! skipped) and asks egui's default fonts.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, FontFamily, FontId};

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

/// Non-ASCII characters inside string or char literals of one line of
/// code (the part before a `//` comment that is outside a literal).
fn literal_chars(line: &str) -> Vec<char> {
    let mut out = vec![];
    let mut in_str = false;
    let mut chars = line.chars().peekable();
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        if !in_str {
            if c == '/' && chars.peek() == Some(&'/') {
                break;
            }
            if c == '"' {
                in_str = true;
            } else if c == '\'' {
                // A char literal such as '…' (not a lifetime).
                let rest: String = chars.clone().take(2).collect();
                let mut it = rest.chars();
                if let (Some(ch), Some('\'')) = (it.next(), it.next())
                    && !ch.is_ascii()
                {
                    out.push(ch);
                }
            }
        } else if c == '"' && prev != '\\' {
            in_str = false;
        } else if !c.is_ascii() {
            out.push(c);
        }
        prev = c;
    }
    out
}

#[test]
fn ui_strings_use_only_glyphs_the_font_has() {
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(Default::default(), |_| {});
    let font = FontId::new(14.0, FontFamily::Proportional);
    let mut files = vec![];
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    let mut missing: BTreeMap<char, Vec<String>> = BTreeMap::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for c in literal_chars(line) {
                // `has_glyph` also says no for glyphs of the face that
                // holds the replacement glyph; a missing glyph has no width.
                let drawn = ctx.fonts_mut(|fonts| {
                    fonts.has_glyph(&font, c) || fonts.glyph_width(&font, c) > 0.0
                });
                // The window title is drawn by the desktop, not this font.
                if !drawn && !(c == '●' && f.ends_with("app.rs")) {
                    missing.entry(c).or_default().push(format!(
                        "{}:{}",
                        f.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                            .unwrap()
                            .display(),
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "characters without a glyph in the UI font: {missing:#?}"
    );
}
