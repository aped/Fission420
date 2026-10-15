//! View cube and navigation bar, drawn in code over the 3D view (original
//! design: a plain shaded cube with face names, small corner and edge hot
//! spots; a row of text buttons at the right end of the timeline strip, below
//! the canvas so it never covers the model).
//!
//! ViewCube: clicking a face runs the matching view command (Top, Front, …);
//! a corner or edge looks at the model from that direction (an isometric
//! view from a corner). Accessible names: "ViewCube <Face>", "ViewCube
//! corner <±x±y±z>", "ViewCube edge <…>".
//!
//! Navigation bar: Orbit / Pan / Zoom set what a left drag does in the view
//! (the middle and right buttons keep orbiting and panning), Fit view, Look
//! At (the selected planar face, or the open sketch), and Display style.

use eframe::egui::{
    self, Color32, Pos2, Rect, Sense, Stroke, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use super::registry::{self, CmdId};
use crate::app::RcadApp;
use crate::probe::Probe as _;

/// What a left drag in the view does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavMode {
    #[default]
    Orbit,
    Pan,
    Zoom,
}

const CUBE: f32 = 34.0; // half-size of the drawn cube (points)

type V3 = [f32; 3];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The six faces: (name, outward normal, command).
const FACES: [(&str, V3, CmdId); 6] = [
    ("Top", [0.0, 0.0, 1.0], CmdId::ViewTop),
    ("Bottom", [0.0, 0.0, -1.0], CmdId::ViewBottom),
    ("Front", [0.0, -1.0, 0.0], CmdId::ViewFront),
    ("Back", [0.0, 1.0, 0.0], CmdId::ViewBack),
    ("Right", [1.0, 0.0, 0.0], CmdId::ViewRight),
    ("Left", [-1.0, 0.0, 0.0], CmdId::ViewLeft),
];

fn sign(v: f32) -> char {
    if v > 0.0 { '+' } else { '-' }
}

impl RcadApp {
    /// Look at the model from direction `dir` (from the target towards the
    /// eye), keeping the distance (corner and edge picks of the view cube).
    pub(crate) fn look_from(&mut self, dir: V3) {
        let n = dot(dir, dir).sqrt();
        if n < 1e-6 {
            return;
        }
        let d = [dir[0] / n, dir[1] / n, dir[2] / n];
        let limit = 89.0_f32.to_radians();
        let pitch = d[2].clamp(-1.0, 1.0).asin().clamp(-limit, limit);
        let yaw = if d[0].abs() + d[1].abs() < 1e-6 {
            -std::f32::consts::FRAC_PI_2
        } else {
            d[1].atan2(d[0])
        };
        let cam = self.camera_mut();
        cam.yaw = yaw.rem_euclid(std::f32::consts::TAU);
        cam.pitch = pitch;
        if let Some(vp) = self.viewport.as_mut() {
            vp.fit();
        }
    }

    /// Look At: square the view onto the selected planar face (or the open
    /// sketch's plane).
    pub(crate) fn look_at_selection(&mut self) {
        if self.sketch.is_some() {
            self.orient_to_sketch();
            return;
        }
        let Some(p) = self.selected_face().and_then(|f| f.plane) else {
            return;
        };
        let n = p.normal.map(|c| c as f32);
        let x = if n[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [-n[1], n[0], 0.0]
        };
        match self.viewport.as_mut() {
            Some(vp) => vp.look_along_normal(n, x),
            None => self.fallback_camera.look_along_normal(n, x),
        }
    }

    pub(crate) fn look_at_blocker(&self) -> Result<(), String> {
        if self.sketch.is_some() || self.selected_face().is_some_and(|f| f.plane.is_some()) {
            Ok(())
        } else {
            Err("Select a planar face (or open a sketch) to look at".into())
        }
    }

    pub(crate) fn set_display_edges(&mut self, on: bool) {
        self.shell.show_edges = on;
        if let Some(vp) = self.viewport.as_mut() {
            vp.set_show_edges(on);
        }
    }

    /// One clickable hot spot of the cube.
    fn cube_spot(&self, ui: &mut egui::Ui, center: Pos2, size: f32, name: &str) -> egui::Response {
        let rect = Rect::from_center_size(center, Vec2::splat(size));
        let id = ui.id().with(("viewcube", name));
        let r = ui.interact(rect, id, Sense::click());
        let n = name.to_string();
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &n));
        r.probe(name)
    }

    /// The view cube in the top-right corner of `view`.
    pub(crate) fn view_cube(&mut self, ui: &mut egui::Ui, view: Rect) {
        let (cam, _) = self.view_camera();
        let (right, up, fwd) = cam.basis();
        let eye = [-fwd[0], -fwd[1], -fwd[2]];
        let c = pos2(view.max.x - CUBE * 1.9, view.min.y + CUBE * 1.9);
        let proj = |v: V3| c + vec2(dot(v, right), -dot(v, up)) * CUBE;
        let painter = ui.painter_at(view);
        // Faces facing the camera, back to front.
        let mut faces: Vec<_> = FACES
            .iter()
            .filter(|(_, n, _)| dot(*n, eye) > 0.02)
            .collect();
        faces.sort_by(|a, b| dot(a.1, eye).total_cmp(&dot(b.1, eye)));
        let mut run: Option<CmdId> = None;
        let mut look: Option<V3> = None;
        for (name, n, id) in faces {
            // Two in-plane axes of the face.
            let (u, v) = if n[2].abs() > 0.5 {
                ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
            } else if n[0].abs() > 0.5 {
                ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
            } else {
                ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
            };
            let corner = |a: f32, b: f32| {
                proj([
                    n[0] + a * u[0] + b * v[0],
                    n[1] + a * u[1] + b * v[1],
                    n[2] + a * u[2] + b * v[2],
                ])
            };
            let shade = (110.0 + 120.0 * dot(*n, eye)) as u8;
            let fill = Color32::from_rgb(shade / 2, shade / 2 + 20, shade);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ],
                fill,
                Stroke::new(1.0, Color32::from_gray(25)),
            ));
            let center = proj(*n);
            let r = self.cube_spot(
                ui,
                center,
                CUBE * 0.9 * dot(*n, eye).max(0.35),
                &format!("ViewCube {name}"),
            );
            if r.hovered() {
                painter.circle_filled(center, 4.0, Color32::from_rgb(255, 200, 90));
            }
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                name.to_uppercase(),
                egui::FontId::proportional(9.0),
                Color32::WHITE,
            );
            if r.on_hover_text(format!("{name} view")).clicked() {
                run = Some(*id);
            }
        }
        // Visible corners and edge midpoints: small hot spots.
        let mut spots: Vec<(V3, String)> = vec![];
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    spots.push((
                        [sx, sy, sz],
                        format!("ViewCube corner {}x{}y{}z", sign(sx), sign(sy), sign(sz)),
                    ));
                }
            }
        }
        for (a, b) in [(0usize, 1usize), (0, 2), (1, 2)] {
            for s1 in [-1.0f32, 1.0] {
                for s2 in [-1.0f32, 1.0] {
                    let mut p = [0.0; 3];
                    p[a] = s1;
                    p[b] = s2;
                    let axes = ['x', 'y', 'z'];
                    spots.push((
                        p,
                        format!(
                            "ViewCube edge {}{}{}{}",
                            sign(s1),
                            axes[a],
                            sign(s2),
                            axes[b]
                        ),
                    ));
                }
            }
        }
        for (p, name) in spots {
            // Visible: the direction faces the camera.
            if dot(p, eye) <= 0.05 {
                continue;
            }
            let at = proj(p);
            let r = self.cube_spot(ui, at, 9.0, &name);
            if r.hovered() {
                painter.circle_filled(at, 4.0, Color32::from_rgb(255, 200, 90));
            } else {
                painter.circle_filled(at, 2.0, Color32::from_gray(200));
            }
            if r.clicked() {
                look = Some(p);
            }
        }
        if let Some(id) = run {
            self.run_command(id);
        } else if let Some(dir) = look {
            self.look_from(dir);
        }
    }

    /// The navigation bar (a left-to-right group of buttons).
    pub(crate) fn nav_bar(&mut self, ui: &mut egui::Ui) {
        let mode = self.shell.nav_mode;
        for (id, m) in [
            (CmdId::Orbit, NavMode::Orbit),
            (CmdId::Pan, NavMode::Pan),
            (CmdId::Zoom, NavMode::Zoom),
        ] {
            let d = registry::def(id);
            let r = ui
                .add(egui::Button::selectable(mode == m, d.label))
                .probe(d.label)
                .on_hover_text(self.command_tooltip(d));
            if r.clicked() {
                (d.run)(self);
            }
        }
        ui.separator();
        self.cmd_button(ui, CmdId::Fit);
        self.cmd_button(ui, CmdId::LookAt);
        ui.separator();
        let edges = self.shell.show_edges;
        let r = ui.menu_button("Display style", |ui| {
            for (id, on) in [(CmdId::StyleEdges, true), (CmdId::StyleShaded, false)] {
                let d = registry::def(id);
                if ui
                    .add(egui::Button::selectable(edges == on, d.label))
                    .probe(d.label)
                    .clicked()
                {
                    (d.run)(self);
                    ui.close();
                }
            }
        });
        r.response.probe("Display style");
    }

    /// Left drag in Pan / Zoom mode (Orbit is the viewport's own drag).
    pub(crate) fn nav_drag(&mut self, resp: &egui::Response) {
        if self.shell.nav_mode == NavMode::Orbit || !resp.dragged_by(egui::PointerButton::Primary) {
            return;
        }
        let d = resp.drag_delta();
        let h = resp.rect.height();
        let mode = self.shell.nav_mode;
        let cam = self.camera_mut();
        match mode {
            NavMode::Pan => cam.pan(d.x, d.y, h),
            NavMode::Zoom => cam.zoom(-d.y * 0.05),
            NavMode::Orbit => {}
        }
    }
}
