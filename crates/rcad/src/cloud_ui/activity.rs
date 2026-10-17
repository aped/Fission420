//! Lists shown from server records: project and organization activity, and a
//! document's share links.

use eframe::egui::{self, RichText};
use rcad_cloud::proto;

use super::fmt_time;
use crate::probe::Probe as _;

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
    for (i, a) in entries.iter().enumerate() {
        let probe = format!("{key} {}", i + 1);
        ui.horizontal_wrapped(|ui| {
            match &a.document {
                Some(doc) => {
                    // A frameless button (not a hyperlink): reachable by its name.
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(a.summary.as_str())
                                    .color(ui.visuals().hyperlink_color),
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
                    ui.label(a.summary.as_str()).probe(&probe);
                }
            }
            ui.label(
                RichText::new(format!("{} · {}", a.actor, fmt_time(a.at, now)))
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
    format!("{version} · {expiry} · {download} · by {}", l.created_by)
}

#[cfg(test)]
mod tests {
    use super::*;

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
