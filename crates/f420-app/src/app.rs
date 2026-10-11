//! The Fission420 UI. Widgets hold only transient edit buffers; the document
//! state shown here is always the last [`DocSummary`] reported by the engine.
//!
//! Layout: command bar (top), browser/feature tree (left), contextual inspector
//! (right), status bar (bottom), 3D view (centre). See [`crate::panels`] for the
//! panels and [`crate::view3d`] for the 3D view.
//!
//! Modes: normal modelling, placement tools (template drag, see
//! [`crate::place`]), and sketch editing ([`SketchSession`]). The sketch editor
//! owns the primary mouse button in the 3D view; camera navigation stays on
//! MMB/RMB/scroll so drawing and camera drags never compete.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use f420_commands::{Command, DocSummary, FeatureSummary, SketchTemplate, end_cap_of};
use f420_document::{DocError, FaceRole, FeatureId, SketchPlane};
use f420_kernel::{Measurements, MeshCheck, VolumeResult};
use f420_viewport::{Camera, RenderMesh, Viewport};

use crate::engine::{Engine, ExecMode, MeshUpdate, Reply};
use crate::place::{self, Phase, Placement, PlaneFrame, Proposal, Tool};
use crate::sketch_edit::SketchEditor;
use crate::view3d::FaceInfo;

/// View size assumed for placement when no viewport has been laid out yet.
pub(crate) const FALLBACK_VIEW_SIZE: [f32; 2] = [800.0, 600.0];

/// Max diagnostics kept in the messages panel.
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
pub(crate) enum Dialog {
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
    pub(crate) fn title(&self) -> &'static str {
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
pub(crate) struct Inspector {
    pub feature: FeatureId,
    pub fields: Vec<(String, f64, String)>, // (name, document value, edit buffer)
}

/// Where a sketch edit session will be committed.
#[derive(Debug, Clone, PartialEq)]
pub enum SketchTarget {
    New { plane: SketchPlane },
    Existing(FeatureId),
}

/// An open sketch edit session (UI-local until Finish Sketch).
pub struct SketchSession {
    pub editor: SketchEditor,
    pub target: SketchTarget,
    pub frame: PlaneFrame,
    pub title: String,
    /// Document this session belongs to (None: adopt the next document opened).
    pub doc_id: Option<u64>,
    /// Camera before entering the sketch (restored on exit).
    pub saved_camera: Option<Camera>,
    /// Finish was pressed; waiting for the commit reply.
    pub committing: bool,
}

/// Actions that discard the open document and therefore need confirmation
/// when there are unsaved changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Guarded {
    New,
    /// New document followed by a sketch on XY (empty-state "Create Sketch").
    NewAndSketch,
    Open(PathBuf),
    Close,
    Exit,
}

/// Inline editor for a dimension of a finished sketch shown in the 3D view.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DocDimEdit {
    pub feature: FeatureId,
    pub name: String,
    pub buffer: String,
    pub constraint: usize,
    pub focus: bool,
}

/// Answer to the unsaved-changes confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    Save,
    Discard,
    Cancel,
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
    pub(crate) engine: Engine,
    pub(crate) summary: DocSummary,
    pub(crate) measurements: Option<Measurements>,
    pub(crate) selected: Option<FeatureId>,
    /// Selected face (display group id) in the 3D view.
    pub(crate) selected_face: Option<u32>,
    pub(crate) inspector: Option<Inspector>,
    pub(crate) dialog: Option<Dialog>,
    /// Title of the dialog shown last frame (detects a newly opened dialog).
    pub(crate) last_dialog: Option<&'static str>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Latest error shown as a banner over the 3D view until dismissed.
    pub(crate) banner: Option<Diagnostic>,
    pub(crate) last_export: Option<MeshCheck>,
    pub viewport: Option<Viewport>,
    pub(crate) mesh_triangles: usize,
    /// Last display mesh (face analysis for selection; display only).
    pub(crate) display_mesh: RenderMesh,
    pub(crate) faces: Vec<FaceInfo>,
    pub(crate) fitted: bool,
    demo: VecDeque<DemoStep>,
    replies_seen: u64,
    /// Active placement tool (rays from the view camera feed it).
    pub(crate) placement: Option<Placement>,
    /// Finished placement whose pre-filled dialog is open; drawn until it closes.
    pub(crate) held_preview: Option<Placement>,
    /// Camera used when there is no GPU viewport (tests).
    pub fallback_camera: Camera,
    pub(crate) view_rect: Option<egui::Rect>,
    pub(crate) sketch: Option<SketchSession>,
    pub(crate) confirm: Option<Guarded>,
    /// Guarded action to run after the pending save succeeds.
    pub(crate) after_save: Option<Guarded>,
    pub(crate) exit_confirmed: bool,
    pub(crate) rename: Option<(FeatureId, String)>,
    pub(crate) doc_dim_edit: Option<DocDimEdit>,
    /// Screen rects of the selected finished sketch's dimension labels.
    pub(crate) doc_dim_targets: Vec<(usize, egui::Rect)>,
    /// Constraint whose inline sketch-dimension editor was open last frame
    /// (a newly opened editor selects its text so typing replaces it).
    pub(crate) sketch_dim_opened: Option<usize>,
    pub(crate) show_help: bool,
    /// Commands in flight with their submit time (FIFO; latency measurement).
    in_flight: VecDeque<(String, Instant)>,
    /// Last command round trips (name, latency) for the status bar / report.
    pub(crate) latencies: VecDeque<(String, Duration)>,
    /// Exponential moving average of UI CPU time per frame (`ui()` duration;
    /// egui repaints on demand, so wall-clock frame intervals are meaningless).
    pub(crate) frame_ema: Option<Duration>,
    /// Sketch to start once the requested new document exists.
    pending_new_sketch: bool,
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
            selected_face: None,
            inspector: None,
            dialog: None,
            last_dialog: None,
            diagnostics: vec![],
            banner: None,
            last_export: None,
            viewport: None,
            mesh_triangles: 0,
            display_mesh: RenderMesh::default(),
            faces: vec![],
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
            sketch: None,
            confirm: None,
            after_save: None,
            exit_confirmed: false,
            rename: None,
            doc_dim_edit: None,
            doc_dim_targets: vec![],
            sketch_dim_opened: None,
            show_help: false,
            in_flight: VecDeque::new(),
            latencies: VecDeque::new(),
            frame_ema: None,
            pending_new_sketch: false,
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
    pub fn selected(&self) -> Option<FeatureId> {
        self.selected
    }
    pub fn selected_face(&self) -> Option<&FaceInfo> {
        let g = self.selected_face?;
        self.faces.iter().find(|f| f.group == g)
    }
    /// The open sketch edit session, if any.
    pub fn sketch_session(&self) -> Option<&SketchSession> {
        self.sketch.as_ref()
    }
    pub fn sketch_session_mut(&mut self) -> Option<&mut SketchSession> {
        self.sketch.as_mut()
    }
    pub fn pending_confirmation(&self) -> Option<&Guarded> {
        self.confirm.as_ref()
    }
    pub fn command_latencies(&self) -> impl Iterator<Item = &(String, Duration)> {
        self.latencies.iter()
    }
    pub fn frame_time(&self) -> Option<Duration> {
        self.frame_ema
    }
    /// Analysed faces of the current display mesh.
    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }
    /// Unsaved changes in the document or in an open sketch session.
    pub fn has_unsaved_changes(&self) -> bool {
        (self.summary.open && self.summary.dirty)
            || self.sketch.as_ref().is_some_and(|s| s.editor.modified())
    }

    /// Window title with the document name and a dirty marker.
    pub fn window_title(&self) -> String {
        if !self.summary.open {
            return "Fission420".into();
        }
        let name = self
            .summary
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
        format!(
            "{}{name} — Fission420",
            if self.summary.dirty { "● " } else { "" }
        )
    }

    /// Called when the window manager asks to close. Returns true when the
    /// window may close now; otherwise a confirmation is shown.
    pub fn allow_close(&mut self) -> bool {
        if self.exit_confirmed || !self.has_unsaved_changes() {
            return true;
        }
        self.confirm = Some(Guarded::Exit);
        false
    }

    /// The user confirmed that the window may close (Discard on Exit).
    pub fn exit_confirmed(&self) -> bool {
        self.exit_confirmed
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

    // ----- placement (template drag tools) -----

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

    pub(crate) fn camera_mut(&mut self) -> &mut Camera {
        match self.viewport.as_mut() {
            Some(vp) => &mut vp.camera,
            None => &mut self.fallback_camera,
        }
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

    /// Absolute screen position of a sketch-plane point of the open sketch session.
    pub fn sketch_to_screen(&self, p: [f64; 2]) -> Option<egui::Pos2> {
        let s = self.sketch.as_ref()?;
        self.world_to_screen(s.frame.to_world(p))
    }

    /// Feed a pointer event (position relative to the view's top-left, in
    /// points) to the active placement tool. Shared by mouse handling, the
    /// demo script and tests. A finished drag opens the matching pre-filled dialog.
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

    // ----- request path -----

    /// Send a document command to the engine (the only way UI mutates state).
    pub fn request(&mut self, cmd: Command) {
        let name = cmd.name().to_string();
        if self.engine.submit(cmd) {
            self.in_flight.push_back((name, Instant::now()));
        } else {
            self.diag(
                true,
                "Worker",
                "session worker thread is not running; restart the app".into(),
            );
        }
    }

    pub(crate) fn diag(&mut self, error: bool, kind: &str, text: String) {
        let d = Diagnostic {
            error,
            kind: kind.into(),
            text,
        };
        if error {
            self.banner = Some(d.clone());
        }
        self.diagnostics.push(d);
        if self.diagnostics.len() > MAX_DIAGNOSTICS {
            self.diagnostics.remove(0);
        }
    }

    fn handle_reply(&mut self, r: Reply) {
        self.replies_seen += 1;
        if let Some((name, t0)) = self.in_flight.pop_front() {
            self.latencies.push_back((name, t0.elapsed()));
            if self.latencies.len() > 20 {
                self.latencies.pop_front();
            }
        }
        let Reply {
            command,
            response,
            summary,
            measurements,
            mesh,
            ..
        } = r;
        let doc_changed = summary.doc_id != self.summary.doc_id;
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
            let hint = response.error.as_ref().map(next_step).unwrap_or_default();
            self.diag(
                true,
                kind,
                format!("{} rejected: {msg}{hint}", response.command),
            );
        }
        if matches!(command, Command::ExportStl { .. }) {
            self.last_export = response.mesh_check.clone();
        }
        if doc_changed {
            self.fitted = false;
            self.selected = None;
            self.selected_face = None;
            self.doc_dim_edit = None;
            self.rename = None;
        }
        // A sketch session belongs to one document; a different document
        // (Open/New/Close completed) ends it without committing.
        if doc_changed && let Some(s) = &mut self.sketch {
            match s.doc_id {
                None if summary.open => s.doc_id = Some(summary.doc_id),
                Some(id) if id != summary.doc_id => {
                    self.sketch = None;
                    self.diag(
                        true,
                        "Sketch",
                        "the document changed while a sketch was open; the sketch edit was discarded"
                            .into(),
                    );
                }
                _ => {}
            }
        }
        let edited_param = match &command {
            Command::SetParameter { feature, name, .. } => Some((*feature, name.clone())),
            _ => None,
        };
        self.summary = summary;
        self.measurements = measurements;
        if let Some(sel) = self.selected
            && !self.summary.features.iter().any(|f| f.id == sel)
        {
            self.selected = None;
        }
        // Sketch commit replies close (or keep, on failure) the session.
        let commit_reply = matches!(
            command,
            Command::EditSketch { .. }
                | Command::CreateSketch {
                    template: SketchTemplate::Custom { .. },
                    ..
                }
        );
        if commit_reply && self.sketch.as_ref().is_some_and(|s| s.committing) {
            if response.ok {
                self.end_sketch_session();
                if let Some(f) = response.feature {
                    self.selected = Some(f);
                    self.selected_face = None;
                }
            } else if let Some(s) = &mut self.sketch {
                s.committing = false;
                s.editor.feedback = Some(crate::sketch_edit::Feedback {
                    error: true,
                    text: format!(
                        "Finish failed, the document is unchanged: {}",
                        response
                            .error
                            .as_ref()
                            .map_or(String::new(), |e| e.to_string())
                    ),
                });
            }
        } else if let (Some(f), true) = (response.feature, response.ok)
            && !matches!(
                command,
                Command::SetParameter { .. } | Command::RenameFeature { .. }
            )
        {
            self.selected = Some(f);
            self.selected_face = None;
        }
        // Continue a guarded action once its pre-save succeeded.
        if matches!(command, Command::Save { .. })
            && let Some(g) = self.after_save.take()
        {
            if response.ok {
                self.run_guarded(g);
            } else {
                self.diag(
                    true,
                    "File",
                    "save failed, so the pending action was cancelled; the document is still open and unsaved"
                        .into(),
                );
            }
        }
        if self.pending_new_sketch && self.summary.open && matches!(command, Command::NewDocument) {
            self.pending_new_sketch = false;
            self.begin_sketch(SketchTarget::New {
                plane: SketchPlane::WorldXY,
            });
        }
        // Re-derive the placement plane from the new document state.
        if let Some(p) = &self.placement
            && !p.dragging()
        {
            let tool = p.tool;
            self.start_tool(tool);
        }
        self.refresh_inspector(edited_param.as_ref());
        match mesh {
            MeshUpdate::Unchanged => {}
            MeshUpdate::Failed(e) => self.diag(true, "Display", format!("tessellation: {e}")),
            MeshUpdate::Replace(m) => {
                self.mesh_triangles = m.triangle_count();
                self.selected_face = None;
                if let Some(vp) = self.viewport.as_mut() {
                    match vp.set_mesh(&m) {
                        Ok(()) => {
                            if !self.fitted && m.triangle_count() > 0 && self.sketch.is_none() {
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
                self.display_mesh = m;
            }
        }
        self.faces = crate::view3d::analyse_faces(&self.display_mesh, &self.summary.features);
        self.sync_viewport_selection();
    }

    /// Rebuild inspector buffers from the document while keeping the user's
    /// un-applied edits. A reply to a parameter edit resets that field, so a
    /// committed edit shows the new value and a rejected one reverts.
    fn refresh_inspector(&mut self, edited: Option<&(FeatureId, String)>) {
        let Some(id) = self.selected else {
            self.inspector = None;
            return;
        };
        let Some(f) = self.summary.features.iter().find(|f| f.id == id) else {
            self.inspector = None;
            return;
        };
        let old = self.inspector.take().filter(|i| i.feature == id);
        let fields = f
            .parameters
            .iter()
            .map(|(n, v)| {
                let keep = old.as_ref().and_then(|o| {
                    o.fields.iter().find(|(on, ov, ob)| {
                        on == n
                            && ob.trim().parse::<f64>().ok() != Some(*ov)
                            && edited != Some(&(id, n.clone()))
                    })
                });
                let buf = keep.map_or_else(|| fmt_num(*v), |(_, _, b)| b.clone());
                (n.clone(), *v, buf)
            })
            .collect();
        self.inspector = Some(Inspector {
            feature: id,
            fields,
        });
    }

    pub(crate) fn reset_inspector(&mut self) {
        self.inspector = None;
        self.refresh_inspector(None);
    }

    pub(crate) fn select(&mut self, id: Option<FeatureId>) {
        if self.selected != id || self.selected_face.is_some() {
            self.selected = id;
            self.selected_face = None;
            self.doc_dim_edit = None;
            self.reset_inspector();
            self.sync_viewport_selection();
        }
    }

    /// Select a face of the display mesh (and its owning feature, when known).
    pub fn select_face(&mut self, group: Option<u32>) {
        self.doc_dim_edit = None;
        match group.and_then(|g| self.faces.iter().find(|f| f.group == g).cloned()) {
            Some(info) => {
                self.selected = info.owner;
                self.selected_face = Some(info.group);
                self.reset_inspector();
            }
            None => {
                self.selected = None;
                self.selected_face = None;
                self.inspector = None;
            }
        }
        self.sync_viewport_selection();
    }

    /// Groups highlighted for the current selection: the picked face, or every
    /// face whose provenance is reliably the selected feature.
    pub fn highlighted_faces(&self) -> Vec<u32> {
        match (self.selected_face, self.selected) {
            (Some(g), _) => vec![g],
            (None, Some(id)) => self
                .faces
                .iter()
                .filter(|f| f.owner == Some(id))
                .map(|f| f.group)
                .collect(),
            _ => vec![],
        }
    }

    /// Push the selection highlight to the GPU view.
    pub(crate) fn sync_viewport_selection(&mut self) {
        let groups = self.highlighted_faces();
        if let Some(vp) = self.viewport.as_mut() {
            vp.set_selection(&groups);
        }
    }

    // ----- guarded document actions -----

    /// Run `g` now, or ask first when there are unsaved changes.
    pub fn guarded(&mut self, g: Guarded) {
        if self.has_unsaved_changes() {
            self.confirm = Some(g);
        } else {
            self.run_guarded(g);
        }
    }

    pub(crate) fn run_guarded(&mut self, g: Guarded) {
        self.sketch = None;
        self.placement = None;
        match g {
            Guarded::New => self.request(Command::NewDocument),
            Guarded::NewAndSketch => {
                self.pending_new_sketch = true;
                self.request(Command::NewDocument);
            }
            Guarded::Open(path) => self.request(Command::Open { path }),
            Guarded::Close => self.request(Command::Close),
            Guarded::Exit => self.exit_confirmed = true,
        }
    }

    /// Confirmation answer: save first (when the document has a path), discard, or cancel.
    pub fn answer_confirm(&mut self, answer: ConfirmAnswer) {
        let Some(g) = self.confirm.take() else {
            return;
        };
        match answer {
            ConfirmAnswer::Cancel => {}
            ConfirmAnswer::Discard => self.run_guarded(g),
            ConfirmAnswer::Save => match self.summary.path.clone() {
                Some(path) if self.sketch.is_none() => {
                    self.after_save = Some(g);
                    self.request(Command::Save { path });
                }
                _ => {
                    self.dialog = Some(Dialog::SaveAs {
                        path: self.default_path("f420.json"),
                    });
                    self.diag(
                        false,
                        "info",
                        "choose a path and save, then repeat the action".into(),
                    );
                }
            },
        }
    }

    // ----- sketch sessions -----

    /// Frame of a sketch plane from the current document summary.
    pub fn plane_frame(&self, plane: &SketchPlane) -> Result<PlaneFrame, String> {
        match plane {
            SketchPlane::WorldXY => Ok(PlaneFrame::WORLD_XY),
            SketchPlane::WorldXZ => Ok(PlaneFrame::from_kernel(&f420_kernel::Frame::XZ)),
            SketchPlane::Face(r) => self
                .summary
                .features
                .iter()
                .flat_map(|f| f.faces.iter())
                .find(|fs| fs.reference == *r)
                .map(|fs| PlaneFrame::from_kernel(&fs.frame))
                .ok_or_else(|| {
                    format!(
                        "{}'s {:?} is not available (the feature must be evaluated)",
                        r.feature, r.role
                    )
                }),
        }
    }

    /// Open the sketch editor for a new sketch or an existing sketch feature.
    pub fn begin_sketch(&mut self, target: SketchTarget) {
        if self.sketch.is_some() {
            self.diag(
                true,
                "Sketch",
                "a sketch is already open: Finish or Cancel it first".into(),
            );
            return;
        }
        let (sketch, frame, title) = match &target {
            SketchTarget::New { plane } => match self.plane_frame(plane) {
                Ok(fr) => (
                    f420_sketch::Sketch::default(),
                    fr,
                    format!("New sketch {}", self.plane_text(plane)),
                ),
                Err(e) => {
                    self.diag(true, "Sketch", format!("cannot create a sketch here: {e}"));
                    return;
                }
            },
            SketchTarget::Existing(id) => {
                let Some(f) = self.summary.features.iter().find(|f| f.id == *id) else {
                    return;
                };
                let (Some(sk), Some(fr)) = (f.sketch.clone(), f.frame) else {
                    self.diag(
                        true,
                        "Sketch",
                        format!(
                            "{} cannot be edited: it did not evaluate (fix the error shown for it first)",
                            f.name
                        ),
                    );
                    return;
                };
                (
                    sk,
                    PlaneFrame::from_kernel(&fr),
                    format!("Editing {}", f.name),
                )
            }
        };
        self.placement = None;
        self.dialog = None;
        self.doc_dim_edit = None;
        let saved_camera = Some(self.view_camera().0);
        self.sketch = Some(SketchSession {
            editor: SketchEditor::new(sketch),
            target,
            frame,
            title,
            doc_id: self.summary.open.then_some(self.summary.doc_id),
            saved_camera,
            committing: false,
        });
        self.orient_to_sketch();
    }

    /// Point the camera along the sketch normal and frame the sketch.
    pub(crate) fn orient_to_sketch(&mut self) {
        let Some(s) = &self.sketch else { return };
        let frame = s.frame;
        let bounds = crate::sketch_view::sketch_bounds(&s.editor.sketch);
        let aspect = {
            let [w, h] = self.view_camera().1;
            if h > 0.0 { w / h } else { 1.5 }
        };
        crate::view3d::look_at_plane(self.camera_mut(), &frame, bounds, aspect);
    }

    /// Finish Sketch: one command → one document undo entry. The session stays
    /// open until the reply confirms the commit (a failure keeps the work).
    pub fn finish_sketch(&mut self) {
        let Some(s) = &mut self.sketch else { return };
        if s.committing {
            return;
        }
        s.editor.cancel_gesture();
        if !s.editor.report.is_success() {
            let why = s.editor.status().1;
            s.editor.feedback = Some(crate::sketch_edit::Feedback {
                error: true,
                text: format!("Cannot finish: {why}. Fix or delete the conflicting constraint."),
            });
            return;
        }
        let sketch = s.editor.sketch.clone();
        let cmd = match &s.target {
            SketchTarget::New { plane } => {
                if sketch.entities.is_empty() {
                    // Nothing drawn: finishing an empty new sketch just closes it.
                    self.end_sketch_session();
                    return;
                }
                Command::CreateSketch {
                    name: None,
                    plane: *plane,
                    template: SketchTemplate::Custom { sketch },
                }
            }
            SketchTarget::Existing(id) => {
                if !s.editor.modified() {
                    self.end_sketch_session();
                    return;
                }
                Command::EditSketch {
                    feature: *id,
                    sketch,
                }
            }
        };
        s.committing = true;
        self.request(cmd);
    }

    /// Cancel Sketch: discard the session; the document was never touched.
    pub fn cancel_sketch(&mut self) {
        if self.sketch.as_ref().is_some_and(|s| s.committing) {
            return;
        }
        self.end_sketch_session();
    }

    fn end_sketch_session(&mut self) {
        if let Some(s) = self.sketch.take()
            && let Some(cam) = s.saved_camera
        {
            *self.camera_mut() = cam;
        }
    }

    // ----- helpers shared by the panels -----

    pub(crate) fn default_path(&self, ext: &str) -> String {
        match &self.summary.path {
            Some(p) if ext == "f420.json" => p.display().to_string(),
            Some(p) => p.with_extension(ext).display().to_string(),
            None => format!("part.{ext}"),
        }
    }

    /// Last sketch not yet consumed by an extrude or cut.
    pub(crate) fn unused_sketch(&self) -> Option<FeatureId> {
        let fs = &self.summary.features;
        fs.iter()
            .rev()
            .filter(|f| f.kind == "sketch")
            .find(|s| !self.consumed(s.id))
            .or_else(|| fs.iter().rev().find(|f| f.kind == "sketch"))
            .map(|f| f.id)
    }

    /// The solid feature that consumes sketch `id`, if any.
    pub(crate) fn consumer_of(&self, id: FeatureId) -> Option<&FeatureSummary> {
        self.summary
            .features
            .iter()
            .find(|o| o.kind != "sketch" && o.depends_on.contains(&id))
    }

    pub(crate) fn consumed(&self, id: FeatureId) -> bool {
        self.consumer_of(id).is_some()
    }

    pub(crate) fn has_body(&self) -> bool {
        self.summary
            .features
            .iter()
            .any(|f| f.kind == "extrude" || f.kind == "revolve")
    }

    pub(crate) fn feature_name(&self, id: FeatureId) -> String {
        self.summary
            .features
            .iter()
            .find(|f| f.id == id)
            .map_or_else(|| format!("{id}?"), |f| f.name.clone())
    }

    pub(crate) fn plane_text(&self, plane: &SketchPlane) -> String {
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

    /// Why Extrude is unavailable for `sketch`, or None if it can run.
    pub(crate) fn extrude_blocker(&self, sketch: &FeatureSummary) -> Option<String> {
        if self.has_body() {
            return Some(
                "Only one base body per document is supported (product limit); use Cut Through All for holes"
                    .into(),
            );
        }
        if let Some(e) = &sketch.profile_error {
            return Some(format!("Profile is not usable: {e}"));
        }
        None
    }

    /// Why Cut Through All is unavailable for `sketch`, or None.
    pub(crate) fn cut_blocker(&self, sketch: &FeatureSummary) -> Option<String> {
        if !self.has_body() {
            return Some("Create a body first (Extrude)".into());
        }
        if let Some(c) = self.consumer_of(sketch.id) {
            return Some(format!("Already used by {}", c.name));
        }
        if let Some(e) = &sketch.profile_error {
            return Some(format!("Profile is not usable: {e}"));
        }
        let circle = sketch
            .sketch
            .as_ref()
            .is_some_and(|s| matches!(s.profile(), Ok(f420_sketch::Profile::Circle { .. })));
        (!circle).then(|| "Only circular through-all cuts are supported (product limit)".into())
    }

    // ----- demo -----

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
        let frame_start = Instant::now();
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
        self.shortcuts(&ctx);

        egui::Panel::top("f420_toolbar").show(ui, |ui| {
            if self.sketch.is_some() {
                self.sketch_toolbar(ui);
            } else {
                self.toolbar(ui, busy);
            }
        });
        egui::Panel::bottom("f420_status").show(ui, |ui| self.status_bar(ui, busy));
        egui::Panel::left("f420_browser")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("browser_scroll")
                    .show(ui, |ui| {
                        self.history(ui);
                        ui.add_space(8.0);
                        ui.separator();
                        self.diagnostics_ui(ui);
                    });
            });
        egui::Panel::right("f420_inspector")
            .resizable(true)
            .default_size(290.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .show(ui, |ui| {
                        if self.sketch.is_some() {
                            self.sketch_inspector(ui);
                        } else {
                            self.inspector_ui(ui, busy);
                        }
                    });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.view_ui(ui));
        self.dialog_ui(&ctx, busy);
        if self.dialog.is_none() {
            self.last_dialog = None;
        }
        if !matches!(
            self.dialog,
            Some(Dialog::Rectangle { .. } | Dialog::CircleOnFace { .. })
        ) {
            self.held_preview = None;
        }
        self.confirm_ui(&ctx);
        self.rename_ui(&ctx);
        self.help_ui(&ctx);
        if busy {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let dt = frame_start.elapsed();
        self.frame_ema = Some(match self.frame_ema {
            Some(e) => e.mul_f64(0.9) + dt.mul_f64(0.1),
            None => dt,
        });
    }

    /// Keyboard shortcuts. Ignored while a text field has focus so typing
    /// never triggers modelling commands.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if ctx.egui_wants_keyboard_input() || self.confirm.is_some() || self.rename.is_some() {
            return;
        }
        let sc = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        if sc(Modifiers::NONE, Key::F1) {
            self.show_help = !self.show_help;
        }
        if self.sketch.is_some() {
            self.sketch_shortcuts(ctx);
            return;
        }
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        // Order matters: shifted variants first.
        if sc(cmd_shift, Key::S) {
            if self.summary.open {
                self.dialog = Some(Dialog::SaveAs {
                    path: self.default_path("f420.json"),
                });
            }
        } else if sc(Modifiers::COMMAND, Key::S) {
            self.save();
        } else if sc(Modifiers::COMMAND, Key::O) {
            self.dialog = Some(Dialog::Open {
                path: self.default_path("f420.json"),
            });
        } else if sc(Modifiers::COMMAND, Key::N) {
            self.guarded(Guarded::New);
        } else if sc(cmd_shift, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
            if self.summary.can_redo.is_some() {
                self.request(Command::Redo);
            }
        } else if sc(Modifiers::COMMAND, Key::Z) {
            if self.summary.can_undo.is_some() {
                self.request(Command::Undo);
            }
        } else if sc(Modifiers::NONE, Key::F) {
            self.fit_view();
        } else if sc(Modifiers::NONE, Key::Escape) {
            if self.placement.is_some() {
                self.cancel_tool();
            } else if self.dialog.is_some() {
                self.dialog = None;
            } else if self.doc_dim_edit.is_some() {
                self.doc_dim_edit = None;
            } else {
                self.select(None);
            }
        }
        for (k, preset) in crate::view3d::VIEW_KEYS {
            if sc(Modifiers::NONE, *k) {
                self.set_view(*preset);
            }
        }
    }

    fn sketch_shortcuts(&mut self, ctx: &egui::Context) {
        use crate::sketch_edit::SketchTool as T;
        use egui::{Key, KeyboardShortcut, Modifiers};
        let sc = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        let Some(s) = &mut self.sketch else { return };
        if s.committing {
            return;
        }
        let ed = &mut s.editor;
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
            ed.redo();
        } else if sc(Modifiers::COMMAND, Key::Z) {
            ed.undo();
        } else if sc(Modifiers::SHIFT, Key::R) {
            ed.set_tool(T::CornerRectangle);
        } else if sc(Modifiers::NONE, Key::R) {
            ed.set_tool(T::CenterRectangle);
        } else if sc(Modifiers::NONE, Key::L) {
            ed.set_tool(T::Line);
        } else if sc(Modifiers::NONE, Key::C) {
            ed.set_tool(T::Circle);
        } else if sc(Modifiers::NONE, Key::D) {
            ed.set_tool(T::Dimension);
        } else if sc(Modifiers::NONE, Key::S) {
            ed.set_tool(T::Select);
        } else if sc(Modifiers::NONE, Key::Delete) || sc(Modifiers::NONE, Key::Backspace) {
            ed.delete_selection();
        } else if sc(Modifiers::NONE, Key::Enter) {
            ed.finish_chain();
        } else if sc(Modifiers::NONE, Key::Escape) {
            ed.escape();
        } else if sc(Modifiers::NONE, Key::F) {
            self.orient_to_sketch();
        }
    }

    pub(crate) fn save(&mut self) {
        if !self.summary.open {
            return;
        }
        match self.summary.path.clone() {
            Some(path) => self.request(Command::Save { path }),
            None => {
                self.dialog = Some(Dialog::SaveAs {
                    path: self.default_path("f420.json"),
                })
            }
        }
    }

    pub(crate) fn fit_view(&mut self) {
        if self.sketch.is_some() {
            self.orient_to_sketch();
        } else if let Some(vp) = self.viewport.as_mut() {
            vp.fit();
        }
    }

    pub(crate) fn set_view(&mut self, preset: f420_viewport::ViewPreset) {
        if let Some(vp) = self.viewport.as_mut() {
            vp.set_view(preset);
            vp.fit();
        } else {
            self.fallback_camera.set_view(preset);
        }
    }
}

pub(crate) fn parse_num(label: &str, s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .map_err(|_| format!("{label}: '{s}' is not a number"))
}

pub(crate) fn build_command(d: &Dialog) -> Result<Command, String> {
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

pub(crate) fn nonempty_path(s: &str) -> Result<PathBuf, String> {
    let t = s.trim();
    if t.is_empty() {
        Err("path is empty".into())
    } else {
        Ok(PathBuf::from(t))
    }
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

/// A practical next step for an error, appended to its message.
pub fn next_step(e: &DocError) -> String {
    let s = match e {
        DocError::Validation(_) => "Check the value and try again; the model is unchanged.",
        DocError::ProductLimit(_) => "This is a current product limit; the model is unchanged.",
        DocError::BrokenReference { .. } => {
            "The referenced feature/face is gone; edit the sketch plane or undo."
        }
        DocError::SketchSolve { .. } => {
            "Edit the sketch and delete or change one of the conflicting constraints."
        }
        DocError::Kernel { .. } => {
            "The geometry kernel could not build this; try a slightly different value, or undo."
        }
        DocError::File(_) => "Check the path and permissions; the open document is unchanged.",
        _ => return String::new(),
    };
    format!(" — {s}")
}

/// Shortest representation that round-trips (so unchanged fields compare equal).
pub(crate) fn fmt_num(v: f64) -> String {
    format!("{v}")
}
