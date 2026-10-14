//! DXF export of a drawing sheet (docs/DRAWINGS.md).
//!
//! R12 (AC1009) ASCII like the sketch exporter (`crate::dxf::export`): HEADER
//! (`$INSUNITS` 4 = mm, `$MEASUREMENT` 1, `$EXTMIN`/`$EXTMAX` = the sheet),
//! TABLES (LTYPE `CONTINUOUS`, `DASHED`, `CENTER`; one LAYER per line style)
//! and ENTITIES, in sheet millimetres (origin bottom-left, y up). Layers:
//! `VISIBLE`, `HIDDEN` (DASHED), `CENTER` (CENTER), `THIN` (dimensions,
//! leaders, title block), `FRAME` (border), `FLAG` (red; flagged views and
//! dangling callouts). Curves: `LINE`, `ARC` (degrees, counter-clockwise),
//! `CIRCLE`; polylines are written as consecutive `LINE`s; filled arrowheads
//! as `SOLID`; text as `TEXT` with horizontal justification, `Ø` as the
//! standard `%%c` control code. Line type scale: dash lengths are in mm, as
//! in the PDF/SVG output. Deterministic: fixed 6-decimal numbers.
use crate::drawing::{Anchor, LineStyle, Prim, SheetPlot, num};
use std::fmt::Write as _;

fn layer(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Visible => "VISIBLE",
        LineStyle::Hidden => "HIDDEN",
        LineStyle::Center => "CENTER",
        LineStyle::Thin => "THIN",
        LineStyle::Frame => "FRAME",
        LineStyle::Flag => "FLAG",
    }
}

const LAYERS: [LineStyle; 6] = [
    LineStyle::Visible,
    LineStyle::Hidden,
    LineStyle::Center,
    LineStyle::Thin,
    LineStyle::Frame,
    LineStyle::Flag,
];

struct W(String);

impl W {
    fn g(&mut self, code: i32, value: &str) {
        let _ = write!(self.0, "{code:>3}\n{value}\n");
    }
    fn f(&mut self, code: i32, v: f64) {
        self.g(code, &num(v, 6));
    }
    fn line(&mut self, l: &str, a: [f64; 2], b: [f64; 2]) {
        self.g(0, "LINE");
        self.g(8, l);
        self.f(10, a[0]);
        self.f(20, a[1]);
        self.f(30, 0.0);
        self.f(11, b[0]);
        self.f(21, b[1]);
        self.f(31, 0.0);
    }
}

/// Text in the DXF code page subset: `Ø` → `%%c`, `×` → `x`, other non-ASCII → `?`.
fn dxf_text(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            'Ø' | 'ø' => o.push_str("%%c"),
            '×' => o.push('x'),
            '°' => o.push_str("%%d"),
            '±' => o.push_str("%%p"),
            '%' => o.push_str("%%%"),
            c if c.is_ascii() && !c.is_ascii_control() => o.push(c),
            _ => o.push('?'),
        }
    }
    o
}

/// Write `sheet` as an R12 ASCII DXF (mm).
pub fn write_dxf(sheet: &SheetPlot) -> String {
    let mut w = W(String::new());
    w.g(0, "SECTION");
    w.g(2, "HEADER");
    w.g(9, "$ACADVER");
    w.g(1, "AC1009");
    w.g(9, "$INSUNITS");
    w.g(70, "4");
    w.g(9, "$MEASUREMENT");
    w.g(70, "1");
    w.g(9, "$EXTMIN");
    w.f(10, 0.0);
    w.f(20, 0.0);
    w.f(30, 0.0);
    w.g(9, "$EXTMAX");
    w.f(10, sheet.width);
    w.f(20, sheet.height);
    w.f(30, 0.0);
    w.g(9, "$LTSCALE");
    w.f(40, 1.0);
    w.g(0, "ENDSEC");
    w.g(0, "SECTION");
    w.g(2, "TABLES");
    w.g(0, "TABLE");
    w.g(2, "LTYPE");
    w.g(70, "3");
    for (name, desc, pattern) in [
        ("CONTINUOUS", "Solid line", &[][..]),
        ("DASHED", "Dashed __ __", LineStyle::Hidden.dashes()),
        ("CENTER", "Center ____ _ ____", LineStyle::Center.dashes()),
    ] {
        w.g(0, "LTYPE");
        w.g(2, name);
        w.g(70, "0");
        w.g(3, desc);
        w.g(72, "65");
        w.g(73, &pattern.len().to_string());
        w.f(40, pattern.iter().sum());
        for (i, d) in pattern.iter().enumerate() {
            // DXF: positive = dash, negative = gap.
            w.f(49, if i % 2 == 0 { *d } else { -d });
        }
    }
    w.g(0, "ENDTAB");
    w.g(0, "TABLE");
    w.g(2, "LAYER");
    w.g(70, &LAYERS.len().to_string());
    for s in LAYERS {
        w.g(0, "LAYER");
        w.g(2, layer(s));
        w.g(70, "0");
        w.g(62, if s == LineStyle::Flag { "1" } else { "7" });
        w.g(
            6,
            match s {
                LineStyle::Hidden => "DASHED",
                LineStyle::Center => "CENTER",
                _ => "CONTINUOUS",
            },
        );
    }
    w.g(0, "ENDTAB");
    w.g(0, "ENDSEC");
    w.g(0, "SECTION");
    w.g(2, "ENTITIES");
    for g in &sheet.groups {
        for p in &g.prims {
            match p {
                Prim::Line { a, b, style } => w.line(layer(*style), *a, *b),
                Prim::Arc {
                    c,
                    r,
                    start,
                    sweep,
                    style,
                } => {
                    if p.is_full_circle() {
                        w.g(0, "CIRCLE");
                        w.g(8, layer(*style));
                        w.f(10, c[0]);
                        w.f(20, c[1]);
                        w.f(30, 0.0);
                        w.f(40, *r);
                    } else {
                        w.g(0, "ARC");
                        w.g(8, layer(*style));
                        w.f(10, c[0]);
                        w.f(20, c[1]);
                        w.f(30, 0.0);
                        w.f(40, *r);
                        w.f(50, start.to_degrees().rem_euclid(360.0));
                        w.f(51, (start + sweep).to_degrees().rem_euclid(360.0));
                    }
                }
                Prim::Polyline { pts, style } => {
                    for s in pts.windows(2) {
                        w.line(layer(*style), s[0], s[1]);
                    }
                }
                Prim::Polygon { pts, fill, style } => {
                    if *fill && pts.len() == 3 {
                        // SOLID: corners 1, 2, 4, 3 (a triangle repeats the last).
                        w.g(0, "SOLID");
                        w.g(8, layer(*style));
                        for (k, q) in [pts[0], pts[1], pts[2], pts[2]].iter().enumerate() {
                            w.f(10 + k as i32, q[0]);
                            w.f(20 + k as i32, q[1]);
                            w.f(30 + k as i32, 0.0);
                        }
                    } else {
                        for i in 0..pts.len() {
                            w.line(layer(*style), pts[i], pts[(i + 1) % pts.len()]);
                        }
                    }
                }
                Prim::Text {
                    at,
                    size,
                    text,
                    anchor,
                    style,
                } => {
                    w.g(0, "TEXT");
                    w.g(8, layer(*style));
                    w.f(10, at[0]);
                    w.f(20, at[1]);
                    w.f(30, 0.0);
                    w.f(40, *size);
                    w.g(1, &dxf_text(text));
                    let j = match anchor {
                        Anchor::Start => 0,
                        Anchor::Middle => 1,
                        Anchor::End => 2,
                    };
                    if j != 0 {
                        w.g(72, &j.to_string());
                        w.f(11, at[0]);
                        w.f(21, at[1]);
                        w.f(31, 0.0);
                    }
                }
            }
        }
    }
    w.g(0, "ENDSEC");
    w.g(0, "EOF");
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::Group;

    #[test]
    fn layers_entities_and_text_codes() {
        let s = SheetPlot {
            width: 297.0,
            height: 210.0,
            title: "t".into(),
            groups: vec![Group {
                id: "view-1".into(),
                label: "v".into(),
                prims: vec![
                    Prim::Line {
                        a: [0.0, 0.0],
                        b: [10.0, 0.0],
                        style: LineStyle::Hidden,
                    },
                    Prim::Arc {
                        c: [5.0, 5.0],
                        r: 2.0,
                        start: 0.0,
                        sweep: std::f64::consts::FRAC_PI_2,
                        style: LineStyle::Visible,
                    },
                    Prim::Text {
                        at: [1.0, 1.0],
                        size: 3.5,
                        text: "2× Ø8".into(),
                        anchor: Anchor::Middle,
                        style: LineStyle::Thin,
                    },
                ],
            }],
        };
        let d = write_dxf(&s);
        assert_eq!(d, write_dxf(&s));
        assert!(d.contains("  0\nLINE\n  8\nHIDDEN\n"));
        assert!(d.contains("  0\nARC\n  8\nVISIBLE\n"));
        assert!(d.contains(" 51\n90.000000\n"));
        assert!(d.contains("  1\n2x %%c8\n"));
        assert!(d.ends_with("  0\nEOF\n"));
    }
}
