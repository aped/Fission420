//! Constrained shape generators: straight slots and regular
//! polygons made of ordinary lines, arcs and construction circles, so they stay
//! editable (drag, dimension edits, trim) like hand-drawn geometry.
//!
//! * **Slot** (centre to centre): two parallel lines and two end arcs, joined
//!   tangentially (4 `Tangent` constraints, `EqualRadius`), with an aligned
//!   `Distance` dimension between the arc centres (`slot_length`, L) and one
//!   between the two tangent points of the first end (`slot_width`, W = 2r: the
//!   lines are external tangents of two equal circles, hence parallel, so those
//!   points are diametrically opposite). Remaining freedom: position and
//!   direction (3 DOF). Area = L·W + π(W/2)².
//! * **Regular polygon** (N sides, centre + first vertex): N lines through
//!   vertices on a construction circle (`PointOnCircle`) with N−1 `EqualLength`
//!   constraints (a convex equilateral polygon inscribed in a circle is
//!   regular). *Inscribed*: the circle through the vertices carries the
//!   `Diameter` dimension. *Circumscribed*: a second, inner construction circle
//!   carries the dimension (across flats) and the first edge's midpoint (a
//!   construction point, `SymmetricAboutPoint`) lies on it. Remaining freedom:
//!   centre and rotation (3 DOF).
//!
//! The generators only add entities, points and constraints; they never move
//! existing geometry. The caller solves (the editor's `try_change`).

use crate::{Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchError};

/// What a slot generator added.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotResult {
    /// Bottom line, end arc at the second centre, top line, end arc at the first.
    pub entities: Vec<EntityId>,
    /// Arc centre points (first, second).
    pub centers: [PointId; 2],
    /// Dimension names: (length, width).
    pub dimensions: (String, String),
}

/// What a polygon generator added.
#[derive(Clone, Debug, PartialEq)]
pub struct PolygonResult {
    /// Edge lines in counter-clockwise order (edge i from vertex i to i+1).
    pub entities: Vec<EntityId>,
    pub vertices: Vec<PointId>,
    pub center: PointId,
    /// Construction circle(s): through the vertices, then (circumscribed) the
    /// inscribed circle.
    pub circles: Vec<EntityId>,
    /// Name of the size dimension (diameter of the dimensioned circle).
    pub dimension: String,
}

/// Size convention of a regular polygon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PolygonFit {
    /// Vertices on the dimensioned circle (diameter across corners).
    Inscribed,
    /// Edges tangent to the dimensioned circle (diameter across flats).
    Circumscribed,
}

/// Most sides accepted for a regular polygon.
pub const MAX_POLYGON_SIDES: usize = 64;

fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

impl Sketch {
    fn push_arc(&mut self, center: PointId, start: PointId, end: PointId) -> EntityId {
        let id = self.next_entity_id();
        self.entities.push(Entity::Arc {
            id,
            center,
            start,
            end,
        });
        id
    }

    /// Add a centre-to-centre straight slot from `c1` to `c2` of total width
    /// `width` (see the module docs). `c1`/`c2` may reuse existing points
    /// (`centers`); otherwise new points are created.
    pub fn add_slot(
        &mut self,
        c1: [f64; 2],
        c2: [f64; 2],
        width: f64,
        centers: [Option<PointId>; 2],
    ) -> Result<SlotResult, SketchError> {
        if !finite(c1) || !finite(c2) || !width.is_finite() || width <= 0.0 {
            return Err(SketchError::InvalidValue(
                "slot: centres must be finite and the width > 0".into(),
            ));
        }
        let (dx, dy) = (c2[0] - c1[0], c2[1] - c1[1]);
        let len = dx.hypot(dy);
        let tol = crate::solve_tolerance(self.scale().max(len).max(width));
        if len <= tol.max(1e-6) {
            return Err(SketchError::DegenerateProfile(
                "slot: the two centres coincide (use a circle)".into(),
            ));
        }
        let r = width / 2.0;
        let (ux, uy) = (dx / len, dy / len);
        let n = [-uy * r, ux * r];
        let mut s = self.clone();
        let p1 = centers[0].unwrap_or_else(|| s.add_point(c1[0], c1[1]));
        let p2 = centers[1].unwrap_or_else(|| s.add_point(c2[0], c2[1]));
        for p in [p1, p2] {
            if s.point(p).is_none() {
                return Err(SketchError::MissingPoint(p));
            }
        }
        // Tangent points: bottom (−n) and top (+n) at each end.
        let b1 = s.add_point(c1[0] - n[0], c1[1] - n[1]);
        let b2 = s.add_point(c2[0] - n[0], c2[1] - n[1]);
        let t2 = s.add_point(c2[0] + n[0], c2[1] + n[1]);
        let t1 = s.add_point(c1[0] + n[0], c1[1] + n[1]);
        // CCW loop: bottom line, arc about c2 (through +u), top line, arc about c1.
        let bottom = s.add_line(b1, b2);
        let a2 = s.push_arc(p2, b2, t2);
        let top = s.add_line(t2, t1);
        let a1 = s.push_arc(p1, t1, b1);
        let ln = s.unique_dimension_name("slot_length");
        let wn = s.unique_dimension_name("slot_width");
        s.constraints.extend([
            Constraint::Tangent {
                line: bottom,
                arc: a2,
            },
            Constraint::Tangent { line: top, arc: a2 },
            Constraint::Tangent { line: top, arc: a1 },
            Constraint::Tangent {
                line: bottom,
                arc: a1,
            },
            Constraint::EqualRadius { a: a1, b: a2 },
            Constraint::Distance {
                a: p1,
                b: p2,
                dim: Dimension::new(ln.clone(), len),
            },
        ]);
        s.constraints.push(Constraint::Distance {
            a: b1,
            b: t1,
            dim: Dimension::new(wn.clone(), width),
        });
        s.validate()?;
        *self = s;
        Ok(SlotResult {
            entities: vec![bottom, a2, top, a1],
            centers: [p1, p2],
            dimensions: (ln, wn),
        })
    }

    /// Add a regular polygon with `sides` edges centred on `center` with its
    /// first vertex at (inscribed) or its first edge midpoint direction toward
    /// (circumscribed) `vertex` (see the module docs). `center_point` may reuse
    /// an existing point.
    pub fn add_regular_polygon(
        &mut self,
        center: [f64; 2],
        vertex: [f64; 2],
        sides: usize,
        fit: PolygonFit,
        center_point: Option<PointId>,
    ) -> Result<PolygonResult, SketchError> {
        if !(3..=MAX_POLYGON_SIDES).contains(&sides) {
            return Err(SketchError::InvalidValue(format!(
                "polygon: {sides} sides (3..={MAX_POLYGON_SIDES})"
            )));
        }
        if !finite(center) || !finite(vertex) {
            return Err(SketchError::InvalidValue(
                "polygon: non-finite point".into(),
            ));
        }
        let (dx, dy) = (vertex[0] - center[0], vertex[1] - center[1]);
        let rho = dx.hypot(dy);
        if rho <= crate::solve_tolerance(self.scale().max(rho)).max(1e-6) {
            return Err(SketchError::DegenerateProfile(
                "polygon: the vertex is at the centre".into(),
            ));
        }
        let step = std::f64::consts::TAU / sides as f64;
        let a0 = dy.atan2(dx);
        // Circumradius R and the angle of vertex 0: inscribed, the clicked point
        // is a vertex; circumscribed, it is the midpoint of edge 0 (apothem rho).
        let (big_r, first) = match fit {
            PolygonFit::Inscribed => (rho, a0),
            PolygonFit::Circumscribed => (rho / (step / 2.0).cos(), a0 - step / 2.0),
        };
        let mut s = self.clone();
        let c = center_point.unwrap_or_else(|| s.add_point(center[0], center[1]));
        if s.point(c).is_none() {
            return Err(SketchError::MissingPoint(c));
        }
        let vs: Vec<PointId> = (0..sides)
            .map(|i| {
                let t = first + step * i as f64;
                s.add_point(center[0] + big_r * t.cos(), center[1] + big_r * t.sin())
            })
            .collect();
        let lines: Vec<EntityId> = (0..sides)
            .map(|i| s.add_line(vs[i], vs[(i + 1) % sides]))
            .collect();
        let outer = s.add_circle(c, big_r);
        s.set_construction(outer, true);
        let name = s.unique_dimension_name(match fit {
            PolygonFit::Inscribed => "polygon_d",
            PolygonFit::Circumscribed => "polygon_flats",
        });
        let mut circles = vec![outer];
        for &v in &vs {
            s.constraints.push(Constraint::PointOnCircle {
                point: v,
                circle: outer,
            });
        }
        for w in lines.windows(2) {
            s.constraints
                .push(Constraint::EqualLength { a: w[0], b: w[1] });
        }
        match fit {
            PolygonFit::Inscribed => s.constraints.push(Constraint::Diameter {
                circle: outer,
                dim: Dimension::new(name.clone(), 2.0 * big_r),
            }),
            PolygonFit::Circumscribed => {
                let inner = s.add_circle(c, rho);
                s.set_construction(inner, true);
                circles.push(inner);
                let m = s.add_point(center[0] + dx, center[1] + dy);
                s.constraints.extend([
                    Constraint::SymmetricAboutPoint {
                        a: vs[0],
                        b: vs[1],
                        center: m,
                    },
                    Constraint::PointOnCircle {
                        point: m,
                        circle: inner,
                    },
                    Constraint::Diameter {
                        circle: inner,
                        dim: Dimension::new(name.clone(), 2.0 * rho),
                    },
                ]);
            }
        }
        s.validate()?;
        *self = s;
        Ok(PolygonResult {
            entities: lines,
            vertices: vs,
            center: c,
            circles,
            dimension: name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SolveStatus;
    use std::f64::consts::PI;

    fn base() -> Sketch {
        let mut s = Sketch::default();
        s.ensure_origin();
        s
    }

    #[test]
    fn slot_has_three_dof_and_the_analytic_area() {
        let mut s = base();
        let r = s
            .add_slot([5.0, 2.0], [35.0, 12.0], 8.0, [None, None])
            .unwrap();
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::UnderConstrained, "{rep:?}");
        assert_eq!(rep.dof, 3, "position + direction");
        let l = 30.0f64.hypot(10.0);
        let area = l * 8.0 + PI * 16.0;
        let reg = s.profile_region().unwrap();
        assert!(reg.holes.is_empty());
        let a = s.regions().unwrap()[0].area;
        assert!((a - area).abs() < 1e-9 * area, "{a} vs {area}");
        // Pin it: first centre at the origin, slot horizontal -> fully constrained.
        let o = s.origin.unwrap();
        s.constraints.push(Constraint::Coincident {
            a: o,
            b: r.centers[0],
        });
        s.constraints.push(Constraint::Horizontal {
            line: r.entities[0],
        });
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        // Dimension edits drive the shape; the area follows the formula.
        s.set_dimension(&r.dimensions.0, 50.0).unwrap();
        s.set_dimension(&r.dimensions.1, 12.0).unwrap();
        assert!(s.solve().is_success());
        let a = s.regions().unwrap()[0].area;
        let e = 50.0 * 12.0 + PI * 36.0;
        assert!((a - e).abs() < 1e-8 * e, "{a} vs {e}");
        let c2 = s.point_xy(r.centers[1]).unwrap();
        assert!((c2[0] - 50.0).abs() < 1e-8 && c2[1].abs() < 1e-8, "{c2:?}");
    }

    #[test]
    fn slot_refusals() {
        let mut s = base();
        assert!(
            s.add_slot([1.0, 1.0], [1.0, 1.0], 2.0, [None, None])
                .is_err()
        );
        assert!(
            s.add_slot([0.0, 0.0], [10.0, 0.0], 0.0, [None, None])
                .is_err()
        );
        assert!(
            s.add_slot([0.0, 0.0], [f64::NAN, 0.0], 1.0, [None, None])
                .is_err()
        );
        assert_eq!(s, base());
    }

    #[test]
    fn regular_polygons_have_three_dof_and_analytic_areas() {
        for (n, fit) in [
            (3, PolygonFit::Inscribed),
            (6, PolygonFit::Inscribed),
            (6, PolygonFit::Circumscribed),
            (8, PolygonFit::Circumscribed),
            (5, PolygonFit::Circumscribed),
        ] {
            let mut s = base();
            let r = s
                .add_regular_polygon([3.0, -2.0], [13.0, 3.0], n, fit, None)
                .unwrap();
            let rep = s.solve();
            assert_eq!(
                rep.status,
                SolveStatus::UnderConstrained,
                "{n} {fit:?} {rep:?}"
            );
            assert_eq!(rep.dof, 3, "{n} {fit:?}: centre + rotation");
            let rho = 10.0f64.hypot(5.0);
            let nf = n as f64;
            let area = match fit {
                PolygonFit::Inscribed => nf / 2.0 * rho * rho * (2.0 * PI / nf).sin(),
                PolygonFit::Circumscribed => nf * rho * rho * (PI / nf).tan(),
            };
            let a = s.regions().unwrap()[0].area;
            assert!((a - area).abs() < 1e-9 * area, "{n} {fit:?}: {a} vs {area}");
            // Size edit: across corners / flats = 30.
            s.set_dimension(&r.dimension, 30.0).unwrap();
            assert!(s.solve().is_success());
            let e = match fit {
                PolygonFit::Inscribed => nf / 2.0 * 225.0 * (2.0 * PI / nf).sin(),
                PolygonFit::Circumscribed => nf * 225.0 * (PI / nf).tan(),
            };
            let a = s.regions().unwrap()[0].area;
            assert!((a - e).abs() < 1e-8 * e, "{n} {fit:?}: {a} vs {e}");
        }
        let mut s = base();
        assert!(
            s.add_regular_polygon([0.0, 0.0], [1.0, 0.0], 2, PolygonFit::Inscribed, None)
                .is_err()
        );
        assert!(
            s.add_regular_polygon([0.0, 0.0], [0.0, 0.0], 6, PolygonFit::Inscribed, None)
                .is_err()
        );
    }
}
