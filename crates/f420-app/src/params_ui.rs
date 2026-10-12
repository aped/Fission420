//! Parameters panel (SESSION_4 §3): the document's named user parameters with
//! their expressions, evaluated values, units and relationships; add, edit,
//! rename and delete through the shared commands (`set_user_parameter`,
//! `rename_user_parameter`, `delete_user_parameter`). Every change is one
//! undoable transaction that regenerates the model; a refused change (unknown
//! name, cycle, division by zero, a value the geometry cannot take) leaves the
//! document unchanged and the error is shown here with the failing name.
use crate::app::F420App;
use crate::probe::Probe as _;
use eframe::egui::{self, Color32, RichText};
use f420_commands::Command;
use f420_document::ParamUnit;

/// Panel state (UI-local edit buffers; the document is authoritative).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParamsPanel {
    pub new_name: String,
    pub new_expr: String,
    pub new_unit: ParamUnit,
    /// Parameter being edited: (original name, name buffer, expression buffer, unit).
    pub edit: Option<(String, String, String, ParamUnit)>,
    /// Last refusal of a parameter command (shown until the next success).
    pub error: Option<String>,
}

fn unit_label(u: ParamUnit) -> &'static str {
    match u {
        ParamUnit::Mm => "mm",
        ParamUnit::Unitless => "no unit",
        ParamUnit::Deg => "deg",
    }
}

fn unit_combo(ui: &mut egui::Ui, id: &str, u: &mut ParamUnit) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(unit_label(*u))
        .width(70.0)
        .show_ui(ui, |ui| {
            for x in [ParamUnit::Mm, ParamUnit::Unitless, ParamUnit::Deg] {
                ui.selectable_value(u, x, unit_label(x));
            }
        });
}

/// Number formatting for values (trims float noise for display only).
pub(crate) fn fmt_value(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

impl F420App {
    /// Command replies that concern parameters: keep or clear the panel's error.
    pub(crate) fn params_reply(&mut self, cmd: &Command, ok: bool, error: Option<String>) {
        let ours = matches!(
            cmd,
            Command::SetUserParameter { .. }
                | Command::RenameUserParameter { .. }
                | Command::DeleteUserParameter { .. }
        );
        if !ours {
            return;
        }
        if ok {
            self.params.error = None;
            self.params.edit = None;
            if matches!(cmd, Command::SetUserParameter { name, .. } if *name == self.params.new_name.trim())
            {
                self.params.new_name.clear();
                self.params.new_expr.clear();
            }
        } else {
            self.params.error = error;
        }
    }

    pub(crate) fn parameters_panel(&mut self, ui: &mut egui::Ui) {
        let n = self.summary.parameters.len();
        egui::CollapsingHeader::new(RichText::new(format!("Parameters ({n})")).strong())
            .id_salt("params_panel")
            .default_open(true)
            .show(ui, |ui| self.parameters_body(ui));
    }

    fn parameters_body(&mut self, ui: &mut egui::Ui) {
        let params = self.summary.parameters.clone();
        let mut send: Vec<Command> = Vec::new();
        if params.is_empty() {
            ui.weak("No parameters. Add one below, then type its name in a dimension (e.g. width = outer_length - 2 * wall).");
        }
        egui::Grid::new("params_grid")
            .num_columns(3)
            .spacing([6.0, 3.0])
            .striped(true)
            .show(ui, |ui| {
                for p in &params {
                    let editing = self
                        .params
                        .edit
                        .as_ref()
                        .is_some_and(|(orig, ..)| *orig == p.name);
                    if editing {
                        let Some((orig, mut name, mut expr, mut unit)) = self.params.edit.clone()
                        else {
                            continue;
                        };
                        let (mut apply, mut cancel) = (false, false);
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                            .named("Parameter name");
                        ui.add(egui::TextEdit::singleline(&mut expr).desired_width(140.0))
                            .named("Parameter expression");
                        ui.horizontal(|ui| {
                            unit_combo(ui, "edit_unit", &mut unit);
                            apply = ui.small_button("Apply").named("Apply parameter").clicked();
                            cancel = ui
                                .small_button("Cancel")
                                .named("Cancel parameter edit")
                                .clicked();
                            let used = !p.used_by.is_empty();
                            let del = ui
                                .add_enabled(!used, egui::Button::new("Delete").small())
                                .named("Delete parameter");
                            let del = if used {
                                del.on_disabled_hover_text(format!(
                                    "Used by {}; change those first",
                                    p.used_by.join(", ")
                                ))
                            } else {
                                del
                            };
                            if del.clicked() {
                                send.push(Command::DeleteUserParameter {
                                    name: p.name.clone(),
                                });
                            }
                        });
                        self.params.edit = Some((orig.clone(), name.clone(), expr.clone(), unit));
                        if cancel {
                            self.params.edit = None;
                        } else if apply {
                            let name = name.trim().to_string();
                            if name != orig && params.iter().any(|x| x.name == name) {
                                // Review run 4 #14: a refused rename must not fall through to
                                // editing the other parameter.
                                self.params.error =
                                    Some(format!("another parameter is already named '{name}'"));
                                continue;
                            }
                            if name != orig {
                                send.push(Command::RenameUserParameter {
                                    name: orig.clone(),
                                    new_name: name.clone(),
                                });
                            }
                            if expr.trim() != p.expression || unit != p.unit {
                                send.push(Command::SetUserParameter {
                                    name,
                                    expression: expr,
                                    unit: Some(unit),
                                    comment: None,
                                });
                            }
                            if send.is_empty() {
                                self.params.edit = None;
                            }
                        }
                    } else {
                        let r = ui
                            .add(egui::Button::new(RichText::new(&p.name).monospace()).frame(false))
                            .named(&format!("param {}", p.name));
                        let hover = {
                            let mut t = String::new();
                            if !p.comment.is_empty() {
                                t += &p.comment;
                                t += "\n";
                            }
                            if !p.depends_on.is_empty() {
                                t += &format!("uses: {}\n", p.depends_on.join(", "));
                            }
                            t += &if p.used_by.is_empty() {
                                "not used yet".to_string()
                            } else {
                                format!("used by: {}", p.used_by.join(", "))
                            };
                            t + "\nClick to edit"
                        };
                        let r = r.on_hover_text(&hover);
                        ui.label(RichText::new(&p.expression).small())
                            .on_hover_text(&hover);
                        let value = match p.value {
                            Some(v) => format!("= {} {}", fmt_value(v), p.unit.symbol()),
                            None => "= ?".into(),
                        };
                        ui.label(RichText::new(value).strong())
                            .named(&format!("value {}", p.name));
                        if r.clicked() {
                            self.params.edit = Some((
                                p.name.clone(),
                                p.name.clone(),
                                p.expression.clone(),
                                p.unit,
                            ));
                        }
                    }
                    ui.end_row();
                }
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.params.new_name)
                    .hint_text("name")
                    .desired_width(80.0),
            )
            .named("New parameter name");
            ui.add(
                egui::TextEdit::singleline(&mut self.params.new_expr)
                    .hint_text("value or expression")
                    .desired_width(110.0),
            )
            .named("New parameter expression");
        });
        ui.horizontal(|ui| {
            unit_combo(ui, "new_unit", &mut self.params.new_unit);
            let can = !self.params.new_name.trim().is_empty()
                && !self.params.new_expr.trim().is_empty()
                && self.summary.open;
            if ui
                .add_enabled(can, egui::Button::new("Add parameter"))
                .named("Add parameter")
                .clicked()
            {
                send.push(Command::SetUserParameter {
                    name: self.params.new_name.trim().to_string(),
                    expression: self.params.new_expr.clone(),
                    unit: Some(self.params.new_unit),
                    comment: None,
                });
            }
        });
        if let Some(e) = &self.params.error {
            ui.label(
                RichText::new(format!("Refused, the model is unchanged: {e}"))
                    .color(Color32::from_rgb(255, 110, 110))
                    .small(),
            );
        }
        ui.small("Lengths in mm. Clearance parameters are per side unless named otherwise. Each change regenerates the model and is one Undo step.");
        for c in send {
            self.request(c);
        }
    }
}
