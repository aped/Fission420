//! Orientation triad drawn in the viewport's bottom-left corner.
//!
//! Labels are drawn as stroked glyphs rather than text so the function needs
//! no font atlas and stays a pure, testable function of camera and rect.

use eframe::egui::{self, Color32, Pos2, Shape, Stroke, pos2, vec2};

use crate::camera::Camera;
use crate::math::{self, Vec3};

/// Axis length in points.
const AXIS_LEN: f32 = 30.0;
/// Distance of the triad origin from the rect's left and bottom edges.
const MARGIN: f32 = 46.0;
/// Glyph half-height in points.
const GLYPH: f32 = 4.5;

pub const AXIS_X_COLOR: Color32 = Color32::from_rgb(230, 70, 70);
pub const AXIS_Y_COLOR: Color32 = Color32::from_rgb(80, 210, 80);
pub const AXIS_Z_COLOR: Color32 = Color32::from_rgb(80, 130, 245);

/// Screen-space centre of the triad for `rect`.
pub fn axis_triad_origin(rect: egui::Rect) -> Pos2 {
    pos2(rect.min.x + MARGIN, rect.max.y - MARGIN)
}

/// Shapes for an XYZ triad (red X, green Y, blue Z, each with a stroked letter)
/// oriented by `camera`, anchored at the bottom-left of `rect`. Axes are
/// ordered back to front so the nearest axis is drawn last.
pub fn axis_triad_shapes(camera: &Camera, rect: egui::Rect) -> Vec<Shape> {
    let (r, u, f) = camera.basis();
    let o = axis_triad_origin(rect);
    let axes: [(Vec3, Color32, char); 3] = [
        ([1.0, 0.0, 0.0], AXIS_X_COLOR, 'X'),
        ([0.0, 1.0, 0.0], AXIS_Y_COLOR, 'Y'),
        ([0.0, 0.0, 1.0], AXIS_Z_COLOR, 'Z'),
    ];
    let mut order: Vec<(f32, Vec3, Color32, char)> = axes
        .iter()
        .map(|&(a, c, l)| (math::dot(a, f), a, c, l))
        .collect();
    // Larger dot with forward = farther from the viewer: draw first.
    order.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut shapes = vec![Shape::circle_filled(
        o,
        AXIS_LEN + 12.0,
        Color32::from_black_alpha(70),
    )];
    for (depth, a, color, letter) in order {
        let dir = vec2(math::dot(a, r), -math::dot(a, u));
        let tip = o + dir * AXIS_LEN;
        // Axes pointing away from the viewer are drawn fainter.
        let color = if depth > 0.3 {
            color.gamma_multiply(0.55)
        } else {
            color
        };
        shapes.push(Shape::line_segment([o, tip], Stroke::new(2.0, color)));
        shapes.push(Shape::circle_filled(tip, 2.0, color));
        let len = dir.length();
        let label_at = if len > 1.0e-3 {
            tip + dir / len * 9.0
        } else {
            tip + vec2(9.0, -9.0)
        };
        shapes.extend(glyph(letter, label_at, Stroke::new(1.6, color)));
    }
    shapes
}

/// Stroked capital X, Y or Z centred at `c`.
fn glyph(letter: char, c: Pos2, stroke: Stroke) -> Vec<Shape> {
    let h = GLYPH;
    let w = GLYPH * 0.75;
    let p = |x: f32, y: f32| c + vec2(x, y);
    let seg = |a: Pos2, b: Pos2| Shape::line_segment([a, b], stroke);
    match letter {
        'X' => vec![seg(p(-w, -h), p(w, h)), seg(p(w, -h), p(-w, h))],
        'Y' => vec![
            seg(p(-w, -h), p(0.0, 0.0)),
            seg(p(w, -h), p(0.0, 0.0)),
            seg(p(0.0, 0.0), p(0.0, h)),
        ],
        'Z' => vec![
            seg(p(-w, -h), p(w, -h)),
            seg(p(w, -h), p(-w, h)),
            seg(p(-w, h), p(w, h)),
        ],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::ViewPreset;

    fn axis_tips(shapes: &[Shape]) -> Vec<(Color32, Pos2)> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::LineSegment { points, stroke } if stroke.width == 2.0 => {
                    Some((stroke.color, points[1]))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn top_view_triad_points_x_right_y_up() {
        let mut c = Camera::default();
        c.set_view(ViewPreset::Top);
        let rect = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let shapes = axis_triad_shapes(&c, rect);
        let o = axis_triad_origin(rect);
        assert!(rect.contains(o));
        let tips = axis_tips(&shapes);
        assert_eq!(tips.len(), 3);
        let tip = |col: Color32| {
            tips.iter()
                .find(|(c, _)| c.r() == col.r() && c.g() == col.g())
                .map(|t| t.1 - o)
                .expect("axis present")
        };
        let x = tip(AXIS_X_COLOR);
        let y = tip(AXIS_Y_COLOR);
        assert!(x.x > 25.0 && x.y.abs() < 1.0, "{x:?}");
        assert!(y.y < -25.0 && y.x.abs() < 1.0, "{y:?}");
        // Z points at the viewer: nearly zero length on screen.
        let z = tips
            .iter()
            .map(|t| t.1 - o)
            .fold(f32::MAX, |m, v| m.min(v.length()));
        assert!(z < 1.0);
        // Every shape stays inside the rect.
        for s in &shapes {
            assert!(rect.contains_rect(s.visual_bounding_rect()));
        }
    }

    #[test]
    fn front_view_triad_points_z_up() {
        let mut c = Camera::default();
        c.set_view(ViewPreset::Front);
        let rect = egui::Rect::from_min_size(pos2(10.0, 20.0), vec2(400.0, 300.0));
        let o = axis_triad_origin(rect);
        let tips = axis_tips(&axis_triad_shapes(&c, rect));
        let z = tips
            .iter()
            .find(|(col, _)| col.b() > 200 && col.r() < 100)
            .map(|t| t.1 - o)
            .expect("z axis");
        assert!(z.y < -25.0 && z.x.abs() < 1.0, "{z:?}");
    }
}
