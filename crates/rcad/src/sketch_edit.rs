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
/// Smallest on-screen spacing (px) of the sketch snapping step (a 1-2-5
/// step in mm, finer than the drawn grid).
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
}

impl SketchTool {
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
            SketchTool::Offset => "Offset",
            SketchTool::Fillet => "Fillet",
            SketchTool::Text => "Text",
            SketchTool::Slot => "Slot",
            SketchTool::Polygon => "Polygon",
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
            SketchTool::Offset => "O",
            SketchTool::Fillet => "toolbar",
            SketchTool::Text => "toolbar",
            SketchTool::Slot => "toolbar",
            SketchTool::Polygon => "toolbar",
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
                "Click the part of a line to remove: it is cut at the nearest intersections with other \
                 curves (a line with none is deleted) · lines only"
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
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to segment `a`-`b`.
pub fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 <= 0.0 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Does constraint `c` reference point `p` directly?
pub(crate) fn constraint_mentions(c: &Constraint, p: PointId) -> bool {
    match c {
        Constraint::FixedPoint { p: q, .. } => *q == p,
        Constraint::HorizontalDistance { a, b, .. }
        | Constraint::VerticalDistance { a, b, .. }
        | Constraint::Distance { a, b, .. }
        | Constraint::Coincident { a, b } => *a == p || *b == p,
        Constraint::SymmetricAboutPoint { a, b, center } => *a == p || *b == p || *center == p,
        Constraint::PointOnLine { point, .. } | Constraint::PointOnCircle { point, .. } => {
            *point == p
        }
        _ => false,
    }
}

/// How a trim/extend end was joined to the curve it was cut at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CutEnd {
    /// An existing point already lies there: its id is shared, so profiles close.
    Shared,
    /// New point held on the cutting curve(s): `PointOnLine` for a line,
    /// `PointOnCircle` for a circle or arc (both at a line/circle crossing).
    Held,
    /// New point on no other curve (nothing to hold it on).
    Free,
}

/// End point for a trim/extend cut at `x`. Reuses an existing point there (other
/// than `own`, the edited curve's points), else adds one and keeps it on the
/// cutting curves (not in `skip`) through `x`: a line whose segment holds `x`
/// (`PointOnLine`) and/or a circle or arc through `x` (`PointOnCircle`). When
/// the cutting curve is later trimmed at the same place, the shared point makes
/// the profile close (see `cut_point`'s reuse rule).
pub(crate) fn cut_point(
    sk: &mut Sketch,
    x: [f64; 2],
    own: &[PointId],
    skip: &[EntityId],
) -> (PointId, CutEnd) {
    let tol = rcad_sketch::solve_tolerance(sk.scale());
    let used = |sk: &Sketch, p: PointId| {
        sk.origin == Some(p)
            || sk
                .entities
                .iter()
                .any(|e| !skip.contains(&e.id()) && e.points().contains(&p))
    };
    if let Some(p) = sk
        .points
        .iter()
        .filter(|q| !own.contains(&q.id) && dist([q.x, q.y], x) <= tol)
        .map(|q| q.id)
        .find(|&q| used(sk, q))
    {
        return (p, CutEnd::Shared);
    }
    let line = sk.entities.iter().find_map(|e| match e {
        Entity::Line { id, a, b } if !skip.contains(id) => {
            let (pa, pb) = (sk.point_xy(*a)?, sk.point_xy(*b)?);
            (seg_dist(x, pa, pb) <= tol && dist(x, pa) > tol && dist(x, pb) > tol).then_some(*id)
        }
        _ => None,
    });
    let round = sk.entities.iter().find_map(|e| match e {
        Entity::Circle { id, center, radius } if !skip.contains(id) => {
            let c = sk.point_xy(*center)?;
            ((dist(x, c) - radius).abs() <= tol).then_some(*id)
        }
        Entity::Arc { id, .. } if !skip.contains(id) => {
            let g = sk.arc_geometry(*id)?;
            ((dist(x, g.center) - g.radius).abs() <= tol && arc_distance(&g, x) <= tol)
                .then_some(*id)
        }
        _ => None,
    });
    let n = sk.add_point(x[0], x[1]);
    if let Some(line) = line {
        sk.constraints
            .push(Constraint::PointOnLine { point: n, line });
    }
    if let Some(circle) = round {
        sk.constraints
            .push(Constraint::PointOnCircle { point: n, circle });
    }
    if line.is_some() || round.is_some() {
        (n, CutEnd::Held)
    } else {
        (n, CutEnd::Free)
    }
}

/// Drop `PointOnLine` / `PointOnCircle` constraints whose point has become an
/// end (or the centre) of that curve: implied, and refused by validation.
fn drop_implied_on_line(sk: &mut Sketch) {
    let ends = |sk: &Sketch, l: EntityId| sk.entity(l).map(|x| x.points()).unwrap_or_default();
    let keep: Vec<bool> = sk
        .constraints
        .iter()
        .map(|c| match c {
            Constraint::PointOnLine { point, line } => !ends(sk, *line).contains(point),
            Constraint::PointOnCircle { point, circle } => !ends(sk, *circle).contains(point),
            _ => true,
        })
        .collect();
    let mut it = keep.into_iter();
    sk.constraints.retain(|_| it.next().unwrap_or(true));
}

/// After `e` was shortened/split into `e` + `piece`: a Tangent naming `e` stays
/// with whichever piece still ends at the tangent point, else it is dropped.
/// Returns the number dropped.
fn retarget_tangents(sk: &mut Sketch, e: EntityId, piece: Option<EntityId>) -> usize {
    // Endpoints only (an arc's centre is not where tangency holds).
    let ends = |sk: &Sketch, id: EntityId| match sk.entity(id) {
        Some(Entity::Line { a, b, .. }) => vec![*a, *b],
        Some(Entity::Arc { start, end, .. }) => vec![*start, *end],
        _ => vec![],
    };
    let shares = |sk: &Sketch, u: EntityId, v: EntityId| {
        let pv = ends(sk, v);
        ends(sk, u).iter().any(|p| pv.contains(p))
    };
    let before = sk.constraints.len();
    let old = std::mem::take(&mut sk.constraints);
    let mut keep = Vec::with_capacity(old.len());
    for c in old {
        // (first, second, rebuild with one of them replaced) for both
        // line–arc and arc–arc tangency.
        let (x, y) = match c {
            Constraint::Tangent { line, arc } => (line, arc),
            Constraint::TangentArcs { a, b } => (a, b),
            _ => {
                keep.push(c);
                continue;
            }
        };
        if x != e && y != e {
            keep.push(c);
            continue;
        }
        let other = if x == e { y } else { x };
        let swap = |id: EntityId| {
            let (nx, ny) = if x == e { (id, y) } else { (x, id) };
            match c {
                Constraint::Tangent { .. } => Constraint::Tangent { line: nx, arc: ny },
                _ => Constraint::TangentArcs { a: nx, b: ny },
            }
        };
        if shares(sk, e, other) {
            keep.push(c);
        } else if let Some(pc) = piece.filter(|pc| shares(sk, *pc, other)) {
            keep.push(swap(pc));
        }
    }
    sk.constraints = keep;
    before - sk.constraints.len()
}

/// Status line suffix for ends that could not be attached.
fn free_note(free: usize) -> String {
    if free == 0 {
        String::new()
    } else {
        format!(" {free} new end(s) lie on no other curve and are not held.")
    }
}

/// Distance from `p` to an arc (to the nearest end outside its angular span).
pub fn arc_distance(g: &rcad_sketch::ArcGeometry, p: [f64; 2]) -> f64 {
    let ang = (p[1] - g.center[1]).atan2(p[0] - g.center[0]);
    let rel = (ang - g.start_angle).rem_euclid(std::f64::consts::TAU);
    if rel <= g.sweep {
        return (dist(p, g.center) - g.radius).abs();
    }
    let at = |a: f64| {
        [
            g.center[0] + g.radius * a.cos(),
            g.center[1] + g.radius * a.sin(),
        ]
    };
    dist(p, at(g.start_angle)).min(dist(p, at(g.start_angle + g.sweep)))
}

/// Exact area of a closed line/arc loop: shoelace over the chords plus the
/// signed circular segments of the arcs.
pub fn loop_area(segs: &[rcad_sketch::LoopSegment]) -> f64 {
    use rcad_sketch::LoopSegment;
    let n = segs.len();
    let mut area = 0.0;
    for i in 0..n {
        let a = segs[i].start();
        let b = segs[(i + 1) % n].start();
        area += 0.5 * (a[0] * b[1] - b[0] * a[1]);
        if let LoopSegment::Arc { center, ccw, .. } = &segs[i] {
            let r = dist(a, *center);
            let (a0, a1) = (
                (a[1] - center[1]).atan2(a[0] - center[0]),
                (b[1] - center[1]).atan2(b[0] - center[0]),
            );
            let sweep = if *ccw {
                (a1 - a0).rem_euclid(std::f64::consts::TAU)
            } else {
                (a0 - a1).rem_euclid(std::f64::consts::TAU)
            };
            let seg = 0.5 * r * r * (sweep - sweep.sin());
            area += if *ccw { seg } else { -seg };
        }
    }
    area
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
        }
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
        if matches!(self.gesture, Gesture::Line { .. } | Gesture::DimFirst(_)) {
            self.cancel_gesture();
        }
    }

    pub fn pointer_move(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
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
                let Some(Entity::Circle { center, .. }) = self.sketch.entity(*c).cloned() else {
                    return;
                };
                let Some(cxy) = self.sketch.point_xy(center) else {
                    return;
                };
                let r = dist(pos, cxy);
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
        let item = match self.pick(pos, screen, view) {
            Some(Item::Point(p)) => DragItem::Point(p),
            Some(Item::Entity(e)) => match self.sketch.entity(e) {
                Some(Entity::Circle { .. }) => DragItem::Radius(e),
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
            SketchTool::Offset => self.offset_click(pos, screen, view),
            SketchTool::Fillet => self.fillet_click(pos, screen, view),
            SketchTool::Text => self.text_click(pos, view),
            SketchTool::Slot => self.slot_click(pos, view),
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

    // ----- trim -----

    /// Trim and Extend compute intersections with lines, arcs and circles
    /// only: in a sketch with an ellipse or spline they refuse rather than
    /// ignore those curves.
    fn free_form_refusal(&self, tool: &str) -> Option<String> {
        self.sketch
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Ellipse { .. } | Entity::Spline { .. }))
            .then(|| {
                format!(
                    "{tool} does not support sketches with ellipses or splines (intersections \
                     with them are not computed); delete and redraw the lines instead."
                )
            })
    }

    /// Parameters t in (0, 1) along line `p -> q` where other curves cross it.
    fn line_cuts(&self, line: EntityId, p: [f64; 2], q: [f64; 2]) -> Vec<f64> {
        self.line_hits(line, p, q, false)
    }

    /// Parameters t along the line through `p -> q` (t = 0 at p, 1 at q) where
    /// other curves cross it: inside the segment, or (`beyond`) outside it.
    fn line_hits(&self, line: EntityId, p: [f64; 2], q: [f64; 2], beyond: bool) -> Vec<f64> {
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tol = rcad_sketch::solve_tolerance(self.sketch.scale());
        let eps = tol / len2.sqrt().max(1e-12);
        let inside = |t: f64| {
            if beyond {
                t < -eps || t > 1.0 + eps
            } else {
                t > eps && t < 1.0 - eps
            }
        };
        let mut ts = Vec::new();
        let circle_hits = |c: [f64; 2], r: f64| -> Vec<f64> {
            // |p + t d - c|^2 = r^2
            let f = [p[0] - c[0], p[1] - c[1]];
            let (a, b, cc) = (
                len2,
                2.0 * (f[0] * d[0] + f[1] * d[1]),
                f[0] * f[0] + f[1] * f[1] - r * r,
            );
            let disc = b * b - 4.0 * a * cc;
            if disc < 0.0 {
                return vec![];
            }
            let s = disc.sqrt();
            vec![(-b - s) / (2.0 * a), (-b + s) / (2.0 * a)]
        };
        for e in &self.sketch.entities {
            if e.id() == line {
                continue;
            }
            match e {
                Entity::Line { a, b, .. } => {
                    let (Some(c), Some(g)) = (self.sketch.point_xy(*a), self.sketch.point_xy(*b))
                    else {
                        continue;
                    };
                    let k = [g[0] - c[0], g[1] - c[1]];
                    let den = d[0] * k[1] - d[1] * k[0];
                    if den.abs() < 1e-12 * len2.sqrt() * k[0].hypot(k[1]) {
                        continue; // parallel
                    }
                    let w = [c[0] - p[0], c[1] - p[1]];
                    let t = (w[0] * k[1] - w[1] * k[0]) / den;
                    let u = (w[0] * d[1] - w[1] * d[0]) / den;
                    if inside(t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
                        ts.push(t);
                    }
                }
                // Refused up front (see `free_form_refusal`).
                Entity::Ellipse { .. } | Entity::Spline { .. } => {}
                Entity::Circle { center, radius, .. } => {
                    if let Some(c) = self.sketch.point_xy(*center) {
                        ts.extend(circle_hits(c, *radius).into_iter().filter(|t| inside(*t)));
                    }
                }
                Entity::Arc { id, .. } => {
                    let Some(g) = self.sketch.arc_geometry(*id) else {
                        continue;
                    };
                    for t in circle_hits(g.center, g.radius) {
                        let x = [p[0] + t * d[0], p[1] + t * d[1]];
                        let ang = (x[1] - g.center[1]).atan2(x[0] - g.center[0]);
                        let rel = (ang - g.start_angle).rem_euclid(std::f64::consts::TAU);
                        // Tolerant at both ends (rel just below 0 wraps to ~2π).
                        let ae = tol / g.radius;
                        if inside(t) && (rel <= g.sweep + ae || rel >= std::f64::consts::TAU - ae) {
                            ts.push(t);
                        }
                    }
                }
            }
        }
        ts.sort_by(f64::total_cmp);
        ts
    }

    /// Extend the line end nearest to the click along the line to the nearest
    /// curve it would cross. The end must be free (used by no other curve).
    fn extend_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if let Some(why) = self.free_form_refusal("Extend") {
            self.fail(why);
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
        let Some(Entity::Line { a, b, .. }) = self.sketch.entity(e).cloned() else {
            self.fail("Extend works on lines only.".into());
            return;
        };
        let (Some(p), Some(q)) = (self.sketch.point_xy(a), self.sketch.point_xy(b)) else {
            return;
        };
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tc = ((pos[0] - p[0]) * d[0] + (pos[1] - p[1]) * d[1]) / len2;
        let at_b = picked_end.map_or(tc >= 0.5, |pt| pt == b);
        let end = if at_b { b } else { a };
        let shared = self
            .sketch
            .entities
            .iter()
            .any(|x| x.id() != e && x.points().contains(&end));
        if shared {
            self.fail(
                "That end is connected to another curve; only a free end can be extended.".into(),
            );
            return;
        }
        let hits = self.line_hits(e, p, q, true);
        let t = if at_b {
            hits.iter()
                .copied()
                .filter(|t| *t > 1.0)
                .fold(f64::INFINITY, f64::min)
        } else {
            hits.iter()
                .copied()
                .filter(|t| *t < 0.0)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        if !t.is_finite() {
            self.fail("Nothing to extend to: no curve crosses the line's extension.".into());
            return;
        }
        let x = [p[0] + t * d[0], p[1] + t * d[1]];
        let origin = self.sketch.origin;
        let mut dropped = 0usize;
        let mut free = 0usize;
        let ok = self.try_change(false, |sk| {
            let before = sk.constraints.len();
            sk.constraints.retain(|c| {
                !matches!(c, Constraint::EqualLength { a: u, b: v } if *u == e || *v == e)
                    && !(Some(end) != origin && constraint_mentions(c, end))
            });
            dropped += before - sk.constraints.len();
            let (n, how) = cut_point(sk, x, &[a, b], &[e]);
            free += usize::from(how == CutEnd::Free);
            if let Some(Entity::Line { a, b, .. }) = sk.entities.iter_mut().find(|x| x.id() == e) {
                if at_b {
                    *b = n;
                } else {
                    *a = n;
                }
            }
            if Some(end) != origin {
                sk.points.retain(|x| x.id != end);
            }
            drop_implied_on_line(sk);
            Ok(())
        });
        if ok {
            self.selection.clear();
            let base = if dropped > 0 {
                format!("Extended; {dropped} constraint(s) on the old end were removed.")
            } else {
                "Extended.".into()
            };
            self.info(format!("{base}{}", free_note(free)));
        }
    }

    /// Polar angles (about `c`) where other curves cross the circle (c, r).
    fn circle_cuts(&self, round: EntityId, c: [f64; 2], r: f64) -> Vec<f64> {
        let tol = rcad_sketch::solve_tolerance(self.sketch.scale());
        let ang = |x: [f64; 2]| (x[1] - c[1]).atan2(x[0] - c[0]);
        let mut out = Vec::new();
        let on_arc = |g: &rcad_sketch::ArcGeometry, x: [f64; 2]| {
            let rel = ((x[1] - g.center[1]).atan2(x[0] - g.center[0]) - g.start_angle)
                .rem_euclid(std::f64::consts::TAU);
            let ae = tol / g.radius;
            rel <= g.sweep + ae || rel >= std::f64::consts::TAU - ae
        };
        let circle_circle = |c2: [f64; 2], r2: f64| -> Vec<[f64; 2]> {
            let dx = [c2[0] - c[0], c2[1] - c[1]];
            let dd = dx[0].hypot(dx[1]);
            if dd <= tol || dd > r + r2 + tol || dd < (r - r2).abs() - tol {
                return vec![];
            }
            let u = [dx[0] / dd, dx[1] / dd];
            let a = (r * r - r2 * r2 + dd * dd) / (2.0 * dd);
            let h = (r * r - a * a).max(0.0).sqrt();
            let m = [c[0] + a * u[0], c[1] + a * u[1]];
            vec![
                [m[0] - h * u[1], m[1] + h * u[0]],
                [m[0] + h * u[1], m[1] - h * u[0]],
            ]
        };
        for e in &self.sketch.entities {
            if e.id() == round {
                continue;
            }
            match e {
                Entity::Line { a, b, .. } => {
                    let (Some(p), Some(q)) = (self.sketch.point_xy(*a), self.sketch.point_xy(*b))
                    else {
                        continue;
                    };
                    let d = [q[0] - p[0], q[1] - p[1]];
                    let f = [p[0] - c[0], p[1] - c[1]];
                    let (aa, bb, cc) = (
                        d[0] * d[0] + d[1] * d[1],
                        2.0 * (f[0] * d[0] + f[1] * d[1]),
                        f[0] * f[0] + f[1] * f[1] - r * r,
                    );
                    let disc = bb * bb - 4.0 * aa * cc;
                    if disc < 0.0 || aa == 0.0 {
                        continue;
                    }
                    for t in [
                        (-bb - disc.sqrt()) / (2.0 * aa),
                        (-bb + disc.sqrt()) / (2.0 * aa),
                    ] {
                        if (-1e-9..=1.0 + 1e-9).contains(&t) {
                            out.push(ang([p[0] + t * d[0], p[1] + t * d[1]]));
                        }
                    }
                }
                // Refused up front (see `free_form_refusal`).
                Entity::Ellipse { .. } | Entity::Spline { .. } => {}
                Entity::Circle { center, radius, .. } => {
                    if let Some(c2) = self.sketch.point_xy(*center) {
                        out.extend(circle_circle(c2, *radius).into_iter().map(ang));
                    }
                }
                Entity::Arc { id, .. } => {
                    if let Some(g) = self.sketch.arc_geometry(*id) {
                        out.extend(
                            circle_circle(g.center, g.radius)
                                .into_iter()
                                .filter(|x| on_arc(&g, *x))
                                .map(ang),
                        );
                    }
                }
            }
        }
        out
    }

    /// Trim a circle (to an arc) or an arc at the nearest crossings around the click.
    fn trim_round(&mut self, e: EntityId, pos: [f64; 2]) {
        use std::f64::consts::TAU;
        let ent = self.sketch.entity(e).cloned();
        let (c, r, start, sweep, center_pt) = match &ent {
            Some(Entity::Circle { center, radius, .. }) => {
                let Some(c) = self.sketch.point_xy(*center) else {
                    return;
                };
                (c, *radius, 0.0, TAU, *center)
            }
            Some(Entity::Arc { id, center, .. }) => {
                let Some(g) = self.sketch.arc_geometry(*id) else {
                    return;
                };
                (g.center, g.radius, g.start_angle, g.sweep, *center)
            }
            _ => return,
        };
        let is_circle = matches!(ent, Some(Entity::Circle { .. }));
        let rel = |a: f64| (a - start).rem_euclid(TAU);
        let eps = rcad_sketch::solve_tolerance(self.sketch.scale()) / r;
        let mut cuts: Vec<f64> = self
            .circle_cuts(e, c, r)
            .into_iter()
            .map(rel)
            .filter(|t| {
                if is_circle {
                    true
                } else {
                    *t > eps && *t < sweep - eps
                }
            })
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() <= eps);
        let tc = rel((pos[1] - c[1]).atan2(pos[0] - c[0]));
        let on = |t: f64| [c[0] + r * (start + t).cos(), c[1] + r * (start + t).sin()];
        let origin = self.sketch.origin;
        let mut dropped = 0usize;
        let mut free = 0usize;
        let ok = if is_circle {
            if cuts.len() < 2 {
                if cuts.is_empty() {
                    self.try_change(false, |sk| {
                        sk.remove_entity(e);
                        Ok(())
                    })
                } else {
                    self.fail("A circle needs two crossings to trim.".into());
                    return;
                }
            } else {
                // Removed interval (lo, hi) around the click, cyclically.
                let hi = cuts
                    .iter()
                    .copied()
                    .find(|t| *t > tc)
                    .unwrap_or(cuts[0] + TAU);
                let lo = cuts
                    .iter()
                    .copied()
                    .rev()
                    .find(|t| *t < tc)
                    .unwrap_or(cuts[cuts.len() - 1] - TAU);
                self.try_change(false, |sk| {
                    // Keep the arc CCW from hi to lo (+ 2π).
                    let (ps, pe) = (on(hi), on(lo));
                    let (s_id, h0) = cut_point(sk, ps, &[], &[e]);
                    let (e_id, h1) = cut_point(sk, pe, &[], &[e]);
                    free += usize::from(h0 == CutEnd::Free) + usize::from(h1 == CutEnd::Free);
                    // The arc keeps the circle's entity id (the
                    // id names generated faces; same centre point, so the
                    // command layer accepts Circle -> Arc for it) and its
                    // construction flag, in place.
                    if let Some(slot) = sk.entities.iter_mut().find(|x| x.id() == e) {
                        *slot = Entity::Arc {
                            id: e,
                            center: center_pt,
                            start: s_id,
                            end: e_id,
                        };
                    }
                    let before = sk.constraints.len();
                    let mut keep = Vec::new();
                    for k in std::mem::take(&mut sk.constraints) {
                        match k {
                            Constraint::Diameter { circle, mut dim } if circle == e => {
                                dim.value /= 2.0;
                                keep.push(Constraint::Radius { entity: e, dim });
                            }
                            // Valid for an arc with the same id as they were for the circle.
                            k @ (Constraint::Radius { .. }
                            | Constraint::EqualRadius { .. }
                            | Constraint::PointOnCircle { .. }) => keep.push(k),
                            k if k.entities().contains(&e) => {}
                            k => keep.push(k),
                        }
                    }
                    sk.constraints = keep;
                    dropped += before.saturating_sub(sk.constraints.len());
                    drop_implied_on_line(sk);
                    Ok(())
                })
            }
        } else {
            let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
            let hi = cuts
                .iter()
                .copied()
                .filter(|t| *t > tc)
                .fold(sweep, f64::min);
            let Some(Entity::Arc {
                start: sp, end: ep, ..
            }) = ent
            else {
                return;
            };
            self.try_change(false, |sk| {
                if lo <= 0.0 && hi >= sweep {
                    sk.remove_entity(e);
                    return Ok(());
                }
                let mut piece = None;
                let set = |sk: &mut Sketch, s: PointId, t: PointId| {
                    if let Some(Entity::Arc { start, end, .. }) =
                        sk.entities.iter_mut().find(|x| x.id() == e)
                    {
                        (*start, *end) = (s, t);
                    }
                };
                let mut freed = Vec::new();
                let own = [sp, ep];
                let mut point = |sk: &mut Sketch, t: f64| {
                    let (n, how) = cut_point(sk, on(t), &own, &[e]);
                    free += usize::from(how == CutEnd::Free);
                    n
                };
                if lo <= 0.0 {
                    let n = point(sk, hi);
                    set(sk, n, ep);
                    freed.push(sp);
                } else if hi >= sweep {
                    let n = point(sk, lo);
                    set(sk, sp, n);
                    freed.push(ep);
                } else {
                    let n0 = point(sk, lo);
                    let n1 = point(sk, hi);
                    set(sk, sp, n0);
                    let id = sk.next_entity_id();
                    sk.entities.push(Entity::Arc {
                        id,
                        center: center_pt,
                        start: n1,
                        end: ep,
                    });
                    if sk.is_construction(e) {
                        sk.construction.insert(id);
                    }
                    sk.constraints.push(Constraint::EqualRadius { a: e, b: id });
                    piece = Some(id);
                }
                dropped += retarget_tangents(sk, e, piece);
                drop_implied_on_line(sk);
                for pt in freed {
                    if Some(pt) != origin && !sk.entities.iter().any(|x| x.points().contains(&pt)) {
                        let before = sk.constraints.len();
                        sk.constraints.retain(|k| !constraint_mentions(k, pt));
                        dropped += before - sk.constraints.len();
                        sk.points.retain(|x| x.id != pt);
                    }
                }
                Ok(())
            })
        };
        if ok {
            self.selection.clear();
            self.trimmed_info(dropped, free);
        }
    }

    fn trimmed_info(&mut self, dropped: usize, free: usize) {
        self.trimmed_info_dims(dropped, free, &[]);
    }

    /// Status line after a trim; `split_dims` names length dimensions removed
    /// because the line they measured was split.
    fn trimmed_info_dims(&mut self, dropped: usize, free: usize, split_dims: &[String]) {
        let base = if dropped > 0 {
            format!("Trimmed; {dropped} constraint(s) that no longer applied were removed.")
        } else {
            "Trimmed.".into()
        };
        let dims = if split_dims.is_empty() {
            String::new()
        } else {
            format!(
                " The split line's length dimension {} was removed (it spanned both pieces).",
                split_dims
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.info(format!("{base}{dims}{}", free_note(free)));
    }

    fn trim_click(&mut self, pos: [f64; 2], screen: Option<egui::Pos2>, view: &View) {
        if let Some(why) = self.free_form_refusal("Trim") {
            self.fail(why);
            return;
        }
        let Some(Item::Entity(e)) = self.pick(pos, screen, view) else {
            self.fail("Click the part of a line to remove (away from its endpoints).".into());
            return;
        };
        let Some(Entity::Line { a, b, .. }) = self.sketch.entity(e).cloned() else {
            self.trim_round(e, pos);
            return;
        };
        let (Some(p), Some(q)) = (self.sketch.point_xy(a), self.sketch.point_xy(b)) else {
            return;
        };
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tc = (((pos[0] - p[0]) * d[0] + (pos[1] - p[1]) * d[1]) / len2).clamp(0.0, 1.0);
        let cuts = self.line_cuts(e, p, q);
        let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
        let hi = cuts.iter().copied().filter(|t| *t > tc).fold(1.0, f64::min);
        let at = |t: f64| [p[0] + t * d[0], p[1] + t * d[1]];
        let mut dropped = 0usize;
        let mut free = 0usize;
        let mut split_dims: Vec<String> = Vec::new();
        let origin = self.sketch.origin;
        let ok = self.try_change(false, |sk| {
            if lo <= 0.0 && hi >= 1.0 {
                sk.remove_entity(e);
                return Ok(());
            }
            // Constraints whose meaning depends on this line's extent.
            let before = sk.constraints.len();
            sk.constraints.retain(
                |c| !matches!(c, Constraint::EqualLength { a: x, b: y } if *x == e || *y == e),
            );
            dropped += before - sk.constraints.len();
            let mut piece = None;
            let mut point = |sk: &mut Sketch, t: f64| {
                let (n, how) = cut_point(sk, at(t), &[a, b], &[e]);
                free += usize::from(how == CutEnd::Free);
                n
            };
            let set_ends = |sk: &mut Sketch, na: PointId, nb: PointId| {
                if let Some(Entity::Line { a, b, .. }) =
                    sk.entities.iter_mut().find(|x| x.id() == e)
                {
                    (*a, *b) = (na, nb);
                }
            };
            let mut freed = Vec::new();
            if lo <= 0.0 {
                let n = point(sk, hi);
                set_ends(sk, n, b);
                freed.push(a);
            } else if hi >= 1.0 {
                let n = point(sk, lo);
                set_ends(sk, a, n);
                freed.push(b);
            } else {
                let n0 = point(sk, lo);
                let n1 = point(sk, hi);
                set_ends(sk, a, n0);
                // A dimension between the original ends now
                // spans two separate lines and no longer measures a length; no
                // piece keeps both measured points, so it is removed (named in
                // the status line).
                sk.constraints.retain(|c| match c {
                    Constraint::Distance { a: x, b: y, dim }
                    | Constraint::HorizontalDistance { a: x, b: y, dim }
                    | Constraint::VerticalDistance { a: x, b: y, dim }
                        if (*x == a && *y == b) || (*x == b && *y == a) =>
                    {
                        split_dims.push(dim.name.clone());
                        false
                    }
                    _ => true,
                });
                let l2 = sk.add_line(n1, b);
                if sk.is_construction(e) {
                    sk.construction.insert(l2);
                }
                piece = Some(l2);
                let hv: Vec<Constraint> = sk
                    .constraints
                    .iter()
                    .filter_map(|c| match c {
                        Constraint::Horizontal { line } if *line == e => {
                            Some(Constraint::Horizontal { line: l2 })
                        }
                        Constraint::Vertical { line } if *line == e => {
                            Some(Constraint::Vertical { line: l2 })
                        }
                        _ => None,
                    })
                    .collect();
                sk.constraints.extend(hv);
            }
            dropped += retarget_tangents(sk, e, piece);
            drop_implied_on_line(sk);
            // A detached endpoint used by nothing else goes, with its constraints.
            for pt in freed {
                if Some(pt) != origin && !sk.entities.iter().any(|x| x.points().contains(&pt)) {
                    let before = sk.constraints.len();
                    sk.constraints.retain(|c| !constraint_mentions(c, pt));
                    dropped += before - sk.constraints.len();
                    sk.points.retain(|x| x.id != pt);
                }
            }
            Ok(())
        });
        if ok {
            self.selection.clear();
            self.trimmed_info_dims(dropped, free, &split_dims);
        }
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
        let value = match ed
            .buffer
            .trim()
            .trim_end_matches("mm")
            .trim_end_matches('°')
            .trim_end_matches("deg")
            .trim()
            .parse::<f64>()
        {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => {
                self.fail(format!(
                    "'{}' is not a valid dimension: enter a positive number (millimetres; \
                     degrees for angles).",
                    ed.buffer.trim()
                ));
                return false;
            }
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
        let ok = self.try_change(false, |sk| {
            sk.set_dimension(&name, value).map_err(|e| e.to_string())
        });
        if ok {
            self.dim_edit = None;
            let angle = matches!(
                self.sketch.constraints.get(ed.constraint),
                Some(Constraint::Angle { .. })
            );
            self.info(format!(
                "{name} = {}{}",
                rcad_sketch::fmt_mm(value),
                if angle { "°" } else { " mm" }
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

    /// A trim that replaces one of two tangent arcs by a piece
    /// moves the arc–arc tangency to the piece that keeps the shared point
    /// (it used to be left behind, then refused or silently dropped).
    #[test]
    fn trim_retargets_arc_arc_tangency() {
        let mut sk = Sketch::default();
        let (c1, c2, c3) = (
            sk.add_point(0.0, 0.0),
            sk.add_point(20.0, 0.0),
            sk.add_point(0.0, 0.1),
        );
        let p = sk.add_point(10.0, 0.0);
        let (s1, e2, s3) = (
            sk.add_point(-10.0, 0.0),
            sk.add_point(30.0, 0.0),
            sk.add_point(0.0, 10.1),
        );
        let arc = |sk: &mut Sketch, center, start, end| {
            let id = sk.next_entity_id();
            sk.entities.push(Entity::Arc {
                id,
                center,
                start,
                end,
            });
            id
        };
        let a = arc(&mut sk, c1, s1, p);
        let b = arc(&mut sk, c2, p, e2);
        sk.constraints.push(Constraint::TangentArcs { a, b });
        // The trim moved `a` off P and a new piece (c3) now ends at P.
        let piece = arc(&mut sk, c3, s3, p);
        for e in &mut sk.entities {
            if let Entity::Arc { id, end, .. } = e
                && *id == a
            {
                *end = s3;
            }
        }
        assert_eq!(retarget_tangents(&mut sk, a, Some(piece)), 0);
        assert!(
            sk.constraints
                .contains(&Constraint::TangentArcs { a: piece, b })
        );
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
            let hits = ed.line_cuts(l, at(th, 5.0), at(th, 15.0));
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
}
