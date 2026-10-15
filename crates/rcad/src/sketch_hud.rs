//! Heads-up typed input while drawing (docs/behavior/sketching.md "Typed
//! input while drawing"): Tab opens the fields of the gesture in progress
//! next to the cursor (Line: length, angle; rectangles: width, height;
//! Circle: diameter), Tab cycles them, Enter places the point from the typed
//! values and dimensions them, Escape closes the fields.
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui;

impl RcadApp {
    pub(crate) fn sketch_hud(&mut self, ui: &egui::Ui) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let ed = &mut s.editor;
        let (Some(fields), Some(mut hud)) = (ed.hud_fields(), ed.hud.clone()) else {
            ed.hud = None;
            return;
        };
        let cursor = ed.cursor.as_ref().map(|c| c.pos);
        let Some(at) = cursor
            .and_then(|p| self.sketch_to_screen(p))
            .or_else(|| self.view_rect.map(|r| r.center()))
        else {
            return;
        };
        let ctx = ui.ctx().clone();
        let (mut apply, mut close) = (false, false);
        let tab = ctx.input(|i| i.key_pressed(egui::Key::Tab));
        let ids: Vec<egui::Id> = (0..fields.len())
            .map(|i| egui::Id::new(("sketch_hud_field", i)))
            .collect();
        // Focus before the fields run (Enter / Escape take it away).
        let focused = ids.iter().any(|id| ctx.memory(|m| m.has_focus(*id)));
        if tab && focused {
            hud.focus = (hud.focus + 1) % fields.len();
            hud.refocus = true;
        }
        egui::Area::new(egui::Id::new("sketch_hud"))
            .order(egui::Order::Foreground)
            .fixed_pos(at + egui::vec2(16.0, 16.0))
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    for (i, f) in fields.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let l = ui.label(*f);
                            let r = ui
                                .add(
                                    egui::TextEdit::singleline(&mut hud.values[i])
                                        .id(ids[i])
                                        .lock_focus(true)
                                        .desired_width(70.0),
                                )
                                .labelled_by(l.id)
                                .probe(f);
                            if hud.refocus && hud.focus == i {
                                r.request_focus();
                            }
                            if r.has_focus() && !hud.refocus {
                                hud.focus = i;
                            }
                        });
                    }
                    ui.small("Tab: next field · Enter: place · Esc: close");
                });
            });
        hud.refocus = false;
        for v in &mut hud.values {
            v.retain(|c| c != '\t');
        }
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Enter) && focused {
                apply = true;
            }
            if i.key_pressed(egui::Key::Escape) && focused {
                close = true;
            }
        });
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        s.editor.hud = Some(hud);
        if close {
            s.editor.hud = None;
        } else if apply && !s.editor.apply_hud() && s.editor.hud.is_some() {
            // Invalid input: keep the fields, focus the first again.
            if let Some(h) = &mut s.editor.hud {
                h.refocus = true;
            }
        }
    }
}
