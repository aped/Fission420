//! SVG planar paths: import to an unconstrained sketch, export of a sketch.
//! Supported subset, units and losses: `docs/formats/svg.md`.
//!
//! # Import rules
//! * Elements: `line`, `rect` (no rounded corners), `circle`, `ellipse` with equal radii,
//!   `polyline`, `polygon`, `path` with `M L H V Z A` (absolute and relative; arcs only
//!   circular, `rx == ry` to 1e-6 relative). `g` carries transforms. Cubic/quadratic
//!   Béziers (`C S Q T`), elliptical arcs, `use`, `text`, `image`, nested `svg`,
//!   non-similarity transforms on circles/arcs and lengths in relative units (`%`, `em`,
//!   `ex`, `rem`, `ch`, `vw`, `vh`, `vmin`, `vmax`, `auto`) are unsupported: reported per
//!   element type and refused unless `allow_partial` (then that element is skipped and
//!   listed). `defs`, `symbol`, `clipPath`, `mask`, `pattern`, `marker`, `style`,
//!   `script`, `metadata`, `title`, `desc` contents and every element in a non-SVG
//!   namespace (Inkscape's `sodipodi:`/`inkscape:` data, RDF) are ignored silently.
//! * Transforms: `matrix`, `translate`, `scale`, `rotate` (with optional centre), `skewX`,
//!   `skewY`, composed per the SVG rules. Lines take any affine map; circles and arcs only
//!   similarity maps (uniform scale, rotation, reflection, translation) to 1e-6 relative.
//!   A non-identity `transform` on the root `svg` is refused: it applies in the
//!   embedding page's CSS space, outside the user coordinates this import keeps.
//! * Units: the root `width`/`height` (> 0) with an absolute unit (`mm`, `cm`, `in`,
//!   `pt`, `pc`) and `viewBox` give millimetres per user unit (`preserveAspectRatio`:
//!   any alignment with `meet` takes the smaller ratio, `slice` the larger, `none` only
//!   with equal ratios). A missing or `auto` width or height follows the other through
//!   the viewBox aspect ratio. Only the scale is applied: user coordinates keep their
//!   origin (the viewBox offset and the centering only frame the page), so a drawing's
//!   coordinates survive export/import. Without a physical size (no width/height, `px`,
//!   unitless or `%`) the file declares no unit: the caller must give
//!   `unit_override_to_mm` (mm per user unit), else [`ExchangeError::UnitsRequired`].
//!   Coordinates with units inside the drawing are converted to user units with CSS
//!   ratios (96 px per inch).
//! * Orientation: SVG's y axis points down; the sketch's points up. Sketch coordinates are
//!   `(x, −y)` of the viewport (mm), so the drawing reads the same looking at the sketch
//!   plane from its normal. Arcs are re-oriented to the sketch convention
//!   (counter-clockwise from start to end) after the full transform.
//! * Endpoints within `join_tolerance_mm` share a sketch point; nothing is constrained
//!   except the origin. The sketch is solved once (under-constrained), like DXF import.
//!
//! # Export
//! The solved sketch's lines, arcs and circles (construction geometry is not written)
//! in its local 2D coordinates: `width`/`height` in mm, `viewBox` in mm, y negated.

use crate::dxf::{EntityCount, FastIds, PointMerger, open_endpoints, push_point};
use crate::{ExchangeError, Result, UnsupportedEntities, check_scale};
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use rcad_sketch::{Entity, Sketch, SolveReport, SolveStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

const FMT: &str = "SVG";
/// CSS absolute units in user units (px).
const PX_PER_MM: f64 = 96.0 / 25.4;
/// Relative tolerance for "equal radii" (`rx`/`ry`) and "similarity transform":
/// files round to 6-8 significant digits (6 digits: up to ~4e-6 apart, e.g.
/// 0.264583 vs 0.264584). Near-equal radii use their mean (shape error at most
/// half this, 0.5 um at r = 100 mm); arc centres are re-derived so both
/// endpoints stay exactly on the arc.
const SIMILAR_REL: f64 = 1e-5;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SvgImportOptions {
    /// Skip unsupported elements (listed in [`SvgImport::skipped`]) instead of failing.
    pub allow_partial: bool,
    /// Millimetres per user unit, used only when the file declares no physical size.
    pub unit_override_to_mm: Option<f64>,
    /// Endpoint join tolerance in mm (finite, >= 0).
    pub join_tolerance_mm: f64,
    pub max_bytes: u64,
    /// Maximum number of XML elements.
    pub max_elements: u64,
}

impl Default for SvgImportOptions {
    fn default() -> Self {
        Self {
            allow_partial: false,
            unit_override_to_mm: None,
            join_tolerance_mm: 1e-6,
            max_bytes: 64 * 1024 * 1024,
            max_elements: 200_000,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SvgUnits {
    /// e.g. "mm (root width 100mm, viewBox 0 0 100 50)" or "user override".
    pub name: String,
    /// Millimetres per user unit at the root.
    pub to_mm: f64,
    /// True when taken from the root's physical width/height.
    pub declared: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SvgImport {
    pub sketch: Sketch,
    pub units: SvgUnits,
    /// Imported elements by type.
    pub imported: Vec<EntityCount>,
    /// Unsupported (with `allow_partial`) and degenerate elements.
    pub skipped: Vec<UnsupportedEntities>,
    pub warnings: Vec<String>,
    pub solve: SolveReport,
}

/// 2D affine map `(x, y) -> (a x + c y + e, b x + d y + f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f64; 6]);

impl Affine {
    const ID: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn then(self, outer: Affine) -> Affine {
        // outer ∘ self
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = outer.0;
        Affine([
            p * a + r * b,
            q * a + s * b,
            p * c + r * d,
            q * c + s * d,
            p * e + r * f + t,
            q * e + s * f + u,
        ])
    }

    fn apply(&self, x: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x[0] + c * x[1] + e, b * x[0] + d * x[1] + f]
    }

    /// Uniform scale factor when this is a similarity up to [`SIMILAR_REL`]
    /// (rounded real-world factors such as Inkscape's 0.26458333 px -> mm):
    /// the square root of |det|, the geometric mean of the two axis scales.
    fn similarity_scale(&self) -> Option<f64> {
        let [a, b, c, d, ..] = self.0;
        // Column lengths equal and columns orthogonal, both relative to the
        // lengths themselves (not their squares, which doubles the error).
        let (l1, l2) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
        let tol = SIMILAR_REL * l1.max(l2);
        ((l1 - l2).abs() <= tol && (a * c + b * d).abs() <= tol * l1.min(l2) && l1 > 0.0)
            .then(|| (a * d - b * c).abs().sqrt())
    }
}

/// Curve in sketch millimetres (after all transforms and the y flip).
#[derive(Clone, Copy, Debug)]
enum Curve {
    Line([f64; 2], [f64; 2]),
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
    },
    Circle([f64; 2], f64),
}

#[derive(Default)]
struct Report {
    imported: BTreeMap<String, usize>,
    unsupported: BTreeMap<(String, String), usize>,
    degenerate: BTreeMap<(String, String), usize>,
}

impl Report {
    fn unsupported(&mut self, elem: &str, why: &str) {
        *self
            .unsupported
            .entry((elem.to_string(), why.to_string()))
            .or_default() += 1;
    }
    fn degenerate(&mut self, elem: &str, why: &str) {
        *self
            .degenerate
            .entry((elem.to_string(), why.to_string()))
            .or_default() += 1;
    }
}

fn malformed(msg: impl Into<String>) -> ExchangeError {
    ExchangeError::Malformed {
        format: FMT.into(),
        line: None,
        message: msg.into(),
    }
}

/// CSS units that need a layout context (font size, viewport, parent size).
const RELATIVE_UNITS: &[&str] = &["%", "em", "ex", "rem", "ch", "vw", "vh", "vmin", "vmax"];

/// Why a length could not be converted.
#[derive(Debug, PartialEq)]
enum LenError {
    /// A relative unit or `auto`: the element is skipped as unsupported.
    Relative(String),
    /// Not a length at all: the file is malformed.
    Malformed(String),
}

/// A length's number (SVG number grammar, exponent included) and unit text.
fn split_length(v: &str) -> std::result::Result<(f64, &str), String> {
    let v = v.trim();
    let mut n = Nums::new(v);
    let x = n.number().map_err(|_| format!("bad length '{v}'"))?;
    Ok((x, v[n.i..].trim()))
}

/// A length in user units (CSS px).
fn length(v: &str) -> std::result::Result<f64, LenError> {
    if v.trim() == "auto" {
        return Err(LenError::Relative("auto".into()));
    }
    let (x, unit) = split_length(v).map_err(LenError::Malformed)?;
    let k = match unit {
        "" | "px" => 1.0,
        "mm" => PX_PER_MM,
        "cm" => 10.0 * PX_PER_MM,
        "in" => 96.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        u if RELATIVE_UNITS.contains(&u) => return Err(LenError::Relative(u.into())),
        u => {
            return Err(LenError::Malformed(format!(
                "unsupported length unit '{u}' in '{}'",
                v.trim()
            )));
        }
    };
    Ok(x * k)
}

/// Physical size of a root `width`/`height` in mm, when it has an absolute unit.
fn physical_mm(v: &str) -> Option<f64> {
    match split_length(v) {
        Ok((_, "mm" | "cm" | "in" | "pt" | "pc")) => length(v).ok().map(|px| px / PX_PER_MM),
        _ => None,
    }
}

/// Number scanner for path data, point lists, viewBox and transforms.
struct Nums<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Nums<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s: s.as_bytes(),
            i: 0,
        }
    }
    fn skip_sep(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip_sep();
        self.s.get(self.i).copied()
    }
    fn at_number(&mut self) -> bool {
        matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }
    fn number(&mut self) -> Result<f64> {
        self.skip_sep();
        let start = self.i;
        let b = self.s;
        let mut j = self.i;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        let mut digits = false;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            digits = true;
        }
        if j < b.len() && b[j] == b'.' {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
                digits = true;
            }
        }
        if !digits {
            return Err(malformed(format!("expected a number at byte {start}")));
        }
        if j < b.len() && matches!(b[j], b'e' | b'E') {
            let mut k = j + 1;
            if k < b.len() && matches!(b[k], b'+' | b'-') {
                k += 1;
            }
            if k < b.len() && b[k].is_ascii_digit() {
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        self.i = j;
        let text = std::str::from_utf8(&b[start..j]).map_err(|_| malformed("bad number"))?;
        let v: f64 = text
            .parse()
            .map_err(|_| malformed(format!("bad number '{text}'")))?;
        if !v.is_finite() {
            return Err(ExchangeError::NonFinite {
                format: FMT.into(),
                line: None,
                message: text.into(),
            });
        }
        Ok(v)
    }
    /// Arc flag: a single `0` or `1` (may be followed directly by the next number).
    fn flag(&mut self) -> Result<bool> {
        match self.peek() {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(malformed("expected an arc flag (0 or 1)")),
        }
    }
}

fn parse_transform(t: &str) -> Result<Affine> {
    let mut m = Affine::ID;
    let mut rest = t.trim();
    while !rest.is_empty() {
        let open = rest
            .find('(')
            .ok_or_else(|| malformed(format!("bad transform '{t}'")))?;
        let close = rest[open..]
            .find(')')
            .map(|c| c + open)
            .ok_or_else(|| malformed(format!("bad transform '{t}'")))?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let mut nums = Nums::new(&rest[open + 1..close]);
        let mut v = Vec::new();
        while nums.at_number() {
            v.push(nums.number()?);
        }
        let deg = |a: f64| a.to_radians();
        let op = match (name, v.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine([*a, *b, *c, *d, *e, *f]),
            ("translate", [x]) => Affine([1.0, 0.0, 0.0, 1.0, *x, 0.0]),
            ("translate", [x, y]) => Affine([1.0, 0.0, 0.0, 1.0, *x, *y]),
            ("scale", [s]) => Affine([*s, 0.0, 0.0, *s, 0.0, 0.0]),
            ("scale", [sx, sy]) => Affine([*sx, 0.0, 0.0, *sy, 0.0, 0.0]),
            ("rotate", [a]) => {
                let (s, c) = deg(*a).sin_cos();
                Affine([c, s, -s, c, 0.0, 0.0])
            }
            ("rotate", [a, cx, cy]) => {
                let (s, c) = deg(*a).sin_cos();
                Affine([1.0, 0.0, 0.0, 1.0, -cx, -cy])
                    .then(Affine([c, s, -s, c, 0.0, 0.0]))
                    .then(Affine([1.0, 0.0, 0.0, 1.0, *cx, *cy]))
            }
            ("skewX", [a]) => Affine([1.0, 0.0, deg(*a).tan(), 1.0, 0.0, 0.0]),
            ("skewY", [a]) => Affine([1.0, deg(*a).tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => {
                return Err(malformed(format!(
                    "unsupported transform '{name}' in '{t}'"
                )));
            }
        };
        // transform="A B" means A ∘ B: B applies first.
        m = op.then(m);
        rest = rest[close + 1..].trim();
    }
    Ok(m)
}

fn attr(e: &BytesStart, name: &str) -> Result<Option<String>> {
    for a in e.attributes() {
        let a = a.map_err(|err| malformed(format!("attribute: {err}")))?;
        if a.key.as_ref() == name.as_bytes() {
            let v = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| malformed(format!("attribute {name}: {err}")))?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

/// Failure while drawing one element.
enum ElemError {
    /// The whole import fails.
    Fatal(ExchangeError),
    /// The element is skipped and reported as unsupported (reason).
    Unsupported(String),
}

impl From<ExchangeError> for ElemError {
    fn from(e: ExchangeError) -> Self {
        ElemError::Fatal(e)
    }
}

/// A length attribute in user units (absent = 0).
fn len_attr(e: &BytesStart, name: &str) -> std::result::Result<f64, ElemError> {
    match attr(e, name)? {
        None => Ok(0.0),
        Some(v) => length(&v).map_err(|err| match err {
            LenError::Relative(u) => {
                ElemError::Unsupported(format!("length in relative unit '{u}'"))
            }
            LenError::Malformed(m) => ElemError::Fatal(malformed(m)),
        }),
    }
}

/// Emits curves for one element in sketch mm, given the element's full map.
struct Emit<'r> {
    map: Affine,
    out: Vec<(String, Curve)>,
    rep: &'r mut Report,
}

impl Emit<'_> {
    fn line(&mut self, elem: &str, p: [f64; 2], q: [f64; 2]) {
        let (a, b) = (self.map.apply(p), self.map.apply(q));
        self.out.push((elem.into(), Curve::Line(a, b)));
    }
    fn circle(&mut self, elem: &str, c: [f64; 2], r: f64) -> bool {
        let Some(k) = self.map.similarity_scale() else {
            self.rep.unsupported(
                elem,
                "circle under a non-uniform scale or skew (an ellipse)",
            );
            return false;
        };
        self.out
            .push((elem.into(), Curve::Circle(self.map.apply(c), r * k)));
        true
    }
    /// SVG arc from `p` to `q` (user units), circular radius `r`.
    fn arc(&mut self, elem: &str, p: [f64; 2], q: [f64; 2], r: f64, large: bool, sweep: bool) {
        if self.map.similarity_scale().is_none() {
            self.rep
                .unsupported(elem, "arc under a non-uniform scale or skew (elliptical)");
            return;
        }
        // Endpoint to centre parameterization (SVG 1.1 F.6.5) for a circle.
        let (x1, y1) = ((p[0] - q[0]) / 2.0, (p[1] - q[1]) / 2.0);
        let d2 = x1 * x1 + y1 * y1;
        if d2 == 0.0 {
            return; // zero-length arc draws nothing
        }
        let r = r.max(d2.sqrt());
        let coef = ((r * r - d2) / d2).max(0.0).sqrt() * if large == sweep { -1.0 } else { 1.0 };
        let c = [
            coef * y1 + (p[0] + q[0]) / 2.0,
            -coef * x1 + (p[1] + q[1]) / 2.0,
        ];
        let t1 = (p[1] - c[1]).atan2(p[0] - c[0]);
        let t2 = (q[1] - c[1]).atan2(q[0] - c[0]);
        let mut dt = (t2 - t1).rem_euclid(TAU);
        if !sweep {
            dt -= TAU;
        }
        let tm = t1 + dt / 2.0;
        let m = [c[0] + r * tm.cos(), c[1] + r * tm.sin()];
        let (s, e, cc, mm) = (
            self.map.apply(p),
            self.map.apply(q),
            self.map.apply(c),
            self.map.apply(m),
        );
        // Under a near-similarity the mapped centre is not exactly equidistant
        // from the mapped ends: move it onto their perpendicular bisector (the
        // ends are shared with neighbouring curves and must not move).
        let cc = {
            let mid = [(s[0] + e[0]) / 2.0, (s[1] + e[1]) / 2.0];
            let u = [e[0] - s[0], e[1] - s[1]];
            let k =
                ((cc[0] - mid[0]) * u[0] + (cc[1] - mid[1]) * u[1]) / (u[0] * u[0] + u[1] * u[1]);
            [cc[0] - k * u[0], cc[1] - k * u[1]]
        };
        let cross = (s[0] - cc[0]) * (mm[1] - cc[1]) - (s[1] - cc[1]) * (mm[0] - cc[0]);
        let (start, end) = if cross > 0.0 { (s, e) } else { (e, s) };
        self.out.push((
            elem.into(),
            Curve::Arc {
                center: cc,
                start,
                end,
            },
        ));
    }
}

/// Point list; `true` when an odd trailing coordinate was dropped (SVG renders
/// up to the error).
fn points_attr(v: &str) -> Result<(Vec<[f64; 2]>, bool)> {
    let mut n = Nums::new(v);
    let mut out = Vec::new();
    while n.at_number() {
        let x = n.number()?;
        if !n.at_number() {
            return Ok((out, true));
        }
        let y = n.number()?;
        out.push([x, y]);
    }
    Ok((out, false))
}

fn path(em: &mut Emit, d: &str) -> Result<()> {
    let mut n = Nums::new(d);
    let mut cur = [0.0, 0.0];
    let mut start = [0.0, 0.0];
    let mut cmd = 0u8;
    loop {
        // Every pass must consume input (a command letter or numbers): guard
        // against any implicit-repeat path that reads nothing.
        n.skip_sep();
        let at = n.i;
        let c = match n.peek() {
            None => break,
            Some(c) if c.is_ascii_alphabetic() => {
                n.i += 1;
                c
            }
            // Closepath takes no parameters: it cannot repeat implicitly.
            Some(c) if matches!(cmd, b'Z' | b'z') => {
                return Err(malformed(format!(
                    "unexpected '{}' after closepath (Z) at byte {}",
                    c as char, n.i
                )));
            }
            Some(_) if cmd != 0 => match cmd {
                b'M' => b'L',
                b'm' => b'l',
                other => other,
            },
            Some(c) => {
                return Err(malformed(format!(
                    "path data must start with a command, got '{}'",
                    c as char
                )));
            }
        };
        if cmd == 0 && !matches!(c, b'M' | b'm') {
            return Err(malformed("path data must start with a moveto (M/m)"));
        }
        cmd = c;
        let rel = c.is_ascii_lowercase();
        let off = |p: [f64; 2], cur: [f64; 2]| {
            if rel {
                [p[0] + cur[0], p[1] + cur[1]]
            } else {
                p
            }
        };
        match c.to_ascii_uppercase() {
            b'M' => {
                let p = off([n.number()?, n.number()?], cur);
                cur = p;
                start = p;
            }
            b'L' => {
                let p = off([n.number()?, n.number()?], cur);
                em.line("path", cur, p);
                cur = p;
            }
            b'H' => {
                let x = n.number()?;
                let p = [if rel { cur[0] + x } else { x }, cur[1]];
                em.line("path", cur, p);
                cur = p;
            }
            b'V' => {
                let y = n.number()?;
                let p = [cur[0], if rel { cur[1] + y } else { y }];
                em.line("path", cur, p);
                cur = p;
            }
            b'Z' => {
                if cur != start {
                    em.line("path", cur, start);
                }
                cur = start;
            }
            b'A' => {
                let (rx, ry, _rot) = (n.number()?.abs(), n.number()?.abs(), n.number()?);
                let (large, sweep) = (n.flag()?, n.flag()?);
                let p = off([n.number()?, n.number()?], cur);
                if rx == 0.0 || ry == 0.0 {
                    em.line("path", cur, p);
                } else if (rx - ry).abs() > SIMILAR_REL * rx.max(ry) {
                    em.rep.unsupported("path", "elliptical arc (rx != ry)");
                } else {
                    em.arc("path", cur, p, (rx + ry) / 2.0, large, sweep);
                }
                cur = p;
            }
            b'C' | b'S' | b'Q' | b'T' => {
                let k = match c.to_ascii_uppercase() {
                    b'C' => 3,
                    b'S' | b'Q' => 2,
                    _ => 1,
                };
                let mut last = cur;
                for _ in 0..k {
                    last = off([n.number()?, n.number()?], cur);
                }
                em.rep.unsupported("path", "Bezier curve segment (C/S/Q/T)");
                cur = last;
            }
            other => {
                return Err(malformed(format!(
                    "unknown path command '{}'",
                    other as char
                )));
            }
        }
        if n.i == at {
            return Err(malformed(format!(
                "path data makes no progress at byte {at}"
            )));
        }
    }
    Ok(())
}

/// Elements whose content draws nothing by itself (skipped with their subtree).
const IGNORED: &[&str] = &[
    "defs",
    "symbol",
    "clipPath",
    "mask",
    "pattern",
    "marker",
    "style",
    "script",
    "metadata",
    "title",
    "desc",
    "linearGradient",
    "radialGradient",
    "filter",
];
/// Elements that draw but are not supported (skipped with their subtree).
const UNSUPPORTED: &[(&str, &str)] = &[
    ("text", "text is not converted to curves"),
    ("use", "references (use) are not expanded"),
    ("image", "raster images"),
    ("svg", "nested svg viewports"),
    ("foreignObject", "foreign content"),
    ("switch", "conditional content"),
    ("a", "links (contents not imported)"),
];

/// Elements handled by [`draw`] (`g` only carries a transform).
const DRAWN: &[&str] = &[
    "g", "line", "rect", "circle", "ellipse", "polyline", "polygon", "path",
];
const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";

/// Emit the curves of one supported element; `Ok(true)` when it is a shape.
fn draw(e: &BytesStart, name: &str, em: &mut Emit) -> std::result::Result<bool, ElemError> {
    Ok(match name {
        "line" => {
            let p = [len_attr(e, "x1")?, len_attr(e, "y1")?];
            let q = [len_attr(e, "x2")?, len_attr(e, "y2")?];
            em.line("line", p, q);
            true
        }
        "rect" => {
            let (x, y) = (len_attr(e, "x")?, len_attr(e, "y")?);
            let (w, h) = (len_attr(e, "width")?, len_attr(e, "height")?);
            // `auto` corner radii (SVG 2) mean "same as the other one": both
            // auto is square.
            let corner = |k: &str| match attr(e, k) {
                Ok(Some(v)) if v.trim() == "auto" => Ok(0.0),
                _ => len_attr(e, k),
            };
            if corner("rx")? > 0.0 || corner("ry")? > 0.0 {
                em.rep.unsupported("rect", "rounded corners (rx/ry)");
                false
            } else if w <= 0.0 || h <= 0.0 {
                em.rep
                    .degenerate("rect", "zero or negative size (not rendered)");
                false
            } else {
                let c = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
                for i in 0..4 {
                    em.line("rect", c[i], c[(i + 1) % 4]);
                }
                true
            }
        }
        "circle" | "ellipse" => {
            let c = [len_attr(e, "cx")?, len_attr(e, "cy")?];
            let (rx, ry) = if name == "circle" {
                let r = len_attr(e, "r")?;
                (r, r)
            } else {
                (len_attr(e, "rx")?, len_attr(e, "ry")?)
            };
            if rx <= 0.0 || ry <= 0.0 {
                em.rep
                    .degenerate(name, "zero or negative radius (not rendered)");
                false
            } else if (rx - ry).abs() > SIMILAR_REL * rx.max(ry) {
                em.rep.unsupported(name, "ellipse with rx != ry");
                false
            } else {
                em.circle(name, c, (rx + ry) / 2.0)
            }
        }
        "polyline" | "polygon" => {
            let (pts, odd) = points_attr(&attr(e, "points")?.unwrap_or_default())?;
            if odd {
                em.rep
                    .degenerate(name, "odd number of coordinates (last one ignored)");
            }
            if pts.len() < 2 {
                em.rep.degenerate(name, "fewer than two points");
                false
            } else {
                for w in pts.windows(2) {
                    em.line(name, w[0], w[1]);
                }
                if name == "polygon" && pts.first() != pts.last() {
                    em.line(name, pts[pts.len() - 1], pts[0]);
                }
                true
            }
        }
        "path" => {
            path(em, &attr(e, "d")?.unwrap_or_default())?;
            true
        }
        _ => false,
    })
}

/// Import an SVG file's planar paths as an unconstrained sketch. See the module docs.
pub fn import(bytes: &[u8], opts: &SvgImportOptions) -> Result<SvgImport> {
    if !(opts.join_tolerance_mm.is_finite() && opts.join_tolerance_mm >= 0.0) {
        return Err(ExchangeError::InvalidOption(format!(
            "join tolerance must be finite and >= 0 mm (got {})",
            opts.join_tolerance_mm
        )));
    }
    if let Some(k) = opts.unit_override_to_mm {
        check_scale("SVG unit override", k)?;
    }
    if bytes.len() as u64 > opts.max_bytes {
        return Err(ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "bytes".into(),
            limit: opts.max_bytes,
            actual: bytes.len() as u64,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|e| malformed(format!("not UTF-8: {e}")))?;
    let mut reader = NsReader::from_str(text);
    let mut rep = Report::default();
    let mut curves: Vec<(String, Curve)> = Vec::new();
    // Stack of (element name, map at that element) for open elements.
    let mut stack: Vec<(String, Affine)> = Vec::new();
    let mut skip_depth: usize = 0;
    let mut root: Option<(Affine, SvgUnits)> = None;
    let mut elements = 0u64;
    loop {
        // Elements bound to another namespace (Inkscape's `sodipodi:namedview`,
        // `inkscape:*`, RDF metadata, ...) or to an undeclared prefix are
        // editor data, not drawing: skipped silently with their subtree. No
        // namespace at all (files without `xmlns`) is read as SVG.
        let (foreign, ev) = match reader.read_resolved_event() {
            Ok((ns, ev)) => {
                let foreign = match ns {
                    ResolveResult::Unbound => false,
                    ResolveResult::Bound(ns) => ns.as_ref() != SVG_NS,
                    ResolveResult::Unknown(_) => true,
                };
                (foreign, ev)
            }
            Err(e) => {
                return Err(malformed(format!(
                    "XML at byte {}: {e}",
                    reader.buffer_position()
                )));
            }
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                } else {
                    stack.pop();
                }
                continue;
            }
            // A public/system DOCTYPE (FreeCAD, older editors) is harmless; an
            // internal subset can declare entities and is refused.
            Event::DocType(t) if !t.as_ref().contains(&b'[') => continue,
            Event::DocType(_) => {
                return Err(ExchangeError::Unsupported {
                    format: FMT.into(),
                    entities: vec![UnsupportedEntities {
                        entity_type: "DOCTYPE".into(),
                        count: 1,
                        reason: "DOCTYPE internal subsets (entity declarations) are refused".into(),
                        first_line: 0,
                    }],
                });
            }
            Event::Eof => break,
            _ => continue,
        };
        elements += 1;
        if elements > opts.max_elements {
            return Err(ExchangeError::LimitExceeded {
                format: FMT.into(),
                what: "elements".into(),
                limit: opts.max_elements,
                actual: elements,
            });
        }
        if stack.len() > 256 {
            return Err(ExchangeError::LimitExceeded {
                format: FMT.into(),
                what: "nesting depth".into(),
                limit: 256,
                actual: stack.len() as u64,
            });
        }
        let qname = e.name();
        let name = std::str::from_utf8(qname.local_name().into_inner())
            .map_err(|_| malformed("element name is not UTF-8"))?
            .to_string();
        if skip_depth > 0 {
            if !empty {
                skip_depth += 1;
            }
            continue;
        }
        let parent = stack.last().map(|s| s.1);
        let (name_ref, is_root) = (name.as_str(), root.is_none());
        if is_root {
            if name_ref != "svg" {
                return Err(malformed(format!("root element is <{name}>, not <svg>")));
            }
            let (map, units) = root_map(&e, opts)?;
            root = Some((map, units));
            if !empty {
                stack.push((name, map));
            }
            continue;
        }
        let Some(parent) = parent else {
            // Content after the root closed.
            continue;
        };
        if foreign || IGNORED.contains(&name_ref) {
            if !empty {
                skip_depth = 1;
            }
            continue;
        }
        if let Some((_, why)) = UNSUPPORTED.iter().find(|(n, _)| *n == name_ref) {
            rep.unsupported(name_ref, why);
            if !empty {
                skip_depth = 1;
            }
            continue;
        }
        let local = match attr(&e, "transform")? {
            Some(t) => parse_transform(&t)?,
            None => Affine::ID,
        };
        let map = local.then(parent);
        if !DRAWN.contains(&name_ref) {
            rep.unsupported(name_ref, "element type not supported");
            if !empty {
                skip_depth = 1;
            }
            continue;
        }
        let mut em = Emit {
            map,
            out: Vec::new(),
            rep: &mut rep,
        };
        let (drew, out) = match draw(&e, name_ref, &mut em) {
            Ok(drew) => (drew, std::mem::take(&mut em.out)),
            Err(ElemError::Fatal(err)) => return Err(err),
            Err(ElemError::Unsupported(why)) => {
                // Nothing of the element is kept (no partial shapes).
                rep.unsupported(name_ref, &why);
                (false, Vec::new())
            }
        };
        if drew && !out.is_empty() {
            *rep.imported.entry(name.clone()).or_default() += 1;
        }
        curves.extend(out);
        if !empty && skip_depth == 0 {
            stack.push((name, map));
        }
    }
    let Some((_, units)) = root else {
        return Err(ExchangeError::Empty {
            format: FMT.into(),
            message: "no <svg> element".into(),
        });
    };
    let unsupported: Vec<UnsupportedEntities> = rep
        .unsupported
        .iter()
        .map(|((t, why), n)| UnsupportedEntities {
            entity_type: t.clone(),
            count: *n,
            reason: why.clone(),
            first_line: 0,
        })
        .collect();
    if !unsupported.is_empty() && !opts.allow_partial {
        return Err(ExchangeError::Unsupported {
            format: FMT.into(),
            entities: unsupported,
        });
    }
    build(curves, units, rep, unsupported, opts.join_tolerance_mm)
}

/// Root viewport: (user units -> sketch mm with y flipped, units report).
fn root_map(e: &BytesStart, opts: &SvgImportOptions) -> Result<(Affine, SvgUnits)> {
    let w = attr(e, "width")?;
    let h = attr(e, "height")?;
    let vb = attr(e, "viewBox")?
        .map(|v| {
            let mut n = Nums::new(&v);
            let mut out = Vec::new();
            while n.at_number() {
                out.push(n.number()?);
            }
            match out.as_slice() {
                [x, y, w, h] if *w > 0.0 && *h > 0.0 => Ok([*x, *y, *w, *h]),
                _ => Err(malformed(format!("bad viewBox '{v}'"))),
            }
        })
        .transpose()?;
    let refuse = |reason: String| ExchangeError::Unsupported {
        format: FMT.into(),
        entities: vec![UnsupportedEntities {
            entity_type: "svg".into(),
            count: 1,
            reason,
            first_line: 0,
        }],
    };
    // A root transform applies in the embedding CSS context (CSS px, about the
    // box centre by default), outside the user coordinates this import keeps:
    // refused rather than guessed. An identity transform is harmless.
    if let Some(t) = attr(e, "transform")?
        && parse_transform(&t)? != Affine::ID
    {
        return Err(refuse(format!(
            "transform=\"{t}\" on the root <svg> (applies in the embedding page)"
        )));
    }
    // Only the scale is used, and every alignment gives the same scale; `slice`
    // takes the larger ratio, `none` needs equal ratios. Without a viewBox the
    // attribute has no effect.
    let par = attr(e, "preserveAspectRatio")?.unwrap_or_default();
    let par: Vec<&str> = par.split_whitespace().filter(|t| *t != "defer").collect();
    const ALIGN: &[&str] = &[
        "xMinYMin", "xMidYMin", "xMaxYMin", "xMinYMid", "xMidYMid", "xMaxYMid", "xMinYMax",
        "xMidYMax", "xMaxYMax",
    ];
    let fit = match par.as_slice() {
        [] => "meet",
        [a] if ALIGN.contains(a) => "meet",
        [a, m @ ("meet" | "slice")] if ALIGN.contains(a) => m,
        ["none"] | ["none", "meet" | "slice"] => "none",
        _ if vb.is_none() => "meet",
        _ => {
            return Err(refuse(format!(
                "preserveAspectRatio=\"{}\" is not valid",
                par.join(" ")
            )));
        }
    };
    let flip = Affine([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
    let wmm = w.as_deref().and_then(physical_mm);
    let hmm = h.as_deref().and_then(physical_mm);
    // SVG: a negative size is an error, zero disables rendering.
    for (name, v) in [("width", wmm), ("height", hmm)] {
        if let Some(v) = v
            && !(v.is_finite() && v > 0.0)
        {
            return Err(malformed(format!("root {name} must be > 0 (got {v} mm)")));
        }
    }
    // A missing or `auto` size follows the other one through the viewBox
    // aspect ratio (SVG 2).
    let auto = |v: &Option<String>| v.as_deref().is_none_or(|v| v.trim() == "auto");
    let (wmm, hmm) = match (wmm, hmm, vb) {
        (Some(wm), None, Some([_, _, vw, vh])) if auto(&h) => (Some(wm), Some(wm * vh / vw)),
        (None, Some(hm), Some([_, _, vw, vh])) if auto(&w) => (Some(hm * vw / vh), Some(hm)),
        other => (other.0, other.1),
    };
    let (to_view, units) = match (wmm, hmm, vb) {
        (Some(wm), Some(hm), Some([x, y, vw, vh])) => {
            // Scale only: drawing (user) coordinates keep their origin; the
            // viewBox offset and centering just frame the page.
            let (sx, sy) = (wm / vw, hm / vh);
            let s = match fit {
                "slice" => sx.max(sy),
                "none" if (sx - sy).abs() > SIMILAR_REL * sx.max(sy) => {
                    return Err(refuse(format!(
                        "preserveAspectRatio=\"none\" with unequal scales ({sx} and {sy} \
                         mm per unit) would distort the drawing"
                    )));
                }
                _ => sx.min(sy),
            };
            check_scale("SVG root scale (mm per user unit)", s)?;
            (
                Affine([s, 0.0, 0.0, s, 0.0, 0.0]),
                SvgUnits {
                    name: format!(
                        "root {}x{} with viewBox {x} {y} {vw} {vh}",
                        w.as_deref().unwrap_or("auto"),
                        h.as_deref().unwrap_or("auto")
                    ),
                    to_mm: s,
                    declared: true,
                },
            )
        }
        (Some(_), Some(_), None) => {
            // No viewBox: user units are CSS px.
            let s = 1.0 / PX_PER_MM;
            (
                Affine([s, 0.0, 0.0, s, 0.0, 0.0]),
                SvgUnits {
                    name: "CSS px (root has a physical size and no viewBox)".into(),
                    to_mm: s,
                    declared: true,
                },
            )
        }
        _ => match opts.unit_override_to_mm {
            // User coordinates scaled directly (a viewBox only frames them).
            Some(k) => (
                Affine([k, 0.0, 0.0, k, 0.0, 0.0]),
                SvgUnits {
                    name: "user override".into(),
                    to_mm: k,
                    declared: false,
                },
            ),
            None => {
                return Err(ExchangeError::UnitsRequired {
                    format: FMT.into(),
                    measurement: None,
                });
            }
        },
    };
    Ok((to_view.then(flip), units))
}

fn build(
    curves: Vec<(String, Curve)>,
    units: SvgUnits,
    rep: Report,
    unsupported: Vec<UnsupportedEntities>,
    tol: f64,
) -> Result<SvgImport> {
    let mut warnings = Vec::new();
    let mut skipped = unsupported;
    let mut degenerate = rep.degenerate;
    let mut sketch = Sketch::default();
    sketch.ensure_origin();
    let mut merger = PointMerger::new(tol);
    let degenerate_tol = tol.max(1e-9);
    for (elem, c) in curves {
        match c {
            Curve::Line(a, b) => {
                if (a[0] - b[0]).hypot(a[1] - b[1]) <= degenerate_tol {
                    *degenerate
                        .entry((elem, "zero-length segment".into()))
                        .or_default() += 1;
                    continue;
                }
                let (pa, pb) = (merger.point(&mut sketch, a)?, merger.point(&mut sketch, b)?);
                if pa == pb {
                    *degenerate
                        .entry((elem, "zero-length segment".into()))
                        .or_default() += 1;
                    continue;
                }
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Line { id, a: pa, b: pb });
            }
            Curve::Arc { center, start, end } => {
                let r = (start[0] - center[0]).hypot(start[1] - center[1]);
                if r <= degenerate_tol
                    || (start[0] - end[0]).hypot(start[1] - end[1]) <= degenerate_tol
                {
                    *degenerate
                        .entry((elem, "zero radius or zero sweep arc".into()))
                        .or_default() += 1;
                    continue;
                }
                let (ps, pe) = (
                    merger.point(&mut sketch, start)?,
                    merger.point(&mut sketch, end)?,
                );
                let pc = push_point(&mut sketch, center);
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Arc {
                    id,
                    center: pc,
                    start: ps,
                    end: pe,
                });
            }
            Curve::Circle(c, r) => {
                if !(r.is_finite() && r > degenerate_tol) {
                    *degenerate
                        .entry((elem, "zero radius circle".into()))
                        .or_default() += 1;
                    continue;
                }
                let pc = push_point(&mut sketch, c);
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Circle {
                    id,
                    center: pc,
                    radius: r,
                });
            }
        }
    }
    for ((t, why), n) in degenerate {
        skipped.push(UnsupportedEntities {
            entity_type: t,
            count: n,
            reason: why,
            first_line: 0,
        });
    }
    if sketch.entities.is_empty() {
        return Err(ExchangeError::Empty {
            format: FMT.into(),
            message: "no supported, non-degenerate geometry".into(),
        });
    }
    if merger.merged() > 0 {
        warnings.push(format!(
            "{} endpoints joined to an existing point within {tol:e} mm",
            merger.merged()
        ));
    }
    let open = open_endpoints(&sketch);
    if open > 0 {
        warnings.push(format!(
            "{open} open endpoint(s): the curves do not form closed loops at the join tolerance"
        ));
    }
    sketch.validate().map_err(|e| match e {
        rcad_sketch::SketchError::TooLarge { unknowns, max } => ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "sketch unknowns (2 per point + 1 per circle)".into(),
            limit: max as u64,
            actual: unknowns as u64,
        },
        other => ExchangeError::Sketch(other.to_string()),
    })?;
    let solve = sketch.solve();
    if !solve.is_success() {
        return Err(ExchangeError::Sketch(format!(
            "the imported geometry does not satisfy the arc equations ({:?}, residual {:e} mm)",
            solve.status, solve.residual
        )));
    }
    if solve.status != SolveStatus::UnderConstrained {
        warnings.push(format!("unexpected solve status {:?}", solve.status));
    }
    Ok(SvgImport {
        sketch,
        units,
        imported: rep
            .imported
            .into_iter()
            .map(|(entity_type, count)| EntityCount { entity_type, count })
            .collect(),
        skipped,
        warnings,
        solve,
    })
}

/// Shortest round-trip number formatting.
fn num(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    format!("{v}")
}

/// Export the sketch's non-construction lines, arcs and circles (solved
/// coordinates as stored) as an SVG document in mm. See the module docs.
pub fn export(sketch: &Sketch) -> Result<String> {
    sketch
        .validate()
        .map_err(|e| ExchangeError::Sketch(e.to_string()))?;
    let xy = |id| {
        sketch
            .point_xy(id)
            .ok_or_else(|| ExchangeError::Sketch(format!("missing point {id:?}")))
    };
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut grow = |p: [f64; 2], r: f64| {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i] - r);
            hi[i] = hi[i].max(p[i] + r);
        }
    };
    let mut body = String::new();
    let mut count = 0usize;
    for e in &sketch.entities {
        if sketch.is_construction(e.id()) {
            continue;
        }
        match e {
            Entity::Line { a, b, .. } => {
                let (p, q) = (xy(*a)?, xy(*b)?);
                grow(p, 0.0);
                grow(q, 0.0);
                body += &format!(
                    "  <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
                    num(p[0]),
                    num(-p[1]),
                    num(q[0]),
                    num(-q[1])
                );
            }
            Entity::Circle { center, radius, .. } => {
                let c = xy(*center)?;
                grow(c, *radius);
                body += &format!(
                    "  <circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>\n",
                    num(c[0]),
                    num(-c[1]),
                    num(*radius)
                );
            }
            Entity::Arc {
                center, start, end, ..
            } => {
                let (c, s, t) = (xy(*center)?, xy(*start)?, xy(*end)?);
                let r = (s[0] - c[0]).hypot(s[1] - c[1]);
                grow(c, r);
                let a0 = (s[1] - c[1]).atan2(s[0] - c[0]);
                let a1 = (t[1] - c[1]).atan2(t[0] - c[0]);
                let sweep = (a1 - a0).rem_euclid(TAU);
                // CCW with y up is decreasing angle in SVG's y-down user space:
                // sweep-flag 0.
                body += &format!(
                    "  <path d=\"M {} {} A {} {} 0 {} 0 {} {}\"/>\n",
                    num(s[0]),
                    num(-s[1]),
                    num(r),
                    num(r),
                    u8::from(sweep > PI),
                    num(t[0]),
                    num(-t[1])
                );
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(ExchangeError::Sketch(
            "the sketch has no (non-construction) lines, arcs or circles".into(),
        ));
    }
    let (w, h) = ((hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6));
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!-- R-CAD sketch export: local sketch coordinates in mm, y negated (SVG y points down). -->\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" viewBox=\"{} {} {} {}\">\n\
         <g fill=\"none\" stroke=\"black\" stroke-width=\"0.1\">\n{body}</g>\n</svg>\n",
        num(w),
        num(h),
        num(lo[0]),
        num(-hi[1]),
        num(w),
        num(h)
    ))
}
