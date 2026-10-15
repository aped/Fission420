//! Command icons: original, deliberately simple line glyphs drawn with the
//! egui painter (no image assets, nothing derived from another product's
//! icon set). Each glyph is drawn in a square `rect` with stroke colour `c`.

use eframe::egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use super::registry::Icon;

/// Accent fills (body, tool, cut) used sparingly so glyphs read at 20 px.
const BODY: Color32 = Color32::from_rgb(110, 150, 200);
const ADD: Color32 = Color32::from_rgb(110, 190, 120);
const CUT: Color32 = Color32::from_rgb(220, 110, 100);
const NOTE: Color32 = Color32::from_rgb(230, 190, 90);

pub fn paint(icon: Icon, p: &Painter, rect: Rect, c: Color32) {
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(s));
    // Unit-square coordinates (0..1, y down) to screen.
    let at = |x: f32, y: f32| -> Pos2 { pos2(r.min.x + x * s, r.min.y + y * s) };
    let w = (s / 14.0).max(1.2);
    let st = Stroke::new(w, c);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], st);
    let poly = |pts: &[(f32, f32)], fill: Color32| {
        let v: Vec<Pos2> = pts.iter().map(|q| at(q.0, q.1)).collect();
        p.add(eframe::egui::Shape::convex_polygon(v, fill, st));
    };
    let rect_at = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let circle = |x: f32, y: f32, rad: f32, fill: Option<Color32>| match fill {
        Some(f) => p.circle(at(x, y), rad * s, f, st),
        None => p.circle_stroke(at(x, y), rad * s, st),
    };
    // A small box in oblique view (the body most glyphs act on).
    let block = |fill: Color32| {
        poly(
            &[(0.15, 0.45), (0.65, 0.45), (0.65, 0.9), (0.15, 0.9)],
            fill,
        );
        poly(
            &[(0.15, 0.45), (0.35, 0.28), (0.85, 0.28), (0.65, 0.45)],
            fill,
        );
        poly(
            &[(0.65, 0.45), (0.85, 0.28), (0.85, 0.72), (0.65, 0.9)],
            fill,
        );
    };
    let arrow_up = |x: f32, y0: f32, y1: f32| {
        line((x, y0), (x, y1));
        line((x, y1), (x - 0.08, y1 + 0.1));
        line((x, y1), (x + 0.08, y1 + 0.1));
    };
    match icon {
        Icon::New => {
            poly(
                &[
                    (0.25, 0.1),
                    (0.62, 0.1),
                    (0.78, 0.26),
                    (0.78, 0.9),
                    (0.25, 0.9),
                ],
                Color32::TRANSPARENT,
            );
            line((0.52, 0.5), (0.52, 0.74));
            line((0.4, 0.62), (0.64, 0.62));
        }
        Icon::Open => {
            poly(
                &[
                    (0.1, 0.3),
                    (0.38, 0.3),
                    (0.45, 0.38),
                    (0.9, 0.38),
                    (0.9, 0.85),
                    (0.1, 0.85),
                ],
                NOTE,
            );
        }
        Icon::Save => {
            p.rect_stroke(
                rect_at(0.15, 0.15, 0.85, 0.85),
                1.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_filled(rect_at(0.3, 0.15, 0.7, 0.4), 0.0, c);
            p.rect_stroke(
                rect_at(0.28, 0.55, 0.72, 0.85),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
        }
        Icon::Undo | Icon::Redo => {
            let flip = matches!(icon, Icon::Redo);
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = std::f32::consts::PI * (i as f32 / 12.0);
                    at(fx(0.5 + 0.28 * t.cos()), 0.55 - 0.25 * t.sin())
                })
                .collect();
            p.add(eframe::egui::Shape::line(pts, st));
            line((fx(0.22), 0.55), (fx(0.12), 0.42));
            line((fx(0.22), 0.55), (fx(0.34), 0.45));
        }
        Icon::Import => {
            p.rect_stroke(
                rect_at(0.15, 0.55, 0.85, 0.88),
                1.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            line((0.5, 0.1), (0.5, 0.62));
            line((0.5, 0.62), (0.38, 0.5));
            line((0.5, 0.62), (0.62, 0.5));
        }
        Icon::Export => {
            p.rect_stroke(
                rect_at(0.15, 0.55, 0.85, 0.88),
                1.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            arrow_up(0.5, 0.7, 0.12);
        }
        Icon::Sketch => {
            p.rect_stroke(
                rect_at(0.12, 0.3, 0.7, 0.85),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            line((0.45, 0.6), (0.85, 0.15));
            line((0.85, 0.15), (0.92, 0.22));
            line((0.92, 0.22), (0.52, 0.67));
        }
        Icon::Extrude => {
            poly(&[(0.15, 0.7), (0.6, 0.7), (0.85, 0.55), (0.4, 0.55)], ADD);
            arrow_up(0.5, 0.6, 0.12);
        }
        Icon::Cut => {
            block(BODY);
            circle(0.42, 0.62, 0.12, Some(CUT));
        }
        Icon::Revolve => {
            line((0.5, 0.08), (0.5, 0.92));
            p.rect_filled(rect_at(0.56, 0.3, 0.8, 0.7), 0.0, ADD);
            let pts: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = std::f32::consts::PI * (0.15 + 0.7 * i as f32 / 10.0);
                    at(0.5 - 0.35 * t.sin(), 0.5 + 0.35 * t.cos())
                })
                .collect();
            p.add(eframe::egui::Shape::line(pts, st));
        }
        Icon::Hole => {
            block(BODY);
            p.circle(at(0.5, 0.37), 0.1 * s, Color32::from_gray(30), st);
        }
        Icon::Thread => {
            for i in 0..5 {
                let y = 0.18 + i as f32 * 0.15;
                line((0.3, y), (0.7, y + 0.08));
            }
            line((0.3, 0.12), (0.3, 0.9));
            line((0.7, 0.12), (0.7, 0.9));
        }
        Icon::Loft => {
            p.rect_stroke(
                rect_at(0.2, 0.7, 0.8, 0.88),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            circle(0.5, 0.2, 0.1, None);
            line((0.2, 0.7), (0.4, 0.2));
            line((0.8, 0.7), (0.6, 0.2));
        }
        Icon::Sweep => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    at(0.15 + 0.7 * t, 0.8 - 0.6 * t * t)
                })
                .collect();
            p.add(eframe::egui::Shape::line(pts, Stroke::new(w * 2.5, ADD)));
            circle(0.15, 0.8, 0.08, None);
        }
        Icon::Pattern => {
            for (x, y) in [(0.3, 0.3), (0.7, 0.3), (0.3, 0.7), (0.7, 0.7)] {
                circle(x, y, 0.12, Some(ADD));
            }
        }
        Icon::Mirror => {
            p.line_segment(
                [at(0.5, 0.08), at(0.5, 0.92)],
                Stroke::new(w, c.gamma_multiply(0.6)),
            );
            poly(&[(0.12, 0.3), (0.4, 0.5), (0.12, 0.7)], ADD);
            poly(
                &[(0.88, 0.3), (0.6, 0.5), (0.88, 0.7)],
                Color32::TRANSPARENT,
            );
        }
        Icon::PressPull => {
            block(BODY);
            arrow_up(0.5, 0.36, 0.05);
        }
        Icon::Fillet => {
            line((0.15, 0.85), (0.15, 0.15));
            line((0.15, 0.85), (0.85, 0.85));
            let pts: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let t = std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
                    at(
                        0.15 + 0.5 * (1.0 - t.cos()),
                        0.15 + 0.5 * (1.0 - t.sin()) + 0.0,
                    )
                })
                .collect();
            p.add(eframe::egui::Shape::line(pts, Stroke::new(w * 2.0, ADD)));
        }
        Icon::Chamfer => {
            line((0.15, 0.85), (0.15, 0.15));
            line((0.15, 0.85), (0.85, 0.85));
            p.line_segment([at(0.15, 0.4), at(0.6, 0.85)], Stroke::new(w * 2.0, ADD));
        }
        Icon::Shell => {
            p.rect_stroke(
                rect_at(0.15, 0.25, 0.85, 0.85),
                0.0,
                Stroke::new(w * 2.5, BODY),
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_filled(rect_at(0.15, 0.15, 0.85, 0.3), 0.0, Color32::TRANSPARENT);
            line((0.15, 0.25), (0.85, 0.25));
        }
        Icon::Align => {
            p.rect_filled(rect_at(0.15, 0.15, 0.5, 0.45), 0.0, BODY);
            p.rect_filled(rect_at(0.3, 0.55, 0.85, 0.85), 0.0, ADD);
            line((0.08, 0.5), (0.92, 0.5));
        }
        Icon::Move => {
            line((0.5, 0.1), (0.5, 0.9));
            line((0.1, 0.5), (0.9, 0.5));
            for (a, b, d) in [
                ((0.5, 0.1), (0.42, 0.2), (0.58, 0.2)),
                ((0.5, 0.9), (0.42, 0.8), (0.58, 0.8)),
                ((0.1, 0.5), (0.2, 0.42), (0.2, 0.58)),
                ((0.9, 0.5), (0.8, 0.42), (0.8, 0.58)),
            ] {
                line(a, b);
                line(a, d);
            }
        }
        Icon::Scale => {
            p.rect_stroke(
                rect_at(0.12, 0.45, 0.55, 0.88),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect_at(0.12, 0.12, 0.88, 0.88),
                0.0,
                Stroke::new(w, c.gamma_multiply(0.5)),
                eframe::egui::StrokeKind::Inside,
            );
            line((0.55, 0.45), (0.85, 0.15));
        }
        Icon::Split => {
            block(BODY);
            p.line_segment([at(0.05, 0.75), at(0.95, 0.35)], Stroke::new(w * 1.5, CUT));
        }
        Icon::Combine => {
            circle(0.38, 0.5, 0.24, Some(BODY));
            circle(0.62, 0.5, 0.24, Some(ADD));
        }
        Icon::Material => {
            circle(0.5, 0.5, 0.32, Some(BODY));
            circle(0.4, 0.4, 0.08, Some(Color32::from_gray(230)));
        }
        Icon::Delete => {
            line((0.2, 0.2), (0.8, 0.8));
            line((0.8, 0.2), (0.2, 0.8));
        }
        Icon::Parameters => {
            p.text(
                r.center(),
                eframe::egui::Align2::CENTER_CENTER,
                "fx",
                eframe::egui::FontId::proportional(s * 0.6),
                c,
            );
        }
        Icon::Component => {
            block(NOTE);
        }
        Icon::Joint => {
            p.rect_filled(rect_at(0.1, 0.55, 0.5, 0.85), 0.0, BODY);
            p.rect_filled(rect_at(0.5, 0.15, 0.9, 0.45), 0.0, ADD);
            circle(0.5, 0.5, 0.1, Some(Color32::from_gray(240)));
        }
        Icon::Bom => {
            for i in 0..4 {
                let y = 0.2 + i as f32 * 0.2;
                line((0.15, y), (0.25, y));
                line((0.35, y), (0.85, y));
            }
        }
        Icon::Configure => {
            for (y, x) in [(0.25, 0.35), (0.5, 0.65), (0.75, 0.45)] {
                line((0.1, y), (0.9, y));
                p.rect_filled(rect_at(x - 0.06, y - 0.08, x + 0.06, y + 0.08), 1.0, NOTE);
            }
        }
        Icon::Plane => {
            poly(
                &[(0.1, 0.65), (0.4, 0.3), (0.9, 0.3), (0.6, 0.65)],
                NOTE.gamma_multiply(0.6),
            );
        }
        Icon::Measure => {
            line((0.1, 0.8), (0.9, 0.2));
            for i in 1..5 {
                let t = i as f32 / 5.0;
                let (x, y) = (0.1 + 0.8 * t, 0.8 - 0.6 * t);
                line((x, y), (x - 0.06, y - 0.08));
            }
        }
        Icon::Interference => {
            p.rect_stroke(
                rect_at(0.12, 0.12, 0.6, 0.6),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect_at(0.4, 0.4, 0.88, 0.88),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_filled(rect_at(0.4, 0.4, 0.6, 0.6), 0.0, CUT);
        }
        Icon::Section => {
            block(BODY);
            p.line_segment([at(0.5, 0.05), at(0.5, 0.95)], Stroke::new(w * 1.5, CUT));
        }
        Icon::Drawing => {
            p.rect_stroke(
                rect_at(0.1, 0.2, 0.9, 0.8),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect_at(0.2, 0.3, 0.45, 0.55),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                rect_at(0.55, 0.3, 0.8, 0.55),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            line((0.55, 0.7), (0.8, 0.7));
        }
        Icon::Select => {
            poly(
                &[(0.25, 0.1), (0.25, 0.8), (0.42, 0.62), (0.7, 0.62)],
                Color32::TRANSPARENT,
            );
        }
        Icon::Mesh => {
            for (a, b) in [
                ((0.1, 0.8), (0.5, 0.15)),
                ((0.5, 0.15), (0.9, 0.8)),
                ((0.1, 0.8), (0.9, 0.8)),
                ((0.3, 0.48), (0.7, 0.48)),
                ((0.3, 0.48), (0.5, 0.8)),
                ((0.7, 0.48), (0.5, 0.8)),
            ] {
                line(a, b);
            }
        }
        Icon::Template => {
            p.rect_stroke(
                rect_at(0.15, 0.1, 0.85, 0.9),
                1.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_filled(rect_at(0.25, 0.2, 0.75, 0.45), 0.0, NOTE);
            line((0.25, 0.6), (0.75, 0.6));
            line((0.25, 0.75), (0.6, 0.75));
        }
        Icon::View => {
            p.rect_stroke(
                rect_at(0.12, 0.12, 0.88, 0.88),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            for (a, b) in [((0.12, 0.3), (0.3, 0.12)), ((0.7, 0.88), (0.88, 0.7))] {
                line(a, b);
            }
        }
        Icon::Help => {
            circle(0.5, 0.5, 0.38, None);
            p.text(
                r.center(),
                eframe::egui::Align2::CENTER_CENTER,
                "?",
                eframe::egui::FontId::proportional(s * 0.55),
                c,
            );
        }
        Icon::Generic => {
            circle(0.5, 0.5, 0.25, None);
        }
    }
    let _ = vec2;
}

/// Gear glyph for Preferences (application bar).
pub fn gear(p: &Painter, rect: Rect, c: Color32) {
    let s = rect.width().min(rect.height());
    let st = Stroke::new((s / 14.0).max(1.2), c);
    p.circle_stroke(rect.center(), s * 0.22, st);
    for i in 0..8 {
        let t = i as f32 * std::f32::consts::TAU / 8.0;
        let d = vec2(t.cos(), t.sin());
        p.line_segment(
            [rect.center() + d * s * 0.28, rect.center() + d * s * 0.42],
            st,
        );
    }
}
