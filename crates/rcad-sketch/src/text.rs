//! Sketch text: a sketch-level item whose glyph outlines become closed profile
//! loops (docs/behavior/sketching.md, "Sketch text").
//!
//! # Model
//! A [`SketchText`] stores the string, the requested font ([`FontSpec`]), its
//! placement (height, anchor, rotation, alignment) and the **resolved outlines**
//! ([`ResolvedText`]): every glyph's contours as the font's exact quadratic /
//! cubic Bézier segments in font units, the pen positions, and the font's
//! identity (family, style, file name, FNV-1a 64 content hash). Regeneration,
//! reload and placement edits (height, position, rotation, alignment) use only
//! the stored outlines; the font file is needed only to change the string or
//! the font ([`Sketch::edit_text`]). There is no silent font substitution.
//!
//! # Height
//! `height` (mm) is the font's **cap height**: OS/2 `sCapHeight`, or the top of
//! the `H` glyph when the font declares none. A capital `H` or `I` of a
//! 10 mm text is 10 mm tall; descenders and accents extend beyond it.
//!
//! # Placement
//! Font units are scaled by `height / cap_height`. The anchor (a sketch point
//! when `anchor` is set, else `position`) is the reference point of the
//! alignment box: horizontally the pen start (`Left`), the middle (`Center`) or
//! the end (`Right`) of the total advance; vertically the baseline, half the cap
//! height (`Middle`) or the cap height (`Top`). `rotation` (radians,
//! counter-clockwise) turns the text about the anchor.
//!
//! # Flattening (tolerance, docs/TOLERANCES.md)
//! Outlines become closed polygons. Each Bézier is split into `n` equal
//! parameter steps with `n` from the classical bound on linear interpolation,
//! `|B(t) − chord| <= h²/8 · max|B''|` with `h = 1/n`, so the chord error is at
//! most [`TEXT_CHORD_TOL`]. Vertices closer than [`TEXT_MIN_EDGE`] to the
//! previous kept vertex are then dropped (moving the outline by less than that
//! distance), so no polygon edge is shorter than the kernel's minimum length;
//! exactly collinear vertices (within 1e-9 mm) are merged. The polygon therefore
//! lies within [`TEXT_OUTLINE_TOL`] = chord + edge tolerance of the exact font
//! outline. Text solids are exact extrusions of these polygons, **not** of the
//! font's curves.
//!
//! # Holes and nesting
//! Counters (the holes of `B`, `8`, `O`) are found by the region code by
//! geometric containment, not by the font's contour direction: every contour
//! becomes a counter-clockwise loop and [`crate::Sketch::regions`] nests loops
//! by point-in-loop tests. Contours that self-intersect, or that touch or cross
//! other loops (overlapping contours, as in some variable fonts, or letters
//! that touch), are refused with an error naming the glyph. Zero-area contours
//! (area within the edge tolerance) are dropped.
//!
//! # Identity
//! Each polygon edge has a synthetic [`EntityId`] with bit 31 set (no ordinary
//! entity uses it): text id, glyph index (position in the string), contour index
//! and edge index ([`text_edge_id`]). [`crate::RegionRef`] reduces a text loop
//! to its contour id (edge bits cleared), so a region keeps its identity across
//! solves, moves, height/rotation edits (which change the edge count) and string
//! edits that keep the character's position and contour count. Side faces of
//! extruded text are named by edge id and break when the edge count changes.

use crate::profile::{Prim, loop_self_intersects};
use crate::{EntityId, PointId, Sketch, SketchError};
use serde::{Deserialize, Serialize};

/// Chord tolerance of Bézier flattening, mm.
pub const TEXT_CHORD_TOL: f64 = 0.005;
/// Shortest polygon edge kept, mm (vertices closer to the previous one are dropped).
pub const TEXT_MIN_EDGE: f64 = 0.005;
/// Bound on the distance between a text polygon and the exact font outline, mm.
pub const TEXT_OUTLINE_TOL: f64 = TEXT_CHORD_TOL + TEXT_MIN_EDGE;
/// Smallest accepted text height (cap height), mm.
pub const MIN_TEXT_HEIGHT: f64 = 0.1;
/// Largest accepted text height, mm.
pub const MAX_TEXT_HEIGHT: f64 = 10_000.0;
/// Text items per sketch (ids 1..=MAX_TEXTS).
pub const MAX_TEXTS: u32 = 63;
/// Characters per text item.
pub const MAX_GLYPHS: usize = 512;
/// Contours per glyph.
pub const MAX_CONTOURS: usize = 16;
/// Polygon edges per contour, and Bézier segments per stored contour.
pub const MAX_EDGES: usize = 4096;

/// Bit 31 marks a synthetic text edge id.
pub const TEXT_ID_BIT: u32 = 1 << 31;
const TEXT_SHIFT: u32 = 25;
const GLYPH_SHIFT: u32 = 16;
const CONTOUR_SHIFT: u32 = 12;

/// Synthetic entity id of edge `edge` of contour `contour` of glyph `glyph` of
/// text `text`, or `None` beyond the limits.
pub fn text_edge_id(text: u32, glyph: usize, contour: usize, edge: usize) -> Option<EntityId> {
    if text == 0
        || text > MAX_TEXTS
        || glyph >= MAX_GLYPHS
        || contour >= MAX_CONTOURS
        || edge >= MAX_EDGES
    {
        return None;
    }
    Some(EntityId(
        TEXT_ID_BIT
            | (text << TEXT_SHIFT)
            | ((glyph as u32) << GLYPH_SHIFT)
            | ((contour as u32) << CONTOUR_SHIFT)
            | edge as u32,
    ))
}

/// `(text, glyph, contour, edge)` of a synthetic text edge id.
pub fn text_edge_parts(id: EntityId) -> Option<(u32, usize, usize, usize)> {
    if id.0 & TEXT_ID_BIT == 0 {
        return None;
    }
    let v = id.0;
    Some((
        (v >> TEXT_SHIFT) & 0x3f,
        ((v >> GLYPH_SHIFT) & 0x1ff) as usize,
        ((v >> CONTOUR_SHIFT) & 0xf) as usize,
        (v & 0xfff) as usize,
    ))
}

/// The contour-level id of a text edge id (edge bits cleared); other ids unchanged.
pub fn text_loop_key(id: EntityId) -> EntityId {
    if id.0 & TEXT_ID_BIT != 0 {
        EntityId(id.0 & !0xfff)
    } else {
        id
    }
}

/// Requested font: an installed family (with bold/italic) or an explicit file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct FontSpec {
    /// Family name, e.g. "Liberation Sans" (case-insensitive). May be empty
    /// when `path` is given.
    pub family: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Explicit font file (overrides the family search).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl FontSpec {
    pub fn family(name: &str) -> Self {
        FontSpec {
            family: name.into(),
            ..Default::default()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Baseline,
    Middle,
    Top,
}

/// One outline segment in font units, from the previous segment's end.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum OutlineSeg {
    Line {
        to: [f64; 2],
    },
    Quad {
        c: [f64; 2],
        to: [f64; 2],
    },
    Cubic {
        c1: [f64; 2],
        c2: [f64; 2],
        to: [f64; 2],
    },
}

impl OutlineSeg {
    fn to(&self) -> [f64; 2] {
        match self {
            OutlineSeg::Line { to }
            | OutlineSeg::Quad { to, .. }
            | OutlineSeg::Cubic { to, .. } => *to,
        }
    }
}

/// A closed glyph contour (implicitly closed back to `start`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GlyphContour {
    pub start: [f64; 2],
    pub segs: Vec<OutlineSeg>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResolvedGlyph {
    pub ch: char,
    /// Pen position (font units) of the glyph origin.
    pub x: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contours: Vec<GlyphContour>,
}

/// The font a text was resolved with.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    pub family: String,
    pub style: String,
    /// File name (no directory) of the font file.
    pub file: String,
    /// Face index in a collection file.
    #[serde(default)]
    pub index: u32,
    /// `fnv1a64:<hex>` of the font file's bytes.
    pub hash: String,
    pub units_per_em: f64,
    /// Cap height, font units (see the module docs).
    pub cap_height: f64,
}

/// Outlines and metrics captured from the font when the string was set.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ResolvedText {
    pub font: ResolvedFont,
    /// Total advance width, font units.
    pub advance: f64,
    pub glyphs: Vec<ResolvedGlyph>,
}

/// User-editable text parameters (commands and the editor dialog).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextSpec {
    pub text: String,
    pub font: FontSpec,
    /// Cap height, mm.
    pub height: f64,
    /// Anchor position (sketch mm), used when `anchor` is `None`.
    pub position: [f64; 2],
    /// Sketch point the anchor follows (solved position), if bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PointId>,
    /// Counter-clockwise, radians.
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub halign: HAlign,
    #[serde(default)]
    pub valign: VAlign,
}

impl TextSpec {
    pub fn new(text: &str, family: &str, height: f64, position: [f64; 2]) -> Self {
        TextSpec {
            text: text.into(),
            font: FontSpec::family(family),
            height,
            position,
            anchor: None,
            rotation: 0.0,
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        }
    }
}

/// A text item of a sketch (see the module docs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SketchText {
    /// 1..=[`MAX_TEXTS`], unique in the sketch.
    pub id: u32,
    #[serde(flatten)]
    pub spec: TextSpec,
    pub resolved: ResolvedText,
}

/// One closed text loop in sketch coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLoop {
    pub text: u32,
    pub glyph: usize,
    pub contour: usize,
    pub ch: char,
    /// Counter-clockwise polygon starting at its lowest (then leftmost) vertex.
    pub polygon: Vec<[f64; 2]>,
    /// Synthetic id of edge `i` (vertex `i` to `i + 1`).
    pub edges: Vec<EntityId>,
}

/// Number of equal parameter steps for a chord error <= `tol` (see the module docs).
fn steps(second_derivative_bound: f64, tol: f64) -> usize {
    let n = (second_derivative_bound / (8.0 * tol)).sqrt().ceil();
    if n.is_finite() {
        (n as usize).clamp(1, MAX_EDGES)
    } else {
        MAX_EDGES
    }
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn norm(v: [f64; 2]) -> f64 {
    v[0].hypot(v[1])
}

/// Flatten a quadratic Bézier `p0 c p1` (mm) into points after `p0` (ending at
/// `p1`) with chord error <= `tol`: `B'' = 2 (p0 − 2c + p1)`.
pub fn flatten_quad(p0: [f64; 2], c: [f64; 2], p1: [f64; 2], tol: f64) -> Vec<[f64; 2]> {
    let d = [p0[0] - 2.0 * c[0] + p1[0], p0[1] - 2.0 * c[1] + p1[1]];
    let n = steps(2.0 * norm(d), tol);
    (1..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            if i == n {
                return p1;
            }
            lerp(lerp(p0, c, t), lerp(c, p1, t), t)
        })
        .collect()
}

/// Flatten a cubic Bézier (mm): `|B''| <= 6 max(|p0 − 2c1 + c2|, |c1 − 2c2 + p1|)`.
pub fn flatten_cubic(
    p0: [f64; 2],
    c1: [f64; 2],
    c2: [f64; 2],
    p1: [f64; 2],
    tol: f64,
) -> Vec<[f64; 2]> {
    let d0 = [p0[0] - 2.0 * c1[0] + c2[0], p0[1] - 2.0 * c1[1] + c2[1]];
    let d1 = [c1[0] - 2.0 * c2[0] + p1[0], c1[1] - 2.0 * c2[1] + p1[1]];
    let n = steps(6.0 * norm(d0).max(norm(d1)), tol);
    (1..=n)
        .map(|i| {
            if i == n {
                return p1;
            }
            let t = i as f64 / n as f64;
            let (a, b, c) = (lerp(p0, c1, t), lerp(c1, c2, t), lerp(c2, p1, t));
            let (d, e) = (lerp(a, b, t), lerp(b, c, t));
            lerp(d, e, t)
        })
        .collect()
}

/// Drop vertices within `min_edge` of the previous kept vertex (and a closing
/// vertex within `min_edge` of the first), then exactly collinear vertices.
fn clean_polygon(pts: Vec<[f64; 2]>, min_edge: f64) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        if v.last()
            .is_none_or(|q| norm([p[0] - q[0], p[1] - q[1]]) >= min_edge)
        {
            v.push(p);
        }
    }
    while v.len() > 1 {
        let (f, l) = (v[0], v[v.len() - 1]);
        if norm([f[0] - l[0], f[1] - l[1]]) < min_edge {
            v.pop();
        } else {
            break;
        }
    }
    // Collinear merge: distance of a vertex from its neighbours' chord <= 1e-9 mm.
    let mut changed = true;
    while changed && v.len() > 3 {
        changed = false;
        let mut out: Vec<[f64; 2]> = Vec::with_capacity(v.len());
        let k = v.len();
        for i in 0..k {
            let a = out.last().copied().unwrap_or(v[(i + k - 1) % k]);
            let (b, c) = (v[i], v[(i + 1) % k]);
            let ac = [c[0] - a[0], c[1] - a[1]];
            let l = norm(ac);
            let cross = (b[0] - a[0]) * ac[1] - (b[1] - a[1]) * ac[0];
            let dot = (b[0] - a[0]) * ac[0] + (b[1] - a[1]) * ac[1];
            // Only between its neighbours (a fold-back spike is not "collinear").
            if l > 0.0 && (cross / l).abs() <= 1e-9 && dot > 0.0 && dot < l * l {
                changed = true;
                continue;
            }
            out.push(b);
        }
        v = out;
    }
    v
}

fn signed_area(v: &[[f64; 2]]) -> f64 {
    let k = v.len();
    (0..k)
        .map(|i| {
            let (p, q) = (v[i], v[(i + 1) % k]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

impl SketchText {
    /// Anchor position in sketch mm (the bound point's current coordinates).
    pub fn anchor_xy(&self, s: &Sketch) -> Result<[f64; 2], SketchError> {
        match self.spec.anchor {
            Some(p) => s.point_xy(p).ok_or(SketchError::MissingPoint(p)),
            None => Ok(self.spec.position),
        }
    }

    /// Font units → sketch mm.
    pub fn transform(&self, s: &Sketch) -> Result<impl Fn([f64; 2]) -> [f64; 2], SketchError> {
        let o = self.anchor_xy(s)?;
        let r = &self.resolved;
        let k = self.spec.height / r.font.cap_height;
        let dx = match self.spec.halign {
            HAlign::Left => 0.0,
            HAlign::Center => -r.advance / 2.0,
            HAlign::Right => -r.advance,
        };
        let dy = match self.spec.valign {
            VAlign::Baseline => 0.0,
            VAlign::Middle => -r.font.cap_height / 2.0,
            VAlign::Top => -r.font.cap_height,
        };
        let (sn, cs) = self.spec.rotation.sin_cos();
        Ok(move |p: [f64; 2]| {
            let (x, y) = ((p[0] + dx) * k, (p[1] + dy) * k);
            [o[0] + cs * x - sn * y, o[1] + sn * x + cs * y]
        })
    }
}

/// One flattened glyph contour: `(glyph index, contour index, points)`.
pub type GlyphPolyline = (usize, usize, Vec<[f64; 2]>);

impl SketchText {
    /// Flattened, cleaned contours (sketch mm, original direction) per glyph:
    /// `(glyph, contour, points)`. Zero-area contours are dropped.
    pub fn polylines(&self, s: &Sketch) -> Result<Vec<GlyphPolyline>, SketchError> {
        let tf = self.transform(s)?;
        let mut out = Vec::new();
        for (gi, g) in self.resolved.glyphs.iter().enumerate() {
            for (ci, c) in g.contours.iter().enumerate() {
                let at = |p: [f64; 2]| tf([p[0] + g.x, p[1]]);
                let mut pts = vec![at(c.start)];
                let mut cur = c.start;
                for seg in &c.segs {
                    let p0 = at(cur);
                    match seg {
                        OutlineSeg::Line { to } => pts.push(at(*to)),
                        OutlineSeg::Quad { c, to } => {
                            pts.extend(flatten_quad(p0, at(*c), at(*to), TEXT_CHORD_TOL))
                        }
                        OutlineSeg::Cubic { c1, c2, to } => {
                            pts.extend(flatten_cubic(p0, at(*c1), at(*c2), at(*to), TEXT_CHORD_TOL))
                        }
                    }
                    cur = seg.to();
                    if pts.len() > 4 * MAX_EDGES {
                        return Err(SketchError::Text(format!(
                            "text {}: glyph '{}' contour {ci} needs more than {MAX_EDGES} edges \
                             at this height",
                            self.id, g.ch
                        )));
                    }
                }
                let poly = clean_polygon(pts, TEXT_MIN_EDGE);
                let perimeter: f64 = (0..poly.len())
                    .map(|i| {
                        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                        norm([q[0] - p[0], q[1] - p[1]])
                    })
                    .sum();
                if poly.len() < 3 || signed_area(&poly).abs() <= TEXT_MIN_EDGE * perimeter / 2.0 {
                    continue;
                }
                if poly.len() > MAX_EDGES {
                    return Err(SketchError::Text(format!(
                        "text {}: glyph '{}' contour {ci} needs {} edges (limit {MAX_EDGES}) at \
                         this height",
                        self.id,
                        g.ch,
                        poly.len()
                    )));
                }
                out.push((gi, ci, poly));
            }
        }
        Ok(out)
    }

    /// The closed loops of this text: CCW polygons starting at the lowest (then
    /// leftmost) vertex, with synthetic edge ids. Self-intersecting contours are
    /// refused.
    pub fn loops(&self, s: &Sketch) -> Result<Vec<TextLoop>, SketchError> {
        let tol = crate::solve_tolerance(s.scale());
        let mut out = Vec::new();
        for (gi, ci, mut poly) in self.polylines(s)? {
            let ch = self.resolved.glyphs[gi].ch;
            if signed_area(&poly) < 0.0 {
                poly.reverse();
            }
            let start = (0..poly.len())
                .min_by(|&i, &j| {
                    (poly[i][1], poly[i][0])
                        .partial_cmp(&(poly[j][1], poly[j][0]))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0);
            poly.rotate_left(start);
            let geo: Vec<Prim> = (0..poly.len())
                .map(|i| Prim::Seg {
                    a: poly[i],
                    b: poly[(i + 1) % poly.len()],
                })
                .collect();
            if loop_self_intersects(&geo, tol) {
                return Err(SketchError::UnsupportedProfile(format!(
                    "text {} ('{}'): glyph '{ch}' contour {ci} intersects itself",
                    self.id,
                    self.text()
                )));
            }
            let edges = (0..poly.len())
                .map(|e| {
                    text_edge_id(self.id, gi, ci, e).ok_or_else(|| {
                        SketchError::Text(format!("text {}: glyph/contour limit exceeded", self.id))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            out.push(TextLoop {
                text: self.id,
                glyph: gi,
                contour: ci,
                ch,
                polygon: poly,
                edges,
            });
        }
        Ok(out)
    }

    pub fn text(&self) -> &str {
        &self.spec.text
    }

    fn validate(&self, s: &Sketch) -> Result<(), SketchError> {
        let bad = |why: String| SketchError::InvalidValue(format!("text {}: {why}", self.id));
        let sp = &self.spec;
        if self.id == 0 || self.id > MAX_TEXTS {
            return Err(bad(format!("id must be 1..={MAX_TEXTS}")));
        }
        if !(sp.height.is_finite() && (MIN_TEXT_HEIGHT..=MAX_TEXT_HEIGHT).contains(&sp.height)) {
            return Err(bad(format!(
                "height {} mm must be within {MIN_TEXT_HEIGHT}..={MAX_TEXT_HEIGHT} mm",
                sp.height
            )));
        }
        if !sp.rotation.is_finite() || !sp.position.iter().all(|v| v.is_finite()) {
            return Err(bad("non-finite position or rotation".into()));
        }
        if let Some(p) = sp.anchor
            && s.point(p).is_none()
        {
            return Err(SketchError::MissingPoint(p));
        }
        let r = &self.resolved;
        let chars: Vec<char> = sp.text.chars().collect();
        if chars.is_empty() || chars.len() > MAX_GLYPHS {
            return Err(bad(format!("text must have 1..={MAX_GLYPHS} characters")));
        }
        if r.glyphs.len() != chars.len() || r.glyphs.iter().zip(&chars).any(|(g, c)| g.ch != *c) {
            return Err(bad(
                "the stored outlines do not match the string; set the string again".into(),
            ));
        }
        let fin = |p: &[f64; 2]| {
            p[0].is_finite() && p[1].is_finite() && p[0].abs() < 1e7 && p[1].abs() < 1e7
        };
        if !(r.font.units_per_em.is_finite()
            && r.font.units_per_em > 0.0
            && r.font.cap_height.is_finite()
            && r.font.cap_height > 0.0
            && r.advance.is_finite())
        {
            return Err(bad("invalid font metrics".into()));
        }
        for g in &r.glyphs {
            if !g.x.is_finite() || g.contours.len() > MAX_CONTOURS {
                return Err(bad(format!(
                    "glyph '{}': invalid position or more than {MAX_CONTOURS} contours",
                    g.ch
                )));
            }
            for c in &g.contours {
                let ok = fin(&c.start)
                    && c.segs.len() <= MAX_EDGES
                    && c.segs.iter().all(|seg| match seg {
                        OutlineSeg::Line { to } => fin(to),
                        OutlineSeg::Quad { c, to } => fin(c) && fin(to),
                        OutlineSeg::Cubic { c1, c2, to } => fin(c1) && fin(c2) && fin(to),
                    });
                if !ok {
                    return Err(bad(format!("glyph '{}': invalid outline", g.ch)));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_texts(s: &Sketch) -> Result<(), SketchError> {
    let mut ids = std::collections::HashSet::new();
    for t in &s.texts {
        if !ids.insert(t.id) {
            return Err(SketchError::InvalidValue(format!(
                "duplicate text id {}",
                t.id
            )));
        }
        t.validate(s)?;
    }
    if !s.texts.is_empty()
        && let Some(e) = s.entities.iter().find(|e| e.id().0 & TEXT_ID_BIT != 0)
    {
        return Err(SketchError::InvalidValue(format!(
            "entity id {} is in the range reserved for text edges",
            e.id().0
        )));
    }
    Ok(())
}

fn check_spec(sp: &TextSpec) -> Result<(), SketchError> {
    if !(sp.height.is_finite() && (MIN_TEXT_HEIGHT..=MAX_TEXT_HEIGHT).contains(&sp.height)) {
        return Err(SketchError::Text(format!(
            "height {} mm must be within {MIN_TEXT_HEIGHT}..={MAX_TEXT_HEIGHT} mm",
            sp.height
        )));
    }
    if !sp.rotation.is_finite() || !sp.position.iter().all(|v| v.is_finite()) {
        return Err(SketchError::Text("non-finite position or rotation".into()));
    }
    if sp.text.trim().is_empty() {
        return Err(SketchError::Text("text is empty".into()));
    }
    Ok(())
}

impl Sketch {
    pub fn text_item(&self, id: u32) -> Option<&SketchText> {
        self.texts.iter().find(|t| t.id == id)
    }

    /// Add a text item: resolves the font now (installed fonts) and stores the
    /// outlines. Returns the new text id. The sketch is unchanged on error.
    pub fn add_text(&mut self, spec: TextSpec) -> Result<u32, SketchError> {
        check_spec(&spec)?;
        let resolved = crate::font::resolve(&spec.text, &spec.font)?;
        self.add_resolved_text(spec, resolved)
    }

    /// Add a text item with outlines resolved elsewhere (e.g. from font bytes
    /// with [`crate::font::resolve_with_data`]).
    pub fn add_resolved_text(
        &mut self,
        spec: TextSpec,
        resolved: ResolvedText,
    ) -> Result<u32, SketchError> {
        check_spec(&spec)?;
        let id = self.texts.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        if id > MAX_TEXTS {
            return Err(SketchError::Text(format!(
                "a sketch holds at most {MAX_TEXTS} text items"
            )));
        }
        let mut next = self.clone();
        next.texts.push(SketchText { id, spec, resolved });
        validate_texts(&next)?;
        next.texts.last().expect("pushed").loops(&next)?;
        *self = next;
        Ok(id)
    }

    /// Change a text item. A new string or font re-resolves the outlines (and
    /// needs the font installed: `FontNotFound` otherwise, text unchanged);
    /// placement edits reuse the stored outlines.
    pub fn edit_text(&mut self, id: u32, spec: TextSpec) -> Result<(), SketchError> {
        check_spec(&spec)?;
        let i = self
            .texts
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| SketchError::Text(format!("no text {id} in this sketch")))?;
        let old = &self.texts[i];
        let resolved = if old.spec.text != spec.text || old.spec.font != spec.font {
            crate::font::resolve(&spec.text, &spec.font)?
        } else {
            old.resolved.clone()
        };
        let mut next = self.clone();
        next.texts[i] = SketchText { id, spec, resolved };
        validate_texts(&next)?;
        next.texts[i].loops(&next)?;
        *self = next;
        Ok(())
    }

    /// Remove a text item; false if it does not exist.
    pub fn remove_text(&mut self, id: u32) -> bool {
        let n = self.texts.len();
        self.texts.retain(|t| t.id != id);
        self.texts.len() != n
    }

    /// Every closed text loop of the sketch (all text items, in id order).
    pub fn text_loops(&self) -> Result<Vec<TextLoop>, SketchError> {
        let mut out = Vec::new();
        for t in &self.texts {
            out.extend(t.loops(self)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance from `p` to the polyline `v` (open).
    fn dist_to_polyline(p: [f64; 2], v: &[[f64; 2]]) -> f64 {
        v.windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let d = [b[0] - a[0], b[1] - a[1]];
                let l2 = d[0] * d[0] + d[1] * d[1];
                let t = if l2 > 0.0 {
                    (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                norm([p[0] - a[0] - t * d[0], p[1] - a[1] - t * d[1]])
            })
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn quad_flattening_respects_the_chord_bound() {
        let (p0, c, p1) = ([0.0, 0.0], [5.0, 12.0], [10.0, 0.0]);
        for tol in [0.1, 0.01, 0.001] {
            let mut v = vec![p0];
            v.extend(flatten_quad(p0, c, p1, tol));
            assert_eq!(*v.last().unwrap(), p1);
            let mut worst: f64 = 0.0;
            for i in 0..=4000 {
                let t = i as f64 / 4000.0;
                let q = lerp(lerp(p0, c, t), lerp(c, p1, t), t);
                worst = worst.max(dist_to_polyline(q, &v));
            }
            assert!(worst <= tol, "tol {tol}: worst {worst}");
            // Not grossly over-refined: within 2x of the bound's own count.
            assert!(
                worst > tol / 8.0,
                "tol {tol}: worst {worst} (n = {})",
                v.len()
            );
        }
    }

    #[test]
    fn cubic_flattening_respects_the_chord_bound() {
        let (p0, c1, c2, p1) = ([0.0, 0.0], [0.0, 20.0], [30.0, -20.0], [30.0, 0.0]);
        for tol in [0.05, 0.005] {
            let mut v = vec![p0];
            v.extend(flatten_cubic(p0, c1, c2, p1, tol));
            let mut worst: f64 = 0.0;
            for i in 0..=8000 {
                let t = i as f64 / 8000.0;
                let (a, b, c) = (lerp(p0, c1, t), lerp(c1, c2, t), lerp(c2, p1, t));
                let q = lerp(lerp(a, b, t), lerp(b, c, t), t);
                worst = worst.max(dist_to_polyline(q, &v));
            }
            assert!(worst <= tol, "tol {tol}: worst {worst}");
        }
    }

    #[test]
    fn edge_ids_round_trip_and_reduce_to_contour_keys() {
        let id = text_edge_id(5, 300, 15, 4095).unwrap();
        assert_eq!(text_edge_parts(id), Some((5, 300, 15, 4095)));
        assert_eq!(text_loop_key(id), text_edge_id(5, 300, 15, 0).unwrap());
        assert!(text_edge_id(64, 0, 0, 0).is_none());
        assert!(text_edge_id(1, 0, 16, 0).is_none());
        assert_eq!(text_loop_key(EntityId(7)), EntityId(7));
    }

    #[test]
    fn cleaning_drops_short_edges_and_collinear_points_but_not_spikes() {
        let v = clean_polygon(
            vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [2.0, 0.0],
                [2.0, 0.001],
                [2.0, 2.0],
                [0.0, 2.0],
                [0.0, 0.002],
            ],
            0.005,
        );
        assert_eq!(v, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]);
    }
}
