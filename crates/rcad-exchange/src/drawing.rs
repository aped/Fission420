//! 2-D drawing sheets (run 6, docs/DRAWINGS.md): a neutral list of plotted
//! primitives in sheet millimetres and its SVG writer. The PDF writer is
//! [`crate::pdf`]. Projection, hidden lines and dimensions are computed by
//! `rcad_document::drawing`; this module only writes what it is given.
//!
//! Sheet coordinates: millimetres, origin at the bottom-left corner of the
//! page, y up (drafting convention). The SVG writer flips y for SVG's y-down
//! user space (`width`/`height` in mm, `viewBox` in mm, so one user unit is
//! one millimetre). Output is deterministic: numbers are written with a fixed
//! number of decimals (`-0` normalised) and nothing time- or
//! environment-dependent is included.
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// How a curve is stroked. Widths and dash patterns (mm) follow common ISO
/// 128 practice: visible 0.5, hidden 0.35 dashed, centre 0.25 chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LineStyle {
    /// Visible edge or silhouette.
    Visible,
    /// Hidden edge (dashed).
    Hidden,
    /// Centre line / centre mark (chain).
    Center,
    /// Dimension, extension and leader lines, title block rules.
    Thin,
    /// Sheet border.
    Frame,
    /// A problem marker (a flagged view or a dangling annotation): red.
    Flag,
}

impl LineStyle {
    /// Stroke width (mm).
    pub fn width(self) -> f64 {
        match self {
            LineStyle::Visible => 0.5,
            LineStyle::Hidden => 0.35,
            LineStyle::Center | LineStyle::Thin => 0.25,
            LineStyle::Frame => 0.7,
            LineStyle::Flag => 0.35,
        }
    }
    /// Dash pattern (mm, on/off...); empty: solid.
    pub fn dashes(self) -> &'static [f64] {
        match self {
            LineStyle::Hidden => &[3.0, 1.5],
            LineStyle::Center => &[8.0, 1.5, 1.5, 1.5],
            _ => &[],
        }
    }
    /// RGB stroke colour in 0..=1.
    pub fn rgb(self) -> [f64; 3] {
        match self {
            LineStyle::Flag => [0.8, 0.0, 0.0],
            _ => [0.0, 0.0, 0.0],
        }
    }
    /// Name used as the SVG `class`.
    pub fn name(self) -> &'static str {
        match self {
            LineStyle::Visible => "visible",
            LineStyle::Hidden => "hidden",
            LineStyle::Center => "center",
            LineStyle::Thin => "thin",
            LineStyle::Frame => "frame",
            LineStyle::Flag => "flag",
        }
    }
}

/// Horizontal text alignment about the anchor point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// One plotted primitive (sheet mm, y up).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Prim {
    Line {
        a: [f64; 2],
        b: [f64; 2],
        style: LineStyle,
    },
    /// Arc about `c` of radius `r` from angle `start` turning `sweep` rad
    /// counter-clockwise (sheet y up); `sweep` >= 2π - 1e-12: a full circle.
    Arc {
        c: [f64; 2],
        r: f64,
        start: f64,
        sweep: f64,
        style: LineStyle,
    },
    Polyline {
        pts: Vec<[f64; 2]>,
        style: LineStyle,
    },
    /// Closed polygon; `fill`: filled (arrowheads), else stroked.
    Polygon {
        pts: Vec<[f64; 2]>,
        fill: bool,
        style: LineStyle,
    },
    /// Single-line text, baseline at `at`, cap-height-ish `size` (mm).
    Text {
        at: [f64; 2],
        size: f64,
        text: String,
        anchor: Anchor,
        style: LineStyle,
    },
}

impl Prim {
    pub fn is_full_circle(&self) -> bool {
        matches!(self, Prim::Arc { sweep, .. } if *sweep >= std::f64::consts::TAU - 1e-12)
    }
}

/// A named group (a view, its dimensions, the title block, the border).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// Identifier written to the file (`view-1`, `dims-1`, `title`, `border`).
    pub id: String,
    /// Human-readable label (e.g. "Front (base)").
    pub label: String,
    pub prims: Vec<Prim>,
}

/// One sheet ready to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetPlot {
    pub width: f64,
    pub height: f64,
    pub title: String,
    pub groups: Vec<Group>,
}

/// Fixed-decimal number without `-0`.
pub(crate) fn num(x: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, x);
    let trimmed = s.trim_start_matches('-');
    if trimmed.chars().all(|c| c == '0' || c == '.') {
        trimmed.to_string()
    } else {
        s
    }
}

fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c if (c as u32) < 0x20 => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

fn stroke_attrs(style: LineStyle) -> String {
    let [r, g, b] = style.rgb().map(|c| (c * 255.0).round() as u8);
    let mut s = format!(
        "class=\"{}\" fill=\"none\" stroke=\"#{r:02x}{g:02x}{b:02x}\" stroke-width=\"{}\"",
        style.name(),
        num(style.width(), 2)
    );
    if !style.dashes().is_empty() {
        let d: Vec<String> = style.dashes().iter().map(|x| num(*x, 2)).collect();
        let _ = write!(s, " stroke-dasharray=\"{}\"", d.join(" "));
    }
    s
}

/// Write `sheet` as an SVG document (mm units, 3 decimals).
pub fn write_svg(sheet: &SheetPlot) -> String {
    let h = sheet.height;
    let x = |v: f64| num(v, 3);
    let y = |v: f64| num(h - v, 3);
    let mut o = String::new();
    let _ = writeln!(o, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        o,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{hh}mm\" viewBox=\"0 0 {w} {hh}\">",
        w = num(sheet.width, 3),
        hh = num(sheet.height, 3)
    );
    let _ = writeln!(o, "<title>{}</title>", xml_escape(&sheet.title));
    let _ = writeln!(
        o,
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>",
        num(sheet.width, 3),
        num(sheet.height, 3)
    );
    for g in &sheet.groups {
        let _ = writeln!(
            o,
            "<g id=\"{}\" data-label=\"{}\">",
            xml_escape(&g.id),
            xml_escape(&g.label)
        );
        for p in &g.prims {
            match p {
                Prim::Line { a, b, style } => {
                    let _ = writeln!(
                        o,
                        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" {}/>",
                        x(a[0]),
                        y(a[1]),
                        x(b[0]),
                        y(b[1]),
                        stroke_attrs(*style)
                    );
                }
                Prim::Arc {
                    c,
                    r,
                    start,
                    sweep,
                    style,
                } => {
                    if p.is_full_circle() {
                        let _ = writeln!(
                            o,
                            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {}/>",
                            x(c[0]),
                            y(c[1]),
                            num(*r, 3),
                            stroke_attrs(*style)
                        );
                    } else {
                        let (s, e) = (*start, start + sweep);
                        let p0 = [c[0] + r * s.cos(), c[1] + r * s.sin()];
                        let p1 = [c[0] + r * e.cos(), c[1] + r * e.sin()];
                        // Counter-clockwise with y up is clockwise in SVG's
                        // y-down space: sweep-flag 0.
                        let large = if *sweep > std::f64::consts::PI { 1 } else { 0 };
                        let _ = writeln!(
                            o,
                            "<path d=\"M {} {} A {} {} 0 {large} 0 {} {}\" {}/>",
                            x(p0[0]),
                            y(p0[1]),
                            num(*r, 3),
                            num(*r, 3),
                            x(p1[0]),
                            y(p1[1]),
                            stroke_attrs(*style)
                        );
                    }
                }
                Prim::Polyline { pts, style } | Prim::Polygon { pts, style, .. } => {
                    let tag = if matches!(p, Prim::Polyline { .. }) {
                        "polyline"
                    } else {
                        "polygon"
                    };
                    let list: Vec<String> = pts
                        .iter()
                        .map(|q| format!("{},{}", x(q[0]), y(q[1])))
                        .collect();
                    let mut attrs = stroke_attrs(*style);
                    if let Prim::Polygon { fill: true, .. } = p {
                        attrs = attrs.replace("fill=\"none\"", "fill=\"#000000\"");
                    }
                    let _ = writeln!(o, "<{tag} points=\"{}\" {attrs}/>", list.join(" "));
                }
                Prim::Text {
                    at,
                    size,
                    text,
                    anchor,
                    style,
                } => {
                    let a = match anchor {
                        Anchor::Start => "start",
                        Anchor::Middle => "middle",
                        Anchor::End => "end",
                    };
                    let [r, g, b] = style.rgb().map(|c| (c * 255.0).round() as u8);
                    let _ = writeln!(
                        o,
                        "<text x=\"{}\" y=\"{}\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"{}\" text-anchor=\"{a}\" fill=\"#{r:02x}{g:02x}{b:02x}\" class=\"{}\">{}</text>",
                        x(at[0]),
                        y(at[1]),
                        num(*size, 3),
                        style.name(),
                        xml_escape(text)
                    );
                }
            }
        }
        let _ = writeln!(o, "</g>");
    }
    let _ = writeln!(o, "</svg>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_flip() {
        assert_eq!(num(-0.0000001, 3), "0.000");
        assert_eq!(num(-1.5, 3), "-1.500");
        let s = SheetPlot {
            width: 297.0,
            height: 210.0,
            title: "T & <x>".into(),
            groups: vec![Group {
                id: "view-1".into(),
                label: "Front".into(),
                prims: vec![Prim::Line {
                    a: [10.0, 10.0],
                    b: [20.0, 10.0],
                    style: LineStyle::Hidden,
                }],
            }],
        };
        let svg = write_svg(&s);
        assert!(svg.contains("y1=\"200.000\""), "{svg}");
        assert!(svg.contains("stroke-dasharray=\"3.00 1.50\""));
        assert!(svg.contains("T &amp; &lt;x&gt;"));
        assert_eq!(svg, write_svg(&s));
    }
}
