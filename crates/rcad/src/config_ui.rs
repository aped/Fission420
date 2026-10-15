//! Configuration table editor and variant export
//! (docs/CONFIGURATIONS.md). A grid with one column per
//! configuration and one row per part number, user parameter, feature
//! (suppressed?) and component occurrence (included?). Edits stay in a draft
//! until **Apply table** sends one `set_configuration_table` (one undo step,
//! validated; the active configuration is re-applied). **Export variants**
//! sends `export_configurations` (files named by configuration and part
//! number; existing files are only replaced with "Overwrite").
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, RichText};
use rcad_commands::Command;
use rcad_document::Configuration;

/// Draft state of the table editor and the export form.
#[derive(Debug, Clone)]
pub struct ConfigTableUi {
    /// The edited table and the (doc_id, revision) it was taken from.
    pub draft: Option<(u64, u64, Vec<Configuration>)>,
    /// Whether the draft differs from the document (kept across revisions).
    pub dirty: bool,
    /// The table sent by the last Apply, until its reply.
    pub pending: Option<Vec<Configuration>>,
    pub export_dir: String,
    pub export_stem: String,
    pub step: bool,
    pub stl: bool,
    pub threemf: bool,
    pub overwrite: bool,
}

impl Default for ConfigTableUi {
    fn default() -> Self {
        Self {
            draft: None,
            dirty: false,
            pending: None,
            export_dir: "variants".into(),
            export_stem: "part".into(),
            step: true,
            stl: false,
            threemf: false,
            overwrite: false,
        }
    }
}

impl ConfigTableUi {
    /// The draft is replaced only after a successful apply;
    /// a refused table keeps the user's cells (and stays "unapplied").
    pub(crate) fn on_reply(&mut self, response: &rcad_commands::Response) {
        if response.command != "set_configuration_table" {
            return;
        }
        let sent = self.pending.take();
        // Replace the draft only when it still holds exactly what was
        // applied; cells edited while the request was in flight are kept.
        if response.ok
            && let (Some(sent), Some((_, _, table))) = (sent, &self.draft)
            && *table == sent
        {
            self.dirty = false;
            self.draft = None;
        }
    }
}

impl RcadApp {
    /// The table editor and variant export, under the Configurations section.
    pub(crate) fn configuration_table(&mut self, ui: &mut egui::Ui) {
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        let configs = self.summary.configurations.clone();
        let st = &mut self.config_table;
        // Take a fresh draft when the document changed and nothing is pending.
        let stale = st
            .draft
            .as_ref()
            .is_none_or(|(d, r, _)| *d != doc || (*r != rev && !st.dirty));
        if stale {
            st.draft = Some((doc, rev, configs.clone()));
            st.dirty = false;
        }
        let params: Vec<String> = self
            .summary
            .parameters
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let features: Vec<(rcad_document::FeatureId, String)> = self
            .summary
            .features
            .iter()
            .filter(|f| f.kind != "sketch")
            .map(|f| (f.id, format!("{} {}", f.id, f.name)))
            .collect();
        let occurrences: Vec<(rcad_document::assembly::OccurrenceId, String)> = self
            .summary
            .assembly
            .occurrences
            .iter()
            .map(|o| (o.id, o.name.clone()))
            .collect();
        let mut send: Vec<Command> = Vec::new();
        let header = egui::CollapsingHeader::new("Configuration table")
            .id_salt("configuration_table")
            .open(self.shell.take_focus(crate::shell::Focus::ConfigTable))
            .show(ui, |ui| {
                let st = &mut self.config_table;
                let Some((_, _, table)) = st.draft.as_mut() else {
                    return;
                };
                let mut changed = false;
                egui::ScrollArea::horizontal()
                    .id_salt("configuration_table_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("configuration_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(RichText::new("Configuration").strong());
                                for (i, c) in table.iter_mut().enumerate() {
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut c.name)
                                                .desired_width(80.0),
                                        )
                                        .named(&format!("Configuration {i} name"))
                                        .changed();
                                }
                                ui.end_row();
                                ui.label("Part number");
                                for (i, c) in table.iter_mut().enumerate() {
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut c.part_number)
                                                .desired_width(80.0),
                                        )
                                        .named(&format!("Configuration {i} part number"))
                                        .changed();
                                }
                                ui.end_row();
                                for p in &params {
                                    ui.label(p).on_hover_text(
                                        "User parameter expression (empty: not set by the \
                                         configuration)",
                                    );
                                    for (i, c) in table.iter_mut().enumerate() {
                                        let mut v = c.values.get(p).cloned().unwrap_or_default();
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(&mut v)
                                                    .desired_width(80.0),
                                            )
                                            .named(&format!("Configuration {i} {p}"))
                                            .changed()
                                        {
                                            changed = true;
                                            if v.trim().is_empty() {
                                                c.values.remove(p);
                                            } else {
                                                c.values.insert(p.clone(), v);
                                            }
                                        }
                                    }
                                    ui.end_row();
                                }
                                for (f, label) in &features {
                                    ui.label(format!("{label} suppressed"));
                                    for (i, c) in table.iter_mut().enumerate() {
                                        let mut on = c.suppressed.contains(f);
                                        if ui
                                            .checkbox(&mut on, "")
                                            .named(&format!("Configuration {i} suppress {f}"))
                                            .changed()
                                        {
                                            changed = true;
                                            if on {
                                                c.suppressed.insert(*f);
                                            } else {
                                                c.suppressed.remove(f);
                                            }
                                        }
                                    }
                                    ui.end_row();
                                }
                                for (o, name) in &occurrences {
                                    ui.label(format!("{name} included"));
                                    for (i, c) in table.iter_mut().enumerate() {
                                        let mut on = !c.excluded_occurrences.contains(o);
                                        if ui
                                            .checkbox(&mut on, "")
                                            .named(&format!("Configuration {i} include {o}"))
                                            .changed()
                                        {
                                            changed = true;
                                            if on {
                                                c.excluded_occurrences.remove(o);
                                            } else {
                                                c.excluded_occurrences.insert(*o);
                                            }
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("+ Column")
                        .named("Add configuration column")
                        .on_hover_text(
                            "A new configuration with the current parameter expressions and \
                             suppression",
                        )
                        .clicked()
                    {
                        let n = (table.len() + 1..)
                            .map(|i| format!("Config{i}"))
                            .find(|n| !table.iter().any(|c| &c.name == n))
                            .unwrap_or_default();
                        table.push(Configuration {
                            name: n,
                            values: self
                                .summary
                                .parameters
                                .iter()
                                .map(|p| (p.name.clone(), p.expression.clone()))
                                .collect(),
                            suppressed: self
                                .summary
                                .features
                                .iter()
                                .filter(|f| f.suppressed)
                                .map(|f| f.id)
                                .collect(),
                            part_number: String::new(),
                            excluded_occurrences: Default::default(),
                        });
                        changed = true;
                    }
                    if !table.is_empty()
                        && ui
                            .button("− Last column")
                            .named("Remove last configuration column")
                            .clicked()
                    {
                        table.pop();
                        changed = true;
                    }
                    if ui
                        .add_enabled(
                            (st.dirty || changed) && st.pending.is_none(),
                            egui::Button::new("Apply table"),
                        )
                        .named("Apply configuration table")
                        .on_hover_text("One undo step; refused unchanged when a row is invalid")
                        .clicked()
                    {
                        st.pending = Some(table.clone());
                        send.push(Command::SetConfigurationTable {
                            configurations: table.clone(),
                        });
                    }
                    if ui
                        .add_enabled(st.dirty || changed, egui::Button::new("Revert"))
                        .named("Revert configuration table")
                        .clicked()
                    {
                        *table = configs.clone();
                        st.dirty = false;
                        changed = false;
                    }
                });
                if changed {
                    st.dirty = true;
                }
                if st.dirty {
                    ui.label(RichText::new("Unapplied changes").small().italics());
                }
                ui.separator();
                ui.label(RichText::new("Variant export").strong());
                ui.horizontal(|ui| {
                    ui.label("Folder");
                    ui.add(egui::TextEdit::singleline(&mut st.export_dir).desired_width(112.0))
                        .named("Variant export folder");
                    crate::panels::browse_button_compact(
                        ui,
                        "Variant export folder",
                        &mut st.export_dir,
                        crate::file_pick::PickMode::Folder,
                        crate::file_pick::NONE,
                    );
                    ui.label("Stem");
                    ui.add(egui::TextEdit::singleline(&mut st.export_stem).desired_width(70.0))
                        .named("Variant export stem");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut st.step, "STEP").named("Variant STEP");
                    ui.checkbox(&mut st.stl, "STL").named("Variant STL");
                    ui.checkbox(&mut st.threemf, "3MF").named("Variant 3MF");
                    ui.checkbox(&mut st.overwrite, "Overwrite")
                        .named("Variant overwrite")
                        .on_hover_text("Without it an existing file is refused, never replaced");
                });
                if ui
                    .add_enabled(
                        !configs.is_empty() && (st.step || st.stl || st.threemf),
                        egui::Button::new("Export variants"),
                    )
                    .named("Export variants")
                    .on_hover_text(
                        "<folder>/<stem>-<configuration>[-<part number>].<ext> for every \
                         configuration; the document and its undo history are restored",
                    )
                    .clicked()
                {
                    send.push(Command::ExportConfigurations {
                        dir: st.export_dir.clone().into(),
                        stem: st.export_stem.clone(),
                        configurations: Vec::new(),
                        step: st.step,
                        stl: st.stl,
                        threemf: st.threemf,
                        overwrite: st.overwrite,
                    });
                }
            });
        header.header_response.probe("Configuration table header");
        for c in send {
            self.request(c);
        }
    }
}
