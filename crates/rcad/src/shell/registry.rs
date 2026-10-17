//! The command registry: one [`CommandDef`] per user-facing command. The
//! application bar, the ribbon (tabs, groups, promoted buttons, group
//! dropdowns), the keymap and the generated shortcut sheet all read this
//! table, so a command's label, key, placement, enabled rule and action are
//! defined once.
//!
//! Label compatibility: a command's `label` is the widget's accessible name
//! and probe key wherever it is drawn (icon or text button), so scripts and
//! kittests address it by label.

use super::Focus;
use super::keymap::{Chord, KeymapLayer, cmd, cmd_shift, k};
use crate::app::{Guarded, PrintTemplate, RcadApp, SketchTarget};
use eframe::egui::Key;
use rcad_document::SketchPlane;

/// Stable command id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmdId {
    // File / application bar
    New,
    Open,
    OpenSafe,
    Save,
    SaveAs,
    Import,
    Close,
    ExportStl,
    ExportStep,
    ExportDxf,
    ExportSvg,
    Export3mf,
    Undo,
    Redo,
    Help,
    About,
    Quit,
    // Cloud (optional server tier)
    #[cfg(feature = "cloud")]
    DataPanel,
    #[cfg(feature = "cloud")]
    SaveVersion,
    #[cfg(feature = "cloud")]
    VersionHistory,
    #[cfg(feature = "cloud")]
    ShareLink,
    #[cfg(feature = "cloud")]
    CloudComments,
    #[cfg(feature = "cloud")]
    SyncNow,
    // View
    Fit,
    Section,
    ViewTop,
    ViewFront,
    ViewRight,
    ViewLeft,
    ViewBack,
    ViewIso,
    ViewBottom,
    Orbit,
    Pan,
    Zoom,
    LookAt,
    StyleEdges,
    StyleShaded,
    // SOLID ▸ CREATE
    CreateSketch,
    SketchXY,
    SketchXZ,
    SketchYZ,
    Extrude,
    CutThroughAll,
    Revolve,
    Hole,
    Thread,
    Loft,
    Sweep,
    Pattern,
    Mirror,
    // Templates (INSERT ▸ Templates)
    TemplateEnclosure,
    TemplateCoupon,
    TemplateBracket,
    TemplateTray,
    RectangleXY,
    RectangleXZ,
    CircleOnFace,
    PlaceRectangle,
    PlaceHole,
    // SOLID ▸ MODIFY
    PressPull,
    Fillet,
    Chamfer,
    FilletLoop,
    ChamferLoop,
    Shell,
    Align,
    DirectEdit,
    MoveCopy,
    Scale,
    SplitBody,
    Combine,
    Material,
    Delete,
    ChangeParameters,
    // ASSEMBLE
    NewComponent,
    Joint,
    RigidGroup,
    DriveJoints,
    BillOfMaterials,
    ExportAssembly,
    // CONFIGURE
    Configure,
    ConfigTable,
    // CONSTRUCT
    OffsetPlane,
    // INSPECT
    Measure,
    Interference,
    MassProperties,
    // INSERT
    NewDrawing,
    DrawingViews,
    // SELECT
    ClearSelection,
    CommandSearch,
    // MESH
    MeshRepair,
    MeshEdit,
    // SHEET METAL / SURFACE
    SheetFlange,
    SheetBase,
    SurfacePatch,
    SurfaceExtrude,
}

/// Ribbon tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Solid,
    Surface,
    Mesh,
    SheetMetal,
    Utilities,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Solid,
        Tab::Surface,
        Tab::Mesh,
        Tab::SheetMetal,
        Tab::Utilities,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Solid => "SOLID",
            Tab::Surface => "SURFACE",
            Tab::Mesh => "MESH",
            Tab::SheetMetal => "SHEET METAL",
            Tab::Utilities => "UTILITIES",
        }
    }
    /// What the workspace holds (tab tooltip).
    pub fn hint(self) -> &'static str {
        match self {
            Tab::Solid => "Solid modeling: sketches, features, assembly, inspection",
            Tab::Surface => "Surfaces from sketches: patches and extruded surfaces",
            Tab::Mesh => "Mesh bodies (STL, 3MF): repair and edit",
            Tab::SheetMetal => "Sheet metal: base faces and flanges",
            Tab::Utilities => "Parameters and export for printing (STL, 3MF)",
        }
    }
}

/// Ribbon group (order of declaration = order on the tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Create,
    Modify,
    Assemble,
    Configure,
    Construct,
    Inspect,
    Insert,
    Select,
    Prepare,
    Make,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Create => "CREATE",
            Group::Modify => "MODIFY",
            Group::Assemble => "ASSEMBLE",
            Group::Configure => "CONFIGURE",
            Group::Construct => "CONSTRUCT",
            Group::Inspect => "INSPECT",
            Group::Insert => "INSERT",
            Group::Select => "SELECT",
            Group::Prepare => "PREPARE",
            Group::Make => "MAKE",
        }
    }
}

/// Where a command is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// File ▾ menu.
    File,
    /// File ▾ ▸ Export ▸.
    FileExport,
    /// Quick-access button on the application bar.
    AppBar,
    /// View menu / navigation.
    View,
    /// Ribbon: `promoted` commands have a button on the ribbon face; every
    /// placed command is listed in its group's dropdown.
    Ribbon {
        tab: Tab,
        group: Group,
        promoted: bool,
    },
}

/// Marking-menu slot (radial compass position, or the context list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmSlot {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
    Ctx,
}

/// Command icon (original glyphs drawn by [`crate::shell`]'s painter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    New,
    Open,
    Save,
    Undo,
    Redo,
    Import,
    Export,
    Sketch,
    Extrude,
    Cut,
    Revolve,
    Hole,
    Thread,
    Loft,
    Sweep,
    Pattern,
    Mirror,
    PressPull,
    Fillet,
    Chamfer,
    Shell,
    Align,
    Move,
    Scale,
    Split,
    Combine,
    Material,
    Delete,
    Parameters,
    Component,
    Joint,
    Bom,
    Configure,
    Plane,
    Measure,
    Interference,
    Section,
    Drawing,
    Select,
    Mesh,
    Template,
    View,
    Help,
    Generic,
}

pub type Enabled = fn(&RcadApp) -> Result<(), String>;

/// One user-facing command.
pub struct CommandDef {
    pub id: CmdId,
    /// Button text, accessible name and probe key.
    pub label: &'static str,
    pub icon: Icon,
    pub tooltip: &'static str,
    pub fusion_keys: &'static [Chord],
    pub classic_keys: &'static [Chord],
    pub places: &'static [Place],
    pub mm: Option<MmSlot>,
    pub enabled: Enabled,
    pub run: fn(&mut RcadApp),
}

impl CommandDef {
    pub fn keys(&self, layer: KeymapLayer) -> &'static [Chord] {
        match layer {
            KeymapLayer::Fusion => self.fusion_keys,
            KeymapLayer::Classic => self.classic_keys,
        }
    }
    /// Keys as text ("Ctrl+Z, Ctrl+Y"); empty when unbound.
    pub fn key_text(&self, layer: KeymapLayer) -> String {
        self.keys(layer)
            .iter()
            .map(|c| c.text())
            .collect::<Vec<_>>()
            .join(", ")
    }
    pub fn ribbon_places(&self) -> impl Iterator<Item = (Tab, Group, bool)> + '_ {
        self.places.iter().filter_map(|p| match p {
            Place::Ribbon {
                tab,
                group,
                promoted,
            } => Some((*tab, *group, *promoted)),
            _ => None,
        })
    }
}

const fn ribbon(tab: Tab, group: Group, promoted: bool) -> Place {
    Place::Ribbon {
        tab,
        group,
        promoted,
    }
}
const S_CREATE: Place = ribbon(Tab::Solid, Group::Create, true);
const S_CREATE_MORE: Place = ribbon(Tab::Solid, Group::Create, false);
const S_MODIFY: Place = ribbon(Tab::Solid, Group::Modify, true);
const S_MODIFY_MORE: Place = ribbon(Tab::Solid, Group::Modify, false);
const S_ASSEMBLE: Place = ribbon(Tab::Solid, Group::Assemble, true);
const S_ASSEMBLE_MORE: Place = ribbon(Tab::Solid, Group::Assemble, false);
const S_CONFIGURE: Place = ribbon(Tab::Solid, Group::Configure, true);
const S_CONSTRUCT: Place = ribbon(Tab::Solid, Group::Construct, true);
const S_CONSTRUCT_MORE: Place = ribbon(Tab::Solid, Group::Construct, false);
const S_INSPECT: Place = ribbon(Tab::Solid, Group::Inspect, true);
const S_INSPECT_MORE: Place = ribbon(Tab::Solid, Group::Inspect, false);
const S_INSERT: Place = ribbon(Tab::Solid, Group::Insert, true);
const S_INSERT_MORE: Place = ribbon(Tab::Solid, Group::Insert, false);
const S_SELECT: Place = ribbon(Tab::Solid, Group::Select, true);

fn always(_: &RcadApp) -> Result<(), String> {
    Ok(())
}
fn open(a: &RcadApp) -> Result<(), String> {
    a.need_open()
}

macro_rules! def {
    ($id:ident, $label:expr, $icon:ident, $tip:expr,
     fusion: [$($fk:expr),*], classic: [$($ck:expr),*],
     places: [$($p:expr),*], mm: $mm:expr,
     enabled: $en:expr, run: $run:expr) => {
        CommandDef {
            id: CmdId::$id,
            label: $label,
            icon: Icon::$icon,
            tooltip: $tip,
            fusion_keys: &[$($fk),*],
            classic_keys: &[$($ck),*],
            places: &[$($p),*],
            mm: $mm,
            enabled: $en,
            run: $run,
        }
    };
}

/// Every command, in menu order within each placement.
pub static COMMANDS: &[CommandDef] = &[
    // ----- File / application bar -----
    def!(New, "New", New, "New document",
        fusion: [cmd(Key::N)], classic: [cmd(Key::N)],
        places: [Place::AppBar, Place::File], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::New)),
    def!(Open, "Open…", Open, "Open a native document",
        fusion: [cmd(Key::O)], classic: [cmd(Key::O)],
        places: [Place::AppBar, Place::File], mm: None,
        enabled: always, run: |a| a.open_dialog(false)),
    def!(OpenSafe, "Open (safe mode)…", Open,
        "Open a document rolled back to just before its first failing feature",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: always, run: |a| a.open_dialog(true)),
    def!(Save, "Save", Save, "Save",
        fusion: [cmd(Key::S)], classic: [cmd(Key::S)],
        places: [Place::AppBar, Place::File], mm: None,
        enabled: open, run: |a| a.save()),
    def!(SaveAs, "Save As…", Save, "Save as",
        fusion: [cmd_shift(Key::S)], classic: [cmd_shift(Key::S)],
        places: [Place::AppBar, Place::File], mm: None,
        enabled: open, run: |a| a.save_as_dialog()),
    def!(Import, "Import…", Import, "Import STEP / STL / DXF into this document (one undoable step)",
        fusion: [], classic: [],
        places: [S_INSERT, Place::File], mm: None,
        enabled: open, run: |a| a.open_import_dialog()),
    def!(Close, "Close", Generic, "Close this document",
        fusion: [cmd(Key::W)], classic: [],
        places: [Place::AppBar, Place::File], mm: None,
        enabled: open, run: |a| a.guarded(Guarded::Close)),
    def!(ExportStl, "Export STL…", Export, "Binary STL of the selected or visible bodies (print checks, placement)",
        fusion: [], classic: [],
        places: [Place::FileExport, ribbon(Tab::Utilities, Group::Make, true)], mm: None,
        enabled: open, run: |a| a.open_print_export_dialog(true)),
    def!(ExportStep, "Export STEP…", Export, "STEP (AP214) of the selected or visible bodies",
        fusion: [], classic: [],
        places: [Place::FileExport], mm: None,
        enabled: open, run: |a| a.export_step_dialog()),
    def!(ExportDxf, "Export DXF (sketch)…", Export, "DXF of a sketch",
        fusion: [], classic: [],
        places: [Place::FileExport], mm: None,
        enabled: |a| a.need_sketch(), run: |a| a.export_sketch_dialog(false)),
    def!(ExportSvg, "Export SVG (sketch)…", Export, "SVG of a sketch",
        fusion: [], classic: [],
        places: [Place::FileExport], mm: None,
        enabled: |a| a.need_sketch(), run: |a| a.export_sketch_dialog(true)),
    def!(Export3mf, "Export 3MF…", Export, "3MF of the selected or visible bodies (print checks, placement)",
        fusion: [], classic: [],
        places: [Place::FileExport, ribbon(Tab::Utilities, Group::Make, true)], mm: None,
        enabled: open, run: |a| a.open_print_export_dialog(false)),
    def!(Undo, "Undo", Undo, "Undo the last document step",
        fusion: [cmd(Key::Z)], classic: [cmd(Key::Z)],
        places: [Place::AppBar], mm: Some(MmSlot::W),
        enabled: |a| a.undo_blocker(), run: |a| a.undo()),
    def!(Redo, "Redo", Redo, "Redo the last undone step",
        fusion: [cmd_shift(Key::Z), cmd(Key::Y)], classic: [cmd_shift(Key::Z), cmd(Key::Y)],
        places: [Place::AppBar], mm: Some(MmSlot::E),
        enabled: |a| a.redo_blocker(), run: |a| a.redo()),
    def!(Help, "Help", Help, "Navigation, keyboard shortcuts (generated from the command list)",
        fusion: [k(Key::F1)], classic: [k(Key::F1)],
        places: [Place::AppBar], mm: None,
        enabled: always, run: |a| a.show_help = !a.show_help),
    def!(About, "About R-CAD", Help, "Version, license and source of this build",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: always, run: |a| a.shell.show_about = true),
    def!(Quit, "Quit", Generic, "Close R-CAD (asks first when there are unsaved changes)",
        fusion: [cmd(Key::Q)], classic: [cmd(Key::Q)],
        places: [Place::File], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::Exit)),
    def!(CommandSearch, "Command search", Select,
        "Find any command by name and run it",
        fusion: [k(Key::S)], classic: [],
        places: [Place::AppBar], mm: None,
        enabled: always, run: |a| a.open_command_search()),
    // ----- Cloud (optional server tier) -----
    #[cfg(feature = "cloud")]
    def!(DataPanel, "Data panel", Generic,
        "Projects and documents on an R-CAD server (optional; R-CAD works fully offline)",
        fusion: [], classic: [],
        places: [Place::AppBar], mm: None,
        enabled: always, run: |a| a.toggle_data_panel()),
    #[cfg(feature = "cloud")]
    def!(SaveVersion, "Save Version to Cloud…", Save,
        "Save the open cloud document as a new version on the server (saves locally first)",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: |a| a.cloud_save_version_blocker(), run: |a| a.cloud_save_version_dialog()),
    #[cfg(feature = "cloud")]
    def!(VersionHistory, "Version History", Generic,
        "Versions of the open cloud document: open read-only, promote, milestones",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: |a| a.cloud_linked_blocker(), run: |a| a.cloud_version_history()),
    #[cfg(feature = "cloud")]
    def!(ShareLink, "Share Link…", Generic,
        "A link to the open cloud document for people without an account",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: |a| a.cloud_linked_blocker(), run: |a| a.cloud_share_dialog()),
    #[cfg(feature = "cloud")]
    def!(CloudComments, "Comments", Generic,
        "Comments on the open cloud document",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: |a| a.cloud_linked_blocker(), run: |a| a.cloud_comments()),
    #[cfg(feature = "cloud")]
    def!(SyncNow, "Sync Now", Generic,
        "Send the versions saved while the server was unreachable",
        fusion: [], classic: [],
        places: [Place::File], mm: None,
        enabled: |a| a.cloud_sync_blocker(), run: |a| a.cloud_sync_now()),
    // ----- View -----
    def!(Fit, "Fit view", View, "Fit the model in the view (in a sketch: look at the sketch)",
        fusion: [k(Key::F6)], classic: [k(Key::F)],
        places: [Place::View], mm: Some(MmSlot::Ctx),
        enabled: always, run: |a| a.fit_view()),
    def!(Section, "Section view…", Section,
        "Display-only clipping plane (not capped); the model and exports are unchanged",
        fusion: [], classic: [],
        places: [Place::View, S_INSPECT_MORE], mm: None,
        enabled: always, run: |a| a.toggle_section()),
    def!(ViewTop, "Top", View, "Top view",
        fusion: [k(Key::Num7)], classic: [k(Key::Num7)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Top)),
    def!(ViewFront, "Front", View, "Front view",
        fusion: [k(Key::Num1)], classic: [k(Key::Num1)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Front)),
    def!(ViewRight, "Right", View, "Right view",
        fusion: [k(Key::Num3)], classic: [k(Key::Num3)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Right)),
    def!(ViewLeft, "Left", View, "Left view",
        fusion: [k(Key::Num4)], classic: [k(Key::Num4)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Left)),
    def!(ViewBack, "Back", View, "Back view",
        fusion: [k(Key::Num6)], classic: [k(Key::Num6)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Back)),
    def!(ViewIso, "Iso", View, "Isometric view",
        fusion: [k(Key::Num0)], classic: [k(Key::Num0)],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Iso)),
    def!(ViewBottom, "Bottom", View, "Bottom view",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_view(rcad_viewport::ViewPreset::Bottom)),
    def!(Orbit, "Orbit", View, "Left drag orbits the view (middle drag always does)",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.shell.nav_mode = super::viewcube::NavMode::Orbit),
    def!(Pan, "Pan", View, "Left drag pans the view (right drag always does)",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.shell.nav_mode = super::viewcube::NavMode::Pan),
    def!(Zoom, "Zoom", View, "Left drag up/down zooms (the wheel always does)",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.shell.nav_mode = super::viewcube::NavMode::Zoom),
    def!(LookAt, "Look At", View, "Look square onto the selected planar face or the open sketch",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: |a| a.look_at_blocker(), run: |a| a.look_at_selection()),
    def!(StyleEdges, "Shaded with edges", View, "Shaded faces with their edges drawn",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_display_edges(true)),
    def!(StyleShaded, "Shaded", View, "Shaded faces without edges",
        fusion: [], classic: [],
        places: [Place::View], mm: None,
        enabled: always, run: |a| a.set_display_edges(false)),
    // ----- SOLID ▸ CREATE -----
    def!(CreateSketch, "Create Sketch", Sketch,
        "Sketch on the selected face, otherwise on the world XY plane",
        fusion: [], classic: [],
        places: [S_CREATE], mm: Some(MmSlot::S),
        enabled: |a| a.create_sketch_target().map(|_| ()), run: |a| a.create_sketch()),
    def!(Extrude, "Extrude…", Extrude, "Extrude a closed sketch profile (new body, join, cut, intersect)",
        fusion: [k(Key::E)], classic: [],
        places: [S_CREATE], mm: Some(MmSlot::Ctx),
        enabled: |a| a.extrude_cmd_blocker(), run: |a| a.open_extrude()),
    def!(Revolve, "Revolve…", Revolve, "Revolve a closed sketch profile about an axis",
        fusion: [], classic: [],
        places: [S_CREATE], mm: None,
        enabled: |a| a.revolve_cmd_blocker(), run: |a| a.open_revolve()),
    def!(Hole, "Hole", Hole, "Round hole (through or blind, optional counterbore) into the selected planar face",
        fusion: [k(Key::H)], classic: [],
        places: [S_CREATE], mm: Some(MmSlot::SE),
        enabled: |a| a.need_sketchable_face().map(|_| ()), run: |a| a.open_hole()),
    def!(CutThroughAll, "Cut through all…", Cut,
        "Cut a closed sketch profile through every body it crosses (Extrude, operation Cut)",
        fusion: [], classic: [],
        places: [S_CREATE], mm: None,
        enabled: |a| a.cut_cmd_blocker(), run: |a| a.open_cut()),
    def!(Thread, "Thread…", Thread,
        "Modeled thread (faceted solid, stated 60° V test profile): a rod or a square nut",
        fusion: [], classic: [],
        places: [S_CREATE], mm: None,
        enabled: open, run: |a| a.open_thread()),
    def!(Loft, "Loft…", Loft,
        "New body between two closed profiles on parallel planes (exact; same-count polygons with parallel edges, or coaxial circles)",
        fusion: [], classic: [],
        places: [S_CREATE_MORE], mm: None,
        enabled: |a| a.need_sketch(), run: |a| a.open_loft_sweep(false)),
    def!(Sweep, "Sweep…", Sweep,
        "New body: a closed profile swept along a line/arc path sketch starting at the profile sketch's origin (exact)",
        fusion: [], classic: [],
        places: [S_CREATE_MORE], mm: None,
        enabled: |a| a.need_sketch(), run: |a| a.open_loft_sweep(true)),
    def!(Pattern, "Pattern", Pattern,
        "Repeat the selected Extrude join/cut or hole in a row, grid or around a centre",
        fusion: [], classic: [],
        places: [S_CREATE_MORE], mm: None,
        enabled: |a| a.pattern_source().map(|_| ()), run: |a| a.open_pattern(false)),
    def!(Mirror, "Mirror", Mirror,
        "Mirror the selected Extrude join/cut or hole across a world plane or the selected face",
        fusion: [], classic: [],
        places: [S_CREATE_MORE], mm: None,
        enabled: |a| a.pattern_source().map(|_| ()), run: |a| a.open_pattern(true)),
    // ----- SOLID ▸ MODIFY -----
    def!(PressPull, "Press Pull", PressPull,
        "Not available yet: use Edit Feature (double-click in History) or Direct Edit on imported faces",
        fusion: [k(Key::Q)], classic: [],
        places: [S_MODIFY_MORE], mm: Some(MmSlot::NE),
        enabled: |_| Err("Press Pull is not available yet: double-click a feature in History to edit it, or use Direct Edit on an imported face".into()),
        run: |_| {}),
    def!(Fillet, "Fillet", Fillet,
        "Round the straight edge where the selected face meets another planar face (exact cylinder)",
        fusion: [k(Key::F)], classic: [],
        places: [S_MODIFY], mm: Some(MmSlot::Ctx),
        enabled: |a| a.need_sketchable_face().map(|_| ()), run: |a| a.open_chamfer(true)),
    def!(Chamfer, "Chamfer", Chamfer,
        "Chamfer the straight edge where the selected face meets another planar face",
        fusion: [], classic: [],
        places: [S_MODIFY], mm: Some(MmSlot::Ctx),
        enabled: |a| a.need_sketchable_face().map(|_| ()), run: |a| a.open_chamfer(false)),
    def!(FilletLoop, "Fillet face loop", Fillet,
        "Round every edge around the selected face's outer boundary",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.need_sketchable_face().map(|_| ()), run: |a| a.open_loop(false)),
    def!(ChamferLoop, "Chamfer face loop", Chamfer,
        "Chamfer every edge around the selected face's outer boundary at 45°",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.need_sketchable_face().map(|_| ()), run: |a| a.open_loop(true)),
    def!(Shell, "Shell", Shell,
        "Hollow the body under the selected face (left open), or a selected New Body extrusion",
        fusion: [], classic: [],
        places: [S_MODIFY], mm: None,
        enabled: |a| a.shell_blocker(), run: |a| a.open_shell()),
    def!(Align, "Align", Align, "Align the selected face's body flush onto a face of another body",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.align_blocker(), run: |a| a.open_align()),
    def!(DirectEdit, "Direct Edit", PressPull,
        "Offset, tilt, resize or remove a face of an imported body (exact)",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.direct_edit_blocker(), run: |a| a.open_direct_edit()),
    def!(MoveCopy, "Move/Copy", Move, "Move or copy the selected body",
        fusion: [k(Key::M)], classic: [],
        places: [S_MODIFY], mm: Some(MmSlot::SW),
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::MoveCopy)),
    def!(Scale, "Scale", Scale, "Scale the selected body about the origin",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::MoveCopy)),
    def!(SplitBody, "Split Body", Split, "Split the selected body by a plane",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::Split)),
    def!(Combine, "Combine…", Combine, "Join, cut or intersect the selected body with others",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: None,
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::Combine)),
    def!(Material, "Physical Material", Material, "Set the selected body's material (density for mass)",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE], mm: Some(MmSlot::Ctx),
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::Material)),
    def!(ChangeParameters, "Change Parameters", Parameters, "User parameters (names, expressions, units)",
        fusion: [], classic: [],
        places: [S_MODIFY_MORE, ribbon(Tab::Utilities, Group::Modify, true)], mm: None,
        enabled: open, run: |a| a.focus(Focus::Parameters)),
    def!(Delete, "Delete", Delete, "Delete the selected feature (Undo restores it)",
        fusion: [k(Key::Delete)], classic: [k(Key::Delete)],
        places: [S_MODIFY_MORE], mm: Some(MmSlot::NW),
        enabled: |a| a.delete_blocker(), run: |a| a.delete_key()),
    // ----- SOLID ▸ ASSEMBLE -----
    def!(NewComponent, "New Component", Component,
        "Create a component (empty, from the selected body, or an external document)",
        fusion: [], classic: [],
        places: [S_ASSEMBLE], mm: Some(MmSlot::Ctx),
        enabled: open, run: |a| a.focus(Focus::Components)),
    def!(Joint, "Joint", Joint, "Joint between two occurrences (select a component row first)",
        fusion: [k(Key::J)], classic: [],
        places: [S_ASSEMBLE], mm: None,
        enabled: |a| a.need_occurrences(2), run: |a| a.focus(Focus::Components)),
    def!(RigidGroup, "Rigid Group", Joint, "Lock several occurrences together (keeps their relative placement)",
        fusion: [], classic: [],
        places: [S_ASSEMBLE_MORE], mm: None,
        enabled: |a| a.need_occurrences(2), run: |a| a.open_rigid_group()),
    def!(DriveJoints, "Drive Joints", Joint, "Set the values of several joints at once",
        fusion: [], classic: [],
        places: [S_ASSEMBLE_MORE], mm: None,
        enabled: |a| a.need_joints(), run: |a| a.open_drive_joints()),
    def!(BillOfMaterials, "Bill of Materials", Bom, "Bill of materials (export CSV)",
        fusion: [], classic: [],
        places: [S_ASSEMBLE_MORE], mm: None,
        enabled: |a| a.need_occurrences(1), run: |a| a.focus(Focus::Components)),
    def!(ExportAssembly, "Export STEP assembly…", Export,
        "STEP with the component structure kept (one product per component)",
        fusion: [], classic: [],
        places: [Place::FileExport, S_ASSEMBLE_MORE], mm: None,
        enabled: |a| a.need_occurrences(1), run: |a| a.export_assembly_dialog()),
    // ----- SOLID ▸ CONFIGURE -----
    def!(Configure, "Configure", Configure, "Save, activate or delete named configurations",
        fusion: [], classic: [],
        places: [S_CONFIGURE], mm: None,
        enabled: open, run: |a| a.focus(Focus::Configurations)),
    def!(ConfigTable, "Configuration Table", Configure, "Edit configurations as a table; export variants",
        fusion: [], classic: [],
        places: [ribbon(Tab::Solid, Group::Configure, false)], mm: None,
        enabled: open, run: |a| a.focus(Focus::ConfigTable)),
    // ----- SOLID ▸ CONSTRUCT -----
    def!(OffsetPlane, "Sketch on offset plane…", Plane,
        "Sketch on a plane offset (and tilted) from a world plane or the selected face",
        fusion: [], classic: [],
        places: [S_CONSTRUCT], mm: None,
        enabled: open, run: |a| a.open_offset_plane_dialog()),
    def!(SketchXY, "Sketch on XY", Plane, "Create a sketch on the world XY plane",
        fusion: [], classic: [],
        places: [S_CONSTRUCT_MORE], mm: None,
        enabled: open, run: |a| a.begin_sketch(SketchTarget::New { plane: SketchPlane::WorldXY })),
    def!(SketchXZ, "Sketch on XZ", Plane, "Create a sketch on the world XZ plane",
        fusion: [], classic: [],
        places: [S_CONSTRUCT_MORE], mm: None,
        enabled: open, run: |a| a.begin_sketch(SketchTarget::New { plane: SketchPlane::WorldXZ })),
    def!(SketchYZ, "Sketch on YZ", Plane, "Create a sketch on the world YZ plane",
        fusion: [], classic: [],
        places: [S_CONSTRUCT_MORE], mm: None,
        enabled: open, run: |a| a.begin_sketch(SketchTarget::New { plane: SketchPlane::WorldYZ })),
    // ----- SOLID ▸ INSPECT -----
    def!(Measure, "Measure", Measure, "Measure the selected face (area, distances; pick a second face)",
        fusion: [k(Key::I)], classic: [],
        places: [S_INSPECT], mm: Some(MmSlot::Ctx),
        enabled: |a| a.need_face(), run: |a| a.start_measure()),
    def!(Interference, "Interference", Interference, "Check interference between occurrences",
        fusion: [], classic: [],
        places: [S_INSPECT_MORE], mm: None,
        enabled: |a| a.need_occurrences(2), run: |a| a.focus(Focus::Components)),
    def!(MassProperties, "Properties", Measure, "Mass properties of the selected body",
        fusion: [], classic: [],
        places: [S_INSPECT_MORE], mm: Some(MmSlot::Ctx),
        enabled: |a| a.need_body(), run: |a| a.focus(Focus::Material)),
    // ----- SOLID ▸ INSERT -----
    def!(NewDrawing, "New Drawing", Drawing, "Create a drawing of the selected body or the assembly",
        fusion: [], classic: [],
        places: [S_INSERT_MORE, Place::File], mm: None,
        enabled: open, run: |a| a.focus(Focus::Drawings)),
    def!(DrawingViews, "Drawing Views", Drawing,
        "Edit or delete drawing views, hole callouts and dimension tolerances",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: |a| a.need_drawing(), run: |a| a.focus(Focus::Drawings)),
    def!(TemplateEnclosure, PrintTemplate::Enclosure.label(), Template,
        "Starts a new document; change its sizes in the Parameters panel",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::NewTemplate(PrintTemplate::Enclosure))),
    def!(TemplateCoupon, PrintTemplate::Coupon.label(), Template,
        "Starts a new document; change its sizes in the Parameters panel",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::NewTemplate(PrintTemplate::Coupon))),
    def!(TemplateBracket, PrintTemplate::Bracket.label(), Template,
        "Starts a new document; change its sizes in the Parameters panel",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::NewTemplate(PrintTemplate::Bracket))),
    def!(TemplateTray, PrintTemplate::Tray.label(), Template,
        "Starts a new document; change its sizes in the Parameters panel",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: always, run: |a| a.guarded(Guarded::NewTemplate(PrintTemplate::Tray))),
    def!(RectangleXY, "Rectangle on XY…", Template, "Dimensioned rectangle sketch on world XY",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: open, run: |a| a.open_rectangle(false)),
    def!(RectangleXZ, "Rectangle on XZ…", Template, "Dimensioned rectangle sketch on world XZ (for a revolve)",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: open, run: |a| a.open_rectangle(true)),
    def!(CircleOnFace, "Circle on face…", Template, "Dimensioned circle sketch on an extrusion cap",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: |a| a.need_extrude(), run: |a| a.open_circle_on_face()),
    def!(PlaceRectangle, "Place rectangle", Template, "Drag a rectangle on the ground plane",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: open, run: |a| a.start_tool(crate::place::Tool::Rectangle)),
    def!(PlaceHole, "Place hole", Hole, "Drag a hole on the end cap of the last extrusion",
        fusion: [], classic: [],
        places: [S_INSERT_MORE], mm: None,
        enabled: |a| a.need_extrude(), run: |a| a.start_place_hole()),
    // ----- SOLID ▸ SELECT -----
    def!(ClearSelection, "Clear Selection", Select, "Clear the selection",
        fusion: [], classic: [],
        places: [S_SELECT], mm: None,
        enabled: |a| a.need_selection(), run: |a| a.clear_selection()),
    // ----- MESH -----
    def!(MeshRepair, "Repair", Mesh, "Check and repair the selected mesh body",
        fusion: [], classic: [],
        places: [ribbon(Tab::Mesh, Group::Prepare, true)], mm: None,
        enabled: |a| a.need_mesh_body(), run: |a| a.focus(Focus::MeshRepair)),
    def!(MeshEdit, "Mesh Edit", Mesh, "Reduce, plane-cut or move the selected mesh body",
        fusion: [], classic: [],
        places: [ribbon(Tab::Mesh, Group::Modify, true)], mm: None,
        enabled: |a| a.need_mesh_body(), run: |a| a.focus(Focus::MeshEdit)),
    // ----- SHEET METAL / SURFACE (forms of the selected sketch) -----
    def!(SheetFlange, "Flange", Extrude, "Sheet-metal flange from the selected open sketch (line chain)",
        fusion: [], classic: [],
        places: [ribbon(Tab::SheetMetal, Group::Create, true)], mm: None,
        enabled: |a| a.need_selected_sketch(), run: |a| a.focus(Focus::SheetFlange)),
    def!(SheetBase, "Base Face", Plane, "Sheet-metal base face from the selected closed sketch",
        fusion: [], classic: [],
        places: [ribbon(Tab::SheetMetal, Group::Create, true)], mm: None,
        enabled: |a| a.need_selected_sketch(), run: |a| a.focus(Focus::SheetBase)),
    def!(SurfacePatch, "Patch", Plane, "Planar patch surface from the selected closed sketch",
        fusion: [], classic: [],
        places: [ribbon(Tab::Surface, Group::Create, true)], mm: None,
        enabled: |a| a.need_selected_sketch(), run: |a| a.focus(Focus::SurfaceFromSketch)),
    def!(SurfaceExtrude, "Extrude Surface", Extrude, "Extruded surface from the selected sketch",
        fusion: [], classic: [],
        places: [ribbon(Tab::Surface, Group::Create, true)], mm: None,
        enabled: |a| a.need_selected_sketch(), run: |a| a.focus(Focus::SurfaceFromSketch)),
];

/// The definition of `id`.
pub fn def(id: CmdId) -> &'static CommandDef {
    COMMANDS
        .iter()
        .find(|d| d.id == id)
        .expect("every CmdId has a CommandDef (registry_ids_are_complete)")
}

/// Commands placed at `place`, in table order.
pub fn at(place: Place) -> impl Iterator<Item = &'static CommandDef> {
    COMMANDS.iter().filter(move |d| d.places.contains(&place))
}

/// Groups of `tab` that hold at least one command, in ribbon order.
pub fn groups(tab: Tab) -> Vec<Group> {
    let mut g: Vec<Group> = COMMANDS
        .iter()
        .flat_map(|d| d.ribbon_places())
        .filter(|(t, _, _)| *t == tab)
        .map(|(_, g, _)| g)
        .collect();
    g.sort();
    g.dedup();
    g
}

/// Commands of `tab` ▸ `group` (`promoted_only`: the ribbon face).
pub fn in_group(
    tab: Tab,
    group: Group,
    promoted_only: bool,
) -> impl Iterator<Item = &'static CommandDef> {
    COMMANDS.iter().filter(move |d| {
        d.ribbon_places()
            .any(|(t, g, p)| t == tab && g == group && (p || !promoted_only))
    })
}

/// One row of the generated shortcut sheet: label, key in `layer`, where.
pub fn help_rows(layer: KeymapLayer) -> Vec<(&'static str, String, String)> {
    COMMANDS
        .iter()
        .filter(|d| !d.keys(layer).is_empty())
        .map(|d| (d.label, d.key_text(layer), placement_text(d)))
        .collect()
}

/// "Ctrl+N new · Ctrl+O open · …" for the bound commands `pick` selects.
pub fn shortcut_line(layer: KeymapLayer, pick: fn(&CommandDef) -> bool) -> String {
    COMMANDS
        .iter()
        .filter(|d| pick(d) && !d.keys(layer).is_empty())
        .map(|d| {
            format!(
                "{} {}",
                d.keys(layer)
                    .iter()
                    .map(|c| c.text())
                    .collect::<Vec<_>>()
                    .join(" / "),
                d.label.trim_end_matches('…').to_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// "SOLID ▸ CREATE", "File", … (first placement).
pub fn placement_text(d: &CommandDef) -> String {
    match d.places.first() {
        Some(Place::File) => "File".into(),
        Some(Place::FileExport) => "File › Export".into(),
        Some(Place::AppBar) => "Application bar".into(),
        Some(Place::View) => "View".into(),
        Some(Place::Ribbon { tab, group, .. }) => format!("{} › {}", tab.label(), group.label()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_IDS: &[CmdId] = &[
        CmdId::New,
        CmdId::Open,
        CmdId::OpenSafe,
        CmdId::Save,
        CmdId::SaveAs,
        CmdId::Import,
        CmdId::Close,
        CmdId::ExportStl,
        CmdId::ExportStep,
        CmdId::ExportDxf,
        CmdId::ExportSvg,
        CmdId::Export3mf,
        CmdId::Undo,
        CmdId::Redo,
        CmdId::Help,
        CmdId::About,
        CmdId::Quit,
        #[cfg(feature = "cloud")]
        CmdId::DataPanel,
        #[cfg(feature = "cloud")]
        CmdId::SaveVersion,
        #[cfg(feature = "cloud")]
        CmdId::VersionHistory,
        #[cfg(feature = "cloud")]
        CmdId::ShareLink,
        #[cfg(feature = "cloud")]
        CmdId::CloudComments,
        #[cfg(feature = "cloud")]
        CmdId::SyncNow,
        CmdId::Fit,
        CmdId::Section,
        CmdId::ViewTop,
        CmdId::ViewFront,
        CmdId::ViewRight,
        CmdId::ViewLeft,
        CmdId::ViewBack,
        CmdId::ViewIso,
        CmdId::ViewBottom,
        CmdId::Orbit,
        CmdId::Pan,
        CmdId::Zoom,
        CmdId::LookAt,
        CmdId::StyleEdges,
        CmdId::StyleShaded,
        CmdId::CreateSketch,
        CmdId::SketchXY,
        CmdId::SketchXZ,
        CmdId::SketchYZ,
        CmdId::Extrude,
        CmdId::CutThroughAll,
        CmdId::Revolve,
        CmdId::Hole,
        CmdId::Thread,
        CmdId::Loft,
        CmdId::Sweep,
        CmdId::Pattern,
        CmdId::Mirror,
        CmdId::TemplateEnclosure,
        CmdId::TemplateCoupon,
        CmdId::TemplateBracket,
        CmdId::TemplateTray,
        CmdId::RectangleXY,
        CmdId::RectangleXZ,
        CmdId::CircleOnFace,
        CmdId::PlaceRectangle,
        CmdId::PlaceHole,
        CmdId::PressPull,
        CmdId::Fillet,
        CmdId::Chamfer,
        CmdId::FilletLoop,
        CmdId::ChamferLoop,
        CmdId::Shell,
        CmdId::Align,
        CmdId::DirectEdit,
        CmdId::MoveCopy,
        CmdId::Scale,
        CmdId::SplitBody,
        CmdId::Combine,
        CmdId::Material,
        CmdId::Delete,
        CmdId::ChangeParameters,
        CmdId::NewComponent,
        CmdId::Joint,
        CmdId::RigidGroup,
        CmdId::DriveJoints,
        CmdId::BillOfMaterials,
        CmdId::ExportAssembly,
        CmdId::Configure,
        CmdId::ConfigTable,
        CmdId::OffsetPlane,
        CmdId::Measure,
        CmdId::Interference,
        CmdId::MassProperties,
        CmdId::NewDrawing,
        CmdId::DrawingViews,
        CmdId::ClearSelection,
        CmdId::CommandSearch,
        CmdId::MeshRepair,
        CmdId::MeshEdit,
        CmdId::SheetFlange,
        CmdId::SheetBase,
        CmdId::SurfacePatch,
        CmdId::SurfaceExtrude,
    ];

    #[test]
    fn registry_ids_are_complete_and_unique() {
        for id in ALL_IDS {
            assert_eq!(
                COMMANDS.iter().filter(|d| d.id == *id).count(),
                1,
                "{id:?} must have exactly one definition"
            );
        }
        assert_eq!(COMMANDS.len(), ALL_IDS.len(), "every definition is listed");
    }

    #[test]
    fn labels_are_unique() {
        for d in COMMANDS {
            assert_eq!(
                COMMANDS.iter().filter(|e| e.label == d.label).count(),
                1,
                "duplicate label {}",
                d.label
            );
        }
    }

    #[test]
    fn no_key_is_bound_twice_in_a_layer() {
        for layer in [KeymapLayer::Fusion, KeymapLayer::Classic] {
            let mut seen: Vec<(Chord, &str)> = vec![];
            for d in COMMANDS {
                for c in d.keys(layer) {
                    if let Some((_, other)) = seen.iter().find(|(s, _)| s == c) {
                        panic!("{layer:?}: {} bound to {} and {other}", c.text(), d.label);
                    }
                    seen.push((*c, d.label));
                }
            }
        }
    }

    #[test]
    fn fusion_layer_has_the_documented_defaults() {
        let key = |id| def(id).key_text(KeymapLayer::Fusion);
        assert_eq!(key(CmdId::Extrude), "E");
        assert_eq!(key(CmdId::Hole), "H");
        assert_eq!(key(CmdId::Fillet), "F");
        assert_eq!(key(CmdId::PressPull), "Q");
        assert_eq!(key(CmdId::MoveCopy), "M");
        assert_eq!(key(CmdId::Joint), "J");
        assert_eq!(key(CmdId::Measure), "I");
        assert_eq!(def(CmdId::Fit).key_text(KeymapLayer::Classic), "F");
    }

    #[test]
    fn every_command_is_placed() {
        for d in COMMANDS {
            assert!(!d.places.is_empty(), "{} has no placement", d.label);
        }
    }
}
