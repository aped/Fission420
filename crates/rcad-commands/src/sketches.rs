//! Sketches built only through the public `rcad_sketch` editing API, in the
//! order an interactive editor issues the edits (origin first, then points,
//! lines, geometric constraints, then dimensions). They are submitted as
//! [`crate::SketchTemplate::Custom`], so documents built from them are
//! editor sketches (with an `origin` point), not templates.
//!
//! Used by the run-2 acceptance scenarios and to generate the replayable
//! example workflows; the app's editor produces the same structure.

use rcad_sketch::{Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchError};

/// Handles to the parts of an editor rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectIds {
    /// Bottom-left, bottom-right, top-right, top-left.
    pub corners: [PointId; 4],
    /// Bottom, right, top, left.
    pub lines: [EntityId; 4],
}

/// Rectangle centred on the sketch origin, dimensioned `width` x `height`
/// (dimension names "width" / "height", or the next free names). Corners are
/// placed at the dimensioned positions; the result is fully constrained.
pub fn centered_rectangle(width: f64, height: f64) -> Result<(Sketch, RectIds), SketchError> {
    let mut s = Sketch::default();
    let ids = add_centered_rectangle(&mut s, width, height)?;
    Ok((s, ids))
}

/// Add a centred, dimensioned rectangle to `s` (creating the origin if needed).
pub fn add_centered_rectangle(
    s: &mut Sketch,
    width: f64,
    height: f64,
) -> Result<RectIds, SketchError> {
    for (name, v) in [("width", width), ("height", height)] {
        if !v.is_finite() || v <= 0.0 {
            return Err(SketchError::InvalidDimension {
                name: name.into(),
                value: v,
            });
        }
    }
    let o = s.ensure_origin();
    let (hw, hh) = (width / 2.0, height / 2.0);
    let corners = [
        s.add_point(-hw, -hh),
        s.add_point(hw, -hh),
        s.add_point(hw, hh),
        s.add_point(-hw, hh),
    ];
    let lines = [
        s.add_line(corners[0], corners[1]),
        s.add_line(corners[1], corners[2]),
        s.add_line(corners[2], corners[3]),
        s.add_line(corners[3], corners[0]),
    ];
    s.constraints.extend([
        Constraint::Horizontal { line: lines[0] },
        Constraint::Horizontal { line: lines[2] },
        Constraint::Vertical { line: lines[1] },
        Constraint::Vertical { line: lines[3] },
        Constraint::SymmetricAboutPoint {
            a: corners[0],
            b: corners[2],
            center: o,
        },
    ]);
    let (wn, hn) = (
        s.unique_dimension_name("width"),
        s.unique_dimension_name("height"),
    );
    s.constraints.push(Constraint::HorizontalDistance {
        a: corners[0],
        b: corners[1],
        dim: Dimension::new(wn, width),
    });
    s.constraints.push(Constraint::VerticalDistance {
        a: corners[0],
        b: corners[3],
        dim: Dimension::new(hn, height),
    });
    s.validate()?;
    Ok(RectIds { corners, lines })
}

/// Circle whose centre is the sketch origin point itself, with a "diameter"
/// dimension. Fully constrained.
pub fn centered_circle(diameter: f64) -> Result<(Sketch, EntityId), SketchError> {
    if !diameter.is_finite() || diameter <= 0.0 {
        return Err(SketchError::InvalidDimension {
            name: "diameter".into(),
            value: diameter,
        });
    }
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c = s.add_circle(o, diameter / 2.0);
    let name = s.unique_dimension_name("diameter");
    s.constraints.push(Constraint::Diameter {
        circle: c,
        dim: Dimension::new(name, diameter),
    });
    s.validate()?;
    Ok((s, c))
}

/// Closed loop of unconstrained lines through `vertices` (see
/// [`crate::polygon_sketch`]); returns the vertex point ids in order.
pub fn polygon(vertices: &[[f64; 2]]) -> Result<(Sketch, Vec<PointId>), SketchError> {
    let s = crate::polygon_sketch(vertices)?;
    let ids = s
        .points
        .iter()
        .map(|p| p.id)
        .filter(|&id| Some(id) != s.origin)
        .collect();
    Ok((s, ids))
}

fn check_positive(name: &str, v: f64) -> Result<(), SketchError> {
    if !v.is_finite() || v <= 0.0 {
        return Err(SketchError::InvalidDimension {
            name: name.into(),
            value: v,
        });
    }
    Ok(())
}

fn arc_center(s: &Sketch, arc: EntityId) -> Result<PointId, SketchError> {
    match s.entity(arc) {
        Some(Entity::Arc { center, .. }) => Ok(*center),
        _ => Err(SketchError::MissingEntity(arc)),
    }
}

/// Handles to the parts of a rounded rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundedRectIds {
    /// Bottom, right, top, left.
    pub lines: [EntityId; 4],
    /// Bottom-left, bottom-right, top-right, top-left corner arcs.
    pub arcs: [EntityId; 4],
}

/// Rounded rectangle centred on the sketch origin: 4 lines and 4 three-point
/// corner arcs sharing endpoints (drawn counter-clockwise), Horizontal /
/// Vertical lines, a Tangent at each of the 8 joints, the bottom-left and
/// top-right arc centres symmetric about the origin, dimensions "width" and
/// "height" between the straight sides, and one "radius" dimension on the
/// bottom-left arc with the other three EqualRadius to it. Fully constrained
/// (26 unknowns, 26 independent equations). Requires `0 < radius < min(width,
/// height) / 2` so every straight side has positive length.
pub fn rounded_rectangle(
    width: f64,
    height: f64,
    radius: f64,
) -> Result<(Sketch, RoundedRectIds), SketchError> {
    for (name, v) in [("width", width), ("height", height), ("radius", radius)] {
        check_positive(name, v)?;
    }
    if 2.0 * radius >= width.min(height) {
        return Err(SketchError::InvalidValue(format!(
            "corner radius {radius} mm must be less than half of min(width, height) = {} mm",
            width.min(height) / 2.0
        )));
    }
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let (x, y, r) = (width / 2.0, height / 2.0, radius);
    let b0 = s.add_point(-x + r, -y);
    let b1 = s.add_point(x - r, -y);
    let r0 = s.add_point(x, -y + r);
    let r1 = s.add_point(x, y - r);
    let t0 = s.add_point(x - r, y);
    let t1 = s.add_point(-x + r, y);
    let l0 = s.add_point(-x, y - r);
    let l1 = s.add_point(-x, -y + r);
    // Mid-arc point of each 90° corner, as a user would click it.
    let k = r * (1.0 - std::f64::consts::FRAC_1_SQRT_2);
    let bottom = s.add_line(b0, b1);
    let br = s.add_arc_3pt(b1, r0, [x - k, -y + k])?;
    let right = s.add_line(r0, r1);
    let tr = s.add_arc_3pt(r1, t0, [x - k, y - k])?;
    let top = s.add_line(t0, t1);
    let tl = s.add_arc_3pt(t1, l0, [-x + k, y - k])?;
    let left = s.add_line(l0, l1);
    let bl = s.add_arc_3pt(l1, b0, [-x + k, -y + k])?;
    s.constraints.extend([
        Constraint::Horizontal { line: bottom },
        Constraint::Horizontal { line: top },
        Constraint::Vertical { line: right },
        Constraint::Vertical { line: left },
    ]);
    for (line, arc) in [
        (bottom, br),
        (right, br),
        (right, tr),
        (top, tr),
        (top, tl),
        (left, tl),
        (left, bl),
        (bottom, bl),
    ] {
        s.constraints.push(Constraint::Tangent { line, arc });
    }
    s.constraints.push(Constraint::SymmetricAboutPoint {
        a: arc_center(&s, bl)?,
        b: arc_center(&s, tr)?,
        center: o,
    });
    for b in [br, tr, tl] {
        s.constraints.push(Constraint::EqualRadius { a: bl, b });
    }
    let (wn, hn, rn) = (
        s.unique_dimension_name("width"),
        s.unique_dimension_name("height"),
        s.unique_dimension_name("radius"),
    );
    s.constraints.extend([
        Constraint::HorizontalDistance {
            a: l0,
            b: r0,
            dim: Dimension::new(wn, width),
        },
        Constraint::VerticalDistance {
            a: b0,
            b: t0,
            dim: Dimension::new(hn, height),
        },
        Constraint::Radius {
            entity: bl,
            dim: Dimension::new(rn, radius),
        },
    ]);
    s.validate()?;
    Ok((
        s,
        RoundedRectIds {
            lines: [bottom, right, top, left],
            arcs: [bl, br, tr, tl],
        },
    ))
}

/// Handles to the parts of a slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotIds {
    /// Bottom, top.
    pub lines: [EntityId; 2],
    /// Right, left end caps.
    pub arcs: [EntityId; 2],
    /// Right, left cap centres.
    pub centers: [PointId; 2],
}

/// Slot (stadium) centred on the sketch origin along X: two horizontal lines
/// and two semicircular three-point arcs, Tangent at the 4 joints, the cap
/// centres symmetric about the origin, dimension "length" between the cap
/// centres and "radius" on the right cap (the left cap's radius follows from
/// tangency and symmetry). Fully constrained (14 unknowns, 14 equations).
pub fn slot(length: f64, radius: f64) -> Result<(Sketch, SlotIds), SketchError> {
    check_positive("length", length)?;
    check_positive("radius", radius)?;
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let (x, r) = (length / 2.0, radius);
    let b0 = s.add_point(-x, -r);
    let b1 = s.add_point(x, -r);
    let t0 = s.add_point(x, r);
    let t1 = s.add_point(-x, r);
    let bottom = s.add_line(b0, b1);
    let right = s.add_arc_3pt(b1, t0, [x + r, 0.0])?;
    let top = s.add_line(t0, t1);
    let left = s.add_arc_3pt(t1, b0, [-x - r, 0.0])?;
    let (cr, cl) = (arc_center(&s, right)?, arc_center(&s, left)?);
    s.constraints.extend([
        Constraint::Horizontal { line: bottom },
        Constraint::Horizontal { line: top },
        Constraint::Tangent {
            line: bottom,
            arc: right,
        },
        Constraint::Tangent {
            line: top,
            arc: right,
        },
        Constraint::Tangent {
            line: top,
            arc: left,
        },
        Constraint::Tangent {
            line: bottom,
            arc: left,
        },
        Constraint::SymmetricAboutPoint {
            a: cl,
            b: cr,
            center: o,
        },
    ]);
    let (ln, rn) = (
        s.unique_dimension_name("length"),
        s.unique_dimension_name("radius"),
    );
    s.constraints.extend([
        Constraint::HorizontalDistance {
            a: cl,
            b: cr,
            dim: Dimension::new(ln, length),
        },
        Constraint::Radius {
            entity: right,
            dim: Dimension::new(rn, radius),
        },
    ]);
    s.validate()?;
    Ok((
        s,
        SlotIds {
            lines: [bottom, top],
            arcs: [right, left],
            centers: [cr, cl],
        },
    ))
}

/// Circle of `diameter` whose centre is placed `dx`, `dy` (> 0) from the sketch
/// origin by horizontal / vertical dimensions "x" and "y". Fully constrained.
pub fn offset_circle(diameter: f64, dx: f64, dy: f64) -> Result<(Sketch, EntityId), SketchError> {
    check_positive("diameter", diameter)?;
    check_positive("x", dx)?;
    check_positive("y", dy)?;
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c = s.add_point(dx, dy);
    let circle = s.add_circle(c, diameter / 2.0);
    s.constraints.extend([
        Constraint::HorizontalDistance {
            a: o,
            b: c,
            dim: Dimension::new("x", dx),
        },
        Constraint::VerticalDistance {
            a: o,
            b: c,
            dim: Dimension::new("y", dy),
        },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("diameter", diameter),
        },
    ]);
    s.validate()?;
    Ok((s, circle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcad_sketch::{LoopSegment, Profile, SolveStatus};
    use std::f64::consts::PI;

    /// Shoelace on chords + r²/2 (θ − sin θ) per arc, written out here
    /// independently of the sketch crate.
    fn area(segs: &[LoopSegment]) -> f64 {
        let n = segs.len();
        let mut a = 0.0;
        for i in 0..n {
            let (p, q) = (segs[i].start(), segs[(i + 1) % n].start());
            a += 0.5 * (p[0] * q[1] - q[0] * p[1]);
            if let LoopSegment::Arc { center: c, ccw, .. } = segs[i] {
                let r = (p[0] - c[0]).hypot(p[1] - c[1]);
                let t0 = (p[1] - c[1]).atan2(p[0] - c[0]);
                let t1 = (q[1] - c[1]).atan2(q[0] - c[0]);
                let mut th = (t1 - t0).rem_euclid(2.0 * PI);
                if !ccw {
                    th -= 2.0 * PI;
                }
                a += 0.5 * r * r * (th - th.sin());
            }
        }
        a
    }

    fn solved_loop(mut s: Sketch) -> Vec<LoopSegment> {
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        match s.profile() {
            Ok(Profile::Loop(v)) => v,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rounded_rectangle_is_fully_constrained_and_radius_driven() {
        let (s, ids) = rounded_rectangle(80.0, 50.0, 8.0).unwrap();
        assert_eq!(
            s.dimensions(),
            vec![
                ("width".to_string(), 80.0),
                ("height".to_string(), 50.0),
                ("radius".to_string(), 8.0)
            ]
        );
        let segs = solved_loop(s.clone());
        assert_eq!(segs.len(), 8);
        // 80*50 - 4*r^2 + pi r^2.
        assert!((area(&segs) - (4000.0 - (4.0 - PI) * 64.0)).abs() < 1e-8);
        let mut t = s;
        t.set_dimension("radius", 12.0).unwrap();
        let segs = solved_loop(t.clone());
        assert!((area(&segs) - (4000.0 - (4.0 - PI) * 144.0)).abs() < 1e-8);
        // Every corner arc follows the one dimension.
        assert!(t.solve().is_success());
        for a in ids.arcs {
            let g = t.arc_geometry(a).unwrap();
            assert!((g.radius - 12.0).abs() < 1e-8);
            assert!((g.sweep - PI / 2.0).abs() < 1e-8);
        }
        assert!(rounded_rectangle(80.0, 50.0, 25.0).is_err());
        assert!(rounded_rectangle(80.0, 50.0, 0.0).is_err());
    }

    #[test]
    fn slot_is_fully_constrained_with_stadium_area() {
        let (s, ids) = slot(60.0, 10.0).unwrap();
        let segs = solved_loop(s.clone());
        assert_eq!(segs.len(), 4);
        assert!((area(&segs) - (1200.0 + 100.0 * PI)).abs() < 1e-8);
        let mut t = s;
        t.set_dimension("length", 40.0).unwrap();
        t.set_dimension("radius", 5.0).unwrap();
        let segs = solved_loop(t.clone());
        assert!((area(&segs) - (40.0 * 10.0 + 25.0 * PI)).abs() < 1e-8);
        assert!(t.solve().is_success());
        let gl = t.arc_geometry(ids.arcs[1]).unwrap();
        assert!((gl.radius - 5.0).abs() < 1e-8 && (gl.sweep - PI).abs() < 1e-8);
        assert!((gl.center[0] + 20.0).abs() < 1e-8 && gl.center[1].abs() < 1e-8);
    }

    #[test]
    fn offset_circle_is_fully_constrained() {
        let (mut s, _) = offset_circle(4.0, 34.5, 19.5).unwrap();
        assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
        match s.profile() {
            Ok(Profile::Circle { center, radius }) => {
                assert!((center[0] - 34.5).abs() < 1e-9 && (center[1] - 19.5).abs() < 1e-9);
                assert!((radius - 2.0).abs() < 1e-12);
            }
            other => panic!("{other:?}"),
        }
    }
}
