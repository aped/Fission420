//! Sketches built only through the public `f420_sketch` editing API, in the
//! order an interactive editor issues the edits (origin first, then points,
//! lines, geometric constraints, then dimensions). They are submitted as
//! [`crate::SketchTemplate::Custom`], so documents built from them are
//! editor sketches (with an `origin` point), not templates.
//!
//! Used by the run-2 acceptance scenarios and to generate the replayable
//! example workflows; the app's editor produces the same structure.

use f420_sketch::{Constraint, Dimension, EntityId, PointId, Sketch, SketchError};

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
