//! Painting of sketches over the 3D view (egui shapes projected with the
//! viewport camera): curves, points, dimension annotations with units, and
//! constraint glyphs. Returns the screen rectangles of selectable annotations
//! so the editor (or the read-only dimension editor) can hit-test them.
//!
//! Dimension layout is derived from the geometry each frame (offsets are in
//! screen pixels, converted to sketch mm with the local scale), so labels stay
//! readable at any zoom and never need to be stored in the document.

use eframe::egui::{self, Color32, Pos2, Stroke};
use rcad_sketch::{Constraint, Entity, EntityId, PointId, Sketch};
use rcad_viewport::Camera;

use crate::place::PlaneFrame;
use crate::sketch_edit::{Item, SketchEditor, SketchTool, SnapKind, StateKind};

/// Offset of a dimension line from its geometry, screen pixels.
const DIM_OFFSET_PX: f64 = 26.0;
const CIRCLE_SEGMENTS: usize = 96;
/// Display segments per exact ellipse / spline span.
const CURVE_SPAN_SEGMENTS: usize = 32;

pub struct Palette {
    pub curve: Color32,
    pub curve_full: Color32,
    pub selected: Color32,
    pub hover: Color32,
    pub dim: Color32,
    pub glyph: Color32,
    pub conflict: Color32,
    pub preview: Color32,
    pub plane: Color32,
    /// Projected (linked, read-only) geometry.
    pub projected: Color32,
}

pub const PALETTE: Palette = Palette {
    curve: Color32::from_rgb(110, 185, 255),
    curve_full: Color32::from_rgb(235, 235, 235),
    selected: Color32::from_rgb(255, 165, 40),
    hover: Color32::from_rgb(255, 225, 100),
    dim: Color32::from_rgb(150, 225, 150),
    glyph: Color32::from_rgb(200, 200, 210),
    conflict: Color32::from_rgb(255, 90, 90),
    preview: Color32::from_rgb(255, 205, 60),
    plane: Color32::from_rgba_premultiplied(8, 16, 30, 30),
    projected: Color32::from_rgb(190, 120, 255),
};

/// Projection of sketch-plane points to absolute screen positions.
#[derive(Clone)]
pub struct Projector {
    pub cam: Camera,
    pub rect: egui::Rect,
    pub frame: PlaneFrame,
}

impl Projector {
    pub fn screen(&self, p: [f64; 2]) -> Option<Pos2> {
        let w = self.frame.to_world(p).map(|v| v as f32);
        let [x, y] = self.cam.project(w, self.rect.width(), self.rect.height())?;
        Some(self.rect.min + egui::vec2(x, y))
    }

    /// Screen pixels per mm around plane point `at` (max over the two axes).
    pub fn px_per_mm(&self, at: [f64; 2]) -> f64 {
        let (Some(o), Some(x), Some(y)) = (
            self.screen(at),
            self.screen([at[0] + 1.0, at[1]]),
            self.screen([at[0], at[1] + 1.0]),
        ) else {
            return 1.0;
        };
        f64::from(o.distance(x).max(o.distance(y))).max(1e-6)
    }

    /// Plane point under a screen position, if the ray hits the plane.
    pub fn unproject(&self, s: Pos2) -> Option<[f64; 2]> {
        let l = s - self.rect.min;
        let ray = self
            .cam
            .ray(l.x, l.y, self.rect.width(), self.rect.height())?;
        self.frame.intersect(&ray)
    }
}

/// Inputs for painting one sketch.
pub struct SketchStyle<'a> {
    pub selection: &'a [Item],
    pub hover: Option<Item>,
    /// Dimension being edited inline (drawn highlighted).
    pub editing: Option<usize>,
    pub state: StateKind,
    /// Constraint indices reported as conflicting/redundant.
    pub conflicts: &'a [usize],
    /// Draw H/V/coincident/symmetry glyphs.
    pub glyphs: bool,
    /// Multiplier for all colours' alpha (dimmed read-only previews).
    pub alpha: f32,
    /// Projected (linked) entities, drawn in the projected colour.
    pub projected: &'a [EntityId],
}

fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

fn bbox(s: &Sketch) -> Option<([f64; 2], [f64; 2])> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut any = false;
    for e in &s.entities {
        let pts: Vec<([f64; 2], f64)> = match e {
            Entity::Line { a, b, .. } => [*a, *b]
                .iter()
                .filter_map(|p| s.point_xy(*p).map(|q| (q, 0.0)))
                .collect(),
            Entity::Circle { center, radius, .. } => s
                .point_xy(*center)
                .map(|q| vec![(q, *radius)])
                .unwrap_or_default(),
            Entity::Arc { id, .. } => s
                .arc_geometry(*id)
                .map(|g| arc_points(&g, 32).into_iter().map(|q| (q, 0.0)).collect())
                .unwrap_or_default(),
            Entity::Ellipse { .. } | Entity::Spline { .. } => {
                rcad_sketch::curves::entity_polyline(s, e, 16)
                    .map(|v| v.into_iter().map(|q| (q, 0.0)).collect())
                    .unwrap_or_default()
            }
        };
        for (p, r) in pts {
            any = true;
            for k in 0..2 {
                lo[k] = lo[k].min(p[k] - r);
                hi[k] = hi[k].max(p[k] + r);
            }
        }
    }
    any.then_some((lo, hi))
}

/// Points along an arc, start to end (CCW), `n` segments.
pub fn arc_points(g: &rcad_sketch::ArcGeometry, n: usize) -> Vec<[f64; 2]> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let t = g.start_angle + g.sweep * i as f64 / n as f64;
            [
                g.center[0] + g.radius * t.cos(),
                g.center[1] + g.radius * t.sin(),
            ]
        })
        .collect()
}

fn arc_segments(g: &rcad_sketch::ArcGeometry) -> usize {
    ((g.sweep / std::f64::consts::TAU) * CIRCLE_SEGMENTS as f64).ceil() as usize + 2
}

/// Sketch-plane bounding box of the curves, if any.
pub fn sketch_bounds(s: &Sketch) -> Option<([f64; 2], [f64; 2])> {
    bbox(s)
}

fn arrow(painter: &egui::Painter, tip: Pos2, from: Pos2, color: Color32) {
    let d = tip - from;
    let len = d.length();
    if len < 1.0 {
        return;
    }
    let u = d / len;
    let n = egui::vec2(-u.y, u.x);
    let size = 7.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            tip - u * size + n * size * 0.4,
            tip - u * size - n * size * 0.4,
        ],
        color,
        Stroke::NONE,
    ));
}

/// Paint the label with a dark background; returns its rect.
fn label(
    painter: &egui::Painter,
    at: Pos2,
    text: String,
    color: Color32,
    strong: bool,
) -> egui::Rect {
    let font = egui::FontId::proportional(if strong { 15.0 } else { 13.5 });
    let galley = painter.layout_no_wrap(text, font, color);
    let r = egui::Rect::from_center_size(at, galley.size()).expand(3.0);
    painter.rect_filled(
        r,
        3.0,
        Color32::from_black_alpha(if strong { 230 } else { 190 }),
    );
    if strong {
        painter.rect_stroke(r, 3.0, Stroke::new(1.5, color), egui::StrokeKind::Outside);
    }
    painter.galley(r.min + egui::vec2(3.0, 3.0), galley, color);
    r
}

/// Draw a sketch. Returns selectable screen targets (dimension labels and glyphs).
pub fn paint_sketch(
    painter: &egui::Painter,
    pr: &Projector,
    s: &Sketch,
    st: &SketchStyle<'_>,
) -> Vec<(Item, egui::Rect)> {
    let mut targets = vec![];
    let base = if st.state == StateKind::Full {
        PALETTE.curve_full
    } else {
        PALETTE.curve
    };
    let color_of = |item: Item| {
        let c = if st.selection.contains(&item) {
            PALETTE.selected
        } else if st.hover == Some(item) {
            PALETTE.hover
        } else {
            base
        };
        fade(c, st.alpha)
    };
    // Text outlines: the flattened glyph contours that become profile
    // loops; a text whose outlines cannot be placed is skipped (its error is
    // reported by the region/profile status).
    for t in &s.texts {
        let Ok(polys) = t.polylines(s) else {
            continue;
        };
        for (_, _, v) in polys {
            let pts: Option<Vec<Pos2>> = v.iter().map(|p| pr.screen(*p)).collect();
            if let Some(pts) = pts {
                painter.add(egui::Shape::closed_line(
                    pts,
                    Stroke::new(1.5, fade(base, st.alpha)),
                ));
            }
        }
    }
    // Curves.
    for e in &s.entities {
        let item = Item::Entity(e.id());
        let construction = s.is_construction(e.id());
        let c = if st.projected.contains(&e.id())
            && !st.selection.contains(&item)
            && st.hover != Some(item)
        {
            fade(
                PALETTE.projected,
                st.alpha * if construction { 0.75 } else { 1.0 },
            )
        } else if construction {
            fade(color_of(item), 0.6)
        } else {
            color_of(item)
        };
        // Construction (reference) curves are dashed.
        let draw = |pts: Vec<Pos2>, w: f32| {
            if construction {
                painter.extend(egui::Shape::dashed_line(
                    &pts,
                    Stroke::new(w * 0.75, c),
                    6.0,
                    4.0,
                ));
            } else {
                painter.add(egui::Shape::line(pts, Stroke::new(w, c)));
            }
        };
        let w = if st.selection.contains(&item) || st.hover == Some(item) {
            3.0
        } else {
            2.0
        };
        match e {
            Entity::Line { a, b, .. } => {
                if let (Some(pa), Some(pb)) = (
                    s.point_xy(*a).and_then(|p| pr.screen(p)),
                    s.point_xy(*b).and_then(|p| pr.screen(p)),
                ) {
                    draw(vec![pa, pb], w);
                }
            }
            Entity::Circle { center, radius, .. } => {
                let Some(cxy) = s.point_xy(*center) else {
                    continue;
                };
                let pts: Vec<Pos2> = (0..=CIRCLE_SEGMENTS)
                    .filter_map(|i| {
                        let t = std::f64::consts::TAU * i as f64 / CIRCLE_SEGMENTS as f64;
                        pr.screen([cxy[0] + radius * t.cos(), cxy[1] + radius * t.sin()])
                    })
                    .collect();
                draw(pts, w);
            }
            Entity::Arc { id, .. } => {
                if let Some(g) = s.arc_geometry(*id) {
                    let pts: Vec<Pos2> = arc_points(&g, arc_segments(&g))
                        .into_iter()
                        .filter_map(|q| pr.screen(q))
                        .collect();
                    draw(pts, w);
                }
            }
            Entity::Ellipse { .. } | Entity::Spline { .. } => {
                if let Entity::Spline { points, closed, .. } = e {
                    // Control polygon: thin dashed guide.
                    let mut cp: Vec<Pos2> = points
                        .iter()
                        .filter_map(|p| s.point_xy(*p).and_then(|q| pr.screen(q)))
                        .collect();
                    if *closed && let Some(f) = cp.first().copied() {
                        cp.push(f);
                    }
                    painter.extend(egui::Shape::dashed_line(
                        &cp,
                        Stroke::new(1.0, fade(PALETTE.glyph, st.alpha)),
                        4.0,
                        4.0,
                    ));
                }
                if let Some(v) = rcad_sketch::curves::entity_polyline(s, e, CURVE_SPAN_SEGMENTS) {
                    draw(v.into_iter().filter_map(|q| pr.screen(q)).collect(), w);
                }
            }
        }
    }
    // Points: entity endpoints and centres; construction points as small x.
    let used: std::collections::HashSet<PointId> = s
        .entities
        .iter()
        .flat_map(|e| match e {
            Entity::Line { a, b, .. } => vec![*a, *b],
            Entity::Circle { center, .. } => vec![*center],
            // Arc centres are construction points (drawn as a small x).
            Entity::Arc { start, end, .. } => vec![*start, *end],
            Entity::Ellipse { center, major, .. } => vec![*center, *major],
            Entity::Spline { points, .. } => points.clone(),
        })
        .collect();
    for p in &s.points {
        let Some(sp) = pr.screen([p.x, p.y]) else {
            continue;
        };
        let item = Item::Point(p.id);
        let c = color_of(item);
        if s.origin == Some(p.id) {
            painter.circle_stroke(sp, 4.5, Stroke::new(1.5, fade(Color32::WHITE, st.alpha)));
            painter.circle_filled(sp, 2.0, fade(Color32::WHITE, st.alpha));
        } else if used.contains(&p.id) {
            let r = if st.selection.contains(&item) || st.hover == Some(item) {
                4.5
            } else {
                3.0
            };
            painter.rect_filled(
                egui::Rect::from_center_size(sp, egui::vec2(2.0 * r, 2.0 * r)),
                0.0,
                c,
            );
        } else {
            let d = 3.5;
            let k = Stroke::new(1.2, fade(PALETTE.glyph, st.alpha));
            painter.line_segment([sp + egui::vec2(-d, -d), sp + egui::vec2(d, d)], k);
            painter.line_segment([sp + egui::vec2(-d, d), sp + egui::vec2(d, -d)], k);
        }
    }
    // Dimensions and glyphs.
    let (lo, hi) = bbox(s).unwrap_or(([0.0; 2], [0.0; 2]));
    let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    for (i, c) in s.constraints.iter().enumerate() {
        let item = Item::Constraint(i);
        let conflict = st.conflicts.contains(&i);
        let col = if conflict {
            PALETTE.conflict
        } else if st.selection.contains(&item) || st.editing == Some(i) {
            PALETTE.selected
        } else if st.hover == Some(item) {
            PALETTE.hover
        } else {
            PALETTE.dim
        };
        let col = fade(col, st.alpha);
        if c.dimension().is_some() {
            if let Some(r) = paint_dimension(painter, pr, s, c, mid, col, st.editing == Some(i)) {
                targets.push((item, r));
            }
            continue;
        }
        if !st.glyphs {
            continue;
        }
        let gcol = if conflict {
            fade(PALETTE.conflict, st.alpha)
        } else if st.selection.contains(&item) {
            fade(PALETTE.selected, st.alpha)
        } else if st.hover == Some(item) {
            fade(PALETTE.hover, st.alpha)
        } else {
            fade(PALETTE.glyph, st.alpha)
        };
        let glyph = |text: &str, at: Pos2| {
            let galley = painter.layout_no_wrap(text.into(), egui::FontId::monospace(11.0), gcol);
            let r = egui::Rect::from_center_size(at, galley.size()).expand(2.0);
            painter.rect_filled(r, 2.0, Color32::from_black_alpha(170));
            painter.rect_stroke(r, 2.0, Stroke::new(1.0, gcol), egui::StrokeKind::Inside);
            painter.galley(r.min + egui::vec2(2.0, 2.0), galley, gcol);
            r
        };
        match c {
            Constraint::Horizontal { line } | Constraint::Vertical { line } => {
                let Some(Entity::Line { a, b, .. }) = s.entity(*line) else {
                    continue;
                };
                let (Some(pa), Some(pb)) = (s.point_xy(*a), s.point_xy(*b)) else {
                    continue;
                };
                let m = [0.5 * (pa[0] + pb[0]), 0.5 * (pa[1] + pb[1])];
                let Some(sm) = pr.screen(m) else { continue };
                // Offset toward the sketch interior so glyphs don't collide with dimensions.
                let inward = egui::vec2(
                    (mid[0] - m[0]).signum() as f32,
                    -(mid[1] - m[1]).signum() as f32,
                );
                let mut off = if matches!(c, Constraint::Horizontal { .. }) {
                    egui::vec2(0.0, 13.0 * inward.y)
                } else {
                    egui::vec2(13.0 * inward.x, 0.0)
                };
                // Slide along the line if a point marker sits under the glyph.
                let crowded = |at: Pos2| {
                    s.points
                        .iter()
                        .filter_map(|q| pr.screen([q.x, q.y]))
                        .any(|q| q.distance(at) < 14.0)
                };
                if crowded(sm + off) {
                    let along = (pr.screen(pb).unwrap_or(sm) - sm).normalized();
                    off += along * 24.0;
                }
                let t = if matches!(c, Constraint::Horizontal { .. }) {
                    "H"
                } else {
                    "V"
                };
                targets.push((item, glyph(t, sm + off)));
            }
            Constraint::Tangent { line, arc } => {
                let shared = match (s.entity(*line), s.entity(*arc)) {
                    (Some(l), Some(Entity::Arc { start, end, .. })) => {
                        l.points().into_iter().find(|p| p == start || p == end)
                    }
                    _ => None,
                };
                if let Some(sp) = shared
                    .and_then(|p| s.point_xy(p))
                    .and_then(|q| pr.screen(q))
                {
                    targets.push((item, glyph("T", sp + egui::vec2(-12.0, -12.0))));
                }
            }
            Constraint::EqualRadius { a, b } => {
                for e in [a, b] {
                    let at = match s.entity(*e) {
                        Some(Entity::Arc { id, .. }) => s.arc_geometry(*id).map(|g| {
                            let m = g.start_angle + 0.5 * g.sweep;
                            [
                                g.center[0] + g.radius * 0.6 * m.cos(),
                                g.center[1] + g.radius * 0.6 * m.sin(),
                            ]
                        }),
                        Some(Entity::Circle { center, .. }) => s.point_xy(*center),
                        _ => None,
                    };
                    if let Some(sp) = at.and_then(|q| pr.screen(q)) {
                        targets.push((item, glyph("=", sp)));
                    }
                }
            }
            Constraint::Parallel { a, b }
            | Constraint::Perpendicular { a, b }
            | Constraint::EqualLength { a, b } => {
                // Different stations along the line per kind, so glyphs of
                // several constraints on the same line don't stack.
                let (t, at) = match c {
                    Constraint::Parallel { .. } => ("//", 0.78),
                    Constraint::Perpendicular { .. } => ("90", 0.22),
                    _ => ("=", 0.62),
                };
                for l in [a, b] {
                    if let Some(Entity::Line { a: p, b: q, .. }) = s.entity(*l)
                        && let (Some(pp), Some(pq)) = (s.point_xy(*p), s.point_xy(*q))
                        && let Some(sm) = pr.screen([
                            (1.0 - at) * pp[0] + at * pq[0],
                            (1.0 - at) * pp[1] + at * pq[1],
                        ])
                    {
                        targets.push((item, glyph(t, sm + egui::vec2(0.0, -12.0))));
                    }
                }
            }
            Constraint::PointOnLine { point, .. } | Constraint::PointOnCircle { point, .. } => {
                if let Some(sp) = s.point_xy(*point).and_then(|p| pr.screen(p)) {
                    targets.push((item, glyph("On", sp + egui::vec2(14.0, -12.0))));
                }
            }
            Constraint::Coincident { a, .. } => {
                if let Some(sp) = s.point_xy(*a).and_then(|p| pr.screen(p)) {
                    targets.push((item, glyph("C", sp + egui::vec2(11.0, -11.0))));
                }
            }
            Constraint::SymmetricAboutPoint { a, b, center } => {
                // "Mid" when the pair is a line's own endpoints, else symmetry.
                let is_mid = s.entities.iter().any(|e| {
                    matches!(e, Entity::Line { a: x, b: y, .. } if (x == a && y == b) || (x == b && y == a))
                });
                if let Some(sp) = s.point_xy(*center).and_then(|p| pr.screen(p)) {
                    let t = if is_mid { "Mid" } else { "Sym" };
                    targets.push((item, glyph(t, sp + egui::vec2(22.0, 16.0))));
                }
            }
            // Pins of projected curves follow their link: no glyph (the
            // curve's colour says it is projected).
            Constraint::FixedPoint { p, .. }
                if s.origin != Some(*p)
                    && !st
                        .projected
                        .iter()
                        .any(|e| s.entity(*e).is_some_and(|x| x.points().contains(p))) =>
            {
                if let Some(sp) = s.point_xy(*p).and_then(|q| pr.screen(q)) {
                    targets.push((item, glyph("Fix", sp + egui::vec2(-16.0, 12.0))));
                }
            }
            _ => {}
        }
    }
    targets
}

fn paint_dimension(
    painter: &egui::Painter,
    pr: &Projector,
    s: &Sketch,
    c: &Constraint,
    mid: [f64; 2],
    col: Color32,
    editing: bool,
) -> Option<egui::Rect> {
    let d = c.dimension()?;
    let v = if d.driven { d.value.abs() } else { d.value };
    let text = match c {
        Constraint::Diameter { .. } => format!("Ø{} mm", rcad_sketch::fmt_mm(v)),
        Constraint::Radius { .. } => format!("R{} mm", rcad_sketch::fmt_mm(v)),
        Constraint::Angle { .. } => format!("{}°", rcad_sketch::fmt_mm(v)),
        _ => format!("{} mm", rcad_sketch::fmt_mm(v)),
    };
    // Driven (measuring) dimensions are shown in parentheses.
    let text = if d.driven { format!("({text})") } else { text };
    let stroke = Stroke::new(1.2, col);
    match c {
        Constraint::HorizontalDistance { a, b, .. }
        | Constraint::VerticalDistance { a, b, .. }
        | Constraint::Distance { a, b, .. } => {
            let (pa, pb) = (s.point_xy(*a)?, s.point_xy(*b)?);
            let m = [0.5 * (pa[0] + pb[0]), 0.5 * (pa[1] + pb[1])];
            let off = DIM_OFFSET_PX / pr.px_per_mm(m);
            // Dimension-line endpoints in the plane.
            let (qa, qb) = match c {
                Constraint::HorizontalDistance { .. } => {
                    let y = if m[1] <= mid[1] {
                        pa[1].min(pb[1]) - off
                    } else {
                        pa[1].max(pb[1]) + off
                    };
                    ([pa[0], y], [pb[0], y])
                }
                Constraint::VerticalDistance { .. } => {
                    let x = if m[0] >= mid[0] {
                        pa[0].max(pb[0]) + off
                    } else {
                        pa[0].min(pb[0]) - off
                    };
                    ([x, pa[1]], [x, pb[1]])
                }
                _ => {
                    let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
                    let l = dx.hypot(dy).max(1e-12);
                    let mut n = [-dy / l, dx / l];
                    if (m[0] - mid[0]) * n[0] + (m[1] - mid[1]) * n[1] < 0.0 {
                        n = [-n[0], -n[1]];
                    }
                    (
                        [pa[0] + n[0] * off, pa[1] + n[1] * off],
                        [pb[0] + n[0] * off, pb[1] + n[1] * off],
                    )
                }
            };
            let (sa, sb, sqa, sqb) = (
                pr.screen(pa)?,
                pr.screen(pb)?,
                pr.screen(qa)?,
                pr.screen(qb)?,
            );
            // Extension lines with a small overshoot.
            for (g, q) in [(sa, sqa), (sb, sqb)] {
                let v = q - g;
                let over = if v.length() > 0.5 {
                    v.normalized() * 4.0
                } else {
                    egui::Vec2::ZERO
                };
                painter.line_segment([g + v.normalized() * 3.0, q + over], Stroke::new(0.8, col));
            }
            painter.line_segment([sqa, sqb], stroke);
            arrow(painter, sqa, sqb, col);
            arrow(painter, sqb, sqa, col);
            Some(label(painter, sqa + (sqb - sqa) * 0.5, text, col, editing))
        }
        Constraint::Radius { entity, .. } => {
            // Leader from the centre through the arc's (or circle's) midpoint.
            let (c, r, mid_ang) = match s.entity(*entity)? {
                Entity::Arc { id, .. } => {
                    let g = s.arc_geometry(*id)?;
                    (g.center, g.radius, g.start_angle + 0.5 * g.sweep)
                }
                Entity::Circle { center, radius, .. } => {
                    (s.point_xy(*center)?, *radius, std::f64::consts::FRAC_PI_4)
                }
                _ => return None,
            };
            let (u, v) = (mid_ang.cos(), mid_ang.sin());
            let rim = [c[0] + r * u, c[1] + r * v];
            let off = DIM_OFFSET_PX / pr.px_per_mm(rim);
            let out = [rim[0] + off * u, rim[1] + off * v];
            let (sc, sr, so) = (pr.screen(c)?, pr.screen(rim)?, pr.screen(out)?);
            painter.line_segment([sc, so], stroke);
            arrow(painter, sr, sc, col);
            let half = 4.0 * text.chars().count() as f32;
            let dir = (so - sc).normalized();
            Some(label(painter, so + dir * (half + 6.0), text, col, editing))
        }
        Constraint::Diameter { circle, .. } => {
            let Some(Entity::Circle { center, radius, .. }) = s.entity(*circle) else {
                return None;
            };
            let cxy = s.point_xy(*center)?;
            let k = std::f64::consts::FRAC_1_SQRT_2;
            let rim = [cxy[0] + radius * k, cxy[1] + radius * k];
            let off = DIM_OFFSET_PX / pr.px_per_mm(rim);
            let out = [rim[0] + off * k, rim[1] + off * k];
            let opp = [cxy[0] - radius * k, cxy[1] - radius * k];
            let (sr, so, sopp) = (pr.screen(rim)?, pr.screen(out)?, pr.screen(opp)?);
            painter.line_segment([sopp, so], stroke);
            arrow(painter, sr, so, col);
            arrow(painter, sopp, sr, col);
            // Label beside the end of the leader (approximate half width).
            let half = 4.0 * text.chars().count() as f32;
            Some(label(
                painter,
                so + egui::vec2(half + 6.0, 0.0),
                text,
                col,
                editing,
            ))
        }
        Constraint::Angle { a, b, .. } => {
            // Label between the two lines' midpoints, with leaders to each.
            let mid_of = |l: &EntityId| -> Option<[f64; 2]> {
                let Some(Entity::Line { a, b, .. }) = s.entity(*l) else {
                    return None;
                };
                let (p, q) = (s.point_xy(*a)?, s.point_xy(*b)?);
                Some([0.5 * (p[0] + q[0]), 0.5 * (p[1] + q[1])])
            };
            let (ma, mb) = (mid_of(a)?, mid_of(b)?);
            let (sa, sb) = (pr.screen(ma)?, pr.screen(mb)?);
            let at = sa + (sb - sa) * 0.5;
            let dashed = Stroke::new(0.8, col);
            for g in [sa, sb] {
                painter.extend(egui::Shape::dashed_line(&[g, at], dashed, 4.0, 3.0));
            }
            Some(label(painter, at, text, col, editing))
        }
        _ => None,
    }
}

/// Sketch plane: translucent patch + local axes through the sketch origin.
pub fn paint_plane(painter: &egui::Painter, pr: &Projector, half_extent: f64) {
    let e = half_extent;
    let corners: Vec<Pos2> = [[-e, -e], [e, -e], [e, e], [-e, e]]
        .iter()
        .filter_map(|p| pr.screen(*p))
        .collect();
    if corners.len() == 4 {
        painter.add(egui::Shape::convex_polygon(
            corners.clone(),
            PALETTE.plane,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(50, 80, 115, 110)),
        ));
    }
    let axis = |to: [f64; 2], c: Color32| {
        if let (Some(o), Some(t)) = (pr.screen([0.0, 0.0]), pr.screen(to)) {
            painter.line_segment([o, t], Stroke::new(1.5, c));
        }
    };
    axis([e, 0.0], Color32::from_rgb(220, 70, 70));
    axis([0.0, e], Color32::from_rgb(80, 200, 80));
}

/// Dashed preview of the open offset entry's copy (new curves only).
fn paint_offset_preview(painter: &egui::Painter, pr: &Projector, ed: &SketchEditor) {
    let Some(Ok((s, ids))) = ed.offset_preview() else {
        return;
    };
    let stroke = Stroke::new(1.8, PALETTE.preview);
    let arc_pts = |c: [f64; 2], r: f64, a0: f64, sweep: f64| -> Vec<Pos2> {
        let n = ((sweep.abs() / 0.1).ceil() as usize).clamp(4, 128);
        (0..=n)
            .filter_map(|k| {
                let a = a0 + sweep * k as f64 / n as f64;
                pr.screen([c[0] + r * a.cos(), c[1] + r * a.sin()])
            })
            .collect()
    };
    for id in ids {
        let pts: Vec<Pos2> = match s.entity(id) {
            Some(Entity::Line { a, b, .. }) => [*a, *b]
                .iter()
                .filter_map(|p| s.point_xy(*p).and_then(|q| pr.screen(q)))
                .collect(),
            Some(Entity::Circle { center, radius, .. }) => s
                .point_xy(*center)
                .map(|c| arc_pts(c, *radius, 0.0, std::f64::consts::TAU))
                .unwrap_or_default(),
            Some(Entity::Arc { .. }) => s
                .arc_geometry(id)
                .map(|g| arc_pts(g.center, g.radius, g.start_angle, g.sweep))
                .unwrap_or_default(),
            Some(e @ (Entity::Ellipse { .. } | Entity::Spline { .. })) => {
                rcad_sketch::curves::entity_polyline(&s, e, CURVE_SPAN_SEGMENTS)
                    .map(|v| v.into_iter().filter_map(|q| pr.screen(q)).collect())
                    .unwrap_or_default()
            }
            None => vec![],
        };
        if pts.len() >= 2 {
            painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
        }
    }
}

/// Dashed preview of the open fillet entry's arc.
fn paint_fillet_preview(painter: &egui::Painter, pr: &Projector, ed: &SketchEditor) {
    let Some(Ok((s, arc))) = ed.fillet_preview() else {
        return;
    };
    let Some(g) = s.arc_geometry(arc) else {
        return;
    };
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .filter_map(|k| {
            let a = g.start_angle + g.sweep * k as f64 / n as f64;
            pr.screen([
                g.center[0] + g.radius * a.cos(),
                g.center[1] + g.radius * a.sin(),
            ])
        })
        .collect();
    if pts.len() >= 2 {
        painter.extend(egui::Shape::dashed_line(
            &pts,
            Stroke::new(1.8, PALETTE.preview),
            6.0,
            4.0,
        ));
    }
}

/// Rubber-band preview and snap cue for the active tool.
pub fn paint_preview(painter: &egui::Painter, pr: &Projector, ed: &SketchEditor) {
    paint_offset_preview(painter, pr, ed);
    paint_fillet_preview(painter, pr, ed);
    let Some(cur) = ed.cursor else {
        return;
    };
    let Some(sc) = pr.screen(cur.pos) else {
        return;
    };
    let col = PALETTE.preview;
    // Snap cue.
    match cur.kind {
        SnapKind::Point(_) => {
            painter.circle_stroke(sc, 7.0, Stroke::new(2.0, col));
        }
        SnapKind::Grid if ed.tool != SketchTool::Select => {
            painter.circle_filled(sc, 2.5, col);
        }
        _ => {}
    }
    if let Some(from) = ed.rubber_band_from()
        && let Some(sf) = pr.screen(from)
    {
        let dashed = |pts: Vec<Pos2>| {
            painter.extend(egui::Shape::dashed_line(
                &pts,
                Stroke::new(1.5, col),
                7.0,
                4.0,
            ));
        };
        match ed.tool {
            SketchTool::Line => {
                dashed(vec![sf, sc]);
                let len = (cur.pos[0] - from[0]).hypot(cur.pos[1] - from[1]);
                let tag = match cur.align {
                    Some(crate::sketch_edit::Align::Horizontal) => "  H",
                    Some(crate::sketch_edit::Align::Vertical) => "  V",
                    None => "",
                };
                label(
                    painter,
                    sc + egui::vec2(40.0, 18.0),
                    format!("{} mm{tag}", rcad_sketch::fmt_mm(len)),
                    col,
                    false,
                );
            }
            SketchTool::CenterRectangle | SketchTool::CornerRectangle => {
                let (lo, hi) = if ed.tool == SketchTool::CenterRectangle {
                    let h = [(cur.pos[0] - from[0]).abs(), (cur.pos[1] - from[1]).abs()];
                    (
                        [from[0] - h[0], from[1] - h[1]],
                        [from[0] + h[0], from[1] + h[1]],
                    )
                } else {
                    (
                        [from[0].min(cur.pos[0]), from[1].min(cur.pos[1])],
                        [from[0].max(cur.pos[0]), from[1].max(cur.pos[1])],
                    )
                };
                let pts: Vec<Pos2> = [
                    [lo[0], lo[1]],
                    [hi[0], lo[1]],
                    [hi[0], hi[1]],
                    [lo[0], hi[1]],
                    [lo[0], lo[1]],
                ]
                .iter()
                .filter_map(|p| pr.screen(*p))
                .collect();
                dashed(pts);
                label(
                    painter,
                    sc + egui::vec2(60.0, 18.0),
                    format!(
                        "{} × {} mm",
                        rcad_sketch::fmt_mm(hi[0] - lo[0]),
                        rcad_sketch::fmt_mm(hi[1] - lo[1])
                    ),
                    col,
                    false,
                );
            }
            SketchTool::Circle => {
                let r = (cur.pos[0] - from[0]).hypot(cur.pos[1] - from[1]);
                let pts: Vec<Pos2> = (0..=72)
                    .filter_map(|i| {
                        let t = std::f64::consts::TAU * f64::from(i) / 72.0;
                        pr.screen([from[0] + r * t.cos(), from[1] + r * t.sin()])
                    })
                    .collect();
                dashed(pts);
                label(
                    painter,
                    sc + egui::vec2(50.0, 18.0),
                    format!("Ø{} mm", rcad_sketch::fmt_mm(2.0 * r)),
                    col,
                    false,
                );
            }
            SketchTool::Arc => match ed.arc_ends() {
                None => dashed(vec![sf, sc]),
                Some((a, b)) => match rcad_sketch::arc_through_3_points(a, b, cur.pos) {
                    Some((center, r, ccw)) => {
                        let (from, to) = if ccw { (a, b) } else { (b, a) };
                        let a0 = (from[1] - center[1]).atan2(from[0] - center[0]);
                        let a1 = (to[1] - center[1]).atan2(to[0] - center[0]);
                        let g = rcad_sketch::ArcGeometry {
                            center,
                            radius: r,
                            start_angle: a0,
                            sweep: (a1 - a0).rem_euclid(std::f64::consts::TAU),
                        };
                        let pts: Vec<Pos2> = arc_points(&g, 64)
                            .into_iter()
                            .filter_map(|q| pr.screen(q))
                            .collect();
                        dashed(pts);
                        label(
                            painter,
                            sc + egui::vec2(50.0, 18.0),
                            format!("R{} mm", rcad_sketch::fmt_mm(r)),
                            col,
                            false,
                        );
                    }
                    None => {
                        if let (Some(pa), Some(pb)) = (pr.screen(a), pr.screen(b)) {
                            dashed(vec![pa, pb]);
                        }
                    }
                },
            },
            SketchTool::Dimension => dashed(vec![sf, sc]),
            SketchTool::Select
            | SketchTool::Trim
            | SketchTool::Extend
            | SketchTool::Break
            | SketchTool::Project
            | SketchTool::Intersect
            | SketchTool::Offset
            | SketchTool::Fillet
            | SketchTool::Text => {}
            SketchTool::Slot | SketchTool::Polygon | SketchTool::Ellipse | SketchTool::Spline => {
                if let Some(v) = ed.shape_preview() {
                    let pts: Vec<Pos2> = v.iter().filter_map(|p| pr.screen(*p)).collect();
                    dashed(pts);
                } else {
                    dashed(vec![sf, sc]);
                }
            }
        }
    }
}
