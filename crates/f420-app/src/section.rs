//! Section view (run 5): a display-only clipping plane through the model, on
//! a principal plane or a selected planar face, with an adjustable offset.
//! Material beyond the plane is not drawn and cannot be picked; the exposed
//! inside surfaces are tinted. The cut is **not capped** (no section face is
//! made) and nothing in the model, measurements or exports changes:
//! measurements still use the exact B-rep faces the user picks.
use crate::app::F420App as App;
use crate::probe::Probe;
use eframe::egui;
use f420_viewport::SectionPlane;

/// Plane the section is parallel to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SectionAxis {
    /// World YZ plane (normal +X).
    X,
    /// World XZ plane (normal +Y).
    Y,
    /// World XY plane (normal +Z).
    Z,
    /// A planar face's plane: point and outward normal (exact, from the summary).
    Face { point: [f64; 3], normal: [f64; 3] },
}

/// Section view state (None on the app = off).
#[derive(Debug, Clone, PartialEq)]
pub struct SectionState {
    pub axis: SectionAxis,
    /// Offset (mm) along the normal from the plane through the origin (world
    /// axes) or from the face's plane; the text buffer of the field.
    pub offset: String,
    /// Hide the other side.
    pub flip: bool,
}

impl SectionState {
    /// Unit normal (toward the hidden side, before flipping) and the offset of
    /// the base plane along it.
    fn base(&self) -> ([f64; 3], f64) {
        match self.axis {
            SectionAxis::X => ([1.0, 0.0, 0.0], 0.0),
            SectionAxis::Y => ([0.0, 1.0, 0.0], 0.0),
            SectionAxis::Z => ([0.0, 0.0, 1.0], 0.0),
            SectionAxis::Face { point, normal } => {
                let d = normal[0] * point[0] + normal[1] * point[1] + normal[2] * point[2];
                (normal, d)
            }
        }
    }

    /// The viewport plane, or None while the offset text is not a number.
    pub fn plane(&self) -> Option<SectionPlane> {
        let off: f64 = self
            .offset
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())?;
        let (n, d) = self.base();
        let s = if self.flip { -1.0 } else { 1.0 };
        Some(SectionPlane {
            normal: n.map(|c| (s * c) as f32),
            offset: (s * (d + off)) as f32,
        })
    }
}

impl App {
    /// The active section plane, if any (drawing and picking use it).
    pub fn section_plane(&self) -> Option<SectionPlane> {
        self.section.as_ref().and_then(SectionState::plane)
    }

    /// Turn the section view on (through the middle of the model, parallel to
    /// world XY) or off.
    pub fn toggle_section(&mut self) {
        self.section = match self.section {
            Some(_) => None,
            None => {
                let mid = self
                    .scene
                    .mesh
                    .bounds()
                    .map_or(0.0, |b| f64::from(0.5 * (b.min[2] + b.max[2])));
                Some(SectionState {
                    axis: SectionAxis::Z,
                    offset: crate::params_ui::fmt_value(mid),
                    flip: false,
                })
            }
        };
    }

    /// Controls of the section view (a small window over the 3D view).
    pub(crate) fn section_window(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.section.clone() else {
            return;
        };
        let face = self
            .selected_face()
            .and_then(|f| f.plane)
            .map(|p| SectionAxis::Face {
                point: p.point,
                normal: p.normal,
            });
        let bounds = self.scene.mesh.bounds();
        let mut open = true;
        egui::Window::new("Section view")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(self.view_rect.map_or(egui::pos2(400.0, 80.0), |r| {
                r.right_top() + egui::vec2(-270.0, 40.0)
            }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Plane");
                    ui.radio_value(&mut st.axis, SectionAxis::X, "YZ (X)")
                        .probe("Section X");
                    ui.radio_value(&mut st.axis, SectionAxis::Y, "XZ (Y)")
                        .probe("Section Y");
                    ui.radio_value(&mut st.axis, SectionAxis::Z, "XY (Z)")
                        .probe("Section Z");
                });
                let on_face = matches!(st.axis, SectionAxis::Face { .. });
                let r = ui
                    .add_enabled(
                        face.is_some(),
                        egui::Button::new("Use selected face").selected(on_face),
                    )
                    .on_disabled_hover_text("Select a planar face first")
                    .probe("Section on selected face");
                if r.clicked()
                    && let Some(f) = face
                {
                    st.axis = f;
                    st.offset = "0".into();
                }
                crate::panels::num_field(ui, "Section offset (mm)", &mut st.offset);
                // Slider over the model's extent along the normal.
                if let (Some(b), Ok(mut v)) = (bounds, st.offset.trim().parse::<f64>()) {
                    let (n, d) = st.base();
                    let corners = [b.min, b.max];
                    let proj: Vec<f64> = (0..8)
                        .map(|i| {
                            let p = [
                                corners[i & 1][0],
                                corners[(i >> 1) & 1][1],
                                corners[(i >> 2) & 1][2],
                            ];
                            (0..3).map(|k| n[k] * f64::from(p[k])).sum::<f64>() - d
                        })
                        .collect();
                    let lo = proj.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = proj.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if hi > lo
                        && ui
                            .add(egui::Slider::new(&mut v, lo..=hi).text("mm"))
                            .probe("Section slider")
                            .changed()
                    {
                        st.offset = crate::params_ui::fmt_value(v);
                    }
                }
                ui.checkbox(&mut st.flip, "Flip (hide the other side)")
                    .probe("Section flip");
                if st.plane().is_none() {
                    ui.colored_label(ui.visuals().warn_fg_color, "Offset must be a number (mm).");
                }
                ui.small(
                    "Display only: the cut is not capped (inside surfaces are tinted) and the \
                     model, measurements and exports are unchanged. Picking ignores hidden \
                     material.",
                );
            });
        self.section = open.then_some(st);
    }
}
