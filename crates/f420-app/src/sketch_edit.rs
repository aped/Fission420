//! Interactive sketch editor: tools, snapping, picking, constraint-respecting
//! drags, dimensions, and a local undo stack.
//!
//! The editor works on a private copy of an [`f420_sketch::Sketch`] in the
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
use f420_sketch::{
    Constraint, Dimension, DragOutcome, Entity, EntityId, PointId, Sketch, SolveReport, SolveStatus,
};

/// Pick tolerance for points and curves, screen pixels.
pub const PICK_PX: f64 = 8.0;
/// Snap radius for existing points, screen pixels.
pub const SNAP_PX: f64 = 10.0;
/// Horizontal / vertical alignment capture band, screen pixels.
pub const ALIGN_PX: f64 = 8.0;
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
                "Click a line (length), circle (diameter), arc (radius), or two points (horizontal/vertical distance)"
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
}

/// Inline numeric editor for a dimension.
#[derive(Clone, Debug, PartialEq)]
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
        Constraint::PointOnLine { point, .. } => *point == p,
        _ => false,
    }
}

/// Distance from `p` to an arc (to the nearest end outside its angular span).
pub fn arc_distance(g: &f420_sketch::ArcGeometry, p: [f64; 2]) -> f64 {
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
pub fn loop_area(segs: &[f420_sketch::LoopSegment]) -> f64 {
    use f420_sketch::LoopSegment;
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
            _ => None,
        }
    }

    pub fn status(&self) -> (StateKind, String) {
        let r = &self.report;
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
        if self.sketch.entities.is_empty() {
            return Err("nothing drawn yet".into());
        }
        match self.sketch.profile() {
            Ok(f420_sketch::Profile::Polygon(v)) => {
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
                    f420_sketch::fmt_mm(a)
                ))
            }
            Ok(f420_sketch::Profile::Circle { radius, .. }) => {
                Ok(format!("circle Ø{} mm", f420_sketch::fmt_mm(2.0 * radius)))
            }
            Ok(f420_sketch::Profile::Loop(v)) => Ok(format!(
                "closed loop, {} lines/arcs, area {} mm²",
                v.len(),
                f420_sketch::fmt_mm(loop_area(&v))
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
                Entity::Arc { .. } => e.points().contains(&id),
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
        self.hover = if self.tool == SketchTool::Select || self.tool == SketchTool::Dimension {
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
        }
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
                f420_sketch::fmt_mm(hi[0] - lo[0]),
                f420_sketch::fmt_mm(hi[1] - lo[1])
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
                f420_sketch::fmt_mm(2.0 * r)
            ));
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
        let c = if dx >= dy {
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
                buffer: f420_sketch::fmt_mm(value),
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
                f420_sketch::fmt_mm(value),
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

    fn selected_entities(&self) -> Vec<EntityId> {
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
            f420_sketch::Profile::Polygon(v) => {
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
        let tol = 260.0 * f420_sketch::solve_tolerance(80.0);
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
    fn review_fixes_stray_point_delete_duplicates_and_corner_snaps() {
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
        let tol = 40.0 * f420_sketch::solve_tolerance(40.0);
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
}
