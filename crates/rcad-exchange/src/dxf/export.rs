//! DXF export of a sketch's lines, arcs and circles in its local 2D coordinates (mm).
//!
//! Flavour: **R12 (AC1009) ASCII** with HEADER, TABLES (LTYPE `CONTINUOUS`, LAYER) and
//! ENTITIES. R12 entities need no handles, owner pointers, subclass markers, CLASSES or
//! OBJECTS dictionaries, which R2000+ (AC1015) readers require to be mutually consistent; a
//! minimal R12 file is the most widely accepted subset (CAD, CAM, laser/CNC tools, FreeCAD,
//! LibreCAD, ezdxf). `$INSUNITS = 4` (mm) and `$MEASUREMENT = 1` are written in the header:
//! they post-date R12, R12 readers skip unknown header variables, and newer readers use them
//! to scale. Numbers are written in the shortest form that round-trips the `f64` exactly.

use crate::{ExchangeError, Result};
use rcad_sketch::{Entity, Sketch};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DxfExportOptions {
    /// Layer of normal geometry.
    pub layer: String,
    /// Also write construction entities (on `construction_layer`).
    pub include_construction: bool,
    pub construction_layer: String,
}

impl Default for DxfExportOptions {
    fn default() -> Self {
        Self {
            layer: "0".into(),
            include_construction: false,
            construction_layer: "CONSTRUCTION".into(),
        }
    }
}

/// Shortest round-trip decimal, always with a decimal point, never `-0`.
fn num(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    let s = format!("{v}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

fn check_layer(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.len() > 255
        || name
            .chars()
            .any(|c| c.is_control() || "<>/\\\":;?*|=`".contains(c));
    if bad {
        Err(ExchangeError::InvalidOption(format!(
            "'{name}' is not a valid DXF layer name"
        )))
    } else {
        Ok(())
    }
}

struct W(String);

impl W {
    fn g(&mut self, code: i32, value: &str) {
        let _ = write!(self.0, "{code:>3}\n{value}\n");
    }
    fn f(&mut self, code: i32, v: f64) {
        self.g(code, &num(v));
    }
}

/// Export `sketch` (current coordinates; solve it first) as an R12 ASCII DXF string.
/// Only `Line`, `Arc` and `Circle` entities exist in the sketch model; construction entities
/// are excluded unless `include_construction`. Errors on an invalid sketch or layer name.
pub fn export(sketch: &Sketch, opts: &DxfExportOptions) -> Result<String> {
    sketch
        .validate()
        .map_err(|e| ExchangeError::Sketch(e.to_string()))?;
    check_layer(&opts.layer)?;
    if opts.include_construction {
        check_layer(&opts.construction_layer)?;
    }
    let mut ents = W(String::new());
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut grow = |p: [f64; 2], r: f64| {
        lo = [lo[0].min(p[0] - r), lo[1].min(p[1] - r)];
        hi = [hi[0].max(p[0] + r), hi[1].max(p[1] + r)];
    };
    let mut used_construction = false;
    for e in &sketch.entities {
        let construction = sketch.is_construction(e.id());
        if construction && !opts.include_construction {
            continue;
        }
        used_construction |= construction;
        let layer = if construction {
            &opts.construction_layer
        } else {
            &opts.layer
        };
        let xy = |id| {
            sketch
                .point_xy(id)
                .ok_or_else(|| ExchangeError::Sketch(format!("missing point {id:?}")))
        };
        match e {
            Entity::Line { a, b, .. } => {
                let (a, b) = (xy(*a)?, xy(*b)?);
                grow(a, 0.0);
                grow(b, 0.0);
                ents.g(0, "LINE");
                ents.g(8, layer);
                ents.f(10, a[0]);
                ents.f(20, a[1]);
                ents.f(30, 0.0);
                ents.f(11, b[0]);
                ents.f(21, b[1]);
                ents.f(31, 0.0);
            }
            Entity::Circle { center, radius, .. } => {
                let c = xy(*center)?;
                grow(c, *radius);
                ents.g(0, "CIRCLE");
                ents.g(8, layer);
                ents.f(10, c[0]);
                ents.f(20, c[1]);
                ents.f(30, 0.0);
                ents.f(40, *radius);
            }
            Entity::Arc { id, .. } => {
                let g = sketch.arc_geometry(*id).ok_or_else(|| {
                    ExchangeError::Sketch(format!("arc {id:?} has no valid geometry"))
                })?;
                // Bounds: conservative (full circle) is not wanted; use endpoints + the
                // axis extremes inside the sweep.
                let at = |a: f64| {
                    [
                        g.center[0] + g.radius * a.cos(),
                        g.center[1] + g.radius * a.sin(),
                    ]
                };
                grow(at(g.start_angle), 0.0);
                grow(at(g.start_angle + g.sweep), 0.0);
                for k in 0..8 {
                    let a = k as f64 * std::f64::consts::FRAC_PI_2;
                    let rel = (a - g.start_angle).rem_euclid(std::f64::consts::TAU);
                    if rel <= g.sweep {
                        grow(at(a), 0.0);
                    }
                }
                let a0 = g.start_angle.to_degrees().rem_euclid(360.0);
                let a1 = (g.start_angle + g.sweep).to_degrees().rem_euclid(360.0);
                ents.g(0, "ARC");
                ents.g(8, layer);
                ents.f(10, g.center[0]);
                ents.f(20, g.center[1]);
                ents.f(30, 0.0);
                ents.f(40, g.radius);
                ents.f(50, a0);
                ents.f(51, a1);
            }
        }
    }
    if !lo[0].is_finite() {
        lo = [0.0; 2];
        hi = [0.0; 2];
    }

    let mut w = W(String::with_capacity(ents.0.len() + 1024));
    w.g(999, "R-CAD sketch export (DXF R12, millimetres)");
    w.g(0, "SECTION");
    w.g(2, "HEADER");
    w.g(9, "$ACADVER");
    w.g(1, "AC1009");
    w.g(9, "$INSUNITS");
    w.g(70, "4");
    w.g(9, "$MEASUREMENT");
    w.g(70, "1");
    w.g(9, "$EXTMIN");
    w.f(10, lo[0]);
    w.f(20, lo[1]);
    w.f(30, 0.0);
    w.g(9, "$EXTMAX");
    w.f(10, hi[0]);
    w.f(20, hi[1]);
    w.f(30, 0.0);
    w.g(0, "ENDSEC");
    w.g(0, "SECTION");
    w.g(2, "TABLES");
    w.g(0, "TABLE");
    w.g(2, "LTYPE");
    w.g(70, "1");
    w.g(0, "LTYPE");
    w.g(2, "CONTINUOUS");
    w.g(70, "0");
    w.g(3, "Solid line");
    w.g(72, "65");
    w.g(73, "0");
    w.f(40, 0.0);
    w.g(0, "ENDTAB");
    let mut layers = vec![(opts.layer.as_str(), 7)];
    if used_construction && !opts.construction_layer.eq_ignore_ascii_case(&opts.layer) {
        layers.push((opts.construction_layer.as_str(), 8));
    }
    w.g(0, "TABLE");
    w.g(2, "LAYER");
    w.g(70, &layers.len().to_string());
    for (name, color) in layers {
        w.g(0, "LAYER");
        w.g(2, name);
        w.g(70, "0");
        w.g(62, &color.to_string());
        w.g(6, "CONTINUOUS");
    }
    w.g(0, "ENDTAB");
    w.g(0, "ENDSEC");
    w.g(0, "SECTION");
    w.g(2, "ENTITIES");
    w.0.push_str(&ents.0);
    w.g(0, "ENDSEC");
    w.g(0, "EOF");
    Ok(w.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip_and_have_a_point() {
        assert_eq!(num(30.0), "30.0");
        assert_eq!(num(-0.0), "0.0");
        assert_eq!(num(0.1), "0.1");
        let v = 1.0 / 3.0;
        assert_eq!(num(v).parse::<f64>().unwrap(), v);
        assert_eq!(num(1e-7).parse::<f64>().unwrap(), 1e-7);
    }

    #[test]
    fn rejects_bad_layer() {
        let s = Sketch::centered_circle(10.0).unwrap();
        let o = DxfExportOptions {
            layer: "a/b".into(),
            ..Default::default()
        };
        assert!(export(&s, &o).is_err());
    }
}
