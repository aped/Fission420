//! The R-CAD UI. Widgets hold only transient edit buffers; the document
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
use rcad_commands::{Command, DocSummary, FeatureSummary, SketchTemplate, end_cap_of};
use rcad_document::{BodyId, DocError, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::{Measurements, MeshCheck, VolumeResult};
use rcad_viewport::{Camera, Viewport};

use crate::engine::{DisplayScene, Engine, ExecMode, MeshUpdate, Reply};
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
    Extrude(crate::extrude::ExtrudeForm),
    CircleOnFace {
        extrude: Option<FeatureId>,
        diameter: String,
        cx: String,
        cy: String,
    },
    /// Circular cut from a cap sketch: through all, or a blind pocket of `depth`.
    Cut {
        sketch: Option<FeatureId>,
        blind: bool,
        depth: String,
    },
    SaveAs {
        path: String,
    },
    Open {
        path: String,
    },
    /// Print export (STL): selection, tessellation, placement, bed, report.
    ExportStl(crate::exchange::PrintForm),
    ExportStep {
        path: String,
        scope: crate::exchange::ExportScope,
    },
    ExportDxf {
        path: String,
        sketch: Option<FeatureId>,
        /// SVG instead of DXF.
        svg: bool,
    },
    /// Print export (3MF, unit in the form).
    Export3mf(crate::exchange::PrintForm),
    Import(crate::exchange::ImportForm),
    /// Linear / circular pattern or mirror of an Extrude Join or Cut.
    Pattern {
        feature: FeatureId,
        kind: PatternKind,
        count: String,
        spacing: String,
        along_y: bool,
        reverse: bool,
        /// Circular: degrees between instances and the centre (sketch mm).
        angle: String,
        center_x: String,
        center_y: String,
        /// Mirror: world plane through the origin, or the face selected when
        /// the dialog opened (`mirror_face`).
        mirror: SketchPlane,
        mirror_face: Option<rcad_document::FaceRef>,
        /// Mirror: construction-plane offset along the plane's normal (mm; 0 =
        /// the plane itself). Bindable afterwards as `plane_offset`.
        mirror_offset: String,
    },
    /// Round hole (with optional counterbore) into the body of face `face`.
    Hole {
        face: rcad_document::FaceRef,
        /// Centre in the face's frame (mm).
        cx: String,
        cy: String,
        diameter: String,
        through: bool,
        depth: String,
        counterbore: bool,
        cb_diameter: String,
        cb_depth: String,
        /// Start-plane offset from the face along its normal (mm; 0 = the
        /// face). The offset plane must hold a face of the target.
        offset: String,
    },
    /// Chamfer the straight edge between face `a` and a chosen second face.
    Chamfer {
        a: rcad_document::FaceRef,
        b: Option<rcad_document::FaceRef>,
        distance: String,
    },
    /// Start a sketch on an offset construction plane (world plane or the
    /// face selected when the dialog opened, moved along its normal).
    OffsetPlane {
        base: rcad_document::PlaneBase,
        /// Face offered as a base (selected when the dialog opened).
        face: Option<(rcad_document::FaceRef, String)>,
        distance: String,
    },
    /// Scoped shell of a New Body extrusion (docs/SHELL.md).
    Shell {
        extrude: FeatureId,
        /// Open at the end cap (else the start cap).
        open_end: bool,
        thickness: String,
    },
}

/// The pattern dialog's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Linear,
    Circular,
    Mirror,
}

impl Dialog {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Dialog::Rectangle { .. } => "New rectangle sketch (XY)",
            Dialog::Extrude(_) => "Extrude",
            Dialog::RectangleXz { .. } => "New rectangle sketch (XZ, for revolve)",
            Dialog::Revolve { .. } => "New revolve (about world Z)",
            Dialog::CircleOnFace { .. } => "New circle sketch on face",
            Dialog::Cut { .. } => "New cut",
            Dialog::SaveAs { .. } => "Save document as",
            Dialog::Open { .. } => "Open document",
            Dialog::ExportStl(_) => "Export STL",
            Dialog::ExportStep { .. } => "Export STEP",
            Dialog::ExportDxf { svg: false, .. } => "Export DXF",
            Dialog::ExportDxf { svg: true, .. } => "Export SVG",
            Dialog::Export3mf(_) => "Export 3MF",
            Dialog::Import(_) => "Import",
            Dialog::Pattern { .. } => "Pattern / mirror",
            Dialog::Chamfer { .. } => "Chamfer edge",
            Dialog::Hole { .. } => "Hole",
            Dialog::Shell { .. } => "Shell (scoped)",
            Dialog::OffsetPlane { .. } => "Sketch on offset plane",
        }
    }
}

/// Inspector edit buffers for the selected feature (preview only until Apply).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Inspector {
    pub feature: FeatureId,
    /// (name, document value, document text: the bound expression or the
    /// value, edit buffer)
    pub fields: Vec<(String, f64, String, String)>,
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
    /// Primary press inside the view (screen position, editor epoch at the
    /// press), until its release. A changed epoch voids the press.
    pub(crate) press: Option<egui::Pos2>,
    pub(crate) press_epoch: u64,
    /// The current press has moved beyond the click slop (a drag).
    pub(crate) press_dragging: bool,
    /// Last click (position, time, egui pass) for double-click detection.
    pub(crate) last_click: Option<(egui::Pos2, f64, u64)>,
}

/// Actions that discard the open document and therefore need confirmation
/// when there are unsaved changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Guarded {
    New,
    /// New document followed by a sketch on XY (empty-state "Create Sketch").
    NewAndSketch,
    /// New document built from a parametric printable template (run 4).
    NewTemplate(PrintTemplate),
    /// Open a recovery copy as an unsaved document (run 4).
    OpenCopy(PathBuf),
    Open(PathBuf),
    Close,
    Exit,
}

/// Parametric printable templates (SESSION_4 §7.7): a new document with the
/// same named parameters and bindings as the committed examples, edited by
/// changing parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintTemplate {
    Enclosure,
    Coupon,
    Bracket,
}

impl PrintTemplate {
    pub fn label(self) -> &'static str {
        match self {
            PrintTemplate::Enclosure => "Parametric enclosure + lid",
            PrintTemplate::Coupon => "Clearance test coupon",
            PrintTemplate::Bracket => "Mounting bracket",
        }
    }

    /// Commands that build the template in a new document.
    pub fn commands(self) -> Vec<Command> {
        use rcad_commands::printables as p;
        let unused = std::path::Path::new("unused.rcad.json");
        let steps = match self {
            PrintTemplate::Enclosure => p::enclosure_workflow(unused),
            PrintTemplate::Coupon => p::coupon_workflow(unused),
            PrintTemplate::Bracket => p::bracket_workflow(unused),
        };
        steps
            .into_iter()
            .map(|s| s.cmd)
            .filter(|c| !matches!(c, Command::Save { .. } | Command::Inspect))
            .collect()
    }
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

pub struct RcadApp {
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
    /// Display-only section view (run 5); None = off.
    pub section: Option<crate::section::SectionState>,
    pub(crate) mesh_triangles: usize,
    /// Last display scene: all visible bodies (face analysis, picking; display only).
    pub(crate) scene: DisplayScene,
    /// Body selected in the browser (or owning the picked face).
    pub(crate) selected_body: Option<BodyId>,
    pub(crate) faces: Vec<FaceInfo>,
    pub(crate) fitted: bool,
    demo: VecDeque<DemoStep>,
    replies_seen: u64,
    /// Commands accepted by the engine. The engine answers every command once,
    /// in order, so the n-th non-follow-up reply answers request n: this is the
    /// request token stale import replies are matched against.
    submitted: u64,
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
    /// Inline rename of a body row in the browser.
    pub(crate) body_rename: Option<crate::bodies::BodyRename>,
    pub(crate) doc_dim_edit: Option<DocDimEdit>,
    /// Screen rects of the selected finished sketch's dimension labels.
    pub(crate) doc_dim_targets: Vec<(usize, egui::Rect)>,
    /// Constraint whose inline sketch-dimension editor was open last frame
    /// (a newly opened editor selects its text so typing replaces it).
    pub(crate) sketch_dim_opened: Option<usize>,
    pub(crate) show_help: bool,
    /// Summary of the last successful import (window until closed).
    pub(crate) import_summary: Option<crate::exchange::ImportSummary>,
    /// Open "Re-attach sketch" window.
    pub(crate) reattach: Option<crate::reattach::Reattach>,
    /// Parameters panel edit buffers.
    pub(crate) params: crate::params_ui::ParamsPanel,
    /// Autosave / recovery state.
    pub(crate) autosave: crate::recovery::Autosave,
    /// First face of a two-face measurement (cleared on any document change).
    pub(crate) measure_from: Option<crate::view3d::FaceInfo>,
    /// Name and outcome of the last command reply (live-test probe).
    last_reply: Option<(String, bool)>,
    /// Sketch named by the last BrokenReference refusal (offered for repair).
    pub(crate) broken_sketch: Option<FeatureId>,
    /// Scripted screenshot scene (see [`crate::scenes`]).
    pub(crate) scene_steps: VecDeque<crate::scenes::SceneStep>,
    pub(crate) scene_fitted: bool,
    /// Scene finished: report busy until this instant (window fade-ins settle).
    pub(crate) scene_hold: Option<Instant>,
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

impl RcadApp {
    pub fn new(mode: ExecMode, repaint: Option<Box<dyn Fn() + Send>>) -> Self {
        Self {
            engine: Engine::new(mode, repaint),
            summary: DocSummary {
                doc_id: 0,
                open: false,
                measuring: false,
                path: None,
                dirty: false,
                revision: 0,
                can_undo: None,
                can_redo: None,
                features: vec![],
                bodies: vec![],
                parameters: vec![],
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
            section: None,
            mesh_triangles: 0,
            scene: DisplayScene::default(),
            selected_body: None,
            faces: vec![],
            fitted: false,
            demo: VecDeque::new(),
            replies_seen: 0,
            submitted: 0,
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
            body_rename: None,
            doc_dim_edit: None,
            doc_dim_targets: vec![],
            sketch_dim_opened: None,
            show_help: false,
            import_summary: None,
            reattach: None,
            params: Default::default(),
            measure_from: None,
            autosave: Default::default(),
            last_reply: None,
            broken_sketch: None,
            scene_steps: VecDeque::new(),
            scene_fitted: true,
            scene_hold: None,
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
    /// Name and outcome (`ok`) of the last command reply.
    pub fn last_reply(&self) -> Option<(&str, bool)> {
        self.last_reply.as_ref().map(|(n, ok)| (n.as_str(), *ok))
    }

    pub fn replies_seen(&self) -> u64 {
        self.replies_seen
    }
    /// A started `--scene` has run all its steps and fitted the view.
    pub fn scene_fitted(&self) -> bool {
        self.scene_fitted && self.scene_steps.is_empty()
    }
    /// No command in flight, no deferred measurement outstanding, no demo steps left.
    pub fn idle(&self) -> bool {
        !self.engine.busy()
            && self.demo.is_empty()
            && self.scene_steps.is_empty()
            && self.scene_hold.is_none_or(|t| Instant::now() >= t)
            && !self.summary.measuring
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
    /// Summary of the last successful import, while its window is open.
    pub fn import_summary(&self) -> Option<&crate::exchange::ImportSummary> {
        self.import_summary.as_ref()
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
            return "R-CAD".into();
        }
        let name = self
            .summary
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
        format!(
            "{}{name} — R-CAD",
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

    /// Queue the GOAL.md acceptance sequence (mirrors rcad_commands::acceptance)
    /// through the same request path as the buttons.
    pub fn start_demo(&mut self, scratch_dir: PathBuf) {
        use DemoStep::*;
        let doc = scratch_dir.join("demo.rcad.json");
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
                bodies: None,
                print: Default::default(),
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
        self.request_token(cmd);
    }

    /// [`RcadApp::request`], returning the request token (sequence number)
    /// its reply will carry (None if the worker is gone).
    pub fn request_token(&mut self, cmd: Command) -> Option<u64> {
        let name = cmd.name().to_string();
        if self.engine.submit(cmd) {
            self.in_flight.push_back((name, Instant::now()));
            self.submitted += 1;
            Some(self.submitted)
        } else {
            self.diag(
                true,
                "Worker",
                "session worker thread is not running; restart the app".into(),
            );
            None
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
        if r.followup {
            // Deferred measurement: apply only if it describes what is shown.
            let same = r.summary.doc_id == self.summary.doc_id
                && r.summary.revision == self.summary.revision;
            if std::env::var_os("RCAD_TRACE_ENGINE").is_some() {
                eprintln!(
                    "{:.3} ui: follow-up doc {} rev {} (showing doc {} rev {}) -> {}",
                    crate::engine::trace_clock(),
                    r.summary.doc_id,
                    r.summary.revision,
                    self.summary.doc_id,
                    self.summary.revision,
                    if same { "applied" } else { "dropped" }
                );
            }
            if same {
                // Per-body (and per-feature) measurements of the same revision:
                // the document itself is unchanged. Applied on a failure too,
                // which still carries the bodies that did measure.
                self.measurements = r.measurements;
                self.summary.bodies = r.summary.bodies;
                self.summary.features = r.summary.features;
                if let (false, Some(e)) = (r.response.ok, &r.response.error) {
                    self.diag(true, error_kind(e), e.to_string());
                }
                self.summary.measuring = r.summary.measuring;
            }
            return;
        }
        self.replies_seen += 1;
        self.last_reply = Some((r.response.command.clone(), r.response.ok));
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
        // A superseded edit was skipped by the worker because a newer edit of the
        // same parameter is queued; its summary is not a result. Keep the user's
        // buffers and wait for the reply that carries the result.
        if response.superseded {
            self.summary = summary;
            return;
        }
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
            if let Some(DocError::BrokenReference { from, .. }) = &response.error
                && summary
                    .features
                    .iter()
                    .any(|f| f.id == *from && f.kind == "sketch")
            {
                self.broken_sketch = Some(*from);
            }
            self.diag(
                true,
                kind,
                format!("{} rejected: {msg}{hint}", response.command),
            );
        }
        if response.ok && matches!(command, Command::Save { .. }) {
            self.drop_autosave();
            // A recovered copy is no longer needed once its work is saved.
            if let Some(p) = self.autosave.replaced.take() {
                let _ = std::fs::remove_file(p);
            }
        }
        if let Command::OpenCopy { path } = &command {
            // Only a successful Open Copy makes the copy replaceable (#13).
            if self.autosave.opening.as_ref() == Some(path) {
                self.autosave.opening = None;
                if response.ok {
                    self.autosave.replaced = Some(path.clone());
                }
            }
        }
        if response.ok
            && let Command::SaveCopy { path } = &command
            && let Some(p) = self.autosave.replaced.take()
            && p != *path
        {
            let _ = std::fs::remove_file(p);
        }
        if response.ok && matches!(command, Command::SetSketchPlane { .. }) {
            self.broken_sketch = None;
        }
        if matches!(command, Command::ExportStl { .. }) {
            self.last_export = response.mesh_check.clone();
        }
        if doc_changed {
            self.fitted = false;
            self.selected = None;
            self.selected_face = None;
            self.selected_body = None;
            self.doc_dim_edit = None;
            self.rename = None;
            self.body_rename = None;
            self.reattach = None;
            self.broken_sketch = None;
            self.params = Default::default();
            // The previous document was saved or deliberately left.
            self.drop_autosave();
            // Dialogs hold ids of the old document (ids restart per document);
            // a pending import is the exception: its reply is this one.
            if !matches!(&self.dialog, Some(Dialog::Import(f)) if f.pending.is_some()) {
                self.dialog = None;
            }
            self.import_summary = None;
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
        if doc_changed || summary.revision != self.summary.revision {
            self.measure_from = None;
        }
        let edited_param = match &command {
            Command::SetParameter { feature, name, .. }
            | Command::SetExpression { feature, name, .. } => Some((*feature, name.clone())),
            _ => None,
        };
        self.summary = summary;
        self.measurements = measurements;
        if let Some(sel) = self.selected
            && !self.summary.features.iter().any(|f| f.id == sel)
        {
            self.selected = None;
        }
        // An inline rename whose target is gone (Undo, Delete) would keep
        // keyboard shortcuts suspended with no field drawn.
        if let Some((f, _)) = &self.rename
            && !self.summary.features.iter().any(|x| x.id == *f)
        {
            self.rename = None;
        }
        if let Some(r) = &self.body_rename
            && !self.summary.bodies.iter().any(|b| b.id == r.body)
        {
            self.body_rename = None;
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
                    self.selected_body = None;
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
            self.selected_body = None;
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
            MeshUpdate::Replace(scene) => {
                for e in &scene.skipped {
                    self.diag(true, "Display", format!("not displayed: {e}"));
                }
                let m = &scene.mesh;
                let mut accepted = true;
                if let Some(vp) = self.viewport.as_mut() {
                    match vp.set_mesh(m) {
                        Ok(()) => {
                            vp.set_group_tints(&scene.tints());
                            if !self.fitted && m.triangle_count() > 0 && self.sketch.is_none() {
                                vp.fit();
                                self.fitted = true;
                            }
                        }
                        Err(e) => {
                            accepted = false;
                            let t = format!("display mesh rejected: {e}");
                            self.diag(true, "Display", t);
                        }
                    }
                }
                // CPU picking must use the mesh the GPU draws.
                if accepted {
                    self.mesh_triangles = scene.mesh.triangle_count();
                    self.selected_face = None;
                    self.scene = scene;
                }
            }
        }
        if let Some(b) = self.selected_body
            && !self.summary.bodies.iter().any(|x| x.id == b)
        {
            self.selected_body = None;
        }
        self.faces =
            crate::view3d::analyse_faces(&self.scene, &self.summary.features, &self.summary.bodies);
        self.import_reply(&command, &response, self.replies_seen);
        self.params_reply(
            &command,
            response.ok,
            response.error.as_ref().map(|e| e.to_string()),
        );
        self.print_export_reply(&command, &response, self.replies_seen);
        if response.ok && matches!(command, Command::Undo | Command::Redo) {
            self.close_dialog_after_undo(&command);
        }
        self.revalidate_dialog();
        self.sync_viewport_selection();
    }

    /// Centroid of a display face's triangles, in the frame of semantic face
    /// `r` (hole dialog default centre).
    pub(crate) fn face_centre_local(
        &self,
        info: &crate::view3d::FaceInfo,
        r: &rcad_document::FaceRef,
    ) -> Option<[f64; 2]> {
        let frame = self
            .summary
            .features
            .iter()
            .flat_map(|f| f.faces.iter())
            .find(|f| f.reference == *r)?
            .frame;
        let m = &self.scene.mesh;
        let (mut sum, mut area) = ([0.0f64; 3], 0.0f64);
        for (t, idx) in m.indices.as_chunks::<3>().0.iter().enumerate() {
            if m.tri_group.get(t) != Some(&info.group) {
                continue;
            }
            let p: Vec<[f64; 3]> = idx
                .iter()
                .filter_map(|&i| m.positions.get(i as usize).map(|v| v.map(f64::from)))
                .collect();
            if p.len() != 3 {
                continue;
            }
            let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let c = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let a = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() / 2.0;
            for k in 0..3 {
                sum[k] += a * (p[0][k] + p[1][k] + p[2][k]) / 3.0;
            }
            area += a;
        }
        if area <= 0.0 {
            return None;
        }
        let d = [
            sum[0] / area - frame.origin[0],
            sum[1] / area - frame.origin[1],
            sum[2] / area - frame.origin[2],
        ];
        let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        Some([dot(frame.x), dot(frame.y)])
    }

    /// Feature and body ids an open dialog acts on.
    fn dialog_refs(d: &Dialog) -> (Vec<FeatureId>, Vec<rcad_document::BodyId>) {
        use crate::exchange::ExportScope;
        let scope = |s: &ExportScope| match s {
            ExportScope::Selected(v) => v.clone(),
            ExportScope::AllVisible => vec![],
        };
        match d {
            Dialog::Extrude(f) => (
                f.sketch.into_iter().collect(),
                f.target.into_iter().collect(),
            ),
            Dialog::Revolve { sketch, .. } | Dialog::Cut { sketch, .. } => {
                (sketch.iter().copied().collect(), vec![])
            }
            Dialog::CircleOnFace { extrude, .. } => (extrude.iter().copied().collect(), vec![]),
            Dialog::ExportDxf { sketch, .. } => (sketch.iter().copied().collect(), vec![]),
            Dialog::Pattern { feature, .. } => (vec![*feature], vec![]),
            Dialog::Hole { face, .. } => (vec![face.feature], vec![]),
            Dialog::Shell { extrude, .. } => (vec![*extrude], vec![]),
            Dialog::OffsetPlane { face, .. } => {
                (face.iter().map(|(r, _)| r.feature).collect(), vec![])
            }
            Dialog::Chamfer { a, b, .. } => (
                std::iter::once(a.feature)
                    .chain(b.map(|b| b.feature))
                    .collect(),
                vec![],
            ),
            Dialog::ExportStep { scope: s, .. } => (vec![], scope(s)),
            Dialog::ExportStl(f) | Dialog::Export3mf(f) => (f.face_features(), scope(&f.scope)),
            Dialog::Import(f) => (f.face.iter().map(|(r, _)| r.feature).collect(), vec![]),
            Dialog::Rectangle { .. }
            | Dialog::RectangleXz { .. }
            | Dialog::SaveAs { .. }
            | Dialog::Open { .. } => (vec![], vec![]),
        }
    }

    /// Undo/Redo can roll the id allocators back, so an id held by an open
    /// dialog may later name a different object: close dialogs (and the
    /// re-attach window) that hold ids instead of letting them act on it.
    fn close_dialog_after_undo(&mut self, cmd: &Command) {
        let what = if matches!(cmd, Command::Undo) {
            "Undo"
        } else {
            "Redo"
        };
        if let Some(d) = &self.dialog {
            let (f, b) = Self::dialog_refs(d);
            let pending_import = matches!(d, Dialog::Import(x) if x.pending.is_some());
            if (!f.is_empty() || !b.is_empty()) && !pending_import {
                let title = d.title();
                self.dialog = None;
                self.diag(
                    false,
                    "info",
                    format!("{what} changed the document: closed \"{title}\" (it referred to objects that may no longer exist)"),
                );
            }
        }
        if self.reattach.take().is_some() {
            self.diag(
                false,
                "info",
                format!("{what} changed the document: closed \"Re-attach sketch\""),
            );
        }
    }

    /// Close a dialog whose feature/body ids no longer exist (Delete, a
    /// failed regeneration elsewhere, ...). Never acts on a missing id.
    fn revalidate_dialog(&mut self) {
        let Some(d) = &self.dialog else {
            return;
        };
        let (fs, bs) = Self::dialog_refs(d);
        let missing_f = fs
            .iter()
            .find(|f| !self.summary.features.iter().any(|x| x.id == **f));
        let missing_b = bs
            .iter()
            .find(|b| !self.summary.bodies.iter().any(|x| x.id == **b));
        let why = match (missing_f, missing_b) {
            (Some(f), _) => format!("feature {f} no longer exists"),
            (None, Some(b)) => format!("body {b} no longer exists"),
            (None, None) => return,
        };
        let title = d.title();
        self.dialog = None;
        self.diag(true, "Dialog", format!("closed \"{title}\": {why}"));
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
                let text = f.expressions.get(n).cloned().unwrap_or_else(|| fmt_num(*v));
                let keep = old.as_ref().and_then(|o| {
                    o.fields.iter().find(|(on, _, ot, ob)| {
                        on == n && ob.trim() != ot.trim() && edited != Some(&(id, n.clone()))
                    })
                });
                let buf = keep.map_or_else(|| text.clone(), |(_, _, _, b)| b.clone());
                (n.clone(), *v, text, buf)
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
        if self.selected != id || self.selected_face.is_some() || self.selected_body.is_some() {
            self.selected = id;
            self.selected_face = None;
            self.selected_body = None;
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
                self.selected_body = info.body;
                self.reset_inspector();
            }
            None => {
                self.selected = None;
                self.selected_face = None;
                self.selected_body = None;
                self.inspector = None;
            }
        }
        self.sync_viewport_selection();
    }

    /// Select a body (browser): highlights all of its faces; the feature
    /// selection is cleared (the inspector shows the body).
    pub fn select_body(&mut self, body: Option<BodyId>) {
        self.selected = None;
        self.selected_face = None;
        self.selected_body = body;
        self.doc_dim_edit = None;
        self.inspector = None;
        self.sync_viewport_selection();
    }

    /// Selected body (browser selection or the body of the picked face).
    pub fn selected_body(&self) -> Option<BodyId> {
        self.selected_body
    }

    /// Display scene (all visible bodies) as last reported by the engine.
    pub fn scene(&self) -> &DisplayScene {
        &self.scene
    }

    /// Groups highlighted for the current selection: the picked face, every
    /// face whose provenance is reliably the selected feature, or every face of
    /// the selected body.
    pub fn highlighted_faces(&self) -> Vec<u32> {
        match (self.selected_face, self.selected) {
            (Some(g), _) => vec![g],
            (None, None) => match self
                .selected_body
                .and_then(|b| self.scene.bodies.iter().find(|d| d.id == b))
            {
                Some(d) => self
                    .faces
                    .iter()
                    .filter(|f| d.groups.contains(&f.group))
                    .map(|f| f.group)
                    .collect(),
                None => vec![],
            },
            (None, Some(id)) => self
                .faces
                .iter()
                .filter(|f| f.owner == Some(id))
                .map(|f| f.group)
                .collect(),
        }
    }

    /// Push the selection highlight to the GPU view (the Extrude dialog's
    /// target body while it is open).
    pub(crate) fn sync_viewport_selection(&mut self) {
        let groups = self
            .extrude_target_groups()
            .unwrap_or_else(|| self.highlighted_faces());
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
            Guarded::NewTemplate(t) => {
                // The workflow's own commands (it starts with NewDocument);
                // Save/Inspect are the example's, not the template's.
                for c in t.commands() {
                    self.request(c);
                }
            }
            Guarded::Open(path) => self.request(Command::Open { path }),
            Guarded::OpenCopy(path) => {
                self.autosave.opening = Some(path.clone());
                self.request(Command::OpenCopy { path })
            }
            Guarded::Close => self.request(Command::Close),
            Guarded::Exit => {
                self.drop_autosave();
                self.exit_confirmed = true
            }
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
                        path: self.default_path("rcad.json"),
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
            SketchPlane::Offset { base, distance } => {
                Ok(self.plane_frame(&base.plane())?.offset(*distance))
            }
            SketchPlane::WorldXY => Ok(PlaneFrame::WORLD_XY),
            SketchPlane::WorldXZ => Ok(PlaneFrame::from_kernel(&rcad_kernel::Frame::XZ)),
            SketchPlane::WorldYZ => Ok(PlaneFrame::from_kernel(&rcad_document::FRAME_YZ)),
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
        if self.engine.busy() {
            // The session copies the sketch/face from the current summary; a
            // queued edit would otherwise be silently undone by Finish.
            self.diag(
                true,
                "Sketch",
                "wait for the current regeneration to finish, then open the sketch".into(),
            );
            return;
        }
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
                    rcad_sketch::Sketch::default(),
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
            press: None,
            press_epoch: 0,
            press_dragging: false,
            last_click: None,
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
            Some(p) if ext == "rcad.json" => p.display().to_string(),
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

    /// The document has an exact (B-rep) body to join to or cut.
    pub(crate) fn has_body(&self) -> bool {
        self.summary.bodies.iter().any(|b| b.kind == "brep")
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
            SketchPlane::Offset { base, distance } => format!(
                "offset {} mm from {}",
                rcad_sketch::fmt_mm(*distance),
                self.plane_text(&base.plane()).trim_start_matches("on ")
            ),
            SketchPlane::WorldXY => "on World XY".into(),
            SketchPlane::WorldXZ => "on World XZ".into(),
            SketchPlane::WorldYZ => "on World YZ".into(),
            SketchPlane::Face(r) => {
                let role = match r.role {
                    FaceRole::ExtrudeEndCap => "EndCap".to_string(),
                    FaceRole::ExtrudeStartCap => "StartCap".to_string(),
                    ref other => crate::face_role_label(other),
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

    /// Features that use `id` (deleting `id` is refused while any exist):
    /// explicit references, and features that modify a body `id` creates.
    pub(crate) fn dependents_of(&self, id: FeatureId) -> Vec<String> {
        let creates: Vec<BodyId> = self
            .summary
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.creates.clone())
            .unwrap_or_default();
        self.summary
            .features
            .iter()
            .filter(|f| f.id != id)
            .filter(|f| f.depends_on.contains(&id) || f.body.is_some_and(|b| creates.contains(&b)))
            .map(|f| f.name.clone())
            .collect()
    }

    /// Delete the selected feature (one undoable step), or explain why not.
    pub(crate) fn delete_selected_feature(&mut self) {
        let Some(id) = self.selected else { return };
        let users = self.dependents_of(id);
        if !users.is_empty() {
            self.diag(
                true,
                "Delete",
                format!(
                    "{} is used by {}; delete those first",
                    self.feature_name(id),
                    users.join(", ")
                ),
            );
            return;
        }
        self.request(Command::DeleteFeature { feature: id });
    }

    /// Why Extrude is unavailable for `sketch`, or None if it can run.
    pub(crate) fn extrude_blocker(&self, sketch: &FeatureSummary) -> Option<String> {
        // Several loops (islands, separate outers): extrude one picked region.
        if sketch.profile_error.is_some() && crate::extrude::sketch_regions(sketch).len() > 1 {
            return None;
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
            .is_some_and(|s| matches!(s.profile(), Ok(rcad_sketch::Profile::Circle { .. })));
        (!circle).then(|| "Only circular cuts and pockets are supported (product limit)".into())
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
            } => id(sketch).map(|s| Command::extrude(Some(name.into()), s, distance)),
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
        self.advance_scene();
        self.autosave_tick();
        if !self.scene_steps.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if self.engine.mode() == ExecMode::Sync && !self.demo.is_empty() {
            ctx.request_repaint();
        }
        if self.engine.worker_lost() && !self.diagnostics.iter().any(|d| d.kind == "Worker") {
            self.diag(true, "Worker", "session worker thread stopped".into());
        }
        let busy = self.engine.busy();
        if (busy || self.summary.measuring) && std::env::var_os("RCAD_TRACE_ENGINE").is_some() {
            eprintln!(
                "{:.3} ui: frame busy={busy} measuring={}",
                crate::engine::trace_clock(),
                self.summary.measuring
            );
        }
        self.shortcuts(&ctx);

        egui::Panel::top("rcad_toolbar").show(ui, |ui| {
            if self.sketch.is_some() {
                self.sketch_toolbar(ui);
            } else {
                self.toolbar(ui, busy);
            }
        });
        egui::Panel::bottom("rcad_status").show(ui, |ui| self.status_bar(ui, busy));
        egui::Panel::left("rcad_browser")
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
        egui::Panel::right("rcad_inspector")
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
        self.sync_viewport_selection();
        self.confirm_ui(&ctx);
        self.recovery_ui(&ctx);
        self.rename_ui(&ctx);
        self.help_ui(&ctx);
        self.section_window(&ctx);
        self.import_summary_ui(&ctx);
        self.reattach_ui(&ctx);
        if busy || self.summary.measuring {
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
        if ctx.egui_wants_keyboard_input()
            || self.confirm.is_some()
            || self.rename.is_some()
            || self.body_rename.is_some()
        {
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
                    path: self.default_path("rcad.json"),
                });
            }
        } else if sc(Modifiers::COMMAND, Key::S) {
            self.save();
        } else if sc(Modifiers::COMMAND, Key::O) {
            self.dialog = Some(Dialog::Open {
                path: self.default_path("rcad.json"),
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
        } else if sc(Modifiers::NONE, Key::Delete) {
            if self.selected_face.is_some() {
                // A picked face selects its owner too; deleting the whole feature
                // from a face pick would be surprising.
                self.diag(
                    false,
                    "info",
                    "faces cannot be deleted; select the feature in the browser to delete it"
                        .into(),
                );
            } else {
                self.delete_selected_feature();
            }
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
        if ed.dragging() {
            // Mid-drag only Escape (restore) is meaningful.
            if sc(Modifiers::NONE, Key::Escape) {
                ed.escape();
            }
            return;
        }
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
        } else if sc(Modifiers::NONE, Key::A) {
            ed.set_tool(T::Arc);
        } else if sc(Modifiers::NONE, Key::X) {
            ed.toggle_construction();
        } else if sc(Modifiers::NONE, Key::D) {
            ed.set_tool(T::Dimension);
        } else if sc(Modifiers::NONE, Key::T) {
            ed.set_tool(T::Trim);
        } else if sc(Modifiers::NONE, Key::E) {
            ed.set_tool(T::Extend);
        } else if sc(Modifiers::NONE, Key::O) {
            ed.set_tool(T::Offset);
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
                    path: self.default_path("rcad.json"),
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

    pub(crate) fn set_view(&mut self, preset: rcad_viewport::ViewPreset) {
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
        Dialog::Extrude(form) => form.command()?,
        Dialog::Hole {
            face,
            cx,
            cy,
            diameter,
            through,
            depth,
            counterbore,
            cb_diameter,
            cb_depth,
            offset,
        } => Command::Hole {
            name: None,
            plane: offset_plane(
                SketchPlane::Face(*face),
                parse_num("start plane offset", offset)?,
            ),
            center: [parse_num("centre X", cx)?, parse_num("centre Y", cy)?],
            diameter: parse_num("hole diameter", diameter)?,
            depth: if *through {
                None
            } else {
                Some(parse_num("hole depth", depth)?)
            },
            counterbore: if *counterbore {
                Some([
                    parse_num("counterbore diameter", cb_diameter)?,
                    parse_num("counterbore depth", cb_depth)?,
                ])
            } else {
                None
            },
            target: None,
        },
        Dialog::OffsetPlane { .. } => {
            return Err("an offset plane starts a sketch session (no command)".into());
        }
        Dialog::Shell {
            extrude,
            open_end,
            thickness,
        } => Command::Shell {
            name: None,
            extrude: *extrude,
            open: if *open_end {
                rcad_document::FaceRole::ExtrudeEndCap
            } else {
                rcad_document::FaceRole::ExtrudeStartCap
            },
            thickness: parse_num("thickness", thickness)?,
        },
        Dialog::Chamfer { a, b, distance } => Command::Chamfer {
            name: None,
            a: *a,
            b: b.ok_or_else(|| "choose the second face of the edge".to_string())?,
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
        Dialog::Cut {
            sketch,
            blind: false,
            ..
        } => Command::CutThroughAll {
            name: None,
            sketch: need(sketch, "profile sketch")?,
        },
        Dialog::Cut {
            sketch,
            blind: true,
            depth,
        } => Command::Pocket {
            name: None,
            sketch: need(sketch, "profile sketch")?,
            depth: parse_num("depth", depth)?,
        },
        Dialog::SaveAs { path } => Command::Save {
            path: nonempty_path(path)?,
        },
        Dialog::Open { path } => Command::Open {
            path: nonempty_path(path)?,
        },
        Dialog::ExportStl(f) | Dialog::Export3mf(f) => f.command()?,
        Dialog::ExportStep { path, scope } => Command::ExportStep {
            path: nonempty_path(path)?,
            bodies: scope.bodies(),
        },
        Dialog::ExportDxf {
            path,
            sketch,
            svg: false,
        } => Command::ExportDxf {
            path: nonempty_path(path)?,
            sketch: need(sketch, "sketch")?,
        },
        Dialog::ExportDxf {
            path,
            sketch,
            svg: true,
        } => Command::ExportSvg {
            path: nonempty_path(path)?,
            sketch: need(sketch, "sketch")?,
        },
        Dialog::Import(form) => form.command()?,
        Dialog::Pattern {
            feature,
            kind,
            count,
            spacing,
            along_y,
            reverse,
            angle,
            center_x,
            center_y,
            mirror,
            mirror_offset,
            ..
        } => {
            let count = || -> Result<u32, String> {
                let n = parse_num("count", count)?;
                if !(n.is_finite() && n >= 1.0 && n.fract() == 0.0 && n <= 100.0) {
                    return Err(format!(
                        "count must be a whole number from 1 to 100, got {n}"
                    ));
                }
                Ok(n as u32)
            };
            match kind {
                PatternKind::Linear => {
                    let s = if *reverse { -1.0 } else { 1.0 };
                    Command::LinearPattern {
                        name: None,
                        feature: *feature,
                        count: count()?,
                        spacing: parse_num("spacing", spacing)?,
                        direction: if *along_y { [0.0, s] } else { [s, 0.0] },
                    }
                }
                PatternKind::Circular => Command::CircularPattern {
                    name: None,
                    feature: *feature,
                    count: count()?,
                    angle_deg: parse_num("angle", angle)?,
                    center: [
                        parse_num("centre x", center_x)?,
                        parse_num("centre y", center_y)?,
                    ],
                },
                PatternKind::Mirror => Command::Mirror {
                    name: None,
                    feature: *feature,
                    plane: offset_plane(*mirror, parse_num("mirror plane offset", mirror_offset)?),
                },
            }
        }
    })
}

/// `plane` moved `distance` mm along its normal (a construction plane), or
/// `plane` itself for a zero distance.
pub(crate) fn offset_plane(plane: SketchPlane, distance: f64) -> SketchPlane {
    use rcad_document::PlaneBase;
    let base = match plane {
        SketchPlane::WorldXY => PlaneBase::WorldXY,
        SketchPlane::WorldXZ => PlaneBase::WorldXZ,
        SketchPlane::WorldYZ => PlaneBase::WorldYZ,
        SketchPlane::Face(r) => PlaneBase::Face(r),
        SketchPlane::Offset { .. } => return plane,
    };
    if distance == 0.0 {
        plane
    } else {
        SketchPlane::Offset { base, distance }
    }
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
        DocError::GeometryCheck { .. } => "GeometryCheck",
        DocError::Expression { .. } => "Expression",
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
            "The referenced face is gone or no longer unique; the model is unchanged. Re-attach the sketch to another face or plane (Re-attach sketch…), then repeat the edit."
        }
        DocError::SketchSolve { .. } => {
            "Edit the sketch and delete or change one of the conflicting constraints."
        }
        DocError::Kernel { .. } => {
            "The geometry kernel could not build this; try a slightly different value, or undo."
        }
        DocError::File(_) => "Check the path and permissions; the open document is unchanged.",
        DocError::GeometryCheck { .. } => {
            "The kernel's result was wrong and was refused; the model is unchanged."
        }
        DocError::Expression { .. } => {
            "Fix the expression or the parameter it names; the model is unchanged."
        }
        _ => return String::new(),
    };
    format!(" — {s}")
}

/// Shortest representation that round-trips (so unchanged fields compare equal).
pub(crate) fn fmt_num(v: f64) -> String {
    format!("{v}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcad_commands::{Response, Session};

    /// Review run 3 #17 (UI side): a measurement follow-up that reports one
    /// failed body still delivers the bodies that did measure.
    #[test]
    fn failed_measurement_followup_still_applies_measured_bodies() {
        let mut s = Session::new();
        s.set_defer_measurements(true);
        s.execute(&Command::NewDocument);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step/two_boxes_compound_mm.step");
        let r = s.execute(&Command::ImportStep {
            path,
            units_to_mm: None,
        });
        assert!(r.ok, "{:?}", r.error);
        let mut app = RcadApp::new(ExecMode::Sync, None);
        app.summary = s.summary();
        let (doc, rev, bodies) = s.pending_measurement().expect("pending");
        let (good, bad) = (bodies[0].0, bodies[1].0);
        let m = rcad_kernel::measure(&bodies[0].1).unwrap();
        assert!(s.adopt_measurements(doc, rev, vec![(good, m)]));
        assert!(s.fail_measurements(doc, rev));
        app.handle_reply(Reply {
            command: Command::Measure,
            response: Response {
                ok: false,
                command: "measure".into(),
                error: Some(DocError::Validation(format!("body {bad}: injected"))),
                ..Default::default()
            },
            summary: s.summary(),
            measurements: None,
            mesh: MeshUpdate::Unchanged,
            followup: true,
        });
        let body = |id| app.summary.bodies.iter().find(|b| b.id == id).unwrap();
        assert!(body(good).measurements.is_some());
        assert!(body(bad).measurements.is_none());
        assert!(app.diagnostics().last().is_some_and(|d| d.error));
    }
}
