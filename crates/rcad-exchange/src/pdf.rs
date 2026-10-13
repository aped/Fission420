//! Minimal vector PDF writer for drawing sheets (run 6, docs/DRAWINGS.md).
//!
//! One page, PDF 1.4, no compression, no embedded fonts: text uses the
//! standard Type 1 font Helvetica with WinAnsiEncoding (every conforming
//! reader provides it). The page's MediaBox is the sheet size in points
//! (1 mm = 72/25.4 pt); the content stream starts with a `cm` scaling to
//! millimetres, so every coordinate, line width and dash length in the stream
//! is in millimetres, y up from the bottom-left corner (the sheet convention).
//! Each [`Group`](crate::drawing::Group) is a marked-content sequence
//! `/RCADGroup <</Name (id)>> BDC … EMC`.
//!
//! Circles and arcs are written as cubic Bézier curves, at most 90° per
//! segment (PDF has no arc operator); the radial deviation of that standard
//! approximation is below 2.8e-4 × radius. Lines are exact. Output is
//! deterministic (no dates, no ids; fixed 4-decimal numbers).
use crate::drawing::{Anchor, LineStyle, Prim, SheetPlot, num};
use std::fmt::Write as _;

/// Points per millimetre.
pub const PT_PER_MM: f64 = 72.0 / 25.4;

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126 (Adobe core-font
/// metrics), used only to place centred / right-aligned text.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278,
    278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // ':'..'@'
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A-Z
    278, 278, 278, 469, 556, 333, // '['..'`'
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a-z
    334, 260, 334, 584, // '{'..'~'
];

/// WinAnsi code and Helvetica width of a character; unsupported → '?'.
fn glyph(c: char) -> (u8, u16) {
    match c {
        ' '..='~' => (c as u8, HELVETICA[(c as u32 - 32) as usize]),
        'Ø' => (0xD8, 778),
        'ø' => (0xF8, 611),
        '×' => (0xD7, 584),
        '°' => (0xB0, 400),
        '±' => (0xB1, 584),
        _ => (b'?', 556),
    }
}

/// Width of `text` at `size` (same units as `size`).
pub fn text_width(text: &str, size: f64) -> f64 {
    text.chars().map(|c| glyph(c).1 as f64).sum::<f64>() * size / 1000.0
}

/// PDF literal string of `text` in WinAnsiEncoding.
fn pdf_string(text: &str) -> String {
    let mut o = String::from("(");
    for c in text.chars() {
        let (b, _) = glyph(c);
        match b {
            b'(' | b')' | b'\\' => {
                o.push('\\');
                o.push(b as char);
            }
            0x20..=0x7e => o.push(b as char),
            _ => {
                let _ = write!(o, "\\{b:03o}");
            }
        }
    }
    o.push(')');
    o
}

fn n(x: f64) -> String {
    num(x, 4)
}

fn set_style(o: &mut String, s: LineStyle) {
    let [r, g, b] = s.rgb();
    let d: Vec<String> = s.dashes().iter().map(|x| n(*x)).collect();
    let _ = writeln!(
        o,
        "{} {} {} RG {} {} {} rg {} w [{}] 0 d",
        n(r),
        n(g),
        n(b),
        n(r),
        n(g),
        n(b),
        n(s.width()),
        d.join(" ")
    );
}

/// Bézier path of an arc (move-to included).
fn arc_path(o: &mut String, c: [f64; 2], r: f64, start: f64, sweep: f64) {
    let segs = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / segs as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let pt = |t: f64| [c[0] + r * t.cos(), c[1] + r * t.sin()];
    let p0 = pt(start);
    let _ = writeln!(o, "{} {} m", n(p0[0]), n(p0[1]));
    for i in 0..segs {
        let (t0, t1) = (start + step * i as f64, start + step * (i + 1) as f64);
        let (a, b) = (pt(t0), pt(t1));
        let c1 = [a[0] - k * r * t0.sin(), a[1] + k * r * t0.cos()];
        let c2 = [b[0] + k * r * t1.sin(), b[1] - k * r * t1.cos()];
        let _ = writeln!(
            o,
            "{} {} {} {} {} {} c",
            n(c1[0]),
            n(c1[1]),
            n(c2[0]),
            n(c2[1]),
            n(b[0]),
            n(b[1])
        );
    }
}

/// The page content stream (mm space after the initial `cm`).
fn content(sheet: &SheetPlot) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{} 0 0 {} 0 0 cm", n(PT_PER_MM), n(PT_PER_MM));
    let _ = writeln!(o, "1 J 1 j");
    for g in &sheet.groups {
        let _ = writeln!(o, "/RCADGroup <</Name {}>> BDC", pdf_string(&g.id));
        let mut current: Option<LineStyle> = None;
        for p in &g.prims {
            let style = match p {
                Prim::Line { style, .. }
                | Prim::Arc { style, .. }
                | Prim::Polyline { style, .. }
                | Prim::Polygon { style, .. }
                | Prim::Text { style, .. } => *style,
            };
            if current != Some(style) {
                set_style(&mut o, style);
                current = Some(style);
            }
            match p {
                Prim::Line { a, b, .. } => {
                    let _ = writeln!(o, "{} {} m {} {} l S", n(a[0]), n(a[1]), n(b[0]), n(b[1]));
                }
                Prim::Arc {
                    c, r, start, sweep, ..
                } => {
                    arc_path(&mut o, *c, *r, *start, *sweep);
                    let _ = writeln!(o, "{}", if p.is_full_circle() { "h S" } else { "S" });
                }
                Prim::Polyline { pts, .. } | Prim::Polygon { pts, .. } => {
                    for (i, q) in pts.iter().enumerate() {
                        let op = if i == 0 { "m" } else { "l" };
                        let _ = writeln!(o, "{} {} {op}", n(q[0]), n(q[1]));
                    }
                    let op = match p {
                        Prim::Polygon { fill: true, .. } => "h f",
                        Prim::Polygon { .. } => "h S",
                        _ => "S",
                    };
                    let _ = writeln!(o, "{op}");
                }
                Prim::Text {
                    at,
                    size,
                    text,
                    anchor,
                    ..
                } => {
                    let w = text_width(text, *size);
                    let x = match anchor {
                        Anchor::Start => at[0],
                        Anchor::Middle => at[0] - w / 2.0,
                        Anchor::End => at[0] - w,
                    };
                    let _ = writeln!(
                        o,
                        "BT /F1 {} Tf {} {} Td {} Tj ET",
                        n(*size),
                        n(x),
                        n(at[1]),
                        pdf_string(text)
                    );
                }
            }
        }
        let _ = writeln!(o, "EMC");
    }
    o
}

/// Write `sheet` as a one-page vector PDF.
pub fn write_pdf(sheet: &SheetPlot) -> Vec<u8> {
    let stream = content(sheet);
    let (w, h) = (sheet.width * PT_PER_MM, sheet.height * PT_PER_MM);
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
            n(w),
            n(h)
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}endstream",
            stream.len(),
            stream
        ),
        format!(
            "<< /Title {} /Producer (R-CAD) >>",
            pdf_string(&sheet.title)
        ),
    ];
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in offsets {
        let _ = writeln!(tail, "{off:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    out.extend_from_slice(tail.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::Group;

    #[test]
    fn xref_offsets_point_at_objects() {
        let s = SheetPlot {
            width: 297.0,
            height: 210.0,
            title: "Plate (Ø8)".into(),
            groups: vec![Group {
                id: "view-1".into(),
                label: "Front".into(),
                prims: vec![
                    Prim::Line {
                        a: [0.0, 0.0],
                        b: [10.0, 0.0],
                        style: LineStyle::Visible,
                    },
                    Prim::Arc {
                        c: [50.0, 50.0],
                        r: 4.0,
                        start: 0.0,
                        sweep: std::f64::consts::TAU,
                        style: LineStyle::Visible,
                    },
                    Prim::Text {
                        at: [5.0, 5.0],
                        size: 3.5,
                        text: "Ø8 (a)".into(),
                        anchor: Anchor::Middle,
                        style: LineStyle::Thin,
                    },
                ],
            }],
        };
        let pdf = write_pdf(&s);
        assert_eq!(pdf, write_pdf(&s));
        // Latin-1 view: one char per byte, so char offsets are byte offsets.
        let text: Vec<char> = pdf.iter().map(|&b| b as char).collect();
        let at = |i: usize, pat: &str| text[i..].iter().take(pat.len()).collect::<String>() == pat;
        let s: String = text.iter().collect();
        let sx = s.rfind("startxref\n").unwrap();
        let xref: usize = s[sx + 10..].lines().next().unwrap().parse().unwrap();
        // `s` has 2-byte chars only in the 4-byte binary comment at the start.
        let xref_chars = xref;
        assert!(at(xref_chars, "xref"));
        let tail: String = text[xref..].iter().collect();
        for (i, line) in tail.lines().skip(3).take(6).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(at(off, &format!("{} 0 obj", i + 1)), "object {}", i + 1);
        }
        let text = s;
        assert!(text.contains("(\\3308 \\(a\\)) Tj"), "{text}");
        assert!(text.contains("841.8898 595.2756"));
    }
}
