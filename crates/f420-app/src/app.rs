//! The Fission420 UI. Widgets hold only transient edit buffers; the document
//! state shown here is always the last [`DocSummary`] reported by the engine.

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui;
use f420_commands::{Command, DocSummary, FeatureSummary, SketchTemplate, end_cap_of};
use f420_document::{DocError, FaceRole, FeatureId, SketchPlane};
use f420_kernel::{Measurements, MeshCheck, VolumeResult};
use f420_viewport::{Camera, Overlay, OverlayLabel, OverlayPolyline, Viewport};

use crate::engine::{Engine, ExecMode, MeshUpdate, Reply};
use crate::place::{self, Phase, Placement, Proposal, Tool};

/// View size assumed for placement when no viewport has been laid out yet.
const FALLBACK_VIEW_SIZE: [f32; 2] = [800.0, 600.0];

/// Max diagnostics kept in the panel.
const MAX_DIAGNOSTICS: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub error: bool,
    /// Error kind (e.g. `Validation`, `ProductLimit`) or `info`.
    pub kind: String,
    pub text: String,
}

/// Open modal-ish form. Only one at a time.
#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    /// Rectangle on world XZ (profile for a revolve about world Z).
    RectangleXz {
        width: String,
        height: String,
        cx: String,
        cz: String,
    },
    Revolve {
        sketch: Option<FeatureId>,
        angle: String,
    },
    Rectangle {
        width: String,
        height: String,
        cx: String,
        cy: String,
    },
    Extrude {
        sketch: Option<FeatureId>,
        distance: String,
    },
    CircleOnFace {
        extrude: Option<FeatureId>,
        diameter: String,
        cx: String,
        cy: String,
    },
    Cut {
        sketch: Option<FeatureId>,
    },
    SaveAs {
        path: String,
    },
    Open {
        path: String,
    },
    ExportStl {
        path: String,
        chord: String,
    },
    ExportStep {
        path: String,
    },
}

impl Dialog {
    fn title(&self) -> &'static str {
        match self {
            Dialog::Rectangle { .. } => "New rectangle sketch (XY)",
            Dialog::Extrude { .. } => "New extrusion",
            Dialog::RectangleXz { .. } => "New rectangle sketch (XZ, for revolve)",
            Dialog::Revolve { .. } => "New revolve (about world Z)",
            Dialog::CircleOnFace { .. } => "New circle sketch on face",
            Dialog::Cut { .. } => "New through-all cut",
            Dialog::SaveAs { .. } => "Save document as",
            Dialog::Open { .. } => "Open document",
            Dialog::ExportStl { .. } => "Export STL",
            Dialog::ExportStep { .. } => "Export STEP",
        }
    }
}

/// Inspector edit buffers for the selected feature (preview only until Apply).
#[derive(Debug, Clone, PartialEq)]
struct Inspector {
    feature: FeatureId,
    fields: Vec<(String, f64, String)>, // (name, document value, edit buffer)
}

/// One step of the `--demo` script, resolved against the current document
/// when it is submitted (feature ids come from earlier replies).
#[derive(Debug, Clone)]
enum DemoStep {
    Cmd(Command),
    ExtrudeOf {
        name: &'static str,
        sketch: &'static str,
        distance: f64,
    },
    CircleOnCapOf {
        name: &'static str,
        extrude: &'static str,
        diameter: f64,
    },
    CutOf {
        name: &'static str,
        sketch: &'static str,
    },
    Set {
        feature: &'static str,
        param: &'static str,
        value: f64,
    },
    Select(&'static str),
    /// Activate "Place hole" on the end cap of the named extrusion.
    PlaceHoleOn(&'static str),
    /// Synthetic pointer event at the screen projection of a world point
    /// (same path as real mouse input).
    Pointer(Phase, [f64; 3]),
    /// Press OK on the open dialog.
    DialogOk,
    /// Cut through all with the most recent unused sketch.
    CutLatest(&'static str),
    /// Select the most recently created sketch.
    SelectLatestSketch,
}

pub struct F420App {
    engine: Engine,
    summary: DocSummary,
    measurements: Option<Measurements>,
    selected: Option<FeatureId>,
    inspector: Option<Inspector>,
    dialog: Option<Dialog>,
    diagnostics: Vec<Diagnostic>,
    last_export: Option<MeshCheck>,
    pub viewport: Option<Viewport>,
    mesh_triangles: usize,
    fitted: bool,
    demo: VecDeque<DemoStep>,
    replies_seen: u64,
    /// Active placement tool (rays from the view camera feed it).
    placement: Option<Placement>,
    /// Finished placement whose pre-filled dialog is open; drawn until it closes.
    held_preview: Option<Placement>,
    /// Camera used for placement when there is no GPU viewport (tests).
    pub fallback_camera: Camera,
    view_rect: Option<egui::Rect>,
}

impl F420App {
    pub fn new(mode: ExecMode, repaint: Option<Box<dyn Fn() + Send>>) -> Self {
        Self {
            engine: Engine::new(mode, repaint),
            summary: DocSummary {
                doc_id: 0,
                open: false,
                path: None,
                dirty: false,
                revision: 0,
                can_undo: None,
                can_redo: None,
                features: vec![],
            },
            measurements: None,
            selected: None,
            inspector: None,
            dialog: None,
            diagnostics: vec![],
            last_export: None,
            viewport: None,
            mesh_triangles: 0,
            fitted: false,
            demo: VecDeque::new(),
            replies_seen: 0,
            placement: None,
            held_preview: None,
            fallback_camera: Camera {
                target: [0.0, 0.0, 5.0],
                distance: 220.0,
                ..Camera::default()
            },
            view_rect: None,
        }
    }

    // ----- read-only accessors (tests, screenshot driver) -----

    pub fn summary(&self) -> &DocSummary {
        &self.summary
    }
    pub fn measurements(&self) -> Option<&Measurements> {
        self.measurements.as_ref()
    }
    pub fn volume(&self) -> Option<f64> {
        match self.measurements.as_ref()?.volume {
            VolumeResult::Value { volume, .. } => Some(volume),
            VolumeResult::Unavailable { .. } => None,
        }
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    pub fn last_export(&self) -> Option<&MeshCheck> {
        self.last_export.as_ref()
    }
    pub fn mesh_triangles(&self) -> usize {
        self.mesh_triangles
    }
    pub fn busy(&self) -> bool {
        self.engine.busy()
    }
    pub fn replies_seen(&self) -> u64 {
        self.replies_seen
    }
    /// No command in flight and no demo steps left.
    pub fn idle(&self) -> bool {
        !self.engine.busy() && self.demo.is_empty()
    }
    pub fn feature_by_name(&self, name: &str) -> Option<&FeatureSummary> {
        self.summary.features.iter().find(|f| f.name == name)
    }

    /// Queue the GOAL.md acceptance sequence (mirrors f420_commands::acceptance)
    /// through the same request path as the buttons.
    pub fn start_demo(&mut self, scratch_dir: PathBuf) {
        use DemoStep::*;
        let doc = scratch_dir.join("demo.f420.json");
        let stl = scratch_dir.join("demo.stl");
        let _ = std::fs::create_dir_all(&scratch_dir);
        self.demo = VecDeque::from(vec![
            Cmd(Command::NewDocument),
            Cmd(Command::CreateSketch {
                name: Some("Base sketch".into()),
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: 80.0,
                    height: 50.0,
                    center: None,
                },
            }),
            ExtrudeOf {
                name: "Plate",
                sketch: "Base sketch",
                distance: 10.0,
            },
            CircleOnCapOf {
                name: "Hole sketch",
                extrude: "Plate",
                diameter: 10.0,
            },
            CutOf {
                name: "Hole",
                sketch: "Hole sketch",
            },
            Set {
                feature: "Base sketch",
                param: "width",
                value: 100.0,
            },
            Set {
                feature: "Plate",
                param: "distance",
                value: 12.0,
            },
            Set {
                feature: "Plate",
                param: "distance",
                value: 10.0,
            },
            Cmd(Command::Undo),
            Cmd(Command::Undo),
            Cmd(Command::Undo),
            Cmd(Command::Redo),
            Cmd(Command::Redo),
            Cmd(Command::Redo),
            Cmd(Command::Save { path: doc.clone() }),
            Cmd(Command::Close),
            Cmd(Command::Open { path: doc }),
            Cmd(Command::ExportStl {
                path: stl,
                chord: Some(0.01),
            }),
            Set {
                feature: "Plate",
                param: "distance",
                value: -10.0,
            },
            Select("Base sketch"),
        ]);
    }

    /// Queue a scripted interactive placement: plate, then an off-centre hole
    /// placed with synthetic pointer events through [`Self::place_pointer`].
    /// With `stop_mid_drag` the script ends while the drag (and its preview)
    /// is still active.
    pub fn start_demo_place(&mut self, stop_mid_drag: bool) {
        use DemoStep::*;
        let z = 10.0;
        let mut steps = vec![
            Cmd(Command::NewDocument),
            Cmd(Command::CreateSketch {
                name: Some("Base sketch".into()),
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: 80.0,
                    height: 50.0,
                    center: None,
                },
            }),
            ExtrudeOf {
                name: "Plate",
                sketch: "Base sketch",
                distance: z,
            },
            PlaceHoleOn("Plate"),
            Pointer(Phase::Press, [20.3, 7.8, z]),
            Pointer(Phase::Move, [23.0, 8.0, z]),
            Pointer(Phase::Move, [26.2, 8.1, z]),
        ];
        if !stop_mid_drag {
            steps.extend([
                Pointer(Phase::Release, [26.2, 8.1, z]),
                DialogOk,
                CutLatest("Hole"),
                SelectLatestSketch,
            ]);
        }
        self.demo = VecDeque::from(steps);
    }

    // ----- placement -----

    pub fn placement(&self) -> Option<&Placement> {
        self.placement.as_ref()
    }

    /// Activate a placement tool (replaces any active one).
    pub fn start_tool(&mut self, tool: Tool) {
        match Placement::for_document(tool, &self.summary.features) {
            Ok(p) => self.placement = Some(p),
            Err(e) => {
                self.placement = None;
                self.diag(true, "Input", format!("cannot place: {e}"));
            }
        }
    }

    pub fn cancel_tool(&mut self) {
        self.placement = None;
    }

    /// Camera and view size (points) used to turn pointer positions into rays.
    pub fn view_camera(&self) -> (Camera, [f32; 2]) {
        if let Some(vp) = &self.viewport
            && vp.last_size()[0] > 0.0
        {
            return (vp.camera.clone(), vp.last_size());
        }
        let size = self
            .view_rect
            .map_or(FALLBACK_VIEW_SIZE, |r| [r.width(), r.height()]);
        (self.fallback_camera.clone(), size)
    }

    /// Screen rect of the 3D view as of the last frame.
    pub fn view_rect(&self) -> Option<egui::Rect> {
        self.view_rect
    }

    /// Absolute screen position of a world point in the 3D view (tests, demo).
    pub fn world_to_screen(&self, world: [f64; 3]) -> Option<egui::Pos2> {
        let (cam, [w, h]) = self.view_camera();
        let [x, y] = cam.project(world.map(|v| v as f32), w, h)?;
        let min = self.view_rect.map_or(egui::Pos2::ZERO, |r| r.min);
        Some(min + egui::vec2(x, y))
    }

    /// Feed a pointer event (position relative to the view's top-left, in
    /// points) to the active tool. Shared by mouse handling, the demo script
    /// and tests. A finished drag opens the matching pre-filled dialog.
    pub fn place_pointer(&mut self, phase: Phase, local: [f32; 2], snapping: bool) {
        let (cam, [w, h]) = self.view_camera();
        let Some(p) = self.placement.as_mut() else {
            return;
        };
        let Some(ray) = cam.ray(local[0], local[1], w, h) else {
            return;
        };
        match p.pointer(phase, &ray, snapping) {
            None => {}
            Some(Err(e)) => self.diag(true, "Input", e),
            Some(Ok(prop)) => {
                self.dialog = Some(match prop {
                    Proposal::Rectangle {
                        width,
                        height,
                        center,
                    } => Dialog::Rectangle {
                        width: place::fmt(width),
                        height: place::fmt(height),
                        cx: place::fmt(center[0]),
                        cy: place::fmt(center[1]),
                    },
                    Proposal::Circle {
                        extrude,
                        diameter,
                        center,
                    } => Dialog::CircleOnFace {
                        extrude: Some(extrude),
                        diameter: place::fmt(diameter),
                        cx: place::fmt(center[0]),
                        cy: place::fmt(center[1]),
                    },
                });
                self.held_preview = self.placement.take();
            }
        }
    }

    fn placement_hint(&self) -> Option<String> {
        let p = self.placement.as_ref()?;
        Some(match p.tool {
            Tool::Rectangle => {
                "Place rectangle: drag corner to corner on world XY · Shift = no snap · Esc cancels"
                    .into()
            }
            Tool::Hole { extrude } => format!(
                "Place hole on {}.EndCap: press at centre, drag to set size · Shift = no snap · Esc cancels",
                self.feature_name(extrude)
            ),
        })
    }

    fn placement_overlay(&self) -> Overlay {
        let mut ov = Overlay::default();
        let (p, live) = match (&self.placement, &self.held_preview) {
            (Some(p), _) => (p, true),
            (None, Some(p)) => (p, false),
            _ => return ov,
        };
        let Some(pv) = p.preview() else {
            return ov;
        };
        let color = if pv.plausible {
            egui::Color32::from_rgb(255, 205, 60)
        } else {
            egui::Color32::from_rgb(240, 70, 70)
        };
        let f = |q: [f64; 3]| q.map(|v| v as f32);
        ov.polylines.push(OverlayPolyline {
            points: pv.outline.iter().map(|q| f(*q)).collect(),
            closed: true,
            color,
            width: 2.0,
            dashed: live,
        });
        for g in &pv.guides {
            ov.polylines.push(OverlayPolyline {
                points: vec![f(g[0]), f(g[1])],
                closed: false,
                color,
                width: 1.0,
                dashed: false,
            });
        }
        let mut text = pv.label;
        if !pv.plausible {
            text += "  (outside base: will be rejected)";
        }
        ov.labels.push(OverlayLabel {
            at: f(pv.label_at),
            text,
            color,
        });
        ov
    }

    // ----- request path -----

    /// Send a document command to the engine (the only way UI mutates state).
    pub fn request(&mut self, cmd: Command) {
        if !self.engine.submit(cmd) {
            self.diag(
                true,
                "Worker",
                "session worker thread is not running".into(),
            );
        }
    }

    fn diag(&mut self, error: bool, kind: &str, text: String) {
        self.diagnostics.push(Diagnostic {
            error,
            kind: kind.into(),
            text,
        });
        if self.diagnostics.len() > MAX_DIAGNOSTICS {
            self.diagnostics.remove(0);
        }
    }

    fn handle_reply(&mut self, r: Reply) {
        self.replies_seen += 1;
        let Reply {
            command,
            response,
            summary,
            measurements,
            mesh,
        } = r;
        if response.ok {
            let mut text = format!("{} ok", response.command);
            if let Some(f) = response.feature {
                text += &format!(" ({f})");
            }
            if let Some(m) = &response.message {
                text += &format!(": {m}");
            }
            self.diag(false, "info", text);
        } else {
            let (kind, msg) = match &response.error {
                Some(e) => (error_kind(e), e.to_string()),
                None => ("Error", "command failed".to_string()),
            };
            self.diag(true, kind, format!("{} rejected: {msg}", response.command));
        }
        if matches!(command, Command::ExportStl { .. }) {
            self.last_export = response.mesh_check.clone();
        }
        if matches!(
            command,
            Command::NewDocument | Command::Open { .. } | Command::Close
        ) && response.ok
        {
            self.fitted = false;
            self.selected = None;
        }
        self.summary = summary;
        self.measurements = measurements;
        if let Some(sel) = self.selected
            && !self.summary.features.iter().any(|f| f.id == sel)
        {
            self.selected = None;
        }
        if let (Some(f), true) = (response.feature, response.ok)
            && !matches!(command, Command::SetParameter { .. })
        {
            self.selected = Some(f);
        }
        // Re-derive the placement plane from the new document state.
        if let Some(p) = &self.placement
            && !p.dragging()
        {
            let tool = p.tool;
            self.start_tool(tool);
        }
        // Rebuild edit buffers from the document: committed edits show the new
        // value, rejected edits revert to the document value.
        self.reset_inspector();
        match mesh {
            MeshUpdate::Unchanged => {}
            MeshUpdate::Failed(e) => self.diag(true, "Display", format!("tessellation: {e}")),
            MeshUpdate::Replace(m) => {
                self.mesh_triangles = m.triangle_count();
                if let Some(vp) = self.viewport.as_mut() {
                    match vp.set_mesh(&m) {
                        Ok(()) => {
                            if !self.fitted && m.triangle_count() > 0 {
                                vp.fit();
                                self.fitted = true;
                            }
                        }
                        Err(e) => {
                            let t = format!("display mesh rejected: {e}");
                            self.diag(true, "Display", t);
                        }
                    }
                }
            }
        }
    }

    fn reset_inspector(&mut self) {
        self.inspector = self.selected.and_then(|id| {
            let f = self.summary.features.iter().find(|f| f.id == id)?;
            Some(Inspector {
                feature: id,
                fields: f
                    .parameters
                    .iter()
                    .map(|(n, v)| (n.clone(), *v, fmt_num(*v)))
                    .collect(),
            })
        });
    }

    fn select(&mut self, id: Option<FeatureId>) {
        if self.selected != id {
            self.selected = id;
            self.reset_inspector();
        }
    }

    fn advance_demo(&mut self) {
        if self.engine.busy() {
            return;
        }
        let Some(step) = self.demo.pop_front() else {
            return;
        };
        let id = |n: &str| self.feature_by_name(n).map(|f| f.id);
        let cmd = match step.clone() {
            DemoStep::Cmd(c) => Some(c),
            DemoStep::ExtrudeOf {
                name,
                sketch,
                distance,
            } => id(sketch).map(|s| Command::Extrude {
                name: Some(name.into()),
                sketch: s,
                distance,
            }),
            DemoStep::CircleOnCapOf {
                name,
                extrude,
                diameter,
            } => id(extrude).map(|e| Command::CreateSketch {
                name: Some(name.into()),
                plane: end_cap_of(e),
                template: SketchTemplate::CenteredCircle {
                    diameter,
                    center: None,
                },
            }),
            DemoStep::CutOf { name, sketch } => id(sketch).map(|s| Command::CutThroughAll {
                name: Some(name.into()),
                sketch: s,
            }),
            DemoStep::Set {
                feature,
                param,
                value,
            } => id(feature).map(|f| Command::SetParameter {
                feature: f,
                name: param.into(),
                value,
            }),
            DemoStep::Select(n) => {
                let f = id(n);
                self.select(f);
                return;
            }
            DemoStep::PlaceHoleOn(n) => match id(n) {
                Some(e) => {
                    self.start_tool(Tool::Hole { extrude: e });
                    return;
                }
                None => None,
            },
            DemoStep::Pointer(phase, world) => {
                // Wait for the first laid-out frame of the real viewport.
                if self
                    .viewport
                    .as_ref()
                    .is_some_and(|v| v.last_size()[0] <= 0.0)
                    || self.view_rect.is_none()
                {
                    self.demo.push_front(step);
                    return;
                }
                let (cam, [w, h]) = self.view_camera();
                match cam.project(world.map(|v| v as f32), w, h) {
                    Some(local) => {
                        self.place_pointer(phase, local, true);
                        return;
                    }
                    None => None,
                }
            }
            DemoStep::DialogOk => match self.dialog.take().map(|d| build_command(&d)) {
                Some(Ok(c)) => Some(c),
                Some(Err(e)) => {
                    self.diag(true, "Input", e);
                    None
                }
                None => None,
            },
            DemoStep::CutLatest(name) => self.unused_sketch().map(|s| Command::CutThroughAll {
                name: Some(name.into()),
                sketch: s,
            }),
            DemoStep::SelectLatestSketch => {
                let f = self
                    .summary
                    .features
                    .iter()
                    .rev()
                    .find(|f| f.kind == "sketch")
                    .map(|f| f.id);
                self.select(f);
                return;
            }
        };
        match cmd {
            Some(c) => self.request(c),
            None => {
                self.diag(
                    true,
                    "Demo",
                    "demo step references a missing feature; stopping".into(),
                );
                self.demo.clear();
            }
        }
    }

    // ----- UI -----

    /// Whole-application UI. Works with or without a GPU viewport.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        while let Some(r) = self.engine.poll() {
            self.handle_reply(r);
        }
        self.advance_demo();
        if self.engine.mode() == ExecMode::Sync && !self.demo.is_empty() {
            ctx.request_repaint();
        }
        if self.engine.worker_lost() && !self.diagnostics.iter().any(|d| d.kind == "Worker") {
            self.diag(true, "Worker", "session worker thread stopped".into());
        }
        let busy = self.engine.busy();

        egui::Panel::top("f420_toolbar").show(ui, |ui| self.toolbar(ui, busy));
        egui::Panel::bottom("f420_status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("f420_browser")
            .resizable(true)
            .default_size(330.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.history(ui);
                    ui.separator();
                    self.inspector_ui(ui, busy);
                    ui.separator();
                    self.diagnostics_ui(ui);
                });
            });
        if self.placement.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel_tool();
        }
        let tool_active = self.placement.is_some();
        let mut events: Vec<(Phase, egui::Pos2)> = vec![];
        let mut snapping = true;
        let mut layer = None;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let resp = match self.viewport.as_mut() {
                Some(vp) => {
                    vp.set_primary_drag_navigates(!tool_active);
                    let r = vp.ui(ui);
                    let h = vp.hovered_group();
                    vp.set_highlight(h);
                    r
                }
                None => {
                    let (rect, r) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!(
                            "3D view unavailable (no wgpu renderer). Display mesh: {} triangles",
                            self.mesh_triangles
                        ),
                        egui::FontId::proportional(14.0),
                        ui.visuals().text_color(),
                    );
                    r
                }
            };
            self.view_rect = Some(resp.rect);
            layer = Some(ui.layer_id());
            if tool_active {
                let prim = egui::PointerButton::Primary;
                let (shift, origin, latest) = ui.input(|i| {
                    (
                        i.modifiers.shift,
                        i.pointer.press_origin(),
                        i.pointer.latest_pos(),
                    )
                });
                snapping = !shift;
                if resp.drag_started_by(prim)
                    && let Some(o) = origin
                {
                    events.push((Phase::Press, o));
                }
                if (resp.dragged_by(prim) || resp.drag_stopped_by(prim))
                    && let Some(p) = latest
                {
                    events.push((Phase::Move, p));
                }
                if resp.drag_stopped_by(prim)
                    && let Some(p) = latest
                {
                    events.push((Phase::Release, p));
                }
            }
        });
        if let Some(rect) = self.view_rect {
            for (phase, pos) in events {
                let l = pos - rect.min;
                self.place_pointer(phase, [l.x, l.y], snapping);
            }
        }
        self.dialog_ui(&ctx, busy);
        if !matches!(
            self.dialog,
            Some(Dialog::Rectangle { .. } | Dialog::CircleOnFace { .. })
        ) {
            self.held_preview = None;
        }
        if let (Some(rect), Some(layer)) = (self.view_rect, layer) {
            let painter = ctx.layer_painter(layer).with_clip_rect(rect);
            let (cam, _) = self.view_camera();
            f420_viewport::paint_overlay(&painter, &cam, rect, &self.placement_overlay());
            if let Some(hint) = self.placement_hint() {
                let pos = rect.min + egui::vec2(10.0, 10.0);
                let galley = painter.layout_no_wrap(
                    hint,
                    egui::FontId::proportional(14.0),
                    egui::Color32::from_rgb(255, 205, 60),
                );
                painter.rect_filled(
                    egui::Rect::from_min_size(pos, galley.size()).expand(4.0),
                    3.0,
                    egui::Color32::from_black_alpha(170),
                );
                painter.galley(pos, galley, egui::Color32::WHITE);
            }
        }
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, busy: bool) {
        let open = self.summary.open;
        let idle = !busy;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Fission420");
            ui.separator();
            if ui.add_enabled(idle, egui::Button::new("New")).clicked() {
                self.request(Command::NewDocument);
            }
            if ui.add_enabled(idle, egui::Button::new("Open…")).clicked() {
                self.dialog = Some(Dialog::Open {
                    path: self.default_path("f420.json"),
                });
            }
            if ui
                .add_enabled(idle && open, egui::Button::new("Save"))
                .clicked()
            {
                match self.summary.path.clone() {
                    Some(path) => self.request(Command::Save { path }),
                    None => {
                        self.dialog = Some(Dialog::SaveAs {
                            path: self.default_path("f420.json"),
                        })
                    }
                }
            }
            if ui
                .add_enabled(idle && open, egui::Button::new("Save As…"))
                .clicked()
            {
                self.dialog = Some(Dialog::SaveAs {
                    path: self.default_path("f420.json"),
                });
            }
            if ui
                .add_enabled(idle && open, egui::Button::new("Close"))
                .clicked()
            {
                self.request(Command::Close);
            }
            ui.separator();
            let undo = match &self.summary.can_undo {
                Some(l) => format!("Undo: {l}"),
                None => "Undo".into(),
            };
            let can_undo = self.summary.can_undo.is_some();
            if ui
                .add_enabled(idle && can_undo, egui::Button::new(undo))
                .clicked()
            {
                self.request(Command::Undo);
            }
            let redo = match &self.summary.can_redo {
                Some(l) => format!("Redo: {l}"),
                None => "Redo".into(),
            };
            let can_redo = self.summary.can_redo.is_some();
            if ui
                .add_enabled(idle && can_redo, egui::Button::new(redo))
                .clicked()
            {
                self.request(Command::Redo);
            }
            ui.separator();
            let has_sketch = self.summary.features.iter().any(|f| f.kind == "sketch");
            let has_extrude = self.summary.features.iter().any(|f| f.kind == "extrude");
            if ui
                .add_enabled(idle && open, egui::Button::new("Rectangle on XY…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Rectangle {
                    width: "80".into(),
                    height: "50".into(),
                    cx: "0".into(),
                    cy: "0".into(),
                });
            }
            if ui
                .add_enabled(idle && has_sketch, egui::Button::new("Extrude…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Extrude {
                    sketch: self.unused_sketch(),
                    distance: "10".into(),
                });
            }
            if ui
                .add_enabled(idle && open, egui::Button::new("Rectangle on XZ…"))
                .clicked()
            {
                self.dialog = Some(Dialog::RectangleXz {
                    width: "20".into(),
                    height: "10".into(),
                    cx: "30".into(),
                    cz: "5".into(),
                });
            }
            if ui
                .add_enabled(idle && has_sketch, egui::Button::new("Revolve…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Revolve {
                    sketch: self.unused_sketch(),
                    angle: "360".into(),
                });
            }
            if ui
                .add_enabled(idle && has_extrude, egui::Button::new("Circle on face…"))
                .clicked()
            {
                let last = self
                    .summary
                    .features
                    .iter()
                    .rev()
                    .find(|f| f.kind == "extrude");
                self.dialog = Some(Dialog::CircleOnFace {
                    extrude: last.map(|f| f.id),
                    diameter: "10".into(),
                    cx: "0".into(),
                    cy: "0".into(),
                });
            }
            if ui
                .add_enabled(idle && has_sketch, egui::Button::new("Cut through all…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Cut {
                    sketch: self.unused_sketch(),
                });
            }
            ui.separator();
            let current = self.placement.as_ref().map(|p| p.tool);
            let rect_on = current == Some(Tool::Rectangle);
            if ui
                .add_enabled(
                    idle && open,
                    egui::Button::new("Place rectangle").selected(rect_on),
                )
                .clicked()
            {
                if rect_on {
                    self.cancel_tool();
                } else {
                    self.start_tool(Tool::Rectangle);
                }
            }
            let hole_on = matches!(current, Some(Tool::Hole { .. }));
            if ui
                .add_enabled(
                    idle && has_extrude,
                    egui::Button::new("Place hole").selected(hole_on),
                )
                .clicked()
            {
                if hole_on {
                    self.cancel_tool();
                } else if let Some(e) = self
                    .summary
                    .features
                    .iter()
                    .rev()
                    .find(|f| f.kind == "extrude")
                {
                    let extrude = e.id;
                    self.start_tool(Tool::Hole { extrude });
                }
            }
            if let Some(Tool::Hole { extrude }) = current {
                let extrudes: Vec<FeatureSummary> = self
                    .summary
                    .features
                    .iter()
                    .filter(|f| f.kind == "extrude")
                    .cloned()
                    .collect();
                let refs: Vec<&FeatureSummary> = extrudes.iter().collect();
                let mut sel = Some(extrude);
                feature_combo(ui, "Hole on end cap of", &mut sel, &refs);
                if let Some(e) = sel
                    && e != extrude
                {
                    self.start_tool(Tool::Hole { extrude: e });
                }
            }
            ui.separator();
            if ui
                .add_enabled(idle && open, egui::Button::new("Export STL…"))
                .clicked()
            {
                self.dialog = Some(Dialog::ExportStl {
                    path: self.default_path("stl"),
                    chord: "0.01".into(),
                });
            }
            if ui
                .add_enabled(idle && open, egui::Button::new("Export STEP…"))
                .clicked()
            {
                self.dialog = Some(Dialog::ExportStep {
                    path: self.default_path("step"),
                });
            }
            if ui.button("Fit view").clicked()
                && let Some(vp) = self.viewport.as_mut()
            {
                vp.fit();
            }
            if busy {
                ui.separator();
                ui.spinner();
                ui.label("Regenerating…");
            }
        });
    }

    fn default_path(&self, ext: &str) -> String {
        match &self.summary.path {
            Some(p) if ext == "f420.json" => p.display().to_string(),
            Some(p) => p.with_extension(ext).display().to_string(),
            None => format!("part.{ext}"),
        }
    }

    /// Last sketch not yet consumed by an extrude or cut.
    fn unused_sketch(&self) -> Option<FeatureId> {
        let fs = &self.summary.features;
        fs.iter()
            .rev()
            .filter(|f| f.kind == "sketch")
            .find(|s| {
                !fs.iter()
                    .any(|o| o.kind != "sketch" && o.depends_on.contains(&s.id))
            })
            .or_else(|| fs.iter().rev().find(|f| f.kind == "sketch"))
            .map(|f| f.id)
    }

    fn feature_name(&self, id: FeatureId) -> String {
        self.summary
            .features
            .iter()
            .find(|f| f.id == id)
            .map_or_else(|| format!("{id}?"), |f| f.name.clone())
    }

    fn plane_text(&self, plane: &SketchPlane) -> String {
        match plane {
            SketchPlane::WorldXY => "on World XY".into(),
            SketchPlane::WorldXZ => "on World XZ".into(),
            SketchPlane::Face(r) => {
                let role = match r.role {
                    FaceRole::ExtrudeEndCap => "EndCap",
                    FaceRole::ExtrudeStartCap => "StartCap",
                };
                format!(
                    "on {}.{role} (semantic: {} {:?})",
                    self.feature_name(r.feature),
                    r.feature,
                    r.role
                )
            }
        }
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        ui.heading("History");
        if !self.summary.open {
            ui.weak("No document open. Use New or Open….");
            return;
        }
        if self.summary.features.is_empty() {
            ui.weak("Empty document. Start with Rectangle on XY….");
        }
        let mut clicked = None;
        for f in &self.summary.features {
            let sel = self.selected == Some(f.id);
            let title = format!("{} {}  [{}]", f.id, f.name, f.kind);
            if ui.selectable_label(sel, title).clicked() {
                clicked = Some(f.id);
            }
            ui.indent(("feat", f.id.0), |ui| {
                if !f.depends_on.is_empty() {
                    let deps: Vec<String> = f
                        .depends_on
                        .iter()
                        .map(|d| format!("{} {}", d, self.feature_name(*d)))
                        .collect();
                    ui.small(format!("uses {}", deps.join(", ")));
                }
                if let Some(p) = &f.plane {
                    ui.small(self.plane_text(p));
                }
                if let Some(st) = f.solve_status {
                    let dof = f.solve_dof.map_or("?".into(), |d| d.to_string());
                    let res = f.solve_residual.map_or("?".into(), |r| format!("{r:.1e}"));
                    ui.small(format!("{st:?} · DOF {dof} · residual {res}"));
                }
            });
        }
        if let Some(id) = clicked {
            self.select(Some(id));
        }
    }

    fn inspector_ui(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading("Inspector");
        let Some(ins) = self.inspector.as_mut() else {
            ui.weak("Select a feature to edit its parameters.");
            return;
        };
        let name = self
            .summary
            .features
            .iter()
            .find(|f| f.id == ins.feature)
            .map_or(String::new(), |f| f.name.clone());
        ui.label(format!("{} {name}", ins.feature));
        if ins.fields.is_empty() {
            ui.weak("No editable parameters.");
        }
        egui::Grid::new("inspector_grid")
            .num_columns(2)
            .show(ui, |ui| {
                for (pname, _, buf) in &mut ins.fields {
                    let unit = if pname.ends_with("_deg") { "deg" } else { "mm" };
                    let l = ui.label(format!("{pname} ({unit})"));
                    ui.add(egui::TextEdit::singleline(buf).desired_width(90.0))
                        .labelled_by(l.id);
                    ui.end_row();
                }
            });
        let modified = ins
            .fields
            .iter()
            .any(|(_, v, b)| b.trim().parse::<f64>().ok() != Some(*v));
        let mut apply = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            apply = ui
                .add_enabled(!busy && modified, egui::Button::new("Apply"))
                .clicked();
            cancel = ui
                .add_enabled(modified, egui::Button::new("Cancel edits"))
                .clicked();
        });
        if modified {
            ui.small("Preview only: not in the document until Apply.");
        }
        if cancel {
            self.reset_inspector();
        } else if apply {
            let ins = ins.clone();
            let mut cmds = vec![];
            let mut bad = vec![];
            for (pname, v, b) in &ins.fields {
                match b.trim().parse::<f64>() {
                    Ok(x) if x != *v => cmds.push(Command::SetParameter {
                        feature: ins.feature,
                        name: pname.clone(),
                        value: x,
                    }),
                    Ok(_) => {}
                    Err(_) => bad.push(format!("{pname}: '{b}' is not a number")),
                }
            }
            if !bad.is_empty() {
                self.diag(true, "Input", bad.join("; "));
                self.reset_inspector();
            }
            for c in cmds {
                self.request(c);
            }
        }
    }

    fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Diagnostics");
            if ui.small_button("Clear").clicked() {
                self.diagnostics.clear();
            }
        });
        if let Some(e) = self.diagnostics.iter().rev().find(|d| d.error) {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Last error [{}]: {}", e.kind, e.text),
            );
        }
        for d in self.diagnostics.iter().rev().take(8) {
            let t = format!("[{}] {}", d.kind, d.text);
            if d.error {
                ui.colored_label(ui.visuals().warn_fg_color, egui::RichText::new(t).small());
            } else {
                ui.small(t);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            match &self.measurements {
                Some(m) => {
                    match &m.volume {
                        VolumeResult::Value {
                            volume,
                            error_estimate,
                            method,
                        } => ui.label(format!(
                            "Volume {volume:.6} mm³ ({method:?}, est. err {error_estimate:.1e})"
                        )),
                        VolumeResult::Unavailable { reason } => {
                            ui.label(format!("Volume unavailable: {reason}"))
                        }
                    };
                    ui.separator();
                    let (a, b) = (m.bounds.min, m.bounds.max);
                    ui.label(format!(
                        "Bounds X[{:.3}, {:.3}] Y[{:.3}, {:.3}] Z[{:.3}, {:.3}]",
                        a[0], b[0], a[1], b[1], a[2], b[2]
                    ));
                    ui.separator();
                    ui.label(format!(
                        "Solids {} · {}",
                        m.shell_count,
                        if m.all_shells_closed {
                            "closed"
                        } else {
                            "OPEN"
                        }
                    ));
                }
                None => {
                    ui.label("No body");
                }
            }
            ui.separator();
            if self.summary.open {
                let path = self
                    .summary
                    .path
                    .as_ref()
                    .map_or("unsaved".into(), |p| p.display().to_string());
                ui.label(format!(
                    "rev {} · {} · {path}",
                    self.summary.revision,
                    if self.summary.dirty {
                        "modified"
                    } else {
                        "saved"
                    }
                ));
            } else {
                ui.label("no document");
            }
            if let Some(vp) = &self.viewport
                && let Some(g) = vp.hovered_group()
            {
                ui.separator();
                ui.label(format!("face {g}"));
            }
        });
    }

    fn dialog_ui(&mut self, ctx: &egui::Context, busy: bool) {
        let Some(mut dlg) = self.dialog.take() else {
            return;
        };
        let features = self.summary.features.clone();
        let sketches: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "sketch").collect();
        let extrudes: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "extrude").collect();
        let mut ok = false;
        let mut cancel = false;
        let mut error: Option<String> = None;
        egui::Window::new(dlg.title())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
                match &mut dlg {
                    Dialog::Rectangle {
                        width,
                        height,
                        cx,
                        cy,
                    } => {
                        num_field(ui, "Rectangle width (mm)", width);
                        num_field(ui, "Rectangle height (mm)", height);
                        num_field(ui, "Rectangle center X (mm)", cx);
                        num_field(ui, "Rectangle center Y (mm)", cy);
                        ui.small("Centre in world XY coordinates.");
                    }
                    Dialog::Extrude { sketch, distance } => {
                        feature_combo(ui, "Profile sketch", sketch, &sketches);
                        num_field(ui, "Extrude distance (mm)", distance);
                    }
                    Dialog::RectangleXz {
                        width,
                        height,
                        cx,
                        cz,
                    } => {
                        num_field(ui, "XZ rectangle width (mm)", width);
                        num_field(ui, "XZ rectangle height (mm)", height);
                        num_field(ui, "XZ rectangle center X (mm)", cx);
                        num_field(ui, "XZ rectangle center Z (mm)", cz);
                        ui.small("Must lie entirely at X > 0 to be revolved about world Z.");
                    }
                    Dialog::Revolve { sketch, angle } => {
                        feature_combo(ui, "Revolve profile sketch", sketch, &sketches);
                        num_field(ui, "Revolve angle (deg)", angle);
                    }
                    Dialog::CircleOnFace {
                        extrude,
                        diameter,
                        cx,
                        cy,
                    } => {
                        feature_combo(ui, "End cap of extrusion", extrude, &extrudes);
                        num_field(ui, "Circle diameter (mm)", diameter);
                        num_field(ui, "Circle center X (mm)", cx);
                        num_field(ui, "Circle center Y (mm)", cy);
                        ui.small("Centre in the end cap's frame (the profile sketch's X/Y).");
                        ui.small("Attached semantically to the extrusion's end cap face.");
                    }
                    Dialog::Cut { sketch } => {
                        feature_combo(ui, "Cut profile sketch", sketch, &sketches);
                    }
                    Dialog::SaveAs { path } | Dialog::Open { path } => {
                        text_field(ui, "File path", path);
                    }
                    Dialog::ExportStl { path, chord } => {
                        text_field(ui, "STL path", path);
                        num_field(ui, "Chord tolerance (mm)", chord);
                        if let Some(mc) = &self.last_export {
                            ui.label(format!(
                                "Last export: watertight {} · {} triangles · mesh volume {:.4} mm³",
                                if mc.watertight { "yes" } else { "NO" },
                                mc.triangles,
                                mc.mesh_volume
                            ));
                        }
                    }
                    Dialog::ExportStep { path } => {
                        text_field(ui, "STEP path", path);
                        ui.label(
                            "Exact B-rep (AP214); refused if an edge cannot be written exactly.",
                        );
                    }
                }
                ui.horizontal(|ui| {
                    ok = ui.add_enabled(!busy, egui::Button::new("OK")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if cancel {
            return; // dialog dropped; nothing created
        }
        if ok {
            match build_command(&dlg) {
                Ok(cmd) => {
                    let keep = matches!(dlg, Dialog::ExportStl { .. });
                    self.request(cmd);
                    if keep {
                        self.dialog = Some(dlg);
                    }
                    return;
                }
                Err(e) => error = Some(e),
            }
        }
        if let Some(e) = error {
            self.diag(true, "Input", e);
        }
        self.dialog = Some(dlg);
    }
}

fn parse_num(label: &str, s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .map_err(|_| format!("{label}: '{s}' is not a number"))
}

fn build_command(d: &Dialog) -> Result<Command, String> {
    let need = |f: &Option<FeatureId>, what: &str| f.ok_or_else(|| format!("choose a {what}"));
    Ok(match d {
        Dialog::Rectangle {
            width,
            height,
            cx,
            cy,
        } => Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: parse_num("width", width)?,
                height: parse_num("height", height)?,
                center: Some([parse_num("center X", cx)?, parse_num("center Y", cy)?]),
            },
        },
        Dialog::RectangleXz {
            width,
            height,
            cx,
            cz,
        } => Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXZ,
            template: SketchTemplate::CenteredRectangle {
                width: parse_num("width", width)?,
                height: parse_num("height", height)?,
                center: Some([parse_num("center X", cx)?, parse_num("center Z", cz)?]),
            },
        },
        Dialog::Revolve { sketch, angle } => Command::Revolve {
            name: None,
            sketch: need(sketch, "profile sketch")?,
            angle_deg: parse_num("angle", angle)?,
        },
        Dialog::Extrude { sketch, distance } => Command::Extrude {
            name: None,
            sketch: need(sketch, "profile sketch")?,
            distance: parse_num("distance", distance)?,
        },
        Dialog::CircleOnFace {
            extrude,
            diameter,
            cx,
            cy,
        } => Command::CreateSketch {
            name: None,
            // Semantic face reference (feature + generated-face role), never an index.
            plane: end_cap_of(need(extrude, "extrusion")?),
            template: SketchTemplate::CenteredCircle {
                diameter: parse_num("diameter", diameter)?,
                center: Some([parse_num("center X", cx)?, parse_num("center Y", cy)?]),
            },
        },
        Dialog::Cut { sketch } => Command::CutThroughAll {
            name: None,
            sketch: need(sketch, "profile sketch")?,
        },
        Dialog::SaveAs { path } => Command::Save {
            path: nonempty_path(path)?,
        },
        Dialog::Open { path } => Command::Open {
            path: nonempty_path(path)?,
        },
        Dialog::ExportStl { path, chord } => Command::ExportStl {
            path: nonempty_path(path)?,
            chord: Some(parse_num("chord", chord)?),
        },
        Dialog::ExportStep { path } => Command::ExportStep {
            path: nonempty_path(path)?,
        },
    })
}

fn nonempty_path(s: &str) -> Result<PathBuf, String> {
    let t = s.trim();
    if t.is_empty() {
        Err("path is empty".into())
    } else {
        Ok(PathBuf::from(t))
    }
}

fn num_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        ui.add(egui::TextEdit::singleline(buf).desired_width(90.0))
            .labelled_by(l.id);
    });
}

fn text_field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        ui.add(egui::TextEdit::singleline(buf).desired_width(320.0))
            .labelled_by(l.id);
    });
}

fn feature_combo(
    ui: &mut egui::Ui,
    label: &str,
    sel: &mut Option<FeatureId>,
    options: &[&FeatureSummary],
) {
    let text = sel
        .and_then(|id| options.iter().find(|f| f.id == id))
        .map_or("(choose)".to_string(), |f| format!("{} {}", f.id, f.name));
    egui::ComboBox::from_label(label)
        .selected_text(text)
        .show_ui(ui, |ui| {
            for f in options {
                ui.selectable_value(sel, Some(f.id), format!("{}: {}", f.id, f.name));
            }
        });
}

/// Variant name of a document error, for the diagnostics area.
pub fn error_kind(e: &DocError) -> &'static str {
    match e {
        DocError::Validation(_) => "Validation",
        DocError::UnknownFeature(_) => "UnknownFeature",
        DocError::UnknownParameter { .. } => "UnknownParameter",
        DocError::BrokenReference { .. } => "BrokenReference",
        DocError::Cycle(_) => "Cycle",
        DocError::SketchSolve { .. } => "SketchSolve",
        DocError::ProductLimit(_) => "ProductLimit",
        DocError::Kernel { .. } => "Kernel",
        DocError::NothingTo(_) => "NothingTo",
        DocError::File(_) => "File",
    }
}

/// Shortest representation that round-trips (so unchanged fields compare equal).
fn fmt_num(v: f64) -> String {
    format!("{v}")
}
