//! Interactive sketch editor: tools, snapping, picking, constraint-respecting
//! drags, dimensions, and a local undo stack.
//!
//! The editor works on a private copy of an [`rcad_sketch::Sketch`] in the
//! sketch plane's 2D frame (mm). Nothing here touches the document: the app
//! sends one `create_sketch` (template `custom`) or `edit_sketch` command when
//! the user presses Finish Sketch, which becomes one document undo entry.
//! Cancel simply drops the editor.
//!
//! Inputs are plane coordinates plus a [`View`] that carries the screen scale,
//! so every pick/snap tolerance is in screen pixels (see [`PICK_PX`],
//! [`SNAP_PX`], [`ALIGN_PX`]) and independent of zoom. Screen-space targets
//! (dimension labels, constraint glyphs) are registered each frame by the
//! painter via [`SketchEditor::set_screen_targets`].
//!
//! Snap priority (deterministic): existing point within [`SNAP_PX`] (nearest
//! wins; reusing the point makes the endpoints shared = coincident) > horizontal
//! / vertical alignment with the previous point of the active gesture within
//! [`ALIGN_PX`] (adds a Horizontal/Vertical constraint to the new line) > grid
//! (position only, no constraint). Shift disables point, alignment and grid snaps.

use eframe::egui;
use rcad_sketch::{
    Constraint, Dimension, DragOutcome, Entity, EntityId, PointId, Sketch, SolveReport, SolveStatus,
};

/// Pick tolerance for points and curves, screen pixels.
pub const PICK_PX: f64 = 8.0;
/// Snap radius for existing points, screen pixels.
pub const SNAP_PX: f64 = 10.0;
/// Horizontal / vertical alignment capture band, screen pixels.
pub const ALIGN_PX: f64 = 8.0;
/// On-screen spacing (px) of the sketch snapping step: a power of ten in mm
/// at least half this far apart (finer than the drawn grid).
pub const SNAP_CELL_PX: f32 = 5.0;
/// Local undo depth inside one edit session.
const MAX_LOCAL_UNDO: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SketchTool {
    Select,
    Line,
    CenterRectangle,
    CornerRectangle,
    Circle,
    /// Three-point arc: start, end, then a point the arc passes through.
    Arc,
    Dimension,
    /// Remove the clicked part of a line between its nearest intersections.
    Trim,
    /// Extend a line's free end to the nearest curve along its direction.
    Extend,
    /// Break a curve at the crossings either side of the click (nothing is
    /// removed; the clicked span becomes its own curve).
    Break,
    /// Project a model edge into the sketch: click a face next to the edge
    /// (the app resolves the face; see `ProjectPick`).
    Project,
    /// Intersect: the section of the clicked body by the sketch plane.
    Intersect,
    /// Offset a closed loop by an editable distance (docs/behavior/sketching.md).
    Offset,
    /// SKETCH corner fillet: round the corner where two lines (or a line and an
    /// arc) meet.
    Fillet,
    /// Sketch text: click to place (or click a text to edit it).
    Text,
    /// Centre-to-centre straight slot: two centres, then the width.
    Slot,
    /// Regular polygon: centre, then a vertex (inscribed) or an edge
    /// midpoint (circumscribed); sides and fit from the toolbar.
    Polygon,
    /// Full ellipse: centre, axis end, then a point setting the other radius.
    Ellipse,
    /// Cubic B-spline through clicked control points (open, or closed by
    /// clicking the first point again).
    Spline,
}

impl SketchTool {
    /// The sketch toolbar's tools, in order (the toolbar iterates this).
    pub const TOOLBAR: [SketchTool; 16] = [
        SketchTool::Select,
        SketchTool::Line,
        SketchTool::CenterRectangle,
        SketchTool::CornerRectangle,
        SketchTool::Circle,
        SketchTool::Arc,
        SketchTool::Ellipse,
        SketchTool::Spline,
        SketchTool::Dimension,
        SketchTool::Trim,
        SketchTool::Extend,
        SketchTool::Offset,
        SketchTool::Fillet,
        SketchTool::Slot,
        SketchTool::Polygon,
        SketchTool::Text,
    ];

    /// Tools on the sketch bar's second row (the first row is full at 1280 px).
    pub const SECOND_ROW: [SketchTool; 3] = [
        SketchTool::Break,
        SketchTool::Project,
        SketchTool::Intersect,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SketchTool::Select => "Select",
            SketchTool::Line => "Line",
            SketchTool::CenterRectangle => "Center Rect",
            SketchTool::CornerRectangle => "2-Point Rect",
            SketchTool::Circle => "Circle",
            SketchTool::Arc => "3-Point Arc",
            SketchTool::Dimension => "Dimension",
            SketchTool::Trim => "Trim",
            SketchTool::Extend => "Extend",
            SketchTool::Break => "Break",
            SketchTool::Project => "Project",
            SketchTool::Intersect => "Intersect",
            SketchTool::Offset => "Offset",
            SketchTool::Fillet => "Fillet",
            SketchTool::Text => "Text",
            SketchTool::Slot => "Slot",
            SketchTool::Polygon => "Polygon",
            SketchTool::Ellipse => "Ellipse",
            SketchTool::Spline => "Spline",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            SketchTool::Select => "S",
            SketchTool::Line => "L",
            SketchTool::CenterRectangle => "R",
            SketchTool::CornerRectangle => "Shift+R",
            SketchTool::Circle => "C",
            SketchTool::Arc => "A",
            SketchTool::Dimension => "D",
            SketchTool::Trim => "T",
            SketchTool::Extend => "E",
            SketchTool::Break => "toolbar",
            SketchTool::Project => "P",
            SketchTool::Intersect => "toolbar",
            SketchTool::Offset => "O",
            SketchTool::Fillet => "toolbar",
            SketchTool::Text => "toolbar",
            SketchTool::Slot => "toolbar",
            SketchTool::Polygon => "toolbar",
            SketchTool::Ellipse => "toolbar",
            SketchTool::Spline => "toolbar",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SketchTool::Select => {
                "Click to select (Shift adds) · drag a vertex or circle rim to move it · \
                 click a selected dimension (or double-click) to edit · Del deletes"
            }
            SketchTool::Line => {
                "Click points to draw connected lines · click the first point to close · \
                 Enter/right-click ends the chain · Esc cancels"
            }
            SketchTool::CenterRectangle => "Click the centre, then a corner · Esc cancels",
            SketchTool::CornerRectangle => {
                "Click one corner, then the opposite corner · Esc cancels"
            }
            SketchTool::Circle => "Click the centre, then a point on the rim · Esc cancels",
            SketchTool::Arc => {
                "Click the start point, the end point, then a point on the arc · Esc cancels"
            }
            SketchTool::Dimension => {
                "Click a line (length), circle (diameter), arc (radius), or two points (horizontal/vertical distance; the same two points again: the other axis)"
            }
            SketchTool::Trim => {
                "Click the part of a line, arc or circle to remove: it is cut at the nearest \
                 intersections with other curves (a curve with none is deleted)"
            }
            SketchTool::Project => {
                "Click a model face next to the edge to project: the face edge nearest the click \
                 is projected into the sketch (purple, read-only, follows the model while linked) · \
                 Projection Link off: the curves are copied unlinked"
            }
            SketchTool::Intersect => {
                "Click a body: its section by the sketch plane is added (purple, read-only, follows \
                 the model while linked)"
            }
            SketchTool::Break => {
                "Click the span of a line, arc or circle to break off: it is split at the nearest \
                 intersections with other curves, nothing is removed (a circle needs two)"
            }
            SketchTool::Extend => {
                "Click a line near its free end: the end moves along the line to the nearest curve it would cross · lines only"
            }
            SketchTool::Fillet => {
                "Sketch corner fillet: click the corner point where two lines (or a line and an \
                 arc) meet, type the radius, Enter: both curves are trimmed and a tangent arc \
                 with an editable radius dimension is added (2D sketch only; solid edges cannot \
                 be filleted) · Esc cancels"
            }
            SketchTool::Slot => {
                "Click the first arc centre, the second arc centre, then a point on the slot's side \
                 (width) · adds lines + tangent arcs with slot_length / slot_width dimensions · Esc \
                 cancels"
            }
            SketchTool::Polygon => {
                "Click the centre, then a vertex (inscribed) or an edge midpoint (circumscribed) · \
                 sides and fit in the toolbar · adds equal lines on a construction circle with a \
                 diameter dimension · Esc cancels"
            }
            SketchTool::Text => {
                "Click to place text (on a point: the text follows it), or click a text to edit \
                 it · set string, font, height (cap height, mm), rotation and alignment, then \
                 Apply · glyph outlines become profile regions (counters are holes) · Esc cancels"
            }
            SketchTool::Ellipse => {
                "Click the centre, then the end of one axis, then a point setting the other radius \
                 (its distance from the axis) · Dimension (D) the ellipse twice for its major / \
                 minor radii · Esc cancels"
            }
            SketchTool::Spline => {
                "Click control points of a cubic B-spline · Enter/right-click finishes an open \
                 spline (4+ points; it passes through its first and last points, which snap to \
                 existing points) · click the first point again to close it (3+ points) · Esc \
                 cancels"
            }
            SketchTool::Offset => {
                "Click a curve of a closed loop, type the distance (+ outward, − inward), Enter: adds the \
                 offset copy with one editable dimension · Esc cancels"
            }
        }
    }
}

/// Something selectable in the sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Point(PointId),
    Entity(EntityId),
    /// Index into `Sketch::constraints` (dimensions and glyph constraints).
    Constraint(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapKind {
    Point(PointId),
    Grid,
    Free,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub pos: [f64; 2],
    pub kind: SnapKind,
    pub align: Option<Align>,
}

/// Screen scale and snapping settings for one input event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen pixels per sketch millimetre near the cursor.
    pub px_per_mm: f64,
    /// Grid spacing in mm (None = no grid snap).
    pub grid: Option<f64>,
    /// False while Shift is held.
    pub snapping: bool,
}

impl View {
    fn mm(&self, px: f64) -> f64 {
        px / self.px_per_mm.max(1e-9)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DragItem {
    Point(PointId),
    Radius(EntityId),
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    Idle,
    /// Polyline in progress: `first` is the chain's first point, `last` the
    /// point the rubber band starts from. `fresh`: `first` was created by this
    /// chain (removed again if the chain ends without a segment).
    Line {
        first: PointId,
        last: PointId,
        fresh: bool,
    },
    /// First click of a rectangle (centre or corner), circle (centre) or arc (start).
    FirstClick(Snap),
    /// Arc tool: start and end chosen, waiting for the point on the arc.
    ArcEnd {
        start: Snap,
        end: Snap,
    },
    Drag {
        item: DragItem,
        before: Box<Sketch>,
        moved: bool,
    },
    /// Dimension tool: first point picked, waiting for the second.
    DimFirst(PointId),
    /// Slot tool: both centres chosen, waiting for the width point.
    SlotWidth {
        c1: Snap,
        c2: Snap,
    },
    /// Ellipse tool: centre and axis end chosen, waiting for the other radius.
    EllipseAxis {
        center: Snap,
        axis: Snap,
    },
    /// Spline tool: control points clicked so far (nothing is added to the
    /// sketch until the spline is finished).
    Spline(Vec<Snap>),
}

/// Open fillet entry: the corner point and the typed radius.
#[derive(Clone, Debug, PartialEq)]
pub struct FilletEdit {
    pub point: PointId,
    /// Radius in mm.
    pub buffer: String,
}

/// Open text dialog of the Text tool: a new text (`id` None) or an
/// existing one, with the editable fields as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub id: Option<u32>,
    pub text: String,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Cap height, mm (as typed).
    pub height: String,
    /// Degrees counter-clockwise (as typed).
    pub rotation: String,
    pub halign: rcad_sketch::HAlign,
    pub valign: rcad_sketch::VAlign,
    pub position: [f64; 2],
    pub anchor: Option<PointId>,
}

/// Open sketch pattern dialog: the selected entities and the typed
/// fields.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternEdit {
    pub entities: Vec<EntityId>,
    pub circular: bool,
    /// Linear: instances and step (mm) of direction 1, optional direction 2.
    pub count: String,
    pub dx: String,
    pub dy: String,
    pub two_directions: bool,
    pub count2: String,
    pub dx2: String,
    pub dy2: String,
    /// Circular: instances, angle between them (degrees), centre (origin or x/y).
    pub ccount: String,
    pub angle: String,
    pub cx: String,
    pub cy: String,
}

/// Default font family of new text (installed on Fedora/Bazzite by default).
pub const DEFAULT_TEXT_FAMILY: &str = "Liberation Sans";

/// Open offset entry: the loop (through `entity`) and the typed distance.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetEdit {
    pub entity: EntityId,
    /// Signed distance in mm (+ outward, − inward).
    pub buffer: String,
}

#[derive(Clone, Debug, PartialEq)]
/// Inline numeric editor for a dimension.
pub struct DimEdit {
    pub constraint: usize,
    pub buffer: String,
    /// True for a dimension just created (Escape keeps it at its measured value).
    pub fresh: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feedback {
    pub error: bool,
    pub text: String,
}

/// A Project / Intersect click for the app to resolve against the model (the
/// editor has no 3D geometry): the face (edge) or body under `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectPick {
    pub at: egui::Pos2,
    /// Intersect (body section) instead of an edge projection.
    pub section: bool,
    /// Keep the projection link (false: copy the curves unlinked).
    pub linked: bool,
}

/// Overall constraint state shown to the user (text + symbol, not only colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateKind {
    Full,
    Under,
    Conflict,
}

pub struct SketchEditor {
    pub sketch: Sketch,
    pub report: SolveReport,
    pub tool: SketchTool,
    gesture: Gesture,
    pub selection: Vec<Item>,
    pub hover: Option<Item>,
    /// Snapped cursor in plane coordinates (for previews).
    pub cursor: Option<Snap>,
    pub dim_edit: Option<DimEdit>,
    /// Offset tool: loop picked, distance being typed.
    pub offset_edit: Option<OffsetEdit>,
    /// Fillet tool: corner picked, radius being typed.
    pub fillet_edit: Option<FilletEdit>,
    /// Text tool: text dialog open.
    pub text_edit: Option<TextEdit>,
    /// Pattern dialog open (selected entities).
    pub pattern_edit: Option<PatternEdit>,
    /// Polygon tool: number of sides (3..=64).
    pub polygon_sides: usize,
    /// Polygon tool: size convention.
    pub polygon_fit: rcad_sketch::PolygonFit,
    pub feedback: Option<Feedback>,
    undo: Vec<Sketch>,
    redo: Vec<Sketch>,
    /// Screen rectangles of dimension labels / constraint glyphs (last frame).
    screen_targets: Vec<(Item, egui::Rect)>,
    /// The sketch as it was when the session started (to detect changes).
    original: Sketch,
    /// Bumped whenever a gesture is cancelled or the tool/history changes, so
    /// the view can drop a pointer press that started before (see `epoch`).
    epoch: u64,
    /// Projected (linked) entities and their link index: read-only.
    pub projected: std::collections::BTreeMap<EntityId, usize>,
    /// A Project / Intersect click waiting for the app.
    pub project_pick: Option<ProjectPick>,
    /// Project tool option: keep the projection link (Fusion's Projection Link).
    pub project_linked: bool,
    /// Evaluated user parameters (name -> value), for dimension expressions.
    pub param_env: std::collections::HashMap<String, rcad_document::params::Quantity>,
    /// Dimension expressions typed in this session (name -> expression; an
    /// empty expression unbinds), committed with the sketch at Finish.
    pub bindings: std::collections::BTreeMap<String, String>,
    /// User parameters created by `name = expr` in this session.
    pub new_params: Vec<rcad_commands::sketch_ops::NewParameter>,
    /// Dimensions bound to an expression in the document when the session
    /// (or the last projection commit) started, with their expressions.
    pub bound: std::collections::BTreeMap<String, String>,
    /// Heads-up typed fields while drawing (Tab opens, Tab cycles).
    pub hud: Option<Hud>,
    /// The last pointer event's view (scale), for typed input.
    last_view: Option<View>,
}

/// Heads-up typed input for the gesture in progress: one text per field of
/// [`SketchEditor::hud_fields`]; an empty field takes the cursor's value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    pub values: Vec<String>,
    /// Field that has (or gets) keyboard focus.
    pub focus: usize,
    /// Focus must be (re)given to `focus` on the next frame.
    pub refocus: bool,
}

/// What the inline dimension editor accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum DimInput {
    /// A number (with an optional unit suffix): no binding.
    Value(f64),
    /// An expression using parameters: its value now, bound to `expr`.
    Bound { value: f64, expr: String },
    /// `name = expr`: a new user parameter; the dimension is bound to it.
    NewParameter {
        value: f64,
        name: String,
        expr: String,
    },
}

/// Parse inline dimension input (docs/behavior/sketching.md "Dimension
/// entry"): a number with an optional unit (mm, cm, m, in; deg or rad for
/// angles), an expression over user parameters (`w/2`, `w + 2 mm`), or
/// `name = expr` (a new user parameter). The result must be finite and > 0.
pub fn parse_dim_input(
    buf: &str,
    angle: bool,
    env: &std::collections::HashMap<String, rcad_document::params::Quantity>,
) -> Result<DimInput, String> {
    use rcad_document::params;
    let text = buf.trim();
    if text.is_empty() {
        return Err("enter a value, a parameter or an expression".into());
    }
    let (name, src) = match text.split_once('=') {
        Some((n, e)) => {
            let n = n.trim();
            params::valid_name(n).map_err(|e| format!("'{n}': {e}"))?;
            if env.contains_key(n) {
                return Err(format!(
                    "parameter '{n}' already exists: type '{n}' to use it, or change it in \
                     Parameters"
                ));
            }
            (Some(n.to_string()), e.trim())
        }
        None => (None, text),
    };
    // Angle unit suffixes (the expression grammar's angles are degrees).
    let (src, to_deg) = if angle {
        if let Some(x) = src.strip_suffix("rad") {
            (x.trim(), 180.0 / std::f64::consts::PI)
        } else {
            (
                src.trim_end_matches('°').trim_end_matches("deg").trim(),
                1.0,
            )
        }
    } else {
        (src, 1.0)
    };
    if src.is_empty() {
        return Err("the expression is empty".into());
    }
    let kind = if angle {
        params::SlotKind::Angle
    } else {
        params::SlotKind::Length
    };
    let value =
        params::evaluate_for_slot(src, env, kind).map_err(|e| format!("'{src}': {e}"))? * to_deg;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "'{text}' gives {}: a dimension must be > 0",
            rcad_sketch::fmt_mm(value)
        ));
    }
    let bound_src = if to_deg == 1.0 {
        src.to_string()
    } else {
        format!("({src}) * 180 / {}", std::f64::consts::PI)
    };
    Ok(match name {
        Some(name) => DimInput::NewParameter {
            value,
            name,
            expr: bound_src,
        },
        None if params::references(src).is_empty() => DimInput::Value(value),
        None => DimInput::Bound {
            value,
            expr: bound_src,
        },
    })
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

use rcad_sketch::constraint_mentions;
pub use rcad_sketch::{arc_distance, seg_dist};

/// Exact area of a closed loop (lines, arcs and ellipse / spline spans):
/// see [`rcad_sketch::loop_area`].
pub fn loop_area(segs: &[rcad_sketch::LoopSegment]) -> f64 {
    rcad_sketch::loop_area(segs)
}

impl SketchEditor {
    /// Start a session on `sketch`. New and editor sketches get an origin point;
    /// template sketches (no origin, one fixed anchor) are edited as they are so
    /// their anchor stays the `center_x`/`center_y` parameters after Finish.
    pub fn new(mut sketch: Sketch) -> Self {
        let has_anchor = sketch
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::FixedPoint { .. }));
        if sketch.origin.is_none() && !has_anchor {
            sketch.ensure_origin();
        }
        let original = sketch.clone();
        let mut solved = sketch.clone();
        let report = solved.solve();
        if report.is_success() {
            sketch = solved;
        }
        Self {
            sketch,
            report,
            tool: SketchTool::Select,
            gesture: Gesture::Idle,
            selection: vec![],
            hover: None,
            cursor: None,
            dim_edit: None,
            offset_edit: None,
            fillet_edit: None,
            text_edit: None,
            pattern_edit: None,
            polygon_sides: 6,
            polygon_fit: rcad_sketch::PolygonFit::Inscribed,
            feedback: None,
            undo: vec![],
            redo: vec![],
            screen_targets: vec![],
            original,
            epoch: 0,
            projected: Default::default(),
            project_pick: None,
            project_linked: true,
            param_env: Default::default(),
            bindings: Default::default(),
            new_params: vec![],
            bound: Default::default(),
            hud: None,
            last_view: None,
        }
    }

    /// Fields of the heads-up input for the current tool and gesture (None:
    /// nothing to type yet): Line → length, angle; rectangles → width,
    /// height; Circle → diameter.
    pub fn hud_fields(&self) -> Option<&'static [&'static str]> {
        self.rubber_band_from()?;
        match (&self.tool, &self.gesture) {
            (SketchTool::Line, Gesture::Line { .. }) => Some(&["Length (mm)", "Angle (deg)"]),
            (SketchTool::CenterRectangle | SketchTool::CornerRectangle, Gesture::FirstClick(_)) => {
                Some(&["Width (mm)", "Height (mm)"])
            }
            (SketchTool::Circle, Gesture::FirstClick(_)) => Some(&["Diameter (mm)"]),
            _ => None,
        }
    }

    /// Tab: open the heads-up fields (focus on the first) or move to the next.
    pub fn hud_tab(&mut self) -> bool {
        let Some(fields) = self.hud_fields() else {
            return false;
        };
        match &mut self.hud {
            Some(h) => {
                h.focus = (h.focus + 1) % fields.len();
                h.refocus = true;
            }
            None => {
                self.hud = Some(Hud {
                    values: vec![String::new(); fields.len()],
                    focus: 0,
                    refocus: true,
                })
            }
        }
        true
    }

    /// Enter in the heads-up fields: place the gesture's next point from the
    /// typed values (empty fields keep the cursor's value) and dimension
    /// what was typed. Invalid input keeps the fields open with the reason.
    pub fn apply_hud(&mut self) -> bool {
        let (Some(h), Some(fields), Some(from)) =
            (self.hud.clone(), self.hud_fields(), self.rubber_band_from())
        else {
            self.hud = None;
            return false;
        };
        let cursor = self.cursor.as_ref().map_or(from, |c| c.pos);
        let mut typed: Vec<Option<f64>> = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            let text = h.values.get(i).map_or("", |v| v.trim());
            if text.is_empty() {
                typed.push(None);
                continue;
            }
            let angle = f.starts_with("Angle");
            let v = if angle {
                rcad_document::params::evaluate_for_slot(
                    text.trim_end_matches('°').trim_end_matches("deg").trim(),
                    &self.param_env,
                    rcad_document::params::SlotKind::Angle,
                )
                .map_err(|e| format!("'{text}': {e}"))
            } else {
                match parse_dim_input(text, false, &self.param_env) {
                    Ok(DimInput::Value(v) | DimInput::Bound { value: v, .. }) => Ok(v),
                    Ok(DimInput::NewParameter { .. }) => Err(format!(
                        "'{text}': name = value is for dimensions; type a value"
                    )),
                    Err(e) => Err(e),
                }
            };
            match v {
                Ok(v) if v.is_finite() => typed.push(Some(v)),
                Ok(_) => typed.push(None),
                Err(e) => {
                    self.fail(format!("{f}: {e}"));
                    return false;
                }
            }
        }
        let d = [cursor[0] - from[0], cursor[1] - from[1]];
        let sign = |x: f64| if x < 0.0 { -1.0 } else { 1.0 };
        let target = match self.tool {
            SketchTool::Line => {
                let len = typed[0].unwrap_or(d[0].hypot(d[1]));
                let ang = typed[1].map_or(d[1].atan2(d[0]), f64::to_radians);
                [from[0] + len * ang.cos(), from[1] + len * ang.sin()]
            }
            SketchTool::CornerRectangle => {
                let w = typed[0].map_or(d[0], |w| w * sign(d[0]));
                let hh = typed[1].map_or(d[1], |v| v * sign(d[1]));
                [from[0] + w, from[1] + hh]
            }
            SketchTool::CenterRectangle => {
                let w = typed[0].map_or(d[0], |w| 0.5 * w * sign(d[0]));
                let hh = typed[1].map_or(d[1], |v| 0.5 * v * sign(d[1]));
                [from[0] + w, from[1] + hh]
            }
            SketchTool::Circle => {
                let r = typed[0].map_or(d[0].hypot(d[1]), |v| 0.5 * v);
                [from[0] + r, from[1]]
            }
            _ => return false,
        };
        let view = View {
            snapping: false,
            grid: None,
            ..self.last_view.unwrap_or(View {
                px_per_mm: 4.0,
                grid: None,
                snapping: false,
            })
        };
        let n_entities = self.sketch.entities.len();
        let tool = self.tool;
        self.hud = None;
        self.pointer_move(target, None, &view);
        self.click(target, None, &view, false);
        if self.sketch.entities.len() == n_entities {
            return false; // refused: the click's feedback says why
        }
        let new: Vec<EntityId> = self.sketch.entities[n_entities..]
            .iter()
            .map(Entity::id)
            .collect();
        // Dimension the typed values (named like the Dimension tool's).
        let mut dims: Vec<EntityId> = vec![];
        match tool {
            SketchTool::Line if typed[0].is_some() => dims.extend(new.last()),
            SketchTool::Circle if typed[0].is_some() => dims.extend(new.first()),
            SketchTool::CenterRectangle | SketchTool::CornerRectangle => {
                let horizontal = |sk: &Sketch, e: &EntityId| match sk.entity(*e) {
                    Some(Entity::Line { a, b, .. }) => {
                        let (p, q) = (sk.point_xy(*a), sk.point_xy(*b));
                        p.zip(q)
                            .is_some_and(|(p, q)| (p[1] - q[1]).abs() < (p[0] - q[0]).abs())
                    }
                    _ => false,
                };
                if typed[0].is_some() {
                    dims.extend(new.iter().find(|e| horizontal(&self.sketch, e)));
                }
                if typed[1].is_some() {
                    dims.extend(new.iter().find(|e| !horizontal(&self.sketch, e)));
                }
            }
            _ => {}
        }
        for e in dims {
            self.dimension_entity(e);
            // Created at the typed (= measured) value; close its field.
            self.dim_edit = None;
        }
        true
    }

    /// True when Finish must also commit typed expressions or parameters.
    pub fn has_bindings(&self) -> bool {
        !self.bindings.is_empty() || !self.new_params.is_empty()
    }

    /// The typed bindings that still name a dimension of the sketch.
    pub fn live_bindings(&self) -> std::collections::BTreeMap<String, String> {
        let dims = self.sketch.dimensions();
        self.bindings
            .iter()
            .filter(|(n, _)| dims.iter().any(|(m, _)| m == *n))
            .map(|(n, e)| (n.clone(), e.clone()))
            .collect()
    }

    /// Mark the curves of each projection link (in link order) read-only.
    pub fn set_projected(&mut self, links: &[Vec<EntityId>]) {
        self.projected = links
            .iter()
            .enumerate()
            .flat_map(|(i, l)| l.iter().map(move |e| (*e, i)))
            .collect();
    }

    /// Continue on `sketch` as committed to the document (after Project
    /// committed the work so far): local undo restarts here, because the
    /// document's undo now holds the earlier steps.
    pub fn rebase(&mut self, sketch: Sketch, links: &[Vec<EntityId>]) {
        self.cancel_gesture();
        self.epoch += 1;
        self.sketch = sketch.clone();
        self.original = sketch;
        self.undo.clear();
        self.redo.clear();
        self.selection.clear();
        self.dim_edit = None;
        self.set_projected(links);
        self.resolve();
    }

    /// Entities drawn by the user (not projected).
    pub fn drawn_entity_count(&self) -> usize {
        self.sketch
            .entities
            .iter()
            .filter(|e| !self.projected.contains_key(&e.id()))
            .count()
    }

    pub fn modified(&self) -> bool {
        self.sketch != self.original || !self.undo.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn set_screen_targets(&mut self, t: Vec<(Item, egui::Rect)>) {
        self.screen_targets = t;
    }

    /// Screen rectangle of a label/glyph as of the last painted frame.
    pub fn screen_target(&self, item: Item) -> Option<egui::Rect> {
        self.screen_targets
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, r)| *r)
    }

    pub fn set_tool(&mut self, tool: SketchTool) {
        self.cancel_gesture();
        self.offset_edit = None;
        self.fillet_edit = None;
        self.tool = tool;
        self.feedback = None;
        self.epoch += 1;
    }

    /// Changes whenever an in-progress pointer press should be forgotten
    /// (tool change, Escape, cancelled gesture, undo/redo).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// A Line chain whose only point is its (new) start point.
    fn lone_chain(&self) -> bool {
        matches!(&self.gesture, Gesture::Line { first, last, fresh: true }
            if first == last && !self.sketch.point_used_by_entity(*first))
    }

    /// End an empty line chain before any other edit, so its start point never
    /// leaks into another step's undo snapshot.
    fn end_lone_chain(&mut self) {
        if self.lone_chain() {
            self.cancel_gesture();
        }
    }

    /// True while a multi-click gesture or drag is in progress.
    pub fn gesture_active(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle)
    }

    /// Anchor of the rubber band (for previews), plane coordinates.
    pub fn rubber_band_from(&self) -> Option<[f64; 2]> {
        match &self.gesture {
            Gesture::Line { last, .. } => self.sketch.point_xy(*last),
            Gesture::FirstClick(s) => Some(s.pos),
            Gesture::ArcEnd { start, .. } => Some(start.pos),
            Gesture::DimFirst(p) => self.sketch.point_xy(*p),
            Gesture::SlotWidth { c1, .. } => Some(c1.pos),
            Gesture::EllipseAxis { center, .. } => Some(center.pos),
            Gesture::Spline(pts) => pts.last().map(|s| s.pos),
            _ => None,
        }
    }

    pub fn status(&self) -> (StateKind, String) {
        let r = &self.report;
        if self.sketch.entities.is_empty() && !self.sketch.texts.is_empty() {
            return (
                StateKind::Full,
                format!(
                    "✔ Text only: {} text item(s); glyph outlines are profile regions",
                    self.sketch.texts.len()
                ),
            );
        }
        if self.sketch.entities.is_empty() {
            return (
                StateKind::Under,
                "Empty sketch: draw with Line (L), Rectangle (R) or Circle (C)".into(),
            );
        }
        match r.status {
            SolveStatus::FullyConstrained => {
                (StateKind::Full, "✔ Fully constrained (DOF 0)".into())
            }
            SolveStatus::UnderConstrained => (
                StateKind::Under,
                format!("~ Under-constrained: {} degree(s) of freedom", r.dof),
            ),
            SolveStatus::OverConstrained => (
                StateKind::Conflict,
                format!(
                    "✖ Over-constrained (redundant): {}",
                    self.labels(&r.conflicting)
                ),
            ),
            SolveStatus::Inconsistent => (
                StateKind::Conflict,
                format!("✖ Conflicting constraints: {}", self.labels(&r.conflicting)),
            ),
            SolveStatus::DidNotConverge => {
                (StateKind::Conflict, "✖ Solver did not converge".into())
            }
            SolveStatus::Invalid => (StateKind::Conflict, "✖ Invalid sketch".into()),
        }
    }

    fn labels(&self, idx: &[usize]) -> String {
        if idx.is_empty() {
            return "not identified".into();
        }
        idx.iter()
            .map(|&i| self.sketch.constraint_label(i))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Profile state for the inspector (closed loop / circle, or why not).
    pub fn profile_state(&self) -> Result<String, String> {
        if !self.sketch.texts.is_empty() {
            // Text: glyphs are separate loops; report the regions.
            return match self.sketch.regions() {
                Ok(rs) if rs.len() == 1 => Ok(format!(
                    "1 region, area {} mm²",
                    rcad_sketch::fmt_mm(rs[0].area)
                )),
                Ok(rs) if !rs.is_empty() => Ok(format!(
                    "{} regions (text glyphs, counters and other loops): pick regions to extrude",
                    rs.len()
                )),
                Ok(_) => Err("no closed loops".into()),
                Err(e) => Err(e.to_string()),
            };
        }
        if self.sketch.entities.is_empty() {
            return Err("nothing drawn yet".into());
        }
        match self.sketch.profile() {
            Ok(rcad_sketch::Profile::Polygon(v)) => {
                let a: f64 = (0..v.len())
                    .map(|i| {
                        let (p, q) = (v[i], v[(i + 1) % v.len()]);
                        p[0] * q[1] - q[0] * p[1]
                    })
                    .sum::<f64>()
                    * 0.5;
                Ok(format!(
                    "closed loop, {} edges, area {} mm²",
                    v.len(),
                    rcad_sketch::fmt_mm(a)
                ))
            }
            Ok(rcad_sketch::Profile::Circle { radius, .. }) => {
                Ok(format!("circle Ø{} mm", rcad_sketch::fmt_mm(2.0 * radius)))
            }
            Ok(rcad_sketch::Profile::Loop(v)) => Ok(format!(
                "closed loop, {} lines/arcs, area {} mm²",
                v.len(),
                rcad_sketch::fmt_mm(loop_area(&v))
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    // ----- picking and snapping -----

    /// Topmost item under the cursor: screen targets (labels, glyphs), then
    /// points, then curves.
    pub fn pick(&self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) -> Option<Item> {
        if let Some(s) = screen
            && let Some((item, _)) = self.screen_targets.iter().find(|(_, r)| r.contains(s))
        {
            return Some(*item);
        }
        let tol = view.mm(PICK_PX);
        let point = self
            .sketch
            .points
            .iter()
            .map(|p| (p.id, dist(pos, [p.x, p.y])))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = point {
            return Some(Item::Point(id));
        }
        self.sketch
            .entities
            .iter()
            .filter_map(|e| {
                let d = match e {
                    Entity::Line { a, b, .. } => {
                        seg_dist(pos, self.sketch.point_xy(*a)?, self.sketch.point_xy(*b)?)
                    }
                    Entity::Circle { center, radius, .. } => {
                        (dist(pos, self.sketch.point_xy(*center)?) - radius).abs()
                    }
                    Entity::Arc { id, .. } => arc_distance(&self.sketch.arc_geometry(*id)?, pos),
                    Entity::Ellipse { .. } | Entity::Spline { .. } => {
                        rcad_sketch::curves::entity_polyline(&self.sketch, e, 32)?
                            .windows(2)
                            .map(|w| seg_dist(pos, w[0], w[1]))
                            .fold(f64::INFINITY, f64::min)
                    }
                };
                (d <= tol).then_some((e.id(), d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| Item::Entity(id))
    }

    /// Snap a raw plane position. `anchor` enables H/V alignment; `exclude`
    /// is a point that must not snap to itself (the one being dragged).
    pub fn snap(
        &self,
        raw: [f64; 2],
        anchor: Option<[f64; 2]>,
        exclude: Option<PointId>,
        view: &View,
    ) -> Snap {
        if !view.snapping {
            return Snap {
                pos: raw,
                kind: SnapKind::Free,
                align: None,
            };
        }
        let tol = view.mm(SNAP_PX);
        let near = self
            .sketch
            .points
            .iter()
            .filter(|p| Some(p.id) != exclude)
            .filter(|p| self.snappable(p.id))
            .map(|p| (p.id, [p.x, p.y], dist(raw, [p.x, p.y])))
            .filter(|(_, _, d)| *d <= tol)
            .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        if let Some((id, pos, _)) = near {
            // An existing point exactly level/plumb with the anchor still gets
            // the alignment constraint (e.g. the closing edge of a loop).
            let exact = 1e-9 * self.sketch.scale();
            let align = anchor.and_then(|a| {
                let (dx, dy) = ((pos[0] - a[0]).abs(), (pos[1] - a[1]).abs());
                if dy <= exact && dx > exact {
                    Some(Align::Horizontal)
                } else if dx <= exact && dy > exact {
                    Some(Align::Vertical)
                } else {
                    None
                }
            });
            return Snap {
                pos,
                kind: SnapKind::Point(id),
                align,
            };
        }
        let mut pos = raw;
        let mut align = None;
        let band = view.mm(ALIGN_PX);
        if let Some(a) = anchor {
            let (dx, dy) = ((raw[0] - a[0]).abs(), (raw[1] - a[1]).abs());
            if dy <= band && dx > band {
                pos[1] = a[1];
                align = Some(Align::Horizontal);
            } else if dx <= band && dy > band {
                pos[0] = a[0];
                align = Some(Align::Vertical);
            }
        }
        let mut kind = SnapKind::Free;
        if let Some(g) = view.grid.filter(|g| g.is_finite() && *g > 0.0) {
            let r = |v: f64| {
                let s = (v / g).round() * g;
                if s == 0.0 { 0.0 } else { s }
            };
            if align != Some(Align::Vertical) {
                pos[0] = r(pos[0]);
            }
            if align != Some(Align::Horizontal) {
                pos[1] = r(pos[1]);
            }
            kind = SnapKind::Grid;
        }
        Snap { pos, kind, align }
    }

    /// Points a cursor may snap to: entity points and the origin (not the
    /// hidden construction centre of a rectangle, to avoid accidental merges).
    fn snappable(&self, id: PointId) -> bool {
        self.sketch.origin == Some(id)
            || self.sketch.entities.iter().any(|e| match e {
                Entity::Line { a, b, .. } => *a == id || *b == id,
                Entity::Circle { center, .. } => *center == id,
                Entity::Arc { .. } | Entity::Ellipse { .. } | Entity::Spline { .. } => {
                    e.points().contains(&id)
                }
            })
    }

    fn point_for(&mut self, s: &Snap) -> PointId {
        match s.kind {
            SnapKind::Point(id) if self.sketch.point(id).is_some() => id,
            _ => self.sketch.add_point(s.pos[0], s.pos[1]),
        }
    }

    // ----- transactions -----

    fn push_undo(&mut self, before: Sketch) {
        self.undo.push(before);
        if self.undo.len() > MAX_LOCAL_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Apply `change` to a copy; keep it only if it solves (and, with
    /// `refuse_redundant`, does not over-constrain). Returns true if kept.
    fn try_change(
        &mut self,
        refuse_redundant: bool,
        change: impl FnOnce(&mut Sketch) -> Result<(), String>,
    ) -> bool {
        let before = self.sketch.clone();
        let mut cand = self.sketch.clone();
        if let Err(e) = change(&mut cand) {
            self.fail(e);
            return false;
        }
        if let Some(why) = self.projected_change(&cand) {
            self.fail(why);
            return false;
        }
        let original = cand.clone();
        let mut solved = cand;
        let rep = solved.solve();
        let labels = |idx: &[usize]| {
            if idx.is_empty() {
                "not identified".to_string()
            } else {
                idx.iter()
                    .map(|&i| original.constraint_label(i))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        };
        if !rep.is_success() {
            self.fail(format!(
                "Rejected: the sketch would not solve ({:?}). Conflicts with: {}. \
                 Delete or change one of these constraints first.",
                rep.status,
                labels(&rep.conflicting)
            ));
            return false;
        }
        let collapsed: Vec<EntityId> = solved
            .collapsed_curves()
            .into_iter()
            .filter(|e| !before.collapsed_curves().contains(e))
            .collect();
        if !collapsed.is_empty() {
            self.fail(format!(
                "Rejected: this would collapse {} to zero size.",
                collapsed
                    .iter()
                    .map(|e| format!("entity {}", e.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return false;
        }
        if refuse_redundant && rep.status == SolveStatus::OverConstrained {
            self.fail(format!(
                "Rejected: this would over-constrain the sketch (redundant with: {}).",
                labels(&rep.conflicting)
            ));
            return false;
        }
        self.sketch = solved;
        self.report = rep;
        self.push_undo(before);
        true
    }

    fn fail(&mut self, text: String) {
        self.feedback = Some(Feedback { error: true, text });
    }

    fn info(&mut self, text: String) {
        self.feedback = Some(Feedback { error: false, text });
    }

    fn resolve(&mut self) {
        let mut s = self.sketch.clone();
        self.report = s.solve();
        if self.report.is_success() {
            self.sketch = s;
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.lone_chain() {
            // The pending start point is the most recent action: drop just it.
            self.cancel_gesture();
            return true;
        }
        self.cancel_gesture();
        self.epoch += 1;
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.sketch, prev);
        self.redo.push(cur);
        self.after_history();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.cancel_gesture();
        self.epoch += 1;
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.sketch, next);
        self.undo.push(cur);
        self.after_history();
        true
    }

    fn after_history(&mut self) {
        self.selection.clear();
        self.dim_edit = None;
        self.feedback = None;
        self.resolve();
    }

    // ----- gestures -----

    /// Escape cascade: inline edit → active gesture → tool. Returns true if
    /// something was cancelled (false = nothing left to cancel in the editor).
    pub fn escape(&mut self) -> bool {
        if self.offset_edit.take().is_some()
            || self.fillet_edit.take().is_some()
            || self.text_edit.take().is_some()
            || self.pattern_edit.take().is_some()
        {
            return true;
        }
        if self.dim_edit.is_some() {
            self.cancel_dim_edit();
            return true;
        }
        if self.gesture_active() {
            self.cancel_gesture();
            return true;
        }
        if self.tool != SketchTool::Select {
            self.set_tool(SketchTool::Select);
            return true;
        }
        if !self.selection.is_empty() {
            self.selection.clear();
            return true;
        }
        false
    }

    /// Abort the current gesture. A drag in progress restores its start state;
    /// a line chain without segments drops the point it created.
    pub fn cancel_gesture(&mut self) {
        self.hud = None;
        if !matches!(self.gesture, Gesture::Idle) {
            self.epoch += 1;
        }
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Drag { before, .. } => {
                self.sketch = *before;
                self.resolve();
                self.info("Drag cancelled; geometry restored.".into());
            }
            Gesture::Line {
                first,
                last,
                fresh: true,
            } if first == last && !self.sketch.point_used_by_entity(first) => {
                // The lone start point was never an undo step: remove it directly.
                self.sketch.points.retain(|q| q.id != first);
                self.sketch
                    .constraints
                    .retain(|c| !constraint_mentions(c, first));
                self.resolve();
            }
            _ => {}
        }
    }

    /// End a polyline chain (Enter / right click), keeping what was drawn.
    pub fn finish_chain(&mut self) {
        if matches!(self.gesture, Gesture::Spline(_)) {
            self.finish_spline(false);
            return;
        }
        if matches!(self.gesture, Gesture::Line { .. } | Gesture::DimFirst(_)) {
            self.cancel_gesture();
        }
    }

    pub fn pointer_move(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        self.last_view = Some(*view);
        if let Gesture::Drag { item, moved, .. } = &mut self.gesture {
            let item = item.clone();
            *moved = true;
            self.drag_to(&item, pos, view);
            return;
        }
        // Same snapping as the click that will follow (preview = result).
        let s = match (&self.tool, &self.gesture) {
            (SketchTool::Line, _) => self.snap(pos, self.rubber_band_from(), None, view),
            (SketchTool::Arc, Gesture::FirstClick(start)) => {
                self.snap(pos, Some(start.pos), None, view)
            }
            (SketchTool::Arc, Gesture::ArcEnd { .. }) => self.via_snap(pos, view),
            _ => self.snap(pos, None, None, view),
        };
        self.cursor = Some(s);
        self.hover = if matches!(
            self.tool,
            SketchTool::Select
                | SketchTool::Dimension
                | SketchTool::Trim
                | SketchTool::Extend
                | SketchTool::Break
                | SketchTool::Offset
                | SketchTool::Fillet
        ) {
            self.pick(pos, screen, view)
        } else {
            None
        };
    }

    fn drag_to(&mut self, item: &DragItem, pos: [f64; 2], view: &View) {
        let res = match item {
            DragItem::Point(p) => {
                let s = self.snap(pos, None, Some(*p), view);
                self.sketch.drag_point(*p, s.pos)
            }
            DragItem::Radius(c) => {
                let r = match self.sketch.entity(*c).cloned() {
                    Some(Entity::Circle { center, .. }) => {
                        let Some(cxy) = self.sketch.point_xy(center) else {
                            return;
                        };
                        dist(pos, cxy)
                    }
                    // Ellipse rim: the minor radius is the distance from the axis.
                    Some(Entity::Ellipse { center, major, .. }) => {
                        let (Some(cxy), Some(mxy)) =
                            (self.sketch.point_xy(center), self.sketch.point_xy(major))
                        else {
                            return;
                        };
                        line_distance(pos, cxy, mxy)
                    }
                    _ => return,
                };
                if r <= view.mm(1.0) {
                    return;
                }
                let r = match view.grid.filter(|_| view.snapping) {
                    Some(g) if g > 0.0 => ((r / g).round() * g).max(g),
                    _ => r,
                };
                self.sketch.drag_radius(*c, r)
            }
        };
        self.report = res.report.clone();
        match res.outcome {
            DragOutcome::Reached => self.feedback = None,
            DragOutcome::Partial => self.info(format!(
                "Constrained: movement limited by {}",
                self.labels(&res.blocked_by)
            )),
            DragOutcome::Blocked => self.fail(format!(
                "Cannot move: held by {}. Delete a constraint or dimension to free it.",
                self.labels(&res.blocked_by)
            )),
        }
    }

    /// Primary-button press (start of a potential drag) in the Select tool.
    pub fn press(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if self.tool != SketchTool::Select || self.dim_edit.is_some() {
            return;
        }
        let hit = self.pick(pos, screen, view);
        let locked = match hit {
            Some(Item::Entity(e)) => self.locked_reason(e),
            Some(Item::Point(p)) => self
                .sketch
                .entities
                .iter()
                .filter(|x| x.points().contains(&p))
                .find_map(|x| self.locked_reason(x.id())),
            _ => None,
        };
        if let Some(why) = locked {
            self.fail(why);
            return;
        }
        let item = match hit {
            Some(Item::Point(p)) => DragItem::Point(p),
            Some(Item::Entity(e)) => match self.sketch.entity(e) {
                Some(Entity::Circle { .. } | Entity::Ellipse { .. }) => DragItem::Radius(e),
                _ => return,
            },
            _ => return,
        };
        self.gesture = Gesture::Drag {
            item,
            before: Box::new(self.sketch.clone()),
            moved: false,
        };
    }

    /// Primary-button release: ends a drag as ONE undoable step.
    pub fn release(&mut self) {
        if let Gesture::Drag { before, moved, .. } =
            std::mem::replace(&mut self.gesture, Gesture::Idle)
            && moved
            && *before != self.sketch
        {
            self.push_undo(*before);
        }
    }

    pub fn dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Drag { .. })
    }

    /// A click (press and release without dragging) at `pos`.
    pub fn click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View, shift: bool) {
        match self.tool {
            SketchTool::Select => {
                let hit = self.pick(pos, screen, view);
                match hit {
                    Some(item) if shift => {
                        if let Some(i) = self.selection.iter().position(|s| *s == item) {
                            self.selection.remove(i);
                        } else {
                            self.selection.push(item);
                        }
                    }
                    // Clicking the already-selected dimension edits it (as does a double click).
                    Some(Item::Constraint(i))
                        if self.selection == [Item::Constraint(i)]
                            && self
                                .sketch
                                .constraints
                                .get(i)
                                .and_then(|c| c.dimension())
                                .is_some() =>
                    {
                        self.begin_dim_edit(i, false)
                    }
                    Some(item) => self.selection = vec![item],
                    None if !shift => self.selection.clear(),
                    None => {}
                }
            }
            SketchTool::Line => self.line_click(pos, view),
            SketchTool::CenterRectangle | SketchTool::CornerRectangle | SketchTool::Circle => {
                let s = self.snap(pos, None, None, view);
                match self.gesture.clone() {
                    Gesture::FirstClick(first) => {
                        self.gesture = Gesture::Idle;
                        match self.tool {
                            SketchTool::Circle => self.make_circle(first, s, view),
                            t => self.make_rectangle(first, s, t == SketchTool::CenterRectangle),
                        }
                    }
                    _ => self.gesture = Gesture::FirstClick(s),
                }
            }
            SketchTool::Arc => self.arc_click(pos, view),
            SketchTool::Dimension => self.dimension_click(pos, screen, view),
            SketchTool::Trim => self.trim_click(pos, screen, view),
            SketchTool::Extend => self.extend_click(pos, screen, view),
            SketchTool::Break => self.break_click(pos, screen, view),
            SketchTool::Project | SketchTool::Intersect => match screen {
                Some(at) => {
                    self.project_pick = Some(ProjectPick {
                        at,
                        section: self.tool == SketchTool::Intersect,
                        linked: self.project_linked,
                    });
                }
                None => self.fail("Project: click a model face in the view.".into()),
            },
            SketchTool::Offset => self.offset_click(pos, screen, view),
            SketchTool::Fillet => self.fillet_click(pos, screen, view),
            SketchTool::Text => self.text_click(pos, view),
            SketchTool::Slot => self.slot_click(pos, view),
            SketchTool::Ellipse => self.ellipse_click(pos, view),
            SketchTool::Spline => self.spline_click(pos, view),
            SketchTool::Polygon => {
                let s = self.snap(pos, None, None, view);
                match self.gesture.clone() {
                    Gesture::FirstClick(c) => {
                        self.gesture = Gesture::Idle;
                        self.make_polygon(c, s, view);
                    }
                    _ => self.gesture = Gesture::FirstClick(s),
                }
            }
        }
    }

    fn fillet_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        match self.pick(pos, screen, view) {
            Some(Item::Point(p)) => {
                let buffer = self
                    .fillet_edit
                    .as_ref()
                    .map_or_else(|| "3".to_string(), |f| f.buffer.clone());
                self.fillet_edit = Some(FilletEdit { point: p, buffer });
                self.info("Type the fillet radius and press Enter.".into());
            }
            _ => self
                .fail("Click the corner point where two lines (or a line and an arc) meet.".into()),
        }
    }

    fn fillet_radius(&self) -> Option<Result<(PointId, f64), String>> {
        let f = self.fillet_edit.as_ref()?;
        let t = f.buffer.trim().trim_end_matches("mm").trim();
        Some(match t.parse::<f64>() {
            Ok(r) if r.is_finite() && r > 0.0 => Ok((f.point, r)),
            _ => Err(format!("'{}' is not a radius > 0 mm", f.buffer)),
        })
    }

    /// Preview of the open fillet entry: the sketch with the fillet and the
    /// new arc; or why it is refused.
    pub fn fillet_preview(&self) -> Option<Result<(Sketch, EntityId), String>> {
        Some(self.fillet_radius()?.and_then(|(p, r)| {
            let mut s = self.sketch.clone();
            s.fillet_corner(p, r, None)
                .map(|x| (s, x.arc))
                .map_err(|e| e.to_string())
        }))
    }

    /// Enter on the fillet entry (one undo step). A refusal keeps the entry
    /// open with the reason; the sketch is unchanged.
    pub fn apply_fillet(&mut self) -> bool {
        let Some(parsed) = self.fillet_radius() else {
            return false;
        };
        let (p, r) = match parsed {
            Ok(v) => v,
            Err(m) => {
                self.fail(m);
                return false;
            }
        };
        let mut name = String::new();
        let ok = self.try_change(false, |sk| {
            let res = sk
                .fillet_corner(p, r, None)
                .map_err(|x| format!("Fillet refused: {x}"))?;
            name = res.dimension;
            Ok(())
        });
        if ok {
            self.fillet_edit = None;
            self.info(format!(
                "Sketch corner rounded (R {r} mm, dimension '{name}'); the original corner stays \
                 as a construction point so its dimensions keep their meaning."
            ));
        }
        ok
    }

    fn offset_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        match self.pick(pos, screen, view) {
            Some(Item::Entity(e)) => {
                let buffer = self
                    .offset_edit
                    .as_ref()
                    .map_or_else(|| "2".to_string(), |o| o.buffer.clone());
                self.offset_edit = Some(OffsetEdit { entity: e, buffer });
                self.info("Type the offset distance (+ outward, − inward) and press Enter.".into());
            }
            _ => self.fail("Click a line, arc or circle of a closed loop to offset it.".into()),
        }
    }

    /// Parsed distance of the open offset entry.
    fn offset_distance(&self) -> Option<Result<(EntityId, f64), String>> {
        let o = self.offset_edit.as_ref()?;
        let t = o
            .buffer
            .trim()
            .trim_end_matches("mm")
            .trim()
            .replace('−', "-");
        Some(match t.parse::<f64>() {
            Ok(d) if d.is_finite() && d != 0.0 => Ok((o.entity, d)),
            _ => Err(format!("'{}' is not a non-zero distance in mm", o.buffer)),
        })
    }

    /// Preview of the open offset entry: the sketch with the copy, and the
    /// copy's entities; or why it is refused.
    pub fn offset_preview(&self) -> Option<Result<(Sketch, Vec<EntityId>), String>> {
        Some(self.offset_distance()?.and_then(|(e, d)| {
            let mut s = self.sketch.clone();
            s.offset_loop(e, d, None)
                .map(|r| (s, r.entities))
                .map_err(|x| x.to_string())
        }))
    }

    /// Enter on the offset entry: add the copy (one undo step). On a refusal
    /// the entry stays open with the reason and the sketch is unchanged.
    pub fn apply_offset(&mut self) -> bool {
        let Some(parsed) = self.offset_distance() else {
            return false;
        };
        let (e, d) = match parsed {
            Ok(v) => v,
            Err(m) => {
                self.fail(m);
                return false;
            }
        };
        let mut name = String::new();
        let ok = self.try_change(false, |sk| {
            let r = sk
                .offset_loop(e, d, None)
                .map_err(|x| format!("Offset refused: {x}"))?;
            name = r.dimension;
            Ok(())
        });
        if ok {
            self.offset_edit = None;
            self.info(format!(
                "Offset by {} mm {}: dimension '{name}' keeps the distance (edit it, or bind it \
                 to a parameter).",
                d.abs(),
                if d > 0.0 { "outward" } else { "inward" }
            ));
        }
        ok
    }

    /// Start and end of the arc being placed (for the preview).
    pub fn arc_ends(&self) -> Option<([f64; 2], [f64; 2])> {
        match &self.gesture {
            Gesture::ArcEnd { start, end } => Some((start.pos, end.pos)),
            _ => None,
        }
    }

    fn arc_click(&mut self, pos: [f64; 2], view: &View) {
        match self.gesture.clone() {
            Gesture::FirstClick(start) => {
                let end = self.snap(pos, Some(start.pos), None, view);
                if dist(start.pos, end.pos) <= view.mm(2.0) {
                    self.fail("Arc end must differ from its start: click another point.".into());
                    return;
                }
                self.gesture = Gesture::ArcEnd { start, end };
            }
            Gesture::ArcEnd { start, end } => {
                // The pass-through point only shapes the arc: grid snapping like
                // every other click, but never merged with an existing point.
                let via = self.via_snap(pos, view);
                self.gesture = Gesture::Idle;
                let ok = self.try_change(true, |sk| {
                    let mut pid = |s: &Snap| match s.kind {
                        SnapKind::Point(id) if sk.point(id).is_some() => id,
                        _ => sk.add_point(s.pos[0], s.pos[1]),
                    };
                    let (a, b) = (pid(&start), pid(&end));
                    if a == b {
                        return Err("arc start and end are the same point".into());
                    }
                    sk.add_arc_3pt(a, b, via.pos).map(|_| ()).map_err(|e| {
                        format!("{e}; click a point that is not in line with the ends")
                    })
                });
                if ok {
                    self.info(
                        "Arc added. Dimension (D) its radius; Tangent/Equal constrain it.".into(),
                    );
                }
            }
            _ => {
                let s = self.snap(pos, None, None, view);
                self.gesture = Gesture::FirstClick(s);
            }
        }
    }

    /// Double click: edit a dimension under the cursor.
    pub fn double_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if self.pick(pos, screen, view).is_none()
            && let Some(id) = self.text_at(pos, view)
        {
            self.begin_text_edit(id);
            return;
        }
        if let Some(Item::Constraint(i)) = self.pick(pos, screen, view)
            && self
                .sketch
                .constraints
                .get(i)
                .and_then(|c| c.dimension())
                .is_some()
        {
            self.begin_dim_edit(i, false);
        }
    }

    fn line_click(&mut self, pos: [f64; 2], view: &View) {
        let anchor = self.rubber_band_from();
        let s = self.snap(pos, anchor, None, view);
        match self.gesture.clone() {
            Gesture::Line { first, last, .. } => {
                let Some(from) = self.sketch.point_xy(last) else {
                    self.gesture = Gesture::Idle;
                    return;
                };
                if dist(from, s.pos) <= view.mm(2.0) {
                    self.fail("Zero-length line ignored: click a different point.".into());
                    return;
                }
                let mut new_last = None;
                let closes = s.kind == SnapKind::Point(first);
                let was_lone = self.lone_chain();
                let ok = self.try_add_line(last, &s, &mut new_last);
                if !ok {
                    return;
                }
                if was_lone && let Some(top) = self.undo.last_mut() {
                    // Undoing the first segment must not leave its start point.
                    top.points.retain(|q| q.id != first);
                }
                self.gesture = match (closes, new_last) {
                    (true, _) => {
                        self.info("Loop closed.".into());
                        Gesture::Idle
                    }
                    (false, Some(p)) => Gesture::Line {
                        first,
                        last: p,
                        fresh: false,
                    },
                    _ => Gesture::Idle,
                };
            }
            _ => {
                // First point of a chain: may be new or an existing point; it
                // only becomes part of the sketch with the first segment.
                let n = self.sketch.points.len();
                let p = self.point_for(&s);
                // A new start point is not an undo step of its own: it is removed
                // if the chain ends without a segment, and stripped from the first
                // segment's undo snapshot (see `line_click`).
                let fresh = self.sketch.points.len() != n;
                self.gesture = Gesture::Line {
                    first: p,
                    last: p,
                    fresh,
                };
            }
        }
    }

    /// Add a line from `from` to the snapped position, with an alignment
    /// constraint when snapped H/V. If the alignment constraint turns out to be
    /// redundant, the line is added without it.
    fn try_add_line(&mut self, from: PointId, s: &Snap, new_last: &mut Option<PointId>) -> bool {
        let s = *s;
        for with_align in [true, false] {
            if !with_align && s.align.is_none() {
                break;
            }
            let mut end = None;
            let ok = self.try_change(true, |sk| {
                let to = match s.kind {
                    SnapKind::Point(id) if sk.point(id).is_some() => id,
                    _ => sk.add_point(s.pos[0], s.pos[1]),
                };
                if to == from {
                    return Err("zero-length line".into());
                }
                let duplicate = sk.entities.iter().any(|e| {
                    matches!(e, Entity::Line { a, b, .. } if (*a == from && *b == to) || (*a == to && *b == from))
                });
                if duplicate {
                    return Err("a line already connects these two points".into());
                }
                let line = sk.add_line(from, to);
                if with_align {
                    match s.align {
                        Some(Align::Horizontal) => {
                            sk.constraints.push(Constraint::Horizontal { line })
                        }
                        Some(Align::Vertical) => sk.constraints.push(Constraint::Vertical { line }),
                        None => {}
                    }
                }
                end = Some(to);
                Ok(())
            });
            if ok {
                *new_last = end;
                if !with_align {
                    // The alignment constraint was redundant; the line itself is fine.
                    self.feedback = None;
                }
                return true;
            }
        }
        false
    }

    /// Snap for an arc's pass-through point: grid like every click, never a point.
    fn via_snap(&self, pos: [f64; 2], view: &View) -> Snap {
        let via = self.snap(pos, None, None, view);
        if matches!(via.kind, SnapKind::Point(_)) {
            Snap {
                pos,
                kind: SnapKind::Free,
                align: None,
            }
        } else {
            via
        }
    }

    fn make_rectangle(&mut self, first: Snap, second: Snap, centered: bool) {
        let (c, k) = (first.pos, second.pos);
        let (lo, hi) = if centered {
            let h = [(k[0] - c[0]).abs(), (k[1] - c[1]).abs()];
            ([c[0] - h[0], c[1] - h[1]], [c[0] + h[0], c[1] + h[1]])
        } else {
            (
                [c[0].min(k[0]), c[1].min(k[1])],
                [c[0].max(k[0]), c[1].max(k[1])],
            )
        };
        if hi[0] - lo[0] <= 1e-9 || hi[1] - lo[1] <= 1e-9 {
            self.fail(
                "Rectangle has zero width or height: click a corner away from the first point."
                    .into(),
            );
            return;
        }
        // Clicked corners that snapped to an existing point reuse it (shared
        // vertex), exactly as the snap cue promised.
        let clicked: Vec<Snap> = if centered {
            vec![second]
        } else {
            vec![first, second]
        };
        let ok = self.try_change(true, |sk| {
            let mut corner = |x: f64, y: f64| {
                let reuse = clicked.iter().find_map(|s| match s.kind {
                    SnapKind::Point(id)
                        if (s.pos[0] - x).abs() <= 1e-9
                            && (s.pos[1] - y).abs() <= 1e-9
                            && sk.point(id).is_some() =>
                    {
                        Some(id)
                    }
                    _ => None,
                });
                reuse.unwrap_or_else(|| sk.add_point(x, y))
            };
            let p1 = corner(lo[0], lo[1]);
            let p2 = corner(hi[0], lo[1]);
            let p3 = corner(hi[0], hi[1]);
            let p4 = corner(lo[0], hi[1]);
            let bottom = sk.add_line(p1, p2);
            let right = sk.add_line(p2, p3);
            let top = sk.add_line(p3, p4);
            let left = sk.add_line(p4, p1);
            sk.constraints.extend([
                Constraint::Horizontal { line: bottom },
                Constraint::Horizontal { line: top },
                Constraint::Vertical { line: right },
                Constraint::Vertical { line: left },
            ]);
            if centered {
                let center = match first.kind {
                    SnapKind::Point(id) if sk.point(id).is_some() => id,
                    _ => sk.add_point(c[0], c[1]),
                };
                sk.constraints.push(Constraint::SymmetricAboutPoint {
                    a: p1,
                    b: p3,
                    center,
                });
            }
            Ok(())
        });
        if ok {
            self.info(format!(
                "Rectangle {} × {} mm added. Use Dimension (D) to drive its size.",
                rcad_sketch::fmt_mm(hi[0] - lo[0]),
                rcad_sketch::fmt_mm(hi[1] - lo[1])
            ));
        }
    }

    fn make_circle(&mut self, center: Snap, rim: Snap, view: &View) {
        let r = dist(center.pos, rim.pos);
        if r <= view.mm(2.0) {
            self.fail("Circle radius is zero: click the rim away from the centre.".into());
            return;
        }
        let ok = self.try_change(true, |sk| {
            let c = match center.kind {
                SnapKind::Point(id) if sk.point(id).is_some() => id,
                _ => sk.add_point(center.pos[0], center.pos[1]),
            };
            sk.add_circle(c, r);
            Ok(())
        });
        if ok {
            self.info(format!(
                "Circle Ø{} mm added.",
                rcad_sketch::fmt_mm(2.0 * r)
            ));
        }
    }

    // ----- trim / extend / break (geometry in `rcad_sketch::trim`) -----

    /// Apply a curve edit (`Sketch::{trim, extend, break_at}`) as one
    /// solved transaction; the status line reports what it did.
    fn curve_edit(
        &mut self,
        verb: &str,
        edit: impl FnOnce(&mut Sketch) -> Result<rcad_sketch::CurveEdit, rcad_sketch::SketchError>,
    ) {
        let mut out = None;
        let ok = self.try_change(false, |sk| {
            out = Some(edit(sk).map_err(|e| e.to_string())?);
            Ok(())
        });
        if ok && let Some(out) = out {
            self.selection.clear();
            self.info(out.message(verb));
        }
    }

    /// Extend the line end nearest to the click along the line to the nearest
    /// curve it would cross. The end must be free (used by no other curve).
    fn extend_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if let Err(why) = self.sketch.free_form_refusal("Extend") {
            self.fail(why.to_string());
            return;
        }
        // A line, or the end point of exactly one line.
        let (e, picked_end) = match self.pick(pos, screen, view) {
            Some(Item::Entity(e)) => (e, None),
            Some(Item::Point(pt)) => {
                let lines: Vec<EntityId> = self
                    .sketch
                    .entities
                    .iter()
                    .filter(|x| matches!(x, Entity::Line { .. }) && x.points().contains(&pt))
                    .map(|x| x.id())
                    .collect();
                match lines.as_slice() {
                    [l] => (*l, Some(pt)),
                    _ => {
                        self.fail(
                            "That end is connected to another curve; only a free end can be extended."
                                .into(),
                        );
                        return;
                    }
                }
            }
            _ => {
                self.fail("Click a line near the end to extend.".into());
                return;
            }
        };
        if let Some(why) = self.locked_reason(e) {
            self.fail(why);
            return;
        }
        self.curve_edit("Extended", |sk| sk.extend(e, pos, picked_end));
    }

    fn trim_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if let Err(why) = self.sketch.free_form_refusal("Trim") {
            self.fail(why.to_string());
            return;
        }
        let Some(Item::Entity(e)) = self.pick(pos, screen, view) else {
            self.fail("Click the part of a line to remove (away from its endpoints).".into());
            return;
        };
        if let Some(why) = self.locked_reason(e) {
            self.fail(why);
            return;
        }
        self.curve_edit("Trimmed", |sk| sk.trim(e, pos));
    }

    /// Why entity `e` may not be edited (projected geometry is read-only).
    pub fn locked_reason(&self, e: EntityId) -> Option<String> {
        self.projected.get(&e).map(|l| {
            format!(
                "Entity {} is projected geometry (link {}) and is read-only: select it and use \
                 Unlink projection first.",
                e.0,
                l + 1
            )
        })
    }

    /// Why `before` -> `after` is refused: it removes or reshapes projected
    /// geometry, or drops a pin of a projected point.
    fn projected_change(&self, after: &Sketch) -> Option<String> {
        for (e, l) in &self.projected {
            let (Some(old), new) = (self.sketch.entity(*e), after.entity(*e)) else {
                continue;
            };
            if new != Some(old) {
                return Some(format!(
                    "Rejected: entity {} is projected geometry (link {}) and is read-only; \
                     unlink the projection first.",
                    e.0,
                    l + 1
                ));
            }
            for p in old.points() {
                let pins = |sk: &Sketch| {
                    sk.constraints
                        .iter()
                        .filter(|c| match c {
                            Constraint::FixedPoint { p: q, .. }
                            | Constraint::FixedX { p: q, .. }
                            | Constraint::FixedY { p: q, .. } => *q == p,
                            _ => false,
                        })
                        .count()
                };
                if pins(after) < pins(&self.sketch) {
                    return Some(format!(
                        "Rejected: that constraint holds projected entity {} (link {}); unlink \
                         the projection first.",
                        e.0,
                        l + 1
                    ));
                }
            }
        }
        None
    }

    fn break_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if let Err(why) = self.sketch.free_form_refusal("Break") {
            self.fail(why.to_string());
            return;
        }
        let Some(Item::Entity(e)) = self.pick(pos, screen, view) else {
            self.fail("Click the span of a curve to break off at its crossings.".into());
            return;
        };
        if let Some(why) = self.locked_reason(e) {
            self.fail(why);
            return;
        }
        self.curve_edit("Broken", |sk| sk.break_at(e, pos));
    }

    // ----- dimensions -----

    fn dimension_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        let hit = self.pick(pos, screen, view);
        match (self.gesture.clone(), hit) {
            (Gesture::DimFirst(a), Some(Item::Point(b))) if a != b => {
                self.gesture = Gesture::Idle;
                self.dimension_points(a, b);
            }
            (Gesture::DimFirst(_), _) => {
                self.gesture = Gesture::Idle;
                self.fail("Pick a second, different point (Esc cancels).".into());
            }
            (_, Some(Item::Point(p))) => self.gesture = Gesture::DimFirst(p),
            (_, Some(Item::Entity(e))) => self.dimension_entity(e),
            (_, Some(Item::Constraint(i))) => {
                if self
                    .sketch
                    .constraints
                    .get(i)
                    .and_then(|c| c.dimension())
                    .is_some()
                {
                    self.begin_dim_edit(i, false);
                }
            }
            (_, None) => {
                self.fail("Nothing to dimension here: click a line, circle or point.".into())
            }
        }
    }

    fn dimension_entity(&mut self, e: EntityId) {
        let Some(ent) = self.sketch.entity(e).cloned() else {
            return;
        };
        let c = match ent {
            Entity::Arc { id, .. } => {
                let Some(g) = self.sketch.arc_geometry(id) else {
                    return;
                };
                Constraint::Radius {
                    entity: id,
                    dim: Dimension::new(self.sketch.unique_dimension_name("radius"), g.radius),
                }
            }
            Entity::Circle { radius, .. } => Constraint::Diameter {
                circle: e,
                dim: Dimension::new(self.sketch.unique_dimension_name("diameter"), 2.0 * radius),
            },
            Entity::Ellipse {
                center,
                major,
                minor,
                ..
            } => {
                // First click: the major radius; second: the minor radius.
                let has = |m: bool| {
                    self.sketch.constraints.iter().any(|c| {
                        matches!(c, Constraint::EllipseRadius { ellipse, major, .. }
                            if *ellipse == e && *major == m)
                    })
                };
                let (Some(c), Some(m)) =
                    (self.sketch.point_xy(center), self.sketch.point_xy(major))
                else {
                    return;
                };
                if !has(true) {
                    Constraint::EllipseRadius {
                        ellipse: e,
                        major: true,
                        dim: Dimension::new(
                            self.sketch.unique_dimension_name("major_radius"),
                            dist(c, m),
                        ),
                    }
                } else if !has(false) {
                    Constraint::EllipseRadius {
                        ellipse: e,
                        major: false,
                        dim: Dimension::new(
                            self.sketch.unique_dimension_name("minor_radius"),
                            minor,
                        ),
                    }
                } else {
                    self.fail("Both ellipse radii are already dimensioned.".into());
                    return;
                }
            }
            Entity::Spline { .. } => {
                self.fail(
                    "A spline has no dimension of its own: dimension or fix its control points."
                        .into(),
                );
                return;
            }
            Entity::Line { a, b, .. } => {
                let (Some(pa), Some(pb)) = (self.sketch.point_xy(a), self.sketch.point_xy(b))
                else {
                    return;
                };
                let horizontal = self
                    .sketch
                    .constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::Horizontal { line } if *line == e));
                let vertical = self
                    .sketch
                    .constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::Vertical { line } if *line == e));
                if horizontal {
                    let (a, b) = if pa[0] <= pb[0] { (a, b) } else { (b, a) };
                    Constraint::HorizontalDistance {
                        a,
                        b,
                        dim: Dimension::new(
                            self.sketch.unique_dimension_name("width"),
                            (pb[0] - pa[0]).abs(),
                        ),
                    }
                } else if vertical {
                    let (a, b) = if pa[1] <= pb[1] { (a, b) } else { (b, a) };
                    Constraint::VerticalDistance {
                        a,
                        b,
                        dim: Dimension::new(
                            self.sketch.unique_dimension_name("height"),
                            (pb[1] - pa[1]).abs(),
                        ),
                    }
                } else {
                    Constraint::Distance {
                        a,
                        b,
                        dim: Dimension::new(
                            self.sketch.unique_dimension_name("length"),
                            dist(pa, pb),
                        ),
                    }
                }
            }
        };
        self.add_dimension(c);
    }

    fn dimension_points(&mut self, a: PointId, b: PointId) {
        let (Some(pa), Some(pb)) = (self.sketch.point_xy(a), self.sketch.point_xy(b)) else {
            return;
        };
        let (dx, dy) = ((pb[0] - pa[0]).abs(), (pb[1] - pa[1]).abs());
        // The larger offset first; dimensioning the same two points again gives
        // the other axis (a corner at (2, 2) needs both).
        let pair = |x: PointId, y: PointId| (x == a && y == b) || (x == b && y == a);
        let has_h = self.sketch.constraints.iter().any(
            |c| matches!(c, Constraint::HorizontalDistance { a: x, b: y, .. } if pair(*x, *y)),
        );
        let has_v =
            self.sketch.constraints.iter().any(
                |c| matches!(c, Constraint::VerticalDistance { a: x, b: y, .. } if pair(*x, *y)),
            );
        let horizontal = match (has_h, has_v) {
            (true, false) => false,
            (false, true) => true,
            _ => dx >= dy,
        };
        let c = if horizontal {
            let (a, b) = if pa[0] <= pb[0] { (a, b) } else { (b, a) };
            Constraint::HorizontalDistance {
                a,
                b,
                dim: Dimension::new(self.sketch.unique_dimension_name("dx"), dx),
            }
        } else {
            let (a, b) = if pa[1] <= pb[1] { (a, b) } else { (b, a) };
            Constraint::VerticalDistance {
                a,
                b,
                dim: Dimension::new(self.sketch.unique_dimension_name("dy"), dy),
            }
        };
        self.add_dimension(c);
    }

    fn add_dimension(&mut self, c: Constraint) {
        let value = c.dimension().map_or(0.0, |d| d.value);
        if !(value.is_finite() && value > 1e-9) {
            self.fail(
                "Cannot dimension a zero distance: the points coincide along that axis.".into(),
            );
            return;
        }
        let idx = self.sketch.constraints.len();
        if self.try_change(true, |sk| {
            sk.constraints.push(c);
            Ok(())
        }) {
            self.begin_dim_edit(idx, true);
        }
    }

    pub fn begin_dim_edit(&mut self, constraint: usize, fresh: bool) {
        let value = self
            .sketch
            .constraints
            .get(constraint)
            .and_then(|c| c.dimension())
            .map(|d| d.value);
        if let Some(value) = value {
            self.cancel_gesture();
            self.dim_edit = Some(DimEdit {
                constraint,
                buffer: rcad_sketch::fmt_mm(value),
                fresh,
            });
            self.selection = vec![Item::Constraint(constraint)];
        }
    }

    /// Escape on the inline editor: a fresh dimension keeps its measured value.
    pub fn cancel_dim_edit(&mut self) {
        self.dim_edit = None;
    }

    /// Enter on the inline editor. On invalid input or a failed solve, the
    /// editor stays open with an error and the sketch is unchanged.
    pub fn apply_dim_edit(&mut self) -> bool {
        let Some(ed) = self.dim_edit.clone() else {
            return false;
        };
        let Some(name) = self
            .sketch
            .constraints
            .get(ed.constraint)
            .and_then(|c| c.dimension())
            .map(|d| d.name.clone())
        else {
            self.dim_edit = None;
            return false;
        };
        let angle = matches!(
            self.sketch.constraints.get(ed.constraint),
            Some(Constraint::Angle { .. } | Constraint::PatternRotate { .. })
        );
        let input = match parse_dim_input(&ed.buffer, angle, &self.param_env) {
            Ok(i) => i,
            Err(why) => {
                self.fail(format!(
                    "'{}' is not a valid dimension: {why}. Enter a positive number (mm, cm, in; \
                     deg or rad for angles), a parameter expression, or name = value.",
                    ed.buffer.trim()
                ));
                return false;
            }
        };
        let value = match &input {
            DimInput::Value(v) | DimInput::Bound { value: v, .. } => *v,
            DimInput::NewParameter { value, .. } => *value,
        };
        let ok = self.try_change(false, |sk| {
            sk.set_dimension(&name, value).map_err(|e| e.to_string())
        });
        if ok {
            self.dim_edit = None;
            let unit = if angle { "°" } else { " mm" };
            let shown = match input {
                DimInput::Value(_) => {
                    // A typed number replaces an expression binding.
                    if self.bound.contains_key(&name) || self.bindings.contains_key(&name) {
                        self.bindings.insert(name.clone(), String::new());
                    }
                    String::new()
                }
                DimInput::Bound { expr, .. } => {
                    let shown = format!(" (= {expr})");
                    self.bindings.insert(name.clone(), expr);
                    shown
                }
                DimInput::NewParameter {
                    value,
                    name: p,
                    expr,
                } => {
                    self.param_env.insert(
                        p.clone(),
                        rcad_document::params::Quantity {
                            value,
                            dim: Some(if angle { 0 } else { 1 }),
                        },
                    );
                    self.new_params
                        .push(rcad_commands::sketch_ops::NewParameter {
                            name: p.clone(),
                            expression: expr.clone(),
                            unit: angle.then_some(rcad_document::ParamUnit::Deg),
                        });
                    self.bindings.insert(name.clone(), p.clone());
                    format!(" (= {p}, new parameter {p} = {expr})")
                }
            };
            self.info(format!(
                "{name} = {}{unit}{shown}",
                rcad_sketch::fmt_mm(value)
            ));
        }
        ok
    }

    // ----- constraints and deletion -----

    pub fn selected_lines(&self) -> Vec<EntityId> {
        self.selection
            .iter()
            .filter_map(|i| match i {
                Item::Entity(e) => match self.sketch.entity(*e) {
                    Some(Entity::Line { .. }) => Some(*e),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    pub fn selected_points(&self) -> Vec<PointId> {
        self.selection
            .iter()
            .filter_map(|i| match i {
                Item::Point(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    /// Why Horizontal/Vertical is unavailable, or None if it can be applied.
    pub fn hv_disabled_reason(&self) -> Option<&'static str> {
        self.selected_lines()
            .is_empty()
            .then_some("Select one or more lines first")
    }

    pub fn coincident_disabled_reason(&self) -> Option<&'static str> {
        (self.selected_points().len() != 2).then_some("Select exactly two points first")
    }

    pub fn constrain_lines(&mut self, vertical: bool) {
        self.end_lone_chain();
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.try_change(true, |sk| {
            for line in lines {
                let exists = sk.constraints.iter().any(|c| match c {
                    Constraint::Horizontal { line: l } | Constraint::Vertical { line: l } => {
                        *l == line
                    }
                    _ => false,
                });
                if exists {
                    return Err(format!(
                        "L{} already has a horizontal/vertical constraint",
                        line.0
                    ));
                }
                sk.constraints.push(if vertical {
                    Constraint::Vertical { line }
                } else {
                    Constraint::Horizontal { line }
                });
            }
            Ok(())
        });
    }

    pub fn selected_entities(&self) -> Vec<EntityId> {
        self.selection
            .iter()
            .filter_map(|i| match i {
                Item::Entity(e) => Some(*e),
                _ => None,
            })
            .collect()
    }

    /// A (line, arc) pair sharing an endpoint, if that is the selection.
    fn tangent_pair(&self) -> Option<(EntityId, EntityId)> {
        let es = self.selected_entities();
        if es.len() != 2 {
            return None;
        }
        let kind = |e: EntityId| self.sketch.entity(e).cloned();
        let (line, arc) = match (kind(es[0])?, kind(es[1])?) {
            (l @ Entity::Line { .. }, a @ Entity::Arc { .. }) => (l, a),
            (a @ Entity::Arc { .. }, l @ Entity::Line { .. }) => (l, a),
            _ => return None,
        };
        let shared = line
            .points()
            .iter()
            .any(|p| matches!(arc, Entity::Arc { start, end, .. } if *p == start || *p == end));
        shared.then_some((line.id(), arc.id()))
    }

    pub fn tangent_disabled_reason(&self) -> Option<&'static str> {
        self.tangent_pair()
            .is_none()
            .then_some("Select a line and an arc that share an endpoint")
    }

    pub fn equal_disabled_reason(&self) -> Option<&'static str> {
        let es = self.selected_entities();
        let round = es.iter().all(|e| {
            matches!(
                self.sketch.entity(*e),
                Some(Entity::Arc { .. } | Entity::Circle { .. })
            )
        });
        let lines = self.selected_lines().len() == 2;
        (self.selection.len() != 2 || es.len() != 2 || !(round || lines))
            .then_some("Select two lines (equal length) or two arcs/circles (equal radius)")
    }

    fn two_lines(&self) -> Option<(EntityId, EntityId)> {
        let l = self.selected_lines();
        (self.selection.len() == 2 && l.len() == 2).then(|| (l[0], l[1]))
    }

    pub fn two_lines_disabled_reason(&self) -> Option<&'static str> {
        self.two_lines()
            .is_none()
            .then_some("Select exactly two lines")
    }

    pub fn constrain_parallel(&mut self, perpendicular: bool) {
        self.end_lone_chain();
        let Some((a, b)) = self.two_lines() else {
            return;
        };
        self.try_change(true, |sk| {
            sk.constraints.push(if perpendicular {
                Constraint::Perpendicular { a, b }
            } else {
                Constraint::Parallel { a, b }
            });
            Ok(())
        });
    }

    /// Angle dimension between the two selected lines: the counter-clockwise
    /// angle from one line's direction to the other's, oriented so the value is
    /// in (0, 180); opens the inline value editor (degrees).
    pub fn constrain_angle(&mut self) {
        self.end_lone_chain();
        let Some((a, b)) = self.two_lines() else {
            return;
        };
        let dir = |l: EntityId| -> Option<[f64; 2]> {
            let Some(Entity::Line { a, b, .. }) = self.sketch.entity(l) else {
                return None;
            };
            let (p, q) = (self.sketch.point_xy(*a)?, self.sketch.point_xy(*b)?);
            Some([q[0] - p[0], q[1] - p[1]])
        };
        let (Some(u), Some(v)) = (dir(a), dir(b)) else {
            return;
        };
        let deg = (u[0] * v[1] - u[1] * v[0])
            .atan2(u[0] * v[0] + u[1] * v[1])
            .to_degrees();
        // Measure from the line that makes the CCW angle positive.
        let (a, b, deg) = if deg >= 0.0 {
            (a, b, deg)
        } else {
            (b, a, -deg)
        };
        if !(deg > 1e-6 && deg < 180.0 - 1e-6) {
            self.fail("The lines are parallel: use Parallel, or move one line first.".into());
            return;
        }
        self.add_dimension(Constraint::Angle {
            a,
            b,
            dim: Dimension::new(self.sketch.unique_dimension_name("angle"), deg),
        });
    }

    pub fn on_line_disabled_reason(&self) -> Option<&'static str> {
        self.midpoint_pair()
            .is_none()
            .then_some("Select one line and one point (not its endpoint)")
    }

    /// Keep the selected point on the (infinite) selected line.
    pub fn constrain_on_line(&mut self) {
        self.end_lone_chain();
        let Some((line, point)) = self.midpoint_pair() else {
            return;
        };
        self.try_change(true, |sk| {
            sk.constraints.push(Constraint::PointOnLine { point, line });
            Ok(())
        });
    }

    pub fn construction_disabled_reason(&self) -> Option<&'static str> {
        self.selected_entities()
            .is_empty()
            .then_some("Select lines, arcs or circles first")
    }

    /// Toggle construction (reference) geometry: it still solves and can be
    /// dimensioned, but profiles ignore it.
    pub fn toggle_construction(&mut self) {
        self.end_lone_chain();
        let es = self.selected_entities();
        if es.is_empty() {
            return;
        }
        let on = !es.iter().all(|e| self.sketch.is_construction(*e));
        self.try_change(false, |sk| {
            for e in &es {
                sk.set_construction(*e, on);
            }
            Ok(())
        });
    }

    /// (line, point) when the selection is exactly one line and one point
    /// that is not one of the line's endpoints.
    fn midpoint_pair(&self) -> Option<(EntityId, PointId)> {
        let lines = self.selected_lines();
        let pts = self.selected_points();
        if self.selection.len() != 2 || lines.len() != 1 || pts.len() != 1 {
            return None;
        }
        let ends = self.sketch.entity(lines[0])?.points();
        (!ends.contains(&pts[0])).then_some((lines[0], pts[0]))
    }

    pub fn midpoint_disabled_reason(&self) -> Option<&'static str> {
        self.midpoint_pair()
            .is_none()
            .then_some("Select one line and one point (not its endpoint)")
    }

    /// Keep the selected point at the midpoint of the selected line.
    pub fn constrain_midpoint(&mut self) {
        self.end_lone_chain();
        let Some((line, p)) = self.midpoint_pair() else {
            return;
        };
        let Some(Entity::Line { a, b, .. }) = self.sketch.entity(line).cloned() else {
            return;
        };
        self.try_change(true, |sk| {
            sk.constraints
                .push(Constraint::SymmetricAboutPoint { a, b, center: p });
            Ok(())
        });
    }

    pub fn constrain_tangent(&mut self) {
        self.end_lone_chain();
        let Some((line, arc)) = self.tangent_pair() else {
            return;
        };
        self.try_change(true, |sk| {
            sk.constraints.push(Constraint::Tangent { line, arc });
            Ok(())
        });
    }

    /// Equal length (two lines) or equal radius (two arcs/circles).
    pub fn constrain_equal_radius(&mut self) {
        self.end_lone_chain();
        let es = self.selected_entities();
        if self.equal_disabled_reason().is_some() {
            return;
        }
        let lines = self.selected_lines().len() == 2;
        self.try_change(true, |sk| {
            sk.constraints.push(if lines {
                Constraint::EqualLength { a: es[0], b: es[1] }
            } else {
                Constraint::EqualRadius { a: es[0], b: es[1] }
            });
            Ok(())
        });
    }

    pub fn constrain_coincident(&mut self) {
        self.end_lone_chain();
        let p = self.selected_points();
        if p.len() != 2 {
            return;
        }
        self.try_change(true, |sk| {
            sk.constraints
                .push(Constraint::Coincident { a: p[0], b: p[1] });
            Ok(())
        });
    }

    /// Delete the selection. The origin and its fix cannot be deleted.
    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.end_lone_chain();
        let sel = std::mem::take(&mut self.selection);
        let kept = sel.clone();
        let origin = self.sketch.origin;
        let ok = self.try_change(false, |sk| {
            let mut cons: Vec<usize> = sel
                .iter()
                .filter_map(|i| match i {
                    Item::Constraint(c) => Some(*c),
                    _ => None,
                })
                .filter(|&c| {
                    !matches!(sk.constraints.get(c), Some(Constraint::FixedPoint { p, .. }) if Some(*p) == origin)
                })
                .collect();
            cons.sort_unstable();
            cons.dedup();
            for c in cons.into_iter().rev() {
                sk.remove_constraint(c);
            }
            let mut ents: Vec<EntityId> = sel
                .iter()
                .filter_map(|i| match i {
                    Item::Entity(e) => Some(*e),
                    _ => None,
                })
                .collect();
            for p in sel.iter().filter_map(|i| match i {
                Item::Point(p) if Some(*p) != origin => Some(*p),
                _ => None,
            }) {
                ents.extend(
                    sk.entities
                        .iter()
                        .filter(|e| e.points().contains(&p))
                        .map(|e| e.id()),
                );
            }
            for e in ents {
                sk.remove_entity(e);
            }
            // Selected points no curve uses any more (e.g. a lone point) go too.
            for p in sel.iter().filter_map(|i| match i {
                Item::Point(p) if Some(*p) != origin => Some(*p),
                _ => None,
            }) {
                if sk.point(p).is_some() && !sk.point_used_by_entity(p) {
                    sk.constraints.retain(|c| !constraint_mentions(c, p));
                    sk.points.retain(|q| q.id != p);
                }
            }
            Ok(())
        });
        if ok {
            // Constraint indices may have shifted: never leave an editor
            // pointing at a different dimension.
            self.dim_edit = None;
        } else {
            self.selection = kept;
        }
    }
}

/// Sketch text: the Text tool and its dialog state.
impl SketchEditor {
    /// The text whose outline box (widened by the pick tolerance) contains `pos`.
    pub fn text_at(&self, pos: [f64; 2], view: &View) -> Option<u32> {
        let pad = view.mm(PICK_PX);
        self.sketch.texts.iter().rev().find_map(|t| {
            let polys = t.polylines(&self.sketch).ok()?;
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for (_, _, v) in &polys {
                for p in v {
                    b = [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ];
                }
            }
            (pos[0] >= b[0] - pad
                && pos[0] <= b[2] + pad
                && pos[1] >= b[1] - pad
                && pos[1] <= b[3] + pad)
                .then_some(t.id)
        })
    }

    fn text_click(&mut self, pos: [f64; 2], view: &View) {
        if let Some(id) = self.text_at(pos, view) {
            self.begin_text_edit(id);
            return;
        }
        let s = self.snap(pos, None, None, view);
        let anchor = match s.kind {
            SnapKind::Point(p) => Some(p),
            _ => None,
        };
        let (family, height) = self
            .text_edit
            .as_ref()
            .map(|t| (t.family.clone(), t.height.clone()))
            .unwrap_or_else(|| (DEFAULT_TEXT_FAMILY.to_string(), "5".to_string()));
        self.text_edit = Some(TextEdit {
            id: None,
            text: "Text".into(),
            family,
            bold: false,
            italic: false,
            height,
            rotation: "0".into(),
            halign: rcad_sketch::HAlign::Left,
            valign: rcad_sketch::VAlign::Baseline,
            position: s.pos,
            anchor,
        });
        self.info(
            "Type the text, font and height (cap height, mm), then Apply. Glyph outlines become \
             profile regions."
                .into(),
        );
    }

    /// Open the dialog on existing text `id`.
    pub fn begin_text_edit(&mut self, id: u32) {
        let Some(t) = self.sketch.text_item(id) else {
            return;
        };
        let sp = &t.spec;
        self.text_edit = Some(TextEdit {
            id: Some(id),
            text: sp.text.clone(),
            family: sp.font.family.clone(),
            bold: sp.font.bold,
            italic: sp.font.italic,
            height: fmt_num(sp.height),
            rotation: fmt_num(sp.rotation.to_degrees()),
            halign: sp.halign,
            valign: sp.valign,
            position: sp.position,
            anchor: sp.anchor,
        });
    }

    /// The typed fields as a text specification, or why they are invalid.
    #[allow(clippy::type_complexity)]
    pub fn text_spec(&self) -> Option<Result<(Option<u32>, rcad_sketch::TextSpec), String>> {
        let t = self.text_edit.as_ref()?;
        let num = |v: &str| {
            v.trim()
                .trim_end_matches("mm")
                .trim_end_matches('°')
                .trim()
                .replace('−', "-")
                .parse::<f64>()
        };
        let height = match num(&t.height) {
            Ok(h) if h.is_finite() && h > 0.0 => h,
            _ => return Some(Err(format!("'{}' is not a height > 0 mm", t.height))),
        };
        let rot = match num(&t.rotation) {
            Ok(r) if r.is_finite() => r.to_radians(),
            _ => return Some(Err(format!("'{}' is not an angle in degrees", t.rotation))),
        };
        // An explicit font file of existing text is kept (the dialog edits families).
        let path =
            t.id.and_then(|id| self.sketch.text_item(id))
                .and_then(|x| x.spec.font.path.clone());
        Some(Ok((
            t.id,
            rcad_sketch::TextSpec {
                text: t.text.clone(),
                font: rcad_sketch::FontSpec {
                    family: t.family.trim().to_string(),
                    bold: t.bold,
                    italic: t.italic,
                    path,
                },
                height,
                position: t.position,
                anchor: t.anchor,
                rotation: rot,
                halign: t.halign,
                valign: t.valign,
            },
        )))
    }

    /// Apply the dialog (one undo step): add or change the text. A refusal
    /// (missing font, missing glyph, overlapping outlines…) keeps the dialog
    /// open with the reason and the sketch unchanged.
    pub fn apply_text(&mut self) -> bool {
        let Some(parsed) = self.text_spec() else {
            return false;
        };
        let (id, spec) = match parsed {
            Ok(v) => v,
            Err(m) => {
                self.fail(m);
                return false;
            }
        };
        let mut new_id = id;
        let ok = self.try_change(false, |sk| {
            match id {
                Some(i) => sk.edit_text(i, spec),
                None => sk.add_text(spec).map(|n| new_id = Some(n)),
            }
            .map_err(|e| format!("Text refused: {e}"))?;
            // Overlaps with other loops are reported now, not at extrusion.
            sk.regions()
                .map(|_| ())
                .map_err(|e| format!("Text refused: {e}"))
        });
        if ok {
            self.text_edit = None;
            let n = new_id.and_then(|i| self.sketch.text_item(i)).map(|t| {
                format!(
                    "Text {} set with {} {} ({}); outlines stored in the sketch.",
                    t.id, t.resolved.font.family, t.resolved.font.style, t.resolved.font.file
                )
            });
            self.info(n.unwrap_or_default());
        }
        ok
    }

    /// Delete the text open in the dialog (one undo step).
    pub fn delete_text(&mut self) -> bool {
        let Some(id) = self.text_edit.as_ref().and_then(|t| t.id) else {
            return false;
        };
        let ok = self.try_change(false, |sk| {
            if sk.remove_text(id) {
                Ok(())
            } else {
                Err(format!("no text {id}"))
            }
        });
        if ok {
            self.text_edit = None;
            self.info(format!("Text {id} deleted."));
        }
        ok
    }
}

/// Short decimal for dialog fields (no trailing zeros).
fn fmt_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Slot and regular polygon tools.
impl SketchEditor {
    fn ellipse_click(&mut self, pos: [f64; 2], view: &View) {
        match self.gesture.clone() {
            Gesture::FirstClick(center) => {
                let axis = self.snap(pos, Some(center.pos), None, view);
                if dist(center.pos, axis.pos) <= view.mm(2.0) {
                    self.fail("The axis end coincides with the centre: click elsewhere.".into());
                    return;
                }
                self.gesture = Gesture::EllipseAxis { center, axis };
            }
            Gesture::EllipseAxis { center, axis } => {
                let b = line_distance(pos, center.pos, axis.pos);
                if b <= view.mm(2.0) {
                    self.fail("The other radius is zero: click beside the axis.".into());
                    return;
                }
                self.gesture = Gesture::Idle;
                let a = dist(center.pos, axis.pos);
                let ok = self.try_change(true, |sk| {
                    let mut pid = |s: &Snap| match s.kind {
                        SnapKind::Point(id) if sk.point(id).is_some() => id,
                        _ => sk.add_point(s.pos[0], s.pos[1]),
                    };
                    let (c, m) = (pid(&center), pid(&axis));
                    if c == m {
                        return Err("the ellipse centre and axis end are the same point".into());
                    }
                    let id = sk.next_entity_id();
                    sk.entities.push(Entity::Ellipse {
                        id,
                        center: c,
                        major: m,
                        minor: b,
                    });
                    Ok(())
                });
                if ok {
                    self.info(format!(
                        "Ellipse added (radii {} × {} mm). Dimension (D) it for editable radii.",
                        rcad_sketch::fmt_mm(a),
                        rcad_sketch::fmt_mm(b)
                    ));
                }
            }
            _ => self.gesture = Gesture::FirstClick(self.snap(pos, None, None, view)),
        }
    }

    fn spline_click(&mut self, pos: [f64; 2], view: &View) {
        let mut pts = match self.gesture.clone() {
            Gesture::Spline(p) => p,
            _ => Vec::new(),
        };
        let s = self.snap(pos, pts.last().map(|p| p.pos), None, view);
        if let Some(first) = pts.first()
            && dist(first.pos, s.pos) <= view.mm(SNAP_PX)
        {
            self.gesture = Gesture::Spline(pts);
            self.finish_spline(true);
            return;
        }
        if pts
            .last()
            .is_some_and(|l| dist(l.pos, s.pos) <= view.mm(2.0))
        {
            self.fail("Control point repeats the previous one: click elsewhere.".into());
            return;
        }
        pts.push(s);
        let n = pts.len();
        self.gesture = Gesture::Spline(pts);
        self.info(format!(
            "{n} control point(s). Enter finishes an open spline (4+), clicking the first point \
             closes it (3+)."
        ));
    }

    /// Add the spline of the pending control points (`closed`: periodic).
    fn finish_spline(&mut self, closed: bool) {
        let Gesture::Spline(pts) = std::mem::replace(&mut self.gesture, Gesture::Idle) else {
            return;
        };
        self.epoch += 1;
        let need = if closed { 3 } else { 4 };
        if pts.len() < need {
            self.fail(format!(
                "{} spline needs at least {need} control points ({} clicked); nothing added.",
                if closed { "A closed" } else { "An open" },
                pts.len()
            ));
            return;
        }
        let n = pts.len();
        let ok = self.try_change(true, |sk| {
            let ids: Vec<PointId> = pts
                .iter()
                .enumerate()
                .map(|(i, s)| match s.kind {
                    // The ends of an open spline may join existing points.
                    SnapKind::Point(id)
                        if !closed && (i == 0 || i + 1 == n) && sk.point(id).is_some() =>
                    {
                        id
                    }
                    _ => sk.add_point(s.pos[0], s.pos[1]),
                })
                .collect();
            let id = sk.next_entity_id();
            sk.entities.push(Entity::Spline {
                id,
                points: ids,
                closed,
            });
            sk.validate().map_err(|e| e.to_string())
        });
        if ok {
            self.info(format!(
                "{} spline added with {n} control points (drag them to reshape).",
                if closed { "Closed" } else { "Open" }
            ));
        }
    }

    fn slot_click(&mut self, pos: [f64; 2], view: &View) {
        match self.gesture.clone() {
            Gesture::FirstClick(c1) => {
                let c2 = self.snap(pos, Some(c1.pos), None, view);
                if dist(c1.pos, c2.pos) <= view.mm(2.0) {
                    self.fail(
                        "The slot centres coincide: click the second centre elsewhere.".into(),
                    );
                    return;
                }
                self.gesture = Gesture::SlotWidth { c1, c2 };
            }
            Gesture::SlotWidth { c1, c2 } => {
                let w = 2.0 * line_distance(pos, c1.pos, c2.pos);
                if w <= view.mm(2.0) {
                    self.fail("Slot width is zero: click beside the centre line.".into());
                    return;
                }
                self.gesture = Gesture::Idle;
                let pid = |s: &Snap| match s.kind {
                    SnapKind::Point(p) => Some(p),
                    _ => None,
                };
                let mut names = (String::new(), String::new());
                let ok = self.try_change(true, |sk| {
                    let ids = [
                        pid(&c1).filter(|p| sk.point(*p).is_some()),
                        pid(&c2).filter(|p| sk.point(*p).is_some()),
                    ];
                    let r = sk
                        .add_slot(c1.pos, c2.pos, w, ids)
                        .map_err(|e| format!("Slot refused: {e}"))?;
                    names = r.dimensions;
                    Ok(())
                });
                if ok {
                    self.info(format!(
                        "Slot added: {} = {} mm, {} = {} mm (editable dimensions).",
                        names.0,
                        rcad_sketch::fmt_mm(dist(c1.pos, c2.pos)),
                        names.1,
                        rcad_sketch::fmt_mm(w)
                    ));
                }
            }
            _ => self.gesture = Gesture::FirstClick(self.snap(pos, None, None, view)),
        }
    }

    fn make_polygon(&mut self, center: Snap, vertex: Snap, view: &View) {
        if dist(center.pos, vertex.pos) <= view.mm(2.0) {
            self.fail("Polygon size is zero: click the vertex away from the centre.".into());
            return;
        }
        let (n, fit) = (self.polygon_sides, self.polygon_fit);
        let mut name = String::new();
        let ok = self.try_change(true, |sk| {
            let c = match center.kind {
                SnapKind::Point(id) if sk.point(id).is_some() => Some(id),
                _ => None,
            };
            let r = sk
                .add_regular_polygon(center.pos, vertex.pos, n, fit, c)
                .map_err(|e| format!("Polygon refused: {e}"))?;
            name = r.dimension;
            Ok(())
        });
        if ok {
            self.info(format!(
                "{n}-sided polygon added ({}); dimension '{name}'.",
                match fit {
                    rcad_sketch::PolygonFit::Inscribed => "across corners",
                    rcad_sketch::PolygonFit::Circumscribed => "across flats",
                }
            ));
        }
    }

    /// Preview outline (sketch mm) of the slot or polygon being placed.
    pub fn shape_preview(&self) -> Option<Vec<[f64; 2]>> {
        let cur = self.cursor?.pos;
        match (&self.gesture, self.tool) {
            (Gesture::EllipseAxis { center, axis }, SketchTool::Ellipse) => {
                let b = line_distance(cur, center.pos, axis.pos);
                let mut s = Sketch::default();
                let (e, ..) = s.add_ellipse(center.pos, axis.pos, b).ok()?;
                rcad_sketch::curves::entity_polyline(&s, s.entity(e)?, 24)
            }
            (Gesture::Spline(pts), SketchTool::Spline) => {
                let mut ctrl: Vec<[f64; 2]> = pts.iter().map(|s| s.pos).collect();
                ctrl.push(cur);
                if ctrl.len() < 4 {
                    return Some(ctrl);
                }
                let mut s = Sketch::default();
                let (e, _) = s.add_spline(&ctrl, false).ok()?;
                rcad_sketch::curves::entity_polyline(&s, s.entity(e)?, 24)
            }
            (Gesture::FirstClick(c), SketchTool::Polygon) => {
                let mut s = Sketch::default();
                let r = s
                    .add_regular_polygon(c.pos, cur, self.polygon_sides, self.polygon_fit, None)
                    .ok()?;
                let mut v: Vec<[f64; 2]> =
                    r.vertices.iter().filter_map(|p| s.point_xy(*p)).collect();
                v.push(*v.first()?);
                Some(v)
            }
            (Gesture::SlotWidth { c1, c2 }, SketchTool::Slot) => {
                let r = line_distance(cur, c1.pos, c2.pos);
                let (a, b) = (c1.pos, c2.pos);
                let l = dist(a, b);
                if l <= 0.0 || r <= 0.0 {
                    return None;
                }
                let u = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
                let th = u[1].atan2(u[0]);
                let mut v = Vec::new();
                for (c, t0) in [
                    (b, th - std::f64::consts::FRAC_PI_2),
                    (a, th + std::f64::consts::FRAC_PI_2),
                ] {
                    for i in 0..=24 {
                        let t = t0 + std::f64::consts::PI * i as f64 / 24.0;
                        v.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
                    }
                }
                v.push(v[0]);
                Some(v)
            }
            _ => None,
        }
    }
}

/// Distance from `p` to the infinite line through `a` and `b`.
fn line_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let l = dist(a, b);
    if l <= 0.0 {
        return dist(p, a);
    }
    ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])).abs() / l
}

/// Sketch patterns: rectangular / circular constrained copies.
impl SketchEditor {
    pub fn pattern_disabled_reason(&self) -> Option<&'static str> {
        self.selected_entities()
            .is_empty()
            .then_some("Select the lines, arcs or circles to pattern first")
    }

    /// Open the pattern dialog on the selected curves.
    pub fn begin_pattern(&mut self) {
        let entities = self.selected_entities();
        if entities.is_empty() {
            return;
        }
        self.pattern_edit = Some(PatternEdit {
            entities,
            circular: false,
            count: "3".into(),
            dx: "10".into(),
            dy: "0".into(),
            two_directions: false,
            count2: "2".into(),
            dx2: "0".into(),
            dy2: "10".into(),
            ccount: "6".into(),
            angle: "60".into(),
            cx: "0".into(),
            cy: "0".into(),
        });
    }

    /// Apply the dialog (one undo step); a refusal keeps it open with the reason.
    pub fn apply_pattern(&mut self) -> bool {
        let Some(p) = self.pattern_edit.clone() else {
            return false;
        };
        let num = |v: &str, what: &str| -> Result<f64, String> {
            let t = v
                .trim()
                .trim_end_matches("mm")
                .trim_end_matches('°')
                .trim()
                .replace('−', "-");
            t.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or(format!("'{v}' is not a number ({what})"))
        };
        let count = |v: &str| -> Result<usize, String> {
            v.trim()
                .parse::<usize>()
                .map_err(|_| format!("'{v}' is not a whole number of instances"))
        };
        let parsed: Result<_, String> = (|| {
            if p.circular {
                Ok((
                    true,
                    vec![],
                    count(&p.ccount)?,
                    num(&p.angle, "angle")?,
                    [num(&p.cx, "centre x")?, num(&p.cy, "centre y")?],
                ))
            } else {
                let mut steps = vec![(
                    [num(&p.dx, "step x")?, num(&p.dy, "step y")?],
                    count(&p.count)?,
                )];
                if p.two_directions {
                    steps.push((
                        [num(&p.dx2, "step 2 x")?, num(&p.dy2, "step 2 y")?],
                        count(&p.count2)?,
                    ));
                }
                Ok((false, steps, 0, 0.0, [0.0, 0.0]))
            }
        })();
        let (circular, steps, n, angle, c) = match parsed {
            Ok(v) => v,
            Err(m) => {
                self.fail(m);
                return false;
            }
        };
        let mut names = vec![];
        let ok = self.try_change(true, |sk| {
            let r = if circular {
                // The origin when the centre is (0, 0) and the sketch has one.
                let center = sk.origin.filter(|o| sk.point_xy(*o) == Some(c));
                sk.pattern_circular(&p.entities, center, c, n, angle)
            } else {
                sk.pattern_linear(&p.entities, &steps)
            }
            .map_err(|e| format!("Pattern refused: {e}"))?;
            names = r.dimensions;
            Ok(())
        });
        if ok {
            self.pattern_edit = None;
            self.info(format!(
                "Pattern added; edit {} to move the copies (copies follow their sources).",
                names.join(", ")
            ));
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            px_per_mm: 4.0,
            grid: Some(1.0),
            snapping: true,
        }
    }

    fn click(ed: &mut SketchEditor, p: [f64; 2]) {
        ed.pointer_move(p, None, &view());
        ed.click(p, None, &view(), false);
    }

    fn area(ed: &SketchEditor) -> f64 {
        match ed.sketch.profile().expect("profile") {
            rcad_sketch::Profile::Polygon(v) => {
                0.5 * (0..v.len())
                    .map(|i| {
                        let (p, q) = (v[i], v[(i + 1) % v.len()]);
                        p[0] * q[1] - q[0] * p[1]
                    })
                    .sum::<f64>()
            }
            _ => panic!("not a polygon"),
        }
    }

    #[test]
    fn centered_rectangle_on_origin_dimensions_to_full_constraint() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CenterRectangle);
        click(&mut ed, [0.2, -0.1]); // snaps to origin point
        click(&mut ed, [30.0, 20.0]);
        assert!(ed.report.is_success());
        assert_eq!(ed.report.status, SolveStatus::UnderConstrained);
        assert_eq!(ed.report.dof, 2);
        // Dimension the bottom line (horizontal) and the right line (vertical).
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [0.0, -20.0]);
        let e = ed.dim_edit.clone().expect("inline editor opens");
        assert!(e.fresh);
        ed.dim_edit.as_mut().unwrap().buffer = "80".into();
        assert!(ed.apply_dim_edit());
        click(&mut ed, [40.0, 0.0]);
        ed.dim_edit.as_mut().unwrap().buffer = "50".into();
        assert!(ed.apply_dim_edit());
        assert_eq!(
            ed.report.status,
            SolveStatus::FullyConstrained,
            "{:?}",
            ed.status()
        );
        let dims = ed.sketch.dimensions();
        assert_eq!(dims, vec![("width".into(), 80.0), ("height".into(), 50.0)]);
        // Coordinates are converged to the solve tolerance (1e-9 mm), so the
        // area error is bounded by perimeter (260 mm) x tolerance.
        let tol = 260.0 * rcad_sketch::solve_tolerance(80.0);
        assert!((area(&ed) - 4000.0).abs() <= tol, "area {}", area(&ed));
        // Corner drag is blocked and names the blocking constraints.
        let before = ed.sketch.clone();
        ed.set_tool(SketchTool::Select);
        let corner = [40.0, 25.0];
        ed.press(corner, None, &view());
        ed.pointer_move([55.0, 31.0], None, &view());
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
        ed.release();
        assert_eq!(ed.sketch, before);
        assert!(!ed.can_redo());
    }

    #[test]
    fn same_two_points_dimensioned_twice_give_both_axes() {
        // Corner at (2, 2): |dx| = |dy|, so one point-to-point dimension cannot
        // place it; the second one between the same points takes the other axis.
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [2.0, 2.0]);
        click(&mut ed, [12.0, 8.0]);
        ed.set_tool(SketchTool::Dimension);
        for want in ["dx", "dy"] {
            click(&mut ed, [0.0, 0.0]); // origin
            click(&mut ed, [2.0, 2.0]); // corner
            let e = ed.dim_edit.clone().expect("inline editor opens");
            assert!(e.fresh);
            ed.dim_edit.as_mut().unwrap().buffer = "3".into();
            assert!(ed.apply_dim_edit(), "{want}: {:?}", ed.feedback);
            assert!(
                ed.sketch
                    .dimensions()
                    .iter()
                    .any(|(n, v)| n == want && *v == 3.0)
            );
        }
        click(&mut ed, [7.0, 3.0]); // bottom edge
        ed.dim_edit.as_mut().unwrap().buffer = "10".into();
        assert!(ed.apply_dim_edit());
        click(&mut ed, [13.0, 5.5]); // right edge (now x = 13, y 3..8)
        ed.dim_edit.as_mut().unwrap().buffer = "6".into();
        assert!(ed.apply_dim_edit());
        assert_eq!(
            ed.report.status,
            SolveStatus::FullyConstrained,
            "{:?}",
            ed.status()
        );
        // A third dimension of the pair is redundant (refused, sketch kept).
        let before = ed.sketch.clone();
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [3.0, 3.0]);
        if let Some(e) = ed.dim_edit.as_mut() {
            e.buffer = "3".into();
            ed.apply_dim_edit();
        }
        assert_eq!(ed.sketch.constraints.len(), before.constraints.len());
    }

    #[test]
    fn redundant_dimension_is_rejected_and_sketch_kept() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CenterRectangle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [30.0, 20.0]);
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [0.0, -20.0]); // width (bottom)
        ed.apply_dim_edit();
        let before = ed.sketch.clone();
        click(&mut ed, [0.0, 20.0]); // top line: also horizontal → redundant width
        assert_eq!(ed.sketch, before);
        assert!(ed.dim_edit.is_none());
        let fb = ed.feedback.clone().expect("feedback");
        assert!(
            fb.error && fb.text.contains("over-constrain"),
            "{}",
            fb.text
        );
    }

    #[test]
    fn invalid_and_conflicting_dimension_values_keep_last_valid_state() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [5.0, 0.0]);
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [0.0, 5.0]);
        assert_eq!(ed.sketch.dimensions(), vec![("diameter".into(), 10.0)]);
        let before = ed.sketch.clone();
        ed.dim_edit.as_mut().unwrap().buffer = "-3".into();
        assert!(!ed.apply_dim_edit());
        assert!(ed.dim_edit.is_some(), "editor stays open on invalid input");
        ed.dim_edit.as_mut().unwrap().buffer = "abc".into();
        assert!(!ed.apply_dim_edit());
        assert_eq!(ed.sketch, before);
        assert!(ed.escape());
        assert!(ed.dim_edit.is_none());
        assert_eq!(ed.sketch, before);
    }

    #[test]
    fn polyline_chamfer_plate_closes_and_has_expected_area() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        for p in [
            [0.0, 0.0],
            [60.0, 0.0],
            [60.0, 30.0],
            [50.0, 40.0],
            [0.0, 40.0],
            [0.3, 0.2],
        ] {
            click(&mut ed, p);
        }
        assert!(
            !ed.gesture_active(),
            "clicking the first point closes the chain"
        );
        assert!((area(&ed) - 2350.0).abs() < 1e-9);
        // Alignment snapping added H/V constraints on the axis-parallel edges.
        let hv = ed
            .sketch
            .constraints
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Constraint::Horizontal { .. } | Constraint::Vertical { .. }
                )
            })
            .count();
        assert_eq!(hv, 4);
        assert!(ed.report.is_success());
    }

    #[test]
    fn drag_free_vertex_is_one_undo_step_and_escape_restores() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        for p in [[10.0, 10.0], [30.0, 13.0], [20.0, 30.0], [10.0, 10.0]] {
            click(&mut ed, p);
        }
        ed.set_tool(SketchTool::Select);
        let undo_depth = ed.undo.len();
        let start = ed.sketch.clone();
        ed.press([20.0, 30.0], None, &view());
        for k in 1..=10 {
            ed.pointer_move([20.0 + k as f64, 30.0 + k as f64], None, &view());
        }
        // Escape mid-drag restores.
        assert!(ed.escape());
        assert_eq!(ed.sketch, start);
        // A completed drag is a single local undo step.
        ed.press([20.0, 30.0], None, &view());
        for k in 1..=10 {
            ed.pointer_move([20.0 + k as f64, 30.0], None, &view());
        }
        ed.release();
        assert_eq!(ed.undo.len(), undo_depth + 1);
        let p = ed.sketch.points.iter().find(|p| p.x == 30.0 && p.y == 30.0);
        assert!(p.is_some(), "vertex reached the snapped target");
        assert!(ed.undo());
        assert_eq!(ed.sketch, start);
    }

    #[test]
    fn circle_rim_drag_changes_radius_unless_dimensioned() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [5.0, 0.0]);
        ed.set_tool(SketchTool::Select);
        ed.press([5.0, 0.0], None, &view());
        ed.pointer_move([8.0, 0.0], None, &view());
        ed.release();
        let r = |ed: &SketchEditor| match ed.sketch.entities[0] {
            Entity::Circle { radius, .. } => radius,
            _ => panic!(),
        };
        assert!((r(&ed) - 8.0).abs() < 1e-9);
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [0.0, 8.0]);
        ed.apply_dim_edit();
        ed.set_tool(SketchTool::Select);
        ed.press([8.0, 0.0], None, &view());
        ed.pointer_move([12.0, 0.0], None, &view());
        ed.release();
        assert!((r(&ed) - 8.0).abs() < 1e-9);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
    }

    #[test]
    fn three_point_arc_closes_a_d_shape_and_takes_a_radius_dimension() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [-10.0, 0.0]);
        click(&mut ed, [10.0, 0.0]);
        assert!(ed.escape(), "end the chain");
        ed.set_tool(SketchTool::Arc);
        click(&mut ed, [10.2, 0.1]); // snaps to the line end
        click(&mut ed, [-9.9, -0.2]); // snaps to the other end
        assert!(ed.arc_ends().is_some());
        click(&mut ed, [0.0, 10.0]); // pass-through point
        assert!(
            ed.feedback.as_ref().is_some_and(|f| !f.error),
            "{:?}",
            ed.feedback
        );
        let state = ed.profile_state().expect("closed loop");
        // Half disc of radius 10: area = π·10²/2, derived independently.
        let want = std::f64::consts::PI * 50.0;
        let got: f64 = state
            .split("area ")
            .nth(1)
            .and_then(|t| t.trim_end_matches(" mm²").parse().ok())
            .expect("area in profile text");
        assert!((got - want).abs() < 1e-3, "{state}");
        // Radius dimension on the arc drives it.
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [0.0, 10.0]);
        let e = ed.dim_edit.clone().expect("radius editor");
        assert!(e.fresh);
        assert_eq!(ed.sketch.dimensions()[0].0, "radius");
        ed.dim_edit.as_mut().unwrap().buffer = "10".into();
        assert!(ed.apply_dim_edit());
        // Tangent needs a line + arc sharing an end; Equal needs two round curves.
        let line = ed.sketch.entities[0].id();
        let arc = ed.sketch.entities[1].id();
        ed.selection = vec![Item::Entity(line), Item::Entity(arc)];
        assert!(ed.tangent_disabled_reason().is_none());
        assert!(ed.equal_disabled_reason().is_some());
        // A collinear pass-through point is rejected without changing the sketch.
        let before = ed.sketch.clone();
        ed.set_tool(SketchTool::Arc);
        click(&mut ed, [20.0, 20.0]);
        click(&mut ed, [40.0, 20.0]);
        click(&mut ed, [30.0, 20.0]);
        assert_eq!(ed.sketch, before);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
    }

    #[test]
    fn stray_point_delete_duplicates_and_corner_snaps() {
        // A Line chain ended without a segment leaves no stray point.
        let mut ed = SketchEditor::new(Sketch::default());
        let n0 = ed.sketch.points.len();
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [30.0, 30.0]);
        assert_eq!(ed.sketch.points.len(), n0 + 1);
        assert!(ed.escape());
        assert_eq!(ed.sketch.points.len(), n0, "lone start point removed");
        assert!(!ed.can_undo());
        // Duplicate line between the same two points is refused.
        click(&mut ed, [0.0, 10.0]);
        click(&mut ed, [20.0, 10.0]);
        click(&mut ed, [0.0, 10.0]);
        assert_eq!(ed.sketch.entities.len(), 1);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
        ed.escape();
        // 2-point rectangle reuses a snapped point (the origin) as its corner.
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [0.2, -0.2]);
        click(&mut ed, [-20.0, -10.0]);
        let origin = ed.sketch.origin.unwrap();
        assert!(ed.sketch.point_used_by_entity(origin), "origin is a corner");
        // Deleting a dimension closes the inline editor for it.
        ed.set_tool(SketchTool::Dimension);
        click(&mut ed, [-10.0, -10.0]);
        assert!(ed.dim_edit.is_some());
        ed.delete_selection();
        assert!(ed.dim_edit.is_none());
        // Deleting an arc's end point deletes the arc.
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Arc);
        click(&mut ed, [10.0, 0.0]);
        click(&mut ed, [-10.0, 0.0]);
        click(&mut ed, [0.0, 10.0]);
        assert_eq!(ed.sketch.entities.len(), 1);
        let Entity::Arc { start, .. } = ed.sketch.entities[0] else {
            panic!("arc")
        };
        ed.selection = vec![Item::Point(start)];
        ed.delete_selection();
        assert!(ed.sketch.entities.is_empty());
    }

    #[test]
    fn midpoint_centres_a_circle_on_an_edge() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [40.0, 0.0]);
        ed.escape();
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [13.0, 9.0]);
        click(&mut ed, [16.0, 9.0]);
        let line = ed.sketch.entities[0].id();
        let Entity::Circle { center, .. } = ed.sketch.entities[1] else {
            panic!("circle")
        };
        ed.set_tool(SketchTool::Select);
        ed.selection = vec![Item::Entity(line)];
        assert!(ed.midpoint_disabled_reason().is_some());
        ed.selection = vec![Item::Entity(line), Item::Point(center)];
        assert!(ed.midpoint_disabled_reason().is_none());
        ed.constrain_midpoint();
        let c = ed.sketch.point_xy(center).unwrap();
        let tol = 40.0 * rcad_sketch::solve_tolerance(40.0);
        // Line endpoints are free too; the centre sits at their midpoint.
        let Entity::Line { a, b, .. } = ed.sketch.entities[0] else {
            panic!()
        };
        let (pa, pb) = (
            ed.sketch.point_xy(a).unwrap(),
            ed.sketch.point_xy(b).unwrap(),
        );
        assert!((c[0] - 0.5 * (pa[0] + pb[0])).abs() <= tol);
        assert!((c[1] - 0.5 * (pa[1] + pb[1])).abs() <= tol);
    }

    #[test]
    fn parallel_perpendicular_equal_on_line_and_construction() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [30.0, 7.0]);
        ed.escape();
        click(&mut ed, [5.0, 20.0]);
        click(&mut ed, [20.0, 31.0]);
        ed.escape();
        let (l1, l2) = (ed.sketch.entities[0].id(), ed.sketch.entities[1].id());
        let dir = |ed: &SketchEditor, l: EntityId| {
            let Some(Entity::Line { a, b, .. }) = ed.sketch.entity(l) else {
                panic!()
            };
            let (p, q) = (
                ed.sketch.point_xy(*a).unwrap(),
                ed.sketch.point_xy(*b).unwrap(),
            );
            let n = (q[0] - p[0]).hypot(q[1] - p[1]);
            ([(q[0] - p[0]) / n, (q[1] - p[1]) / n], n)
        };
        ed.selection = vec![Item::Entity(l1), Item::Entity(l2)];
        assert!(ed.two_lines_disabled_reason().is_none());
        ed.constrain_parallel(false);
        let ((u1, _), (u2, _)) = (dir(&ed, l1), dir(&ed, l2));
        assert!((u1[0] * u2[1] - u1[1] * u2[0]).abs() < 1e-9, "parallel");
        // Equal length on two lines.
        ed.selection = vec![Item::Entity(l1), Item::Entity(l2)];
        assert!(ed.equal_disabled_reason().is_none());
        ed.constrain_equal_radius();
        let ((_, n1), (_, n2)) = (dir(&ed, l1), dir(&ed, l2));
        assert!((n1 - n2).abs() < 1e-8, "equal length {n1} {n2}");
        // Parallel + Perpendicular on the same pair is refused with culprits.
        let before = ed.sketch.clone();
        ed.selection = vec![Item::Entity(l1), Item::Entity(l2)];
        ed.constrain_parallel(true);
        assert_eq!(ed.sketch, before);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
        // Point on line: a circle centre onto l1's infinite line.
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [40.0, 30.0]);
        click(&mut ed, [43.0, 30.0]);
        let Some(Entity::Circle { center, .. }) = ed.sketch.entities.last().cloned() else {
            panic!()
        };
        ed.set_tool(SketchTool::Select);
        ed.selection = vec![Item::Entity(l1), Item::Point(center)];
        assert!(ed.on_line_disabled_reason().is_none());
        ed.constrain_on_line();
        let Some(Entity::Line { a, b, .. }) = ed.sketch.entity(l1).cloned() else {
            panic!()
        };
        let (p, q, c) = (
            ed.sketch.point_xy(a).unwrap(),
            ed.sketch.point_xy(b).unwrap(),
            ed.sketch.point_xy(center).unwrap(),
        );
        let cross = (q[0] - p[0]) * (c[1] - p[1]) - (q[1] - p[1]) * (c[0] - p[0]);
        assert!(
            cross.abs() / (q[0] - p[0]).hypot(q[1] - p[1]) < 1e-8,
            "on line"
        );
        // Construction toggle (and back).
        ed.selection = vec![Item::Entity(l2)];
        ed.toggle_construction();
        assert!(ed.sketch.is_construction(l2));
        ed.toggle_construction();
        assert!(!ed.sketch.is_construction(l2));
    }

    #[test]
    fn angle_dimension_between_two_lines_edits_in_degrees() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [30.0, 0.0]);
        ed.escape();
        click(&mut ed, [0.0, 10.0]);
        click(&mut ed, [20.0, 25.0]);
        ed.escape();
        let (l1, l2) = (ed.sketch.entities[0].id(), ed.sketch.entities[1].id());
        let ang = |ed: &SketchEditor, l: EntityId| {
            let Some(Entity::Line { a, b, .. }) = ed.sketch.entity(l) else {
                panic!()
            };
            let (p, q) = (
                ed.sketch.point_xy(*a).unwrap(),
                ed.sketch.point_xy(*b).unwrap(),
            );
            (q[1] - p[1]).atan2(q[0] - p[0]).to_degrees()
        };
        // Selected in reverse order: the editor orients the angle to be positive.
        ed.selection = vec![Item::Entity(l2), Item::Entity(l1)];
        ed.constrain_angle();
        let idx = ed.sketch.constraints.len() - 1;
        assert!(matches!(ed.sketch.constraints[idx], Constraint::Angle { a, .. } if a == l1));
        let edit = ed.dim_edit.as_mut().expect("inline editor opened");
        edit.buffer = "60°".into();
        assert!(ed.apply_dim_edit());
        assert!(
            (ang(&ed, l2) - ang(&ed, l1) - 60.0).abs() < 1e-7,
            "{} {}",
            ang(&ed, l1),
            ang(&ed, l2)
        );
        // Parallel lines are refused with a reason.
        click(&mut ed, [0.0, 40.0]);
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [0.0, 40.0]);
        click(&mut ed, [30.0, 40.0]);
        ed.escape();
        let l3 = ed.sketch.entities.last().unwrap().id();
        let before = ed.sketch.clone();
        ed.selection = vec![Item::Entity(l1), Item::Entity(l3)];
        ed.constrain_angle();
        assert_eq!(ed.sketch, before);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
    }

    fn line_ends(ed: &SketchEditor, l: EntityId) -> ([f64; 2], [f64; 2]) {
        let Some(Entity::Line { a, b, .. }) = ed.sketch.entity(l) else {
            panic!("{l:?} is not a line")
        };
        (
            ed.sketch.point_xy(*a).unwrap(),
            ed.sketch.point_xy(*b).unwrap(),
        )
    }

    #[test]
    fn extend_free_ends_to_the_nearest_curve() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [40.0, 20.0]);
        let corner_line = ed.sketch.entities[0].id();
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [5.0, 10.0]);
        click(&mut ed, [15.0, 10.0]);
        ed.escape();
        let l = ed.sketch.entities.last().unwrap().id();
        let n_points = ed.sketch.points.len();
        ed.set_tool(SketchTool::Extend);
        click(&mut ed, [13.0, 10.0]);
        let (p, q) = line_ends(&ed, l);
        assert!(
            (p[0] - 5.0).abs() < 1e-9 && (q[0] - 40.0).abs() < 1e-9,
            "{p:?} {q:?} {:?}",
            ed.feedback
        );
        click(&mut ed, [8.0, 10.0]);
        let (p, q) = line_ends(&ed, l);
        assert!(
            p[0].abs() < 1e-9 && (q[0] - 40.0).abs() < 1e-9,
            "{p:?} {q:?}"
        );
        assert_eq!(
            ed.sketch.points.len(),
            n_points,
            "old ends replaced, not added"
        );
        // A connected end is refused, sketch unchanged.
        let before = ed.sketch.clone();
        let (p, q) = line_ends(&ed, corner_line);
        let near = [p[0] + 0.8 * (q[0] - p[0]), p[1] + 0.8 * (q[1] - p[1])];
        click(&mut ed, near);
        assert_eq!(ed.sketch, before);
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
    }

    #[test]
    fn trim_circle_to_arc_then_trim_the_arc() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [10.0, 0.0]);
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [-20.0, 0.5]);
        click(&mut ed, [20.0, 0.5]);
        ed.escape();
        click(&mut ed, [0.5, -20.0]);
        click(&mut ed, [0.5, 20.0]);
        ed.escape();
        ed.set_tool(SketchTool::Trim);
        // Click the top-right quarter: removed between the crossings either side.
        click(&mut ed, [7.0, 7.0]);
        let arc = ed
            .sketch
            .entities
            .iter()
            .find_map(|e| match e {
                Entity::Arc { id, .. } => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("circle became an arc: {:?}", ed.feedback));
        assert!(
            !ed.sketch
                .entities
                .iter()
                .any(|e| matches!(e, Entity::Circle { .. }))
        );
        let g = ed.sketch.arc_geometry(arc).unwrap();
        // Remaining arc: three quarters (minus the small offsets of the lines).
        assert!((g.radius - 10.0).abs() < 1e-9);
        assert!(
            g.sweep > 1.4 * std::f64::consts::PI && g.sweep < 1.6 * std::f64::consts::PI,
            "{}",
            g.sweep
        );
        // Trim the bottom-left quarter of the arc: it splits into two arcs.
        click(&mut ed, [-7.0, -7.0]);
        let arcs: Vec<_> = ed
            .sketch
            .entities
            .iter()
            .filter_map(|e| match e {
                Entity::Arc { id, .. } => ed.sketch.arc_geometry(*id),
                _ => None,
            })
            .collect();
        assert_eq!(arcs.len(), 2, "{:?}", ed.feedback);
        let total: f64 = arcs.iter().map(|g| g.sweep).sum();
        assert!(
            total > 0.9 * std::f64::consts::PI && total < 1.1 * std::f64::consts::PI,
            "{total}"
        );
        assert!(
            ed.sketch
                .constraints
                .iter()
                .any(|c| matches!(c, Constraint::EqualRadius { .. }))
        );
    }

    #[test]
    fn trim_overhangs_split_and_delete() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [40.0, 20.0]);
        let rect: Vec<EntityId> = ed.sketch.entities.iter().map(|e| e.id()).collect();
        // A vertical line crossing the rectangle (overhanging 5 mm both ends).
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [20.0, -5.0]);
        click(&mut ed, [20.0, 25.0]);
        ed.escape();
        let v = ed.sketch.entities.last().unwrap().id();
        let n_points = ed.sketch.points.len();
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [20.0, 22.0]);
        let (p, q) = line_ends(&ed, v);
        assert!(
            (p[1] + 5.0).abs() < 1e-9 && (q[1] - 20.0).abs() < 1e-9,
            "{p:?} {q:?}"
        );
        click(&mut ed, [20.0, -2.5]);
        let (p, q) = line_ends(&ed, v);
        assert!(
            p[1].abs() < 1e-9 && (q[1] - 20.0).abs() < 1e-9,
            "{p:?} {q:?}"
        );
        // Two new end points, both old ones (now unused) removed.
        assert_eq!(ed.sketch.points.len(), n_points);
        // Split the bottom edge between x = 0 and the cross line... the bottom
        // edge is cut only at x = 20 (the trimmed line's end lies on it).
        let bottom = *rect
            .iter()
            .find(|l| {
                let (p, q) = line_ends(&ed, **l);
                p[1].abs() < 1e-9 && q[1].abs() < 1e-9
            })
            .unwrap();
        let entities = ed.sketch.entities.len();
        click(&mut ed, [30.0, 0.0]);
        assert_eq!(
            ed.sketch.entities.len(),
            entities,
            "overhang trim keeps the count"
        );
        let (p, q) = line_ends(&ed, bottom);
        let xs = [p[0].min(q[0]), p[0].max(q[0])];
        assert!(xs[0].abs() < 1e-9 && (xs[1] - 20.0).abs() < 1e-9, "{xs:?}");
        // Splitting: a horizontal line crossed by two verticals, trim the middle.
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        for (a, b) in [
            ([0.0, 0.0], [30.0, 0.0]),
            ([10.0, -5.0], [10.0, 5.0]),
            ([20.0, -5.0], [20.0, 5.0]),
        ] {
            click(&mut ed, a);
            click(&mut ed, b);
            ed.escape();
        }
        let h = ed.sketch.entities[0].id();
        let count_h = |ed: &SketchEditor| {
            ed.sketch
                .constraints
                .iter()
                .filter(|c| matches!(c, Constraint::Horizontal { .. }))
                .count()
        };
        // The line tool made it Horizontal (alignment snap).
        let h_before = count_h(&ed);
        assert!(h_before >= 1);
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [15.0, 0.0]);
        assert_eq!(ed.sketch.entities.len(), 4, "split adds a line");
        assert_eq!(
            count_h(&ed),
            2 * h_before,
            "Horizontal copied to the new piece"
        );
        let (p, q) = line_ends(&ed, h);
        assert!((p[0].max(q[0]) - 10.0).abs() < 1e-9);
        // A line with no intersections is deleted.
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [0.0, 30.0]);
        click(&mut ed, [10.0, 30.0]);
        ed.escape();
        let n = ed.sketch.entities.len();
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [5.0, 30.0]);
        assert_eq!(ed.sketch.entities.len(), n - 1);
    }

    #[test]
    fn lone_start_point_never_costs_or_breaks_undo() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [5.0, 5.0]);
        click(&mut ed, [25.0, 15.0]);
        let rect = ed.sketch.clone();
        // Ctrl+Z with only a pending start point cancels just that point.
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [40.0, 40.0]);
        assert!(ed.undo());
        assert_eq!(ed.sketch, rect, "rectangle kept, start point gone");
        // An edit made while a start point is pending survives Escape.
        click(&mut ed, [40.0, 40.0]);
        let line = ed.sketch.entities[0].id();
        ed.selection = vec![Item::Entity(line)];
        ed.toggle_construction();
        assert!(ed.sketch.is_construction(line));
        assert!(ed.escape());
        assert!(ed.sketch.is_construction(line), "construction toggle kept");
        assert_eq!(ed.sketch.points.len(), rect.points.len(), "no stray point");
        // Undoing a chain's first segment leaves no stray start point.
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [40.0, 40.0]);
        click(&mut ed, [50.0, 47.0]);
        assert!(ed.escape());
        let n_with = ed.sketch.points.len();
        assert!(ed.undo());
        assert_eq!(ed.sketch.points.len(), n_with - 2);
    }

    #[test]
    fn delete_keeps_origin_and_escape_cascade() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [5.0, 5.0]);
        assert!(ed.gesture_active());
        assert!(ed.escape(), "first Esc cancels the gesture");
        assert!(!ed.gesture_active());
        assert_eq!(ed.tool, SketchTool::CornerRectangle);
        assert!(ed.escape(), "second Esc leaves the tool");
        assert_eq!(ed.tool, SketchTool::Select);
        assert!(!ed.escape(), "nothing left to cancel");
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [5.0, 5.0]);
        click(&mut ed, [15.0, 10.0]);
        assert_eq!(ed.sketch.entities.len(), 4);
        ed.set_tool(SketchTool::Select);
        let origin = ed.sketch.origin.unwrap();
        ed.selection = ed
            .sketch
            .entities
            .iter()
            .map(|e| Item::Entity(e.id()))
            .chain([Item::Point(origin)])
            .collect();
        ed.delete_selection();
        assert!(ed.sketch.entities.is_empty());
        assert_eq!(ed.sketch.points.len(), 1);
        assert_eq!(ed.sketch.origin, Some(origin));
    }

    /// Trimmed ends share the cutting curve's point, so the
    /// left half of a rectangle cut by a vertical line closes into a profile.
    #[test]
    fn trimmed_ends_attach_and_the_profile_closes() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [40.0, 20.0]);
        let right = *ed
            .sketch
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Line { .. }))
            .map(|e| e.id())
            .collect::<Vec<_>>()
            .iter()
            .find(|l| {
                let (p, q) = line_ends(&ed, **l);
                (p[0] - 40.0).abs() < 1e-9 && (q[0] - 40.0).abs() < 1e-9
            })
            .unwrap();
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [20.0, -5.0]);
        click(&mut ed, [20.0, 25.0]);
        ed.escape();
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [20.0, 22.0]);
        click(&mut ed, [20.0, -2.5]);
        // The trimmed ends are held on the top/bottom edges.
        let on_line = ed
            .sketch
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::PointOnLine { .. }))
            .count();
        assert_eq!(on_line, 2, "{:?}", ed.sketch.constraints);
        ed.sketch.remove_entity(right);
        click(&mut ed, [30.0, 20.0]);
        click(&mut ed, [30.0, 0.0]);
        let prof = ed.sketch.profile();
        let Ok(rcad_sketch::Profile::Polygon(poly)) = &prof else {
            panic!("{prof:?} / {:?}", ed.feedback);
        };
        let area = poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f64>()
            / 2.0;
        assert!((area - 400.0).abs() < 1e-6, "{area}");
        assert!(ed.sketch.solve().is_success());
    }

    /// A split construction line stays construction.
    #[test]
    fn trim_keeps_the_construction_flag() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        for (a, b) in [
            ([0.0, 0.0], [30.0, 0.0]),
            ([10.0, -5.0], [10.0, 5.0]),
            ([20.0, -5.0], [20.0, 5.0]),
        ] {
            click(&mut ed, a);
            click(&mut ed, b);
            ed.escape();
        }
        let h = ed.sketch.entities[0].id();
        ed.sketch.set_construction(h, true);
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [15.0, 0.0]);
        let horizontal: Vec<EntityId> = ed
            .sketch
            .entities
            .iter()
            .filter(|e| {
                let (p, q) = line_ends(&ed, e.id());
                p[1].abs() < 1e-9 && q[1].abs() < 1e-9
            })
            .map(|e| e.id())
            .collect();
        assert_eq!(horizontal.len(), 2, "{:?}", ed.feedback);
        assert!(horizontal.iter().all(|l| ed.sketch.is_construction(*l)));
    }

    /// A line through an arc's start point sees the arc,
    /// whatever the start angle (rounding used to wrap it to ~2π).
    #[test]
    fn arc_start_point_crossings_are_found_at_every_angle() {
        for k in 0..360 {
            let th = (k as f64).to_radians();
            let mut sk = Sketch::default();
            let at = |a: f64, r: f64| [r * a.cos(), r * a.sin()];
            let s0 = sk.add_point(at(th, 10.0)[0], at(th, 10.0)[1]);
            let e0 = sk.add_point(at(th + 1.5, 10.0)[0], at(th + 1.5, 10.0)[1]);
            sk.add_arc_3pt(s0, e0, at(th + 0.75, 10.0)).unwrap();
            // Radial line through the arc's start point (not sharing it).
            let a = sk.add_point(at(th, 5.0)[0], at(th, 5.0)[1]);
            let b = sk.add_point(at(th, 15.0)[0], at(th, 15.0)[1]);
            let l = sk.add_line(a, b);
            let ed = SketchEditor::new(sk);
            let hits = ed.sketch.line_hits(l, at(th, 5.0), at(th, 15.0), false);
            assert!(
                hits.iter().any(|t| (t - 0.5).abs() < 1e-9),
                "{k} deg: {hits:?}"
            );
        }
    }

    /// A Tangent at the end of the line that is kept survives.
    #[test]
    fn trim_keeps_a_tangent_at_the_kept_end() {
        let mut sk = Sketch::default();
        let p0 = sk.add_point(0.0, 0.0);
        let p1 = sk.add_point(20.0, 0.0);
        let line = sk.add_line(p0, p1);
        // Arc tangent to the line at (0, 0): centre (0, 10).
        let p2 = sk.add_point(-10.0, 10.0);
        let arc = sk
            .add_arc_3pt(p2, p0, [-(50f64.sqrt()), 10.0 - 50f64.sqrt()])
            .unwrap();
        sk.constraints.push(Constraint::Tangent { line, arc });
        let c0 = sk.add_point(15.0, -5.0);
        let c1 = sk.add_point(15.0, 5.0);
        sk.add_line(c0, c1);
        let mut ed = SketchEditor::new(sk);
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [17.5, 0.0]);
        let (p, q) = line_ends(&ed, line);
        assert!(
            (p[0].max(q[0]) - 15.0).abs() < 1e-9,
            "{p:?} {q:?} {:?}",
            ed.feedback
        );
        assert!(
            ed.sketch
                .constraints
                .contains(&Constraint::Tangent { line, arc }),
            "{:?}",
            ed.feedback
        );
    }

    /// Extend joins the new end to the curve it reaches: an existing point
    /// there is shared, otherwise the end is held on the line.
    #[test]
    fn extend_attaches_the_new_end() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Line);
        for (a, b) in [
            ([0.0, 0.0], [10.0, 0.0]),
            ([20.0, -5.0], [20.0, 5.0]),
            ([0.0, 10.0], [10.0, 10.0]),
            ([30.0, 10.0], [30.0, 20.0]),
        ] {
            click(&mut ed, a);
            click(&mut ed, b);
            ed.escape();
        }
        let ids: Vec<EntityId> = ed.sketch.entities.iter().map(|e| e.id()).collect();
        ed.set_tool(SketchTool::Extend);
        // y = 0 line reaches the interior of x = 20: Point-on-line.
        click(&mut ed, [8.0, 0.0]);
        let (_, q) = line_ends(&ed, ids[0]);
        assert!((q[0] - 20.0).abs() < 1e-9, "{q:?} {:?}", ed.feedback);
        let Some(Entity::Line { b: end0, .. }) = ed.sketch.entity(ids[0]).cloned() else {
            panic!()
        };
        assert!(ed.sketch.constraints.contains(&Constraint::PointOnLine {
            point: end0,
            line: ids[1]
        }));
        // y = 10 line reaches the end point (30, 10) of x = 30: shared point.
        click(&mut ed, [8.0, 10.0]);
        let Some(Entity::Line { b: end2, .. }) = ed.sketch.entity(ids[2]).cloned() else {
            panic!()
        };
        let Some(Entity::Line { a: start3, .. }) = ed.sketch.entity(ids[3]).cloned() else {
            panic!()
        };
        assert_eq!(end2, start3, "{:?}", ed.feedback);
        assert!(ed.sketch.solve().is_success());
    }

    /// A construction circle trimmed to an arc stays construction.
    #[test]
    fn trim_circle_keeps_construction() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::Circle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [10.0, 0.0]);
        let circle = ed.sketch.entities[0].id();
        ed.sketch.set_construction(circle, true);
        ed.set_tool(SketchTool::Line);
        click(&mut ed, [-20.0, 0.5]);
        click(&mut ed, [20.0, 0.5]);
        ed.escape();
        click(&mut ed, [0.5, -20.0]);
        click(&mut ed, [0.5, 20.0]);
        ed.escape();
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [7.0, 7.0]);
        let arc = ed
            .sketch
            .entities
            .iter()
            .find(|e| matches!(e, Entity::Arc { .. }))
            .map(|e| e.id())
            .unwrap_or_else(|| panic!("{:?}", ed.feedback));
        assert!(ed.sketch.is_construction(arc));
    }

    /// Rectangle 40x20 (lines bottom, right, top, left) and a circle Ø12
    /// centred on the right edge at (40, 10). Returns (editor, right edge, circle).
    fn rect_with_circle_on_edge() -> (SketchEditor, EntityId, EntityId) {
        let mut sk = Sketch::default();
        let p: Vec<PointId> = [[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]]
            .iter()
            .map(|q| sk.add_point(q[0], q[1]))
            .collect();
        for i in 0..4 {
            sk.add_line(p[i], p[(i + 1) % 4]);
        }
        let right = sk.entities[1].id();
        let c = sk.add_point(40.0, 10.0);
        let circle = sk.add_circle(c, 6.0);
        (SketchEditor::new(sk), right, circle)
    }

    fn on_circle_count(ed: &SketchEditor) -> usize {
        ed.sketch
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::PointOnCircle { .. }))
            .count()
    }

    /// Circles/arcs: trimmed ends on a circle are held
    /// on it (PointOnCircle), stay there after the circle changes, and when
    /// the circle is trimmed at the same places the ends are shared, so the
    /// line/arc profile closes; the arc keeps the circle's id (#15).
    #[test]
    fn trimmed_ends_on_a_circle_are_held_and_the_profile_closes() {
        let (mut ed, right, circle) = rect_with_circle_on_edge();
        ed.set_tool(SketchTool::Trim);
        // Remove the right edge's part inside the circle (y 4..16).
        click(&mut ed, [40.0, 13.0]);
        assert_eq!(on_circle_count(&ed), 2, "{:?}", ed.feedback);
        let fb = ed.feedback.clone().unwrap();
        assert!(!fb.error && !fb.text.contains("not held"), "{fb:?}");
        let (p, q) = line_ends(&ed, right);
        assert!(
            (q[1] - 4.0).abs() < 1e-9 && p[1].abs() < 1e-9,
            "{p:?} {q:?}"
        );
        assert!(ed.sketch.solve().is_success());
        // The held ends follow a radius change (solver), staying on the circle
        // and on the edge's line.
        let mut moved = ed.sketch.clone();
        let r = moved.drag_radius(circle, 7.0);
        assert!(r.report.is_success(), "{r:?}");
        for c in &moved.constraints {
            if let Constraint::PointOnCircle { point, .. } = c {
                let x = moved.point_xy(*point).unwrap();
                assert!(
                    ((x[0] - 40.0).hypot(x[1] - 10.0) - 7.0).abs() < 1e-8,
                    "{x:?}"
                );
            }
        }
        // Trim the circle's part inside the rectangle: the arc's ends are the
        // edge pieces' ends (shared), the implied on-circle holds go.
        click(&mut ed, [34.2, 10.0]);
        let arc = ed.sketch.entity(circle).cloned();
        assert!(
            matches!(arc, Some(Entity::Arc { .. })),
            "circle id kept by the arc: {arc:?} / {:?}",
            ed.feedback
        );
        assert_eq!(on_circle_count(&ed), 0, "{:?}", ed.sketch.constraints);
        assert!(ed.sketch.validate().is_ok());
        let prof = ed.sketch.profile();
        let Ok(rcad_sketch::Profile::Loop(segs)) = &prof else {
            panic!("{prof:?} / {:?}", ed.feedback);
        };
        let want = 800.0 + 18.0 * std::f64::consts::PI;
        assert!((loop_area(segs) - want).abs() < 1e-6, "{}", loop_area(segs));
        assert!(ed.sketch.solve().is_success());
    }

    /// Extend onto a circle: the new end is held on it and follows it.
    #[test]
    fn extended_end_on_a_circle_is_held() {
        let mut sk = Sketch::default();
        let a = sk.add_point(0.0, 0.0);
        let b = sk.add_point(10.0, 0.0);
        let line = sk.add_line(a, b);
        let c = sk.add_point(30.0, 0.0);
        let circle = sk.add_circle(c, 5.0);
        let mut ed = SketchEditor::new(sk);
        ed.set_tool(SketchTool::Extend);
        click(&mut ed, [8.0, 0.0]);
        let (_, q) = line_ends(&ed, line);
        assert!((q[0] - 25.0).abs() < 1e-9, "{q:?} / {:?}", ed.feedback);
        assert_eq!(on_circle_count(&ed), 1, "{:?}", ed.sketch.constraints);
        let r = ed.sketch.drag_radius(circle, 8.0);
        assert!(r.report.is_success(), "{r:?}");
        // The (unfixed) centre may move too; the end stays on the new circle.
        let (_, q) = line_ends(&ed, line);
        let cc = ed.sketch.point_xy(c).unwrap();
        let Some(Entity::Circle { radius, .. }) = ed.sketch.entity(circle) else {
            panic!()
        };
        assert!((radius - 8.0).abs() < 1e-9, "{radius}");
        assert!(
            ((q[0] - cc[0]).hypot(q[1] - cc[1]) - 8.0).abs() < 1e-8,
            "{q:?} {cc:?}"
        );
    }

    /// Splitting a line removes its full-length dimension
    /// (it would span two separate lines) and says so.
    #[test]
    fn split_line_drops_its_length_dimension_and_says_so() {
        let mut sk = Sketch::default();
        let a = sk.add_point(0.0, 0.0);
        let b = sk.add_point(40.0, 0.0);
        let l = sk.add_line(a, b);
        for x in [15.0, 25.0] {
            let p = sk.add_point(x, -5.0);
            let q = sk.add_point(x, 5.0);
            sk.add_line(p, q);
        }
        sk.constraints.push(Constraint::Distance {
            a,
            b,
            dim: rcad_sketch::Dimension::new("len", 40.0),
        });
        let mut ed = SketchEditor::new(sk);
        ed.set_tool(SketchTool::Trim);
        click(&mut ed, [20.0, 0.0]);
        let lines = ed
            .sketch
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Line { .. }))
            .count();
        assert_eq!(lines, 4, "split into two pieces: {:?}", ed.feedback);
        let (p, q) = line_ends(&ed, l);
        assert!(p[0].abs() < 1e-9 && (q[0] - 15.0).abs() < 1e-9);
        assert!(
            !ed.sketch
                .constraints
                .iter()
                .any(|c| matches!(c, Constraint::Distance { .. })),
            "{:?}",
            ed.sketch.constraints
        );
        let fb = ed.feedback.clone().unwrap();
        assert!(
            fb.text.contains("length dimension 'len' was removed"),
            "{fb:?}"
        );
        // Trimming one end (no split) keeps a dimension between the kept points.
        assert!(ed.sketch.solve().is_success());
    }

    /// Offset tool: a refused distance keeps the entry open with the reason
    /// and the sketch unchanged; a valid one adds the copy as one undo step.
    #[test]
    fn offset_tool_refuses_collapse_and_applies_as_one_undo_step() {
        let mut ed = SketchEditor::new(Sketch::default());
        ed.set_tool(SketchTool::CornerRectangle);
        click(&mut ed, [0.0, 0.0]);
        click(&mut ed, [40.0, 20.0]);
        let before = ed.sketch.clone();
        ed.set_tool(SketchTool::Offset);
        click(&mut ed, [20.0, 0.0]);
        assert!(ed.offset_edit.is_some(), "{:?}", ed.feedback);
        ed.offset_edit.as_mut().unwrap().buffer = "-10".into();
        assert!(matches!(ed.offset_preview(), Some(Err(_))));
        assert!(!ed.apply_offset());
        assert!(ed.feedback.as_ref().is_some_and(|f| f.error));
        assert!(ed.offset_edit.is_some(), "entry stays open");
        assert_eq!(ed.sketch, before);
        ed.offset_edit.as_mut().unwrap().buffer = "abc".into();
        assert!(!ed.apply_offset());
        ed.offset_edit.as_mut().unwrap().buffer = "1.5".into();
        assert!(ed.apply_offset());
        assert!(ed.offset_edit.is_none());
        assert_eq!(ed.sketch.entities.len(), 8);
        let text = ed.feedback.clone().unwrap().text;
        assert!(
            text.contains("outward") && text.contains("'offset'"),
            "{text}"
        );
        assert!(ed.undo());
        assert_eq!(ed.sketch.entities.len(), 4);
        // Escape closes an open entry.
        click(&mut ed, [20.0, 0.0]);
        assert!(ed.escape() && ed.offset_edit.is_none());
    }

    #[test]
    fn dimension_input_units_expressions_and_new_parameters() {
        use rcad_document::params::Quantity;
        let mut env = std::collections::HashMap::new();
        env.insert(
            "w".to_string(),
            Quantity {
                value: 40.0,
                dim: Some(1),
            },
        );
        env.insert(
            "a".to_string(),
            Quantity {
                value: 30.0,
                dim: Some(0),
            },
        );
        let p = |s: &str, angle: bool| parse_dim_input(s, angle, &env);
        assert_eq!(p("12.5", false), Ok(DimInput::Value(12.5)));
        assert_eq!(p("12.5 mm", false), Ok(DimInput::Value(12.5)));
        assert_eq!(p("1 in", false), Ok(DimInput::Value(25.4)));
        assert_eq!(p("2cm", false), Ok(DimInput::Value(20.0)));
        assert_eq!(
            p("w/2", false),
            Ok(DimInput::Bound {
                value: 20.0,
                expr: "w/2".into()
            })
        );
        assert_eq!(
            p("d = w + 5 mm", false),
            Ok(DimInput::NewParameter {
                value: 45.0,
                name: "d".into(),
                expr: "w + 5 mm".into()
            })
        );
        assert_eq!(p("30deg", true), Ok(DimInput::Value(30.0)));
        assert_eq!(p("45°", true), Ok(DimInput::Value(45.0)));
        match p("0.5 rad", true) {
            Ok(DimInput::Value(v)) => {
                assert!((v - 0.5 * 180.0 / std::f64::consts::PI).abs() < 1e-12)
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(p("a * 2", true), Ok(DimInput::Bound { value, .. }) if value == 60.0));
        for (bad, angle) in [
            ("", false),
            ("q/2", false),
            ("10 - 30", false),
            ("w = 3", false),
            ("2w = 3", false),
            ("30 mm", true),
            ("x = ", false),
        ] {
            assert!(p(bad, angle).is_err(), "{bad} accepted");
        }
    }
}
