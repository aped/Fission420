//! Lists shown from server records: project and organization activity, and a
//! document's share links.

use eframe::egui::{self, RichText};
use rcad_cloud::proto;

use super::fmt_time;
use crate::probe::Probe as _;

/// Longest one-line text shown from a server record (names, summaries).
pub const MAX_LINE_CHARS: usize = 200;
/// Longest comment body shown.
pub const MAX_BODY_CHARS: usize = 4000;
/// Most activity entries, comments or share links listed at once.
pub const MAX_ROWS: usize = 500;

/// Server text made safe to lay out: control characters (newlines too, unless
/// `multiline`), bidirectional controls and other invisible format characters
/// become spaces, and the text is cut to `max` characters with an ellipsis. A
/// hostile server can then neither fake extra rows nor send megabytes of text
/// that are laid out every frame.
pub fn display_text(s: &str, max: usize, multiline: bool) -> String {
    let mut out = String::with_capacity(s.len().min(max * 4));
    for (n, c) in s.chars().enumerate() {
        if n == max {
            out.push('…');
            break;
        }
        let keep_newline = multiline && c == '\n';
        let bad = (c.is_control() && !keep_newline)
            || matches!(
                c,
                '\u{061C}'
                    | '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{2069}'
                    | '\u{FEFF}'
            );
        out.push(if bad { ' ' } else { c });
    }
    out
}

/// The date part of [`fmt_time`] ("2026-10-24 08:00 UTC"), for times that may
/// lie in the future (expiry).
pub fn fmt_date(t: i64) -> String {
    let full = fmt_time(t, t);
    full.split(" (").next().unwrap_or(&full).to_string()
}

/// Activity entries, newest first: summary, who and when. An entry naming a
/// document is a link; clicking it returns that document's id. Each row is
/// probed as `<key> <n>` (1 = newest).
pub fn activity_list(
    ui: &mut egui::Ui,
    entries: &[proto::ActivityEntry],
    now: i64,
    key: &str,
) -> Option<String> {
    let mut open = None;
    if entries.is_empty() {
        ui.weak("No activity yet.");
        return None;
    }
    for (i, a) in entries.iter().take(MAX_ROWS).enumerate() {
        let probe = format!("{key} {}", i + 1);
        let summary = display_text(&a.summary, MAX_LINE_CHARS, false);
        ui.horizontal_wrapped(|ui| {
            match &a.document {
                Some(doc) => {
                    // A frameless button (not a hyperlink): reachable by its name.
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(summary.as_str()).color(ui.visuals().hyperlink_color),
                            )
                            .frame(false),
                        )
                        .named(&probe)
                        .on_hover_text("Open this document")
                        .clicked()
                    {
                        open = Some(doc.clone());
                    }
                }
                None => {
                    ui.label(summary.as_str()).probe(&probe);
                }
            }
            ui.label(
                RichText::new(format!(
                    "{} · {}",
                    display_text(&a.actor, MAX_LINE_CHARS, false),
                    fmt_time(a.at, now)
                ))
                .small()
                .weak(),
            );
        });
    }
    open
}

/// One line describing a share link: version, expiry, download, creator.
pub fn share_line(l: &proto::ShareLink) -> String {
    let version = l
        .version
        .map_or("latest version".to_string(), |v| format!("v{v}"));
    let expiry = l.expires_at.map_or("never expires".to_string(), |t| {
        format!("expires {}", fmt_date(t))
    });
    let download = if l.allow_download {
        "download allowed"
    } else {
        "view only"
    };
    format!(
        "{version} · {expiry} · {download} · by {}",
        display_text(&l.created_by, MAX_LINE_CHARS, false)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_text_cannot_fake_rows_or_flood_the_layout() {
        assert_eq!(display_text("plain", 200, false), "plain");
        assert_eq!(
            display_text("saved v2\nalice deleted everything", 200, false),
            "saved v2 alice deleted everything"
        );
        assert_eq!(
            display_text("a\u{202E}b\u{2066}c\u{200B}d\te", 200, false),
            "a b c d e"
        );
        assert_eq!(
            display_text("line 1\nline 2\r", 200, true),
            "line 1\nline 2 "
        );
        let long = "x".repeat(5_000_000);
        let shown = display_text(&long, MAX_LINE_CHARS, false);
        assert_eq!(shown.chars().count(), MAX_LINE_CHARS + 1);
        assert!(shown.ends_with('…'));
        // Cut by characters, never inside one.
        assert_eq!(display_text("ééééé", 3, false), "ééé…");
    }

    #[test]
    fn share_lines_name_version_expiry_download_and_creator() {
        let mut l = proto::ShareLink {
            id: "s1".into(),
            document: "d1".into(),
            version: Some(3),
            created_by: "alice".into(),
            created_at: 0,
            expires_at: Some(86_400 * 7),
            allow_download: true,
        };
        assert_eq!(
            share_line(&l),
            "v3 · expires 1970-01-08 00:00 UTC · download allowed · by alice"
        );
        l.version = None;
        l.expires_at = None;
        l.allow_download = false;
        assert_eq!(
            share_line(&l),
            "latest version · never expires · view only · by alice"
        );
    }
}
