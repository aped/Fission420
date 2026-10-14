//! ASCII DXF (R12 … R2018 group-code files): import to an unconstrained sketch, export of a
//! sketch's lines/arcs/circles. Supported subset, units and limits: `docs/formats/dxf.md`.
//!
//! # Import geometry rules
//! * Supported entities: `LINE`, `ARC`, `CIRCLE`, `LWPOLYLINE` and 2D `POLYLINE`/`VERTEX`/
//!   `SEQEND` (straight segments and bulge arcs; `bulge = tan(sweep / 4)`, positive = CCW);
//!   full `ELLIPSE`s (sketch ellipses) and `SPLINE`s that are exactly sketch splines (open,
//!   non-rational, degree 3, clamped uniform knots, control points given). Elliptical arcs
//!   and other splines are unsupported (refused, or skipped and listed with
//!   `allow_partial`), never approximated.
//! * Planarity: only entities in the WCS XY plane. `LINE` endpoints (WCS) must have
//!   `|z| <= PLANAR_REL_TOL × max(1, |coordinates|)`; OCS entities (`ARC`, `CIRCLE`,
//!   polylines) must have extrusion `(0,0,±1)` (normalised; x/y components within the same
//!   relative tolerance) and zero elevation by the same test. For extrusion `(0,0,−1)` the
//!   arbitrary-axis algorithm gives OCS x = −WCS x, OCS y = WCS y, so points map as
//!   `(x, y) -> (−x, y)` and a CCW OCS arc becomes clockwise in the WCS; it is imported with
//!   its endpoints swapped so it stays CCW (the sketch convention). Any other extrusion is
//!   unsupported.
//! * Endpoint sharing: line/arc endpoints within `join_tolerance_mm` (after unit scaling) are
//!   one sketch point (coordinates of the first occurrence in file order). Circle and arc
//!   centres are never shared. The sketch origin (`Sketch::ensure_origin`, fixed at (0,0)) is
//!   a separate point and is not merged with geometry.
//! * Arcs: [`rcad_sketch::Entity::Arc`] runs counter-clockwise from `start` to `end`
//!   (sweep in (0, 2π)), exactly the DXF ARC convention (CCW from `50` to `51`). A bulge
//!   arc from vertex p to q is `start = p, end = q` for a positive bulge and swapped for a
//!   negative one.
//! * No constraints are created except the origin's `FixedPoint`. The sketch is solved once
//!   so the implied arc equation `|start − centre| = |end − centre|` holds at the sketch
//!   tolerance (endpoints written with finite precision or merged within the join tolerance
//!   move by at most that amount); the import reports the solve (under-constrained).
//! * Degenerate entities (zero length/radius/sweep after scaling and merging) carry no
//!   geometry: they are dropped and listed in `skipped` with the reason and in `warnings`,
//!   without triggering the all-or-nothing policy.

mod export;
mod reader;

pub use export::{DxfExportOptions, export};

use crate::{ExchangeError, Result, UnsupportedEntities};
use rcad_sketch::{Entity, PointId, Sketch, SketchPoint, SolveReport, SolveStatus};
use reader::{FMT, RawEntity, malformed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Default endpoint join tolerance, mm. Endpoints closer than this become one point.
pub const DEFAULT_JOIN_TOLERANCE_MM: f64 = 1e-6;
/// Relative planarity / extrusion-direction tolerance (dimensionless), see module docs.
pub const PLANAR_REL_TOL: f64 = 1e-9;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DxfImportOptions {
    /// Import only entities on these layers (case-insensitive, like AutoCAD). `None`: all.
    pub layers: Option<Vec<String>>,
    /// Skip unsupported entities (listed in [`DxfImport::skipped`]) instead of failing.
    pub allow_partial: bool,
    /// Millimetres per drawing unit, used only when the file declares no unit
    /// (`$INSUNITS` missing or 0). A declared unit always wins.
    pub unit_override_to_mm: Option<f64>,
    /// Endpoint join tolerance in mm (finite, >= 0).
    pub join_tolerance_mm: f64,
    pub max_bytes: u64,
    /// Maximum entities in the ENTITIES section (a POLYLINE's VERTEX records count).
    pub max_entities: u64,
}

impl Default for DxfImportOptions {
    fn default() -> Self {
        Self {
            layers: None,
            allow_partial: false,
            unit_override_to_mm: None,
            join_tolerance_mm: DEFAULT_JOIN_TOLERANCE_MM,
            max_bytes: 64 * 1024 * 1024,
            max_entities: 200_000,
        }
    }
}

/// The length unit applied to the file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LengthUnit {
    /// E.g. "millimeters", "inches"; "user override" when chosen by the caller.
    pub name: String,
    /// Millimetres per drawing unit.
    pub to_mm: f64,
    /// True when taken from the file's `$INSUNITS`; false when the override was used.
    pub declared: bool,
    /// The raw `$INSUNITS` code, if present.
    pub insunits: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DxfLayer {
    pub name: String,
    /// Entities on this layer in the ENTITIES section (POLYLINE counts once).
    pub entities: usize,
    /// Selected by [`DxfImportOptions::layers`] (always true without a filter).
    pub selected: bool,
    /// Listed in the TABLES/LAYER table.
    pub in_table: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EntityCount {
    pub entity_type: String,
    pub count: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DxfImport {
    /// Unconstrained sketch (origin fixed) in mm, solved once.
    pub sketch: Sketch,
    pub units: LengthUnit,
    /// `$ACADVER` (e.g. "AC1009" = R12, "AC1015" = R2000), if present.
    pub version: Option<String>,
    /// `$MEASUREMENT` (0 imperial, 1 metric), if present.
    pub measurement: Option<i64>,
    pub layers: Vec<DxfLayer>,
    /// Imported DXF entities by type.
    pub imported: Vec<EntityCount>,
    /// Entities not imported: unsupported ones (only with `allow_partial`) and degenerate ones.
    pub skipped: Vec<UnsupportedEntities>,
    pub warnings: Vec<String>,
    /// The post-import solve (status `UnderConstrained` for any non-empty import).
    pub solve: SolveReport,
}

/// `$INSUNITS` code -> (name, mm per unit), from the AutoCAD DXF reference table.
/// 0 (unitless) and 17..=20 (gigametres, astronomical units, light years, parsecs) are
/// not mapped.
pub fn insunits_to_mm(code: i64) -> Option<(&'static str, f64)> {
    const US_FOOT: f64 = 1200.0 / 3937.0 * 1000.0; // US survey foot in mm
    Some(match code {
        1 => ("inches", 25.4),
        2 => ("feet", 304.8),
        3 => ("miles", 1_609_344.0),
        4 => ("millimeters", 1.0),
        5 => ("centimeters", 10.0),
        6 => ("meters", 1000.0),
        7 => ("kilometers", 1_000_000.0),
        8 => ("microinches", 25.4e-6),
        9 => ("mils", 25.4e-3),
        10 => ("yards", 914.4),
        11 => ("angstroms", 1e-7),
        12 => ("nanometers", 1e-6),
        13 => ("microns", 1e-3),
        14 => ("decimeters", 100.0),
        15 => ("decameters", 10_000.0),
        16 => ("hectometers", 100_000.0),
        21 => ("US survey feet", US_FOOT),
        22 => ("US survey inches", US_FOOT / 12.0),
        23 => ("US survey yards", US_FOOT * 3.0),
        24 => ("US survey miles", US_FOOT * 5280.0),
        _ => return None,
    })
}

/// 2D curve in WCS drawing units, before scaling.
#[derive(Clone, Debug)]
enum Curve {
    Line {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Full ellipse: centre, absolute end point of the major axis, minor/major ratio.
    Ellipse {
        center: [f64; 2],
        major: [f64; 2],
        ratio: f64,
    },
    /// Uniform clamped non-rational cubic B-spline (control points).
    Spline {
        ctrl: Vec<[f64; 2]>,
    },
}

/// Accumulates unsupported/degenerate entity reports by (type, reason).
#[derive(Default)]
struct Skips(BTreeMap<(String, String), (usize, usize)>);

impl Skips {
    fn add(&mut self, kind: &str, reason: &str, line: usize) {
        let e = self
            .0
            .entry((kind.to_owned(), reason.to_owned()))
            .or_insert((0, line));
        e.0 += 1;
        e.1 = e.1.min(line);
    }

    fn list(&self) -> Vec<UnsupportedEntities> {
        let mut v: Vec<_> = self
            .0
            .iter()
            .map(|((t, r), (c, l))| UnsupportedEntities {
                entity_type: t.clone(),
                count: *c,
                reason: r.clone(),
                first_line: *l,
            })
            .collect();
        v.sort_by_key(|u| u.first_line);
        v
    }
}

/// Result of classifying one entity.
enum Classified {
    Curves(Vec<Curve>),
    Unsupported(&'static str),
}

fn planar_tol(coords: &[f64]) -> f64 {
    PLANAR_REL_TOL * coords.iter().fold(1.0_f64, |m, v| m.max(v.abs()))
}

/// OCS orientation of an entity: `Some(false)` for (0,0,1), `Some(true)` for (0,0,−1)
/// (mirrored x), `None` for any other direction.
fn ocs_mirrored(e: &RawEntity) -> Result<Option<bool>> {
    let (x, y, z) = (e.f_or(210, 0.0)?, e.f_or(220, 0.0)?, e.f_or(230, 1.0)?);
    let len = (x * x + y * y + z * z).sqrt();
    if !(len.is_finite() && len > 0.0) {
        return Err(malformed(
            e.line,
            format!("{}: zero extrusion vector", e.kind),
        ));
    }
    let (x, y, z) = (x / len, y / len, z / len);
    if x.abs() > PLANAR_REL_TOL || y.abs() > PLANAR_REL_TOL {
        return Ok(None);
    }
    Ok(Some(z < 0.0))
}

fn ocs_to_wcs(p: [f64; 2], mirrored: bool) -> [f64; 2] {
    if mirrored { [-p[0], p[1]] } else { p }
}

const R_PAPER: &str = "paper-space entity (only model space is imported)";
const R_NOT_XY: &str = "not in the XY plane (nonzero Z or elevation)";
const R_EXTRUSION: &str = "extrusion direction is not (0,0,1) or (0,0,-1) (tilted OCS)";
const R_3D_POLY: &str = "3D polyline / polygon mesh / polyface mesh";
const R_FIT_POLY: &str = "curve-fit or spline-fit polyline (fit vertices are approximations)";

/// Arc from `p` to `q` with bulge `b` (`tan(sweep/4)`, positive CCW), in one plane.
/// Returns `None` for a straight segment (b == 0).
fn bulge_arc(p: [f64; 2], q: [f64; 2], b: f64) -> Option<Curve> {
    if b == 0.0 {
        return None;
    }
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let m = [0.5 * (p[0] + q[0]), 0.5 * (p[1] + q[1])];
    // centre = midpoint + left normal × d(1 − b²)/(4b) / d  (left normal of p->q, unnormalised)
    let k = (1.0 - b * b) / (4.0 * b);
    let center = [m[0] - dy * k, m[1] + dx * k];
    Some(if b > 0.0 {
        Curve::Arc {
            center,
            start: p,
            end: q,
        }
    } else {
        Curve::Arc {
            center,
            start: q,
            end: p,
        }
    })
}

/// Vertices `(x, y, bulge)` of a polyline -> curves (OCS, before mirroring).
fn polyline_curves(v: &[([f64; 2], f64)], closed: bool) -> Vec<Curve> {
    let n = v.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    (0..segs)
        .map(|i| {
            let (p, b) = v[i];
            let q = v[(i + 1) % n].0;
            bulge_arc(p, q, b).unwrap_or(Curve::Line { a: p, b: q })
        })
        .collect()
}

fn mirror_curve(c: Curve, mirrored: bool) -> Curve {
    if !mirrored {
        return c;
    }
    match c {
        Curve::Line { a, b } => Curve::Line {
            a: ocs_to_wcs(a, true),
            b: ocs_to_wcs(b, true),
        },
        // Mirroring reverses orientation: swap to stay CCW.
        Curve::Arc { center, start, end } => Curve::Arc {
            center: ocs_to_wcs(center, true),
            start: ocs_to_wcs(end, true),
            end: ocs_to_wcs(start, true),
        },
        Curve::Circle { center, radius } => Curve::Circle {
            center: ocs_to_wcs(center, true),
            radius,
        },
        // WCS entities: never mirrored.
        c @ (Curve::Ellipse { .. } | Curve::Spline { .. }) => c,
    }
}

/// Relative tolerance for "full ellipse" parameters and uniform knot spacing.
const CURVE_PARAM_TOL: f64 = 1e-9;

/// A DXF ELLIPSE (WCS centre and major axis vector): only full ellipses
/// (parameters 0 to 2π) in the XY plane are imported.
fn classify_ellipse(e: &RawEntity) -> Result<std::result::Result<Curve, &'static str>> {
    if ocs_mirrored(e)?.is_none() {
        return Ok(Err(R_EXTRUSION));
    }
    let (cx, cy, cz) = (e.f_req(10)?, e.f_req(20)?, e.f_or(30, 0.0)?);
    let (mx, my, mz) = (e.f_req(11)?, e.f_req(21)?, e.f_or(31, 0.0)?);
    let ratio = e.f_req(40)?;
    let (t0, t1) = (e.f_or(41, 0.0)?, e.f_or(42, std::f64::consts::TAU)?);
    let tol = planar_tol(&[cx, cy, mx, my]);
    if cz.abs() > tol || mz.abs() > tol {
        return Ok(Err(R_NOT_XY));
    }
    if ((t1 - t0) - std::f64::consts::TAU).abs() > CURVE_PARAM_TOL * std::f64::consts::TAU {
        return Ok(Err(
            "elliptical arcs are not supported (full ellipses only)",
        ));
    }
    Ok(Ok(Curve::Ellipse {
        center: [cx, cy],
        major: [cx + mx, cy + my],
        ratio,
    }))
}

/// A DXF SPLINE: imported only when it is exactly a sketch spline, i.e. an
/// open, non-rational degree-3 B-spline with a clamped uniform knot vector
/// (any affine knot scaling) and control points in the XY plane. Fit-point
/// splines, other degrees, rational, periodic or non-uniform splines are
/// refused (they are not representable exactly).
fn classify_spline(e: &RawEntity) -> Result<std::result::Result<Curve, &'static str>> {
    let flags = e.int(70)?.unwrap_or(0);
    if e.int(71)?.unwrap_or(0) != 3 {
        return Ok(Err(
            "spline degree is not 3 (only cubic splines are imported)",
        ));
    }
    if flags & 2 != 0 {
        return Ok(Err("periodic splines are not supported"));
    }
    if flags & 1 != 0 {
        return Ok(Err("closed splines are not supported (open splines only)"));
    }
    let mut knots = Vec::new();
    let mut weights = Vec::new();
    let mut ctrl: Vec<[f64; 3]> = Vec::new();
    for p in &e.pairs {
        match p.code {
            40 => knots.push(reader::parse_f64(p)?),
            41 => weights.push(reader::parse_f64(p)?),
            10 => ctrl.push([reader::parse_f64(p)?, f64::NAN, 0.0]),
            20 | 30 => {
                let Some(last) = ctrl.last_mut() else {
                    return Err(malformed(p.line, "SPLINE coordinate before its x value"));
                };
                last[if p.code == 20 { 1 } else { 2 }] = reader::parse_f64(p)?;
            }
            _ => {}
        }
    }
    if ctrl.is_empty() {
        return Ok(Err(
            "fit-point splines are not supported (control points required)",
        ));
    }
    if ctrl.iter().any(|c| c[1].is_nan()) {
        return Err(malformed(e.line, "SPLINE control point without a y value"));
    }
    let n = ctrl.len();
    if !(4..=rcad_sketch::curves::MAX_SPLINE_POINTS).contains(&n) {
        return Ok(Err("spline control point count outside 4..=256"));
    }
    if weights.iter().any(|w| (w - 1.0).abs() > CURVE_PARAM_TOL) {
        return Ok(Err(
            "rational splines (weights other than 1) are not supported",
        ));
    }
    if knots.len() != n + 4 {
        return Ok(Err("spline knot count does not match its control points"));
    }
    // Clamped uniform: 4 equal knots at each end, equal interior steps.
    let (k0, k1) = (knots[0], knots[n + 3]);
    let step = (k1 - k0) / (n - 3) as f64;
    let ok = step > 0.0
        && knots.iter().enumerate().all(|(i, k)| {
            let want = k0 + step * (i as f64 - 3.0).clamp(0.0, (n - 3) as f64);
            (k - want).abs() <= CURVE_PARAM_TOL * (k1 - k0).abs().max(1.0)
        });
    if !ok {
        return Ok(Err(
            "non-uniform or unclamped spline knots are not supported (uniform clamped only)",
        ));
    }
    let coords: Vec<f64> = ctrl.iter().flat_map(|c| [c[0], c[1]]).collect();
    if ctrl.iter().any(|c| c[2].abs() > planar_tol(&coords)) {
        return Ok(Err(R_NOT_XY));
    }
    let (f, l) = (ctrl[0], ctrl[n - 1]);
    if (f[0] - l[0]).hypot(f[1] - l[1]) <= planar_tol(&coords) {
        return Ok(Err(
            "the spline's first and last control points coincide (a closed spline; open \
             splines only)",
        ));
    }
    Ok(Ok(Curve::Spline {
        ctrl: ctrl.iter().map(|c| [c[0], c[1]]).collect(),
    }))
}

fn classify(e: &RawEntity, vertices: &[RawEntity]) -> Result<Classified> {
    use Classified::Unsupported as U;
    if e.int(67)? == Some(1) {
        return Ok(U(R_PAPER));
    }
    match e.kind.as_str() {
        "LINE" => {
            let (x1, y1, z1) = (e.f_req(10)?, e.f_req(20)?, e.f_or(30, 0.0)?);
            let (x2, y2, z2) = (e.f_req(11)?, e.f_req(21)?, e.f_or(31, 0.0)?);
            let tol = planar_tol(&[x1, y1, x2, y2]);
            if z1.abs() > tol || z2.abs() > tol {
                return Ok(U(R_NOT_XY));
            }
            Ok(Classified::Curves(vec![Curve::Line {
                a: [x1, y1],
                b: [x2, y2],
            }]))
        }
        "CIRCLE" | "ARC" => {
            let Some(mirrored) = ocs_mirrored(e)? else {
                return Ok(U(R_EXTRUSION));
            };
            let (cx, cy, cz) = (e.f_req(10)?, e.f_req(20)?, e.f_or(30, 0.0)?);
            let r = e.f_req(40)?;
            if cz.abs() > planar_tol(&[cx, cy, r]) {
                return Ok(U(R_NOT_XY));
            }
            let c = [cx, cy];
            let curve = if e.kind == "CIRCLE" {
                Curve::Circle {
                    center: c,
                    radius: r,
                }
            } else {
                let (a0, a1) = (e.f_req(50)?.to_radians(), e.f_req(51)?.to_radians());
                let at = |a: f64| [cx + r * a.cos(), cy + r * a.sin()];
                // Zero sweep (a1 == a0 mod 360) gives start == end: caught as degenerate.
                Curve::Arc {
                    center: c,
                    start: at(a0),
                    end: at(a1),
                }
            };
            Ok(Classified::Curves(vec![mirror_curve(curve, mirrored)]))
        }
        "LWPOLYLINE" => {
            let Some(mirrored) = ocs_mirrored(e)? else {
                return Ok(U(R_EXTRUSION));
            };
            let elevation = e.f_or(38, 0.0)?;
            let flags = e.int(70)?.unwrap_or(0);
            let mut verts: Vec<([f64; 2], f64)> = Vec::new();
            let mut pending_x: Option<f64> = None;
            for p in &e.pairs {
                match p.code {
                    10 => pending_x = Some(reader::parse_f64(p)?),
                    20 => {
                        let x = pending_x.take().ok_or_else(|| {
                            malformed(p.line, "LWPOLYLINE: group 20 without a preceding 10")
                        })?;
                        verts.push(([x, reader::parse_f64(p)?], 0.0));
                    }
                    42 => {
                        let b = reader::parse_f64(p)?;
                        let last = verts.last_mut().ok_or_else(|| {
                            malformed(p.line, "LWPOLYLINE: bulge before the first vertex")
                        })?;
                        last.1 = b;
                    }
                    _ => {}
                }
            }
            if let Some(n) = e.int(90)?
                && n != verts.len() as i64
            {
                return Err(malformed(
                    e.line,
                    format!("LWPOLYLINE declares {n} vertices but has {}", verts.len()),
                ));
            }
            let coords: Vec<f64> = verts.iter().flat_map(|(p, _)| *p).collect();
            if elevation.abs() > planar_tol(&coords) {
                return Ok(U(R_NOT_XY));
            }
            let curves = polyline_curves(&verts, flags & 1 != 0);
            Ok(Classified::Curves(
                curves
                    .into_iter()
                    .map(|c| mirror_curve(c, mirrored))
                    .collect(),
            ))
        }
        "POLYLINE" => {
            let flags = e.int(70)?.unwrap_or(0);
            if flags & (8 | 16 | 64) != 0 {
                return Ok(U(R_3D_POLY));
            }
            if flags & (2 | 4) != 0 {
                return Ok(U(R_FIT_POLY));
            }
            let Some(mirrored) = ocs_mirrored(e)? else {
                return Ok(U(R_EXTRUSION));
            };
            let elevation = e.f_or(30, 0.0)?;
            let mut verts = Vec::with_capacity(vertices.len());
            for v in vertices {
                let vf = v.int(70)?.unwrap_or(0);
                // 1 extra fit vertex, 8 spline vertex, 16 spline frame, 32/64/128 3D/mesh.
                if vf & (1 | 8 | 16 | 32 | 64 | 128) != 0 {
                    return Ok(U(R_FIT_POLY));
                }
                let (x, y, z) = (v.f_req(10)?, v.f_req(20)?, v.f_or(30, elevation)?);
                if (z - elevation).abs() > planar_tol(&[x, y]) {
                    return Ok(U(R_NOT_XY));
                }
                verts.push(([x, y], v.f_or(42, 0.0)?));
            }
            let coords: Vec<f64> = verts.iter().flat_map(|(p, _)| *p).collect();
            if elevation.abs() > planar_tol(&coords) {
                return Ok(U(R_NOT_XY));
            }
            let curves = polyline_curves(&verts, flags & 1 != 0);
            Ok(Classified::Curves(
                curves
                    .into_iter()
                    .map(|c| mirror_curve(c, mirrored))
                    .collect(),
            ))
        }
        "SPLINE" => Ok(match classify_spline(e)? {
            Ok(c) => Classified::Curves(vec![c]),
            Err(why) => U(why),
        }),
        "ELLIPSE" => Ok(match classify_ellipse(e)? {
            Ok(c) => Classified::Curves(vec![c]),
            Err(why) => U(why),
        }),
        "INSERT" => Ok(U("block references are not expanded")),
        "TEXT" | "MTEXT" | "ATTDEF" | "ATTRIB" => Ok(U("text/annotation is not geometry")),
        "DIMENSION" | "LEADER" | "MLEADER" | "MULTILEADER" | "TOLERANCE" => {
            Ok(U("dimensions/annotation are not imported"))
        }
        "HATCH" | "SOLID" | "TRACE" | "WIPEOUT" => Ok(U("filled areas are not supported")),
        "POINT" => Ok(U("points are not imported")),
        "3DFACE" | "3DSOLID" | "BODY" | "REGION" | "SURFACE" | "MESH" | "POLYFACE" => {
            Ok(U("3D/ACIS geometry is not supported"))
        }
        "RAY" | "XLINE" => Ok(U("infinite construction lines are not supported")),
        "VIEWPORT" => Ok(U("viewports are not geometry")),
        "IMAGE" | "OLE2FRAME" | "OLEFRAME" | "UNDERLAY" | "PDFUNDERLAY" => {
            Ok(U("embedded/raster objects are not supported"))
        }
        _ => Ok(U("unsupported entity type")),
    }
}

/// Grid hash merging points within `tol` (first occurrence wins).
pub(crate) struct PointMerger {
    tol: f64,
    cells: HashMap<(i64, i64), Vec<PointId>>,
    merged: usize,
}

impl PointMerger {
    pub(crate) fn new(tol: f64) -> Self {
        Self {
            tol,
            cells: HashMap::new(),
            merged: 0,
        }
    }

    /// Points that were merged into an existing one.
    pub(crate) fn merged(&self) -> usize {
        self.merged
    }

    fn cell(&self, p: [f64; 2]) -> Result<(i64, i64)> {
        let cx = (p[0] / self.tol).floor();
        let cy = (p[1] / self.tol).floor();
        const LIM: f64 = 4.0e18;
        if !(cx.abs() < LIM && cy.abs() < LIM) {
            return Err(ExchangeError::InvalidOption(format!(
                "coordinate {p:?} mm is too large for the join tolerance {} mm",
                self.tol
            )));
        }
        Ok((cx as i64, cy as i64))
    }

    pub(crate) fn point(&mut self, s: &mut Sketch, p: [f64; 2]) -> Result<PointId> {
        if self.tol > 0.0 {
            let (cx, cy) = self.cell(p)?;
            let mut best: Option<(f64, PointId)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(ids) = self.cells.get(&(cx + dx, cy + dy)) {
                        for &id in ids {
                            if let Some(q) = s.point_xy(id) {
                                let d = (q[0] - p[0]).hypot(q[1] - p[1]);
                                if d <= self.tol && best.is_none_or(|(bd, _)| d < bd) {
                                    best = Some((d, id));
                                }
                            }
                        }
                    }
                }
            }
            if let Some((_, id)) = best {
                self.merged += 1;
                return Ok(id);
            }
            let id = push_point(s, p);
            self.cells.entry((cx, cy)).or_default().push(id);
            Ok(id)
        } else {
            Ok(push_point(s, p))
        }
    }
}

/// O(1) point insertion (ids are dense and allocated here).
pub(crate) fn push_point(s: &mut Sketch, p: [f64; 2]) -> PointId {
    let id = PointId(s.points.len() as u32);
    s.points.push(SketchPoint {
        id,
        x: p[0],
        y: p[1],
    });
    id
}

/// Import an ASCII DXF as an unconstrained sketch. See the module docs.
pub fn import(bytes: &[u8], opts: &DxfImportOptions) -> Result<DxfImport> {
    if !(opts.join_tolerance_mm.is_finite() && opts.join_tolerance_mm >= 0.0) {
        return Err(ExchangeError::InvalidOption(format!(
            "join tolerance must be finite and >= 0 mm (got {})",
            opts.join_tolerance_mm
        )));
    }
    if let Some(s) = opts.unit_override_to_mm {
        crate::check_scale("unit override", s)?;
    }
    if bytes.len() as u64 > opts.max_bytes {
        return Err(ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "bytes".into(),
            limit: opts.max_bytes,
            actual: bytes.len() as u64,
        });
    }
    // A pair needs at least 4 bytes ("0\nX\n"), so this never truncates a valid file.
    let pairs = reader::tokenize(bytes, bytes.len() as u64 / 4 + 1)?;
    let file = reader::parse(&pairs, opts.max_entities)?;
    if !file.has_entities_section {
        return Err(ExchangeError::Empty {
            format: FMT.into(),
            message: "the file has no ENTITIES section".into(),
        });
    }

    let header_int = |name: &str| -> Result<Option<i64>> {
        match file.header.get(name).and_then(|v| v.first()) {
            Some(p) => reader::parse_int(p).map(Some),
            None => Ok(None),
        }
    };
    let insunits = header_int("$INSUNITS")?;
    let measurement = header_int("$MEASUREMENT")?;
    let version = file
        .header
        .get("$ACADVER")
        .and_then(|v| v.first())
        .map(|p| p.value.trim().to_owned());

    // Group POLYLINE + VERTEX* + SEQEND into one logical entity.
    let mut logical: Vec<(&RawEntity, &[RawEntity])> = Vec::new();
    let ents = &file.entities;
    let mut i = 0;
    while i < ents.len() {
        let e = &ents[i];
        if e.kind == "POLYLINE" {
            let mut k = i + 1;
            while k < ents.len() && ents[k].kind == "VERTEX" {
                k += 1;
            }
            if k >= ents.len() || ents[k].kind != "SEQEND" {
                return Err(malformed(
                    e.line,
                    "POLYLINE without a SEQEND after its VERTEX records",
                ));
            }
            logical.push((e, &ents[i + 1..k]));
            i = k + 1;
        } else if e.kind == "VERTEX" || e.kind == "SEQEND" {
            return Err(malformed(e.line, format!("{} outside a POLYLINE", e.kind)));
        } else {
            logical.push((e, &[]));
            i += 1;
        }
    }

    // Layers.
    let wanted: Option<Vec<String>> = opts
        .layers
        .as_ref()
        .map(|v| v.iter().map(|s| s.trim().to_ascii_lowercase()).collect());
    let selected = |layer: &str| {
        wanted
            .as_ref()
            .is_none_or(|w| w.iter().any(|x| *x == layer.to_ascii_lowercase()))
    };
    let mut layers: Vec<DxfLayer> = file
        .table_layers
        .iter()
        .map(|n| DxfLayer {
            name: n.clone(),
            entities: 0,
            selected: selected(n),
            in_table: true,
        })
        .collect();
    let mut warnings: Vec<String> = Vec::new();

    // Classify (unit independent).
    let mut skips = Skips::default();
    let mut unsupported = Skips::default();
    let mut imported: BTreeMap<String, usize> = BTreeMap::new();
    let mut curves: Vec<(Curve, &str, usize)> = Vec::new(); // curve, DXF type, line
    for (e, verts) in &logical {
        let layer = e.layer();
        match layers
            .iter_mut()
            .find(|l| l.name.eq_ignore_ascii_case(layer))
        {
            Some(l) => l.entities += 1,
            None => layers.push(DxfLayer {
                name: layer.to_owned(),
                entities: 1,
                selected: selected(layer),
                in_table: false,
            }),
        }
        if !selected(layer) {
            continue;
        }
        if e.f_or(39, 0.0)? != 0.0 {
            warnings.push(format!(
                "line {}: {} thickness (group 39) ignored; only the 2D curve is imported",
                e.line, e.kind
            ));
        }
        let widths = [40, 41, 43].iter().try_fold(false, |acc, &c| {
            let has = if e.kind == "LWPOLYLINE" {
                e.pairs
                    .iter()
                    .filter(|p| p.code == c)
                    .try_fold(false, |a, p| {
                        Ok::<_, ExchangeError>(a || reader::parse_f64(p)? != 0.0)
                    })?
            } else if e.kind == "POLYLINE" {
                e.f_or(c, 0.0)? != 0.0
                    || verts.iter().try_fold(false, |a, v| {
                        Ok::<_, ExchangeError>(a || v.f_or(c, 0.0)? != 0.0)
                    })?
            } else {
                false
            };
            Ok::<_, ExchangeError>(acc || has)
        })?;
        if widths {
            warnings.push(format!(
                "line {}: {} width ignored; only the centre line is imported",
                e.line, e.kind
            ));
        }
        match classify(e, verts)? {
            Classified::Curves(cs) if cs.is_empty() => {
                skips.add(&e.kind, "polyline with fewer than 2 vertices", e.line);
                warnings.push(format!(
                    "line {}: dropped {} with fewer than 2 vertices",
                    e.line, e.kind
                ));
            }
            Classified::Curves(cs) => {
                *imported.entry(e.kind.clone()).or_default() += 1;
                curves.extend(cs.into_iter().map(|c| (c, e.kind.as_str(), e.line)));
            }
            Classified::Unsupported(reason) => unsupported.add(&e.kind, reason, e.line),
        }
    }
    if let Some(w) = &wanted {
        for name in w {
            if !layers.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
                warnings.push(format!("layer '{name}' does not exist in the file"));
            }
        }
    }
    if !unsupported.0.is_empty() && !opts.allow_partial {
        return Err(ExchangeError::Unsupported {
            format: FMT.into(),
            entities: unsupported.list(),
        });
    }
    for u in unsupported.list() {
        warnings.push(format!(
            "skipped {} {} ({})",
            u.count, u.entity_type, u.reason
        ));
        skips
            .0
            .insert((u.entity_type, u.reason), (u.count, u.first_line));
    }

    // Units.
    let units = match insunits.filter(|&c| c != 0) {
        Some(code) => match insunits_to_mm(code) {
            Some((name, to_mm)) => LengthUnit {
                name: name.into(),
                to_mm,
                declared: true,
                insunits,
            },
            None => {
                return Err(ExchangeError::UnsupportedUnits {
                    format: FMT.into(),
                    message: format!("$INSUNITS = {code}"),
                });
            }
        },
        None => match opts.unit_override_to_mm {
            Some(to_mm) => LengthUnit {
                name: "user override".into(),
                to_mm,
                declared: false,
                insunits,
            },
            None => {
                return Err(ExchangeError::UnitsRequired {
                    format: FMT.into(),
                    measurement: measurement.map(|m| m as i32),
                });
            }
        },
    };
    let k = units.to_mm;

    // Scale + build the sketch.
    let tol = opts.join_tolerance_mm;
    let mut sketch = Sketch::default();
    sketch.ensure_origin();
    let mut merger = PointMerger {
        tol,
        cells: HashMap::new(),
        merged: 0,
    };
    let sc = |p: [f64; 2]| [p[0] * k, p[1] * k];
    let finite = |p: [f64; 2], line: usize| {
        if p[0].is_finite() && p[1].is_finite() {
            Ok(p)
        } else {
            Err(ExchangeError::NonFinite {
                format: FMT.into(),
                line: Some(line),
                message: "coordinate overflows after unit scaling".into(),
            })
        }
    };
    let degenerate_tol = tol.max(f64::MIN_POSITIVE);
    let mut degenerate = |kind: &str, reason: &str, line: usize, w: &mut Vec<String>| {
        skips.add(kind, reason, line);
        w.push(format!(
            "line {line}: dropped degenerate {kind} segment ({reason})"
        ));
    };
    for (c, kind, line) in curves {
        match c {
            Curve::Line { a, b } => {
                let (a, b) = (finite(sc(a), line)?, finite(sc(b), line)?);
                if (b[0] - a[0]).hypot(b[1] - a[1]) <= degenerate_tol {
                    degenerate(kind, "zero-length line segment", line, &mut warnings);
                    continue;
                }
                let pa = merger.point(&mut sketch, a)?;
                let pb = merger.point(&mut sketch, b)?;
                if pa == pb {
                    degenerate(kind, "zero-length line segment", line, &mut warnings);
                    continue;
                }
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Line { id, a: pa, b: pb });
            }
            Curve::Arc { center, start, end } => {
                let (c, s, e) = (
                    finite(sc(center), line)?,
                    finite(sc(start), line)?,
                    finite(sc(end), line)?,
                );
                let r = (s[0] - c[0]).hypot(s[1] - c[1]);
                if r <= degenerate_tol || (e[0] - s[0]).hypot(e[1] - s[1]) <= degenerate_tol {
                    degenerate(
                        kind,
                        "zero radius or zero/full sweep arc",
                        line,
                        &mut warnings,
                    );
                    continue;
                }
                let ps = merger.point(&mut sketch, s)?;
                let pe = merger.point(&mut sketch, e)?;
                if ps == pe {
                    degenerate(
                        kind,
                        "zero radius or zero/full sweep arc",
                        line,
                        &mut warnings,
                    );
                    continue;
                }
                let pc = push_point(&mut sketch, c);
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Arc {
                    id,
                    center: pc,
                    start: ps,
                    end: pe,
                });
            }
            Curve::Circle { center, radius } => {
                let c = finite(sc(center), line)?;
                let r = radius * k;
                if !(r.is_finite() && r > degenerate_tol) {
                    degenerate(kind, "zero or negative radius circle", line, &mut warnings);
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
            Curve::Ellipse {
                center,
                major,
                ratio,
            } => {
                let (c, m) = (finite(sc(center), line)?, finite(sc(major), line)?);
                let a = (m[0] - c[0]).hypot(m[1] - c[1]);
                let b = a * ratio;
                if !(a > degenerate_tol && b.is_finite() && b > degenerate_tol) {
                    degenerate(kind, "zero or negative ellipse radius", line, &mut warnings);
                    continue;
                }
                let (pc, pm) = (push_point(&mut sketch, c), push_point(&mut sketch, m));
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Ellipse {
                    id,
                    center: pc,
                    major: pm,
                    minor: b,
                });
            }
            Curve::Spline { ctrl } => {
                let n = ctrl.len();
                let mut ids = Vec::with_capacity(n);
                for (i, p) in ctrl.iter().enumerate() {
                    let q = finite(sc(*p), line)?;
                    // The ends join other curves; interior control points stay separate.
                    ids.push(if i == 0 || i + 1 == n {
                        merger.point(&mut sketch, q)?
                    } else {
                        push_point(&mut sketch, q)
                    });
                }
                let mut seen = std::collections::HashSet::new();
                if !ids.iter().all(|p| seen.insert(*p)) {
                    // Never drop an outline silently: refuse the file.
                    return Err(ExchangeError::Sketch(format!(
                        "line {line}: the SPLINE's ends meet within the join tolerance (a closed \
                         spline); closed DXF splines are not supported"
                    )));
                }
                let id = sketch.next_entity_id_fast();
                sketch.entities.push(Entity::Spline {
                    id,
                    points: ids,
                    closed: false,
                });
            }
        }
    }
    if sketch.entities.is_empty() {
        return Err(ExchangeError::Empty {
            format: FMT.into(),
            message: if logical.is_empty() {
                "the ENTITIES section is empty".into()
            } else {
                "no supported, non-degenerate entity on the selected layers".into()
            },
        });
    }
    if merger.merged > 0 {
        warnings.push(format!(
            "{} endpoints joined to an existing point within {tol:e} mm",
            merger.merged
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
    let before: Vec<[f64; 2]> = sketch.points.iter().map(|p| [p.x, p.y]).collect();
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
    let shift = sketch
        .points
        .iter()
        .zip(&before)
        .map(|(p, b)| (p.x - b[0]).hypot(p.y - b[1]))
        .fold(0.0, f64::max);
    if shift > tol.max(rcad_sketch::solve_tolerance(sketch.scale())) {
        warnings.push(format!(
            "solving the arc equations moved a point by {shift:e} mm"
        ));
    }

    Ok(DxfImport {
        sketch,
        units,
        version,
        measurement,
        layers,
        imported: imported
            .into_iter()
            .map(|(entity_type, count)| EntityCount { entity_type, count })
            .collect(),
        skipped: skips.list(),
        warnings,
        solve,
    })
}

/// Line/arc/open-spline endpoints used by exactly one curve.
pub(crate) fn open_endpoints(s: &Sketch) -> usize {
    let mut deg: HashMap<PointId, usize> = HashMap::new();
    for e in &s.entities {
        if let Some((a, b)) = e.ends() {
            *deg.entry(a).or_default() += 1;
            *deg.entry(b).or_default() += 1;
        }
    }
    deg.values().filter(|&&d| d == 1).count()
}

pub(crate) trait FastIds {
    fn next_entity_id_fast(&self) -> rcad_sketch::EntityId;
}

impl FastIds for Sketch {
    /// Entities are only appended here with dense ids starting at 1.
    fn next_entity_id_fast(&self) -> rcad_sketch::EntityId {
        rcad_sketch::EntityId(self.entities.len() as u32 + 1)
    }
}
