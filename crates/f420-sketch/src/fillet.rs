//! SKETCH corner fillet: round the corner where two lines share a point with
//! a tangent arc of a given radius (2D sketch geometry only; solid-edge
//! fillets are not available — Truck 0.6 has no fillet operation).
//!
//! Construction for corner `P` between lines `P–A` and `P–B` (interior angle
//! θ, 0 < θ < π): tangent distance `t = r / tan(θ/2)`, tangent points
//! `T1 = P + t·u1`, `T2 = P + t·u2`, centre on the bisector at `r / sin(θ/2)`
//! from `P`. The lines are shortened to end at `T1`/`T2`, the arc `T1–T2` is
//! added with `Tangent` constraints to both lines and a named `Radius`
//! dimension (default `fillet`, made unique; editable and bindable).
//!
//! The corner point `P` is kept as a construction point (no entity uses it)
//! held on both lines with `PointOnLine`, so every dimension or constraint on
//! the corner (e.g. a rectangle's width between corners) keeps its meaning —
//! it now measures the virtual sharp corner. A fully constrained sketch stays
//! fully constrained (the fillet adds 6 unknowns and 6 equations).
//!
//! Refused (sketch unchanged): a point not shared by exactly two lines (or
//! used by any other curve), collinear or degenerate lines, a non-positive
//! radius, and a radius that does not fit (the tangent distance must be
//! shorter than both lines, measured to their other ends).

use crate::{
    Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchError, solve_tolerance,
};

/// Result of [`Sketch::fillet_corner`].
#[derive(Clone, Debug, PartialEq)]
pub struct FilletResult {
    pub arc: EntityId,
    /// Name of the radius dimension.
    pub dimension: String,
}

impl Sketch {
    /// Round the corner at `corner` (shared by exactly two lines) with a
    /// tangent arc of `radius` mm; see the module docs. On error the sketch is
    /// unchanged.
    pub fn fillet_corner(
        &mut self,
        corner: PointId,
        radius: f64,
        name: Option<&str>,
    ) -> Result<FilletResult, SketchError> {
        let refuse = |m: String| SketchError::UnsupportedProfile(format!("sketch fillet: {m}"));
        if !radius.is_finite() || radius <= 0.0 {
            return Err(SketchError::InvalidValue(format!(
                "fillet radius must be > 0 mm, got {radius}"
            )));
        }
        let p = self
            .point_xy(corner)
            .ok_or(SketchError::MissingPoint(corner))?;
        let users: Vec<&Entity> = self
            .entities
            .iter()
            .filter(|e| e.points().contains(&corner))
            .collect();
        let lines: Vec<(EntityId, PointId)> = users
            .iter()
            .filter_map(|e| match e {
                Entity::Line { id, a, b } if *a == corner => Some((*id, *b)),
                Entity::Line { id, a, b } if *b == corner => Some((*id, *a)),
                _ => None,
            })
            .collect();
        if users.len() != 2 || lines.len() != 2 {
            return Err(refuse(format!(
                "the point must be the shared end of exactly two lines (it is used by {} \
                 curve(s), {} of them lines)",
                users.len(),
                lines.len()
            )));
        }
        if self.is_construction(lines[0].0) != self.is_construction(lines[1].0) {
            return Err(refuse("both lines must be construction or both not".into()));
        }
        let (l1, a) = lines[0];
        let (l2, b) = lines[1];
        let (pa, pb) = (
            self.point_xy(a).ok_or(SketchError::MissingPoint(a))?,
            self.point_xy(b).ok_or(SketchError::MissingPoint(b))?,
        );
        let sub = |x: [f64; 2], y: [f64; 2]| [x[0] - y[0], x[1] - y[1]];
        let norm = |v: [f64; 2]| v[0].hypot(v[1]);
        let (d1, d2) = (sub(pa, p), sub(pb, p));
        let (n1, n2) = (norm(d1), norm(d2));
        let tol = solve_tolerance(self.scale().max(radius));
        if n1 <= tol || n2 <= tol {
            return Err(refuse("a line at the corner has zero length".into()));
        }
        let (u1, u2) = ([d1[0] / n1, d1[1] / n1], [d2[0] / n2, d2[1] / n2]);
        let cos = (u1[0] * u2[0] + u1[1] * u2[1]).clamp(-1.0, 1.0);
        let theta = cos.acos();
        if theta < 1e-6 || std::f64::consts::PI - theta < 1e-6 {
            return Err(refuse(
                "the two lines are collinear (no corner to round)".into(),
            ));
        }
        let t = radius / (theta / 2.0).tan();
        if t >= n1 - tol || t >= n2 - tol {
            return Err(refuse(format!(
                "radius {radius} mm does not fit: the arc needs {t:.4} mm along each line, the \
                 lines are {n1:.4} and {n2:.4} mm long"
            )));
        }
        let t1 = [p[0] + t * u1[0], p[1] + t * u1[1]];
        let t2 = [p[0] + t * u2[0], p[1] + t * u2[1]];
        let bis = [u1[0] + u2[0], u1[1] + u2[1]];
        let bl = norm(bis);
        let h = radius / (theta / 2.0).sin();
        let c = [p[0] + h * bis[0] / bl, p[1] + h * bis[1] / bl];

        let base = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("fillet");
        let taken = |n: &str| {
            self.constraints
                .iter()
                .any(|k| k.dimension().is_some_and(|d| d.name == n))
        };
        let dim = if !taken(base) {
            base.to_string()
        } else {
            (2..)
                .map(|i| format!("{base}{i}"))
                .find(|n| !taken(n))
                .expect("a free name")
        };

        let mut s = self.clone();
        let (q1, q2, qc) = (
            s.add_point(t1[0], t1[1]),
            s.add_point(t2[0], t2[1]),
            s.add_point(c[0], c[1]),
        );
        for e in &mut s.entities {
            if let Entity::Line { id, a, b } = e
                && (*id == l1 || *id == l2)
            {
                let q = if *id == l1 { q1 } else { q2 };
                if *a == corner {
                    *a = q;
                } else {
                    *b = q;
                }
            }
        }
        // CCW from start to end over the short arc.
        let cross = (t1[0] - c[0]) * (t2[1] - c[1]) - (t1[1] - c[1]) * (t2[0] - c[0]);
        let (start, end) = if cross > 0.0 { (q1, q2) } else { (q2, q1) };
        let arc = s.next_entity_id();
        s.entities.push(Entity::Arc {
            id: arc,
            center: qc,
            start,
            end,
        });
        if s.is_construction(l1) {
            s.construction.insert(arc);
        }
        s.constraints.extend([
            Constraint::PointOnLine {
                point: corner,
                line: l1,
            },
            Constraint::PointOnLine {
                point: corner,
                line: l2,
            },
            Constraint::Tangent { line: l1, arc },
            Constraint::Tangent { line: l2, arc },
            Constraint::Radius {
                entity: arc,
                dim: Dimension::new(dim.clone(), radius),
            },
        ]);
        s.validate()?;
        let was = self.clone().solve();
        let before = s.clone();
        let rep = s.solve();
        let worse = rep.status == crate::SolveStatus::OverConstrained
            && was.status != crate::SolveStatus::OverConstrained;
        if !rep.is_success() || worse {
            return Err(refuse(format!(
                "the sketch does not solve with the fillet ({:?})",
                rep.status
            )));
        }
        for (x, y) in before.points.iter().zip(&s.points) {
            if (x.x - y.x).hypot(x.y - y.y) > 1e3 * tol {
                return Err(refuse(
                    "the solved fillet differs from the constructed one".into(),
                ));
            }
        }
        *self = s;
        Ok(FilletResult {
            arc,
            dimension: dim,
        })
    }
}
