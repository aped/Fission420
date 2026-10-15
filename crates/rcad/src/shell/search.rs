//! Command search (S in the Fusion-style layer): a popup over the command
//! registry. Every typed word must occur in a command's label, tooltip or
//! placement; the list shows each command's key and ribbon place, disabled
//! commands keep their reason on hover. Enter runs the first enabled match,
//! Esc closes.

use eframe::egui::{self, RichText};

use super::registry::{self, CommandDef};
use crate::app::RcadApp;
use crate::probe::Probe as _;

const MAX_ROWS: usize = 12;

/// Commands matching `query` (all words, case-insensitive), label matches
/// first.
pub fn matches(query: &str) -> Vec<&'static CommandDef> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let first = words.first().cloned().unwrap_or_default();
    let mut hits: Vec<(&'static CommandDef, u8)> = registry::COMMANDS
        .iter()
        .filter_map(|d| {
            let label = d.label.to_lowercase();
            let hay = format!(
                "{label} {} {}",
                d.tooltip.to_lowercase(),
                registry::placement_text(d).to_lowercase()
            );
            words.iter().all(|w| hay.contains(w.as_str())).then(|| {
                let rank = if label.starts_with(first.as_str()) {
                    0
                } else if words.iter().all(|w| label.contains(w.as_str())) {
                    1
                } else {
                    2
                };
                (d, rank)
            })
        })
        .collect();
    // Stable: labels starting with the query, then label matches, then
    // tooltip/placement matches.
    hits.sort_by_key(|(_, rank)| *rank);
    hits.into_iter().map(|(d, _)| d).collect()
}

impl RcadApp {
    pub(crate) fn open_command_search(&mut self) {
        self.shell.search = Some(String::new());
    }

    pub(crate) fn command_search_ui(&mut self, ctx: &egui::Context) {
        let Some(mut query) = self.shell.search.take() else {
            return;
        };
        let mut run = None;
        let mut close = false;
        let layer = self.shell.prefs.keymap;
        egui::Window::new("Find command")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .default_width(420.0)
            .show(ctx, |ui| {
                // Before the text field (which takes Escape to drop focus).
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Type a command name…")
                            .desired_width(400.0),
                    )
                    .named("Search commands");
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                let hits = matches(&query);
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                // Escape (the field drops focus) or a click elsewhere closes it.
                if r.lost_focus() && !enter {
                    close = true;
                }
                if enter {
                    run = hits
                        .iter()
                        .find(|d| (d.enabled)(self).is_ok())
                        .map(|d| d.id);
                }
                if hits.is_empty() {
                    ui.weak("No command matches");
                }
                for d in hits.iter().take(MAX_ROWS) {
                    let state = (d.enabled)(self);
                    ui.horizontal(|ui| {
                        let b = ui
                            .add_enabled(state.is_ok(), egui::Button::new(d.label).frame(false))
                            .named(&format!("Search result {}", d.label));
                        let b = match &state {
                            Ok(()) => b.on_hover_text(d.tooltip),
                            Err(why) => b.on_disabled_hover_text(why.as_str()),
                        };
                        if b.clicked() {
                            run = Some(d.id);
                        }
                        ui.label(RichText::new(d.key_text(layer)).weak().small());
                        ui.label(RichText::new(registry::placement_text(d)).weak().small());
                    });
                }
                if hits.len() > MAX_ROWS {
                    ui.weak(format!("… {} more", hits.len() - MAX_ROWS));
                }
            })
            .map(|r| r.response.probe("Command search"));
        if let Some(id) = run {
            self.run_command(id);
        } else if !close {
            self.shell.search = Some(query);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_words_must_match_and_label_hits_come_first() {
        let m = matches("ext");
        assert_eq!(m[0].label, "Extrude…");
        assert!(matches("export").iter().any(|d| d.label == "Export STL…"));
        let m = matches("fillet loop");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].label, "Fillet face loop");
        assert!(matches("zzzz").is_empty());
        assert_eq!(matches("").len(), registry::COMMANDS.len());
    }
}
