//! SKETCH corner fillet: round the corner where two lines (or, since run 5, a
//! line and an arc; see `fillet_line_arc`) share a point with
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
        let arcs: Vec<EntityId> = users
            .iter()
            .filter_map(|e| match e {
                Entity::Arc { id, start, end, .. } if *start == corner || *end == corner => {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        if users.len() == 2 && lines.len() == 1 && arcs.len() == 1 {
            return self.fillet_line_arc(corner, lines[0], arcs[0], radius, name);
        }
        if users.len() != 2 || lines.len() != 2 {
            return Err(refuse(format!(
                "the point must be the shared end of exactly two lines, or of one line and \
                 one arc (it is used by {} curve(s), {} of them lines, {} arcs)",
                users.len(),
                lines.len(),
                arcs.len()
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

        let dim = self.fillet_dim_name(name);

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
        self.accept_fillet(s, arc, dim, tol)
    }

    /// A unique name for a fillet's radius dimension (default `fillet`).
    fn fillet_dim_name(&self, name: Option<&str>) -> String {
        let base = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("fillet");
        let taken = |n: &str| {
            self.constraints
                .iter()
                .any(|k| k.dimension().is_some_and(|d| d.name == n))
        };
        if !taken(base) {
            base.to_string()
        } else {
            (2..)
                .map(|i| format!("{base}{i}"))
                .find(|n| !taken(n))
                .expect("a free name")
        }
    }

    /// Validate and solve the filleted sketch `s`; accept it only when it
    /// solves (no worse than before) to the constructed geometry.
    fn accept_fillet(
        &mut self,
        mut s: Sketch,
        arc: EntityId,
        dim: String,
        tol: f64,
    ) -> Result<FilletResult, SketchError> {
        let refuse = |m: String| SketchError::UnsupportedProfile(format!("sketch fillet: {m}"));
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

    /// Line–arc corner (run 5). Corner `P` shared by line `P–A` and an arc
    /// (centre `O`, radius `R`). With `u` the unit direction `P→A` and `v` the
    /// arc's unit tangent at `P` pointing along the arc, the fillet lies in
    /// the corner between them: its centre `F` is on the line offset by `r`
    /// toward `v`, and at `|F − O| = R − r` when the corner's interior is
    /// inside the arc's circle (internal tangency), else `R + r`. With
    /// `F = P + r n + s u` that is a quadratic in `s`; the smallest root
    /// `s > 0` gives the tangent points `T1 = P + s u` (line) and
    /// `T2 = O + R (F − O)/|F − O|` (arc). Refused when no root exists, `T1`
    /// is not inside the line, `T2` is not inside the arc's span, the curves
    /// are tangent already, or (internal) `r >= R`. The arc keeps its centre,
    /// other end and radius; constraints: `Tangent` (line, fillet),
    /// `TangentArcs` (fillet, arc), the corner kept on both curves
    /// (`PointOnLine`, `PointOnCircle`), and the named `Radius`.
    fn fillet_line_arc(
        &mut self,
        corner: PointId,
        line: (EntityId, PointId),
        arc_id: EntityId,
        radius: f64,
        name: Option<&str>,
    ) -> Result<FilletResult, SketchError> {
        let refuse = |m: String| SketchError::UnsupportedProfile(format!("sketch fillet: {m}"));
        let (l, a) = line;
        if self.is_construction(l) != self.is_construction(arc_id) {
            return Err(refuse(
                "the line and the arc must both be construction or both not".into(),
            ));
        }
        let Some(Entity::Arc {
            center, start, end, ..
        }) = self.entity(arc_id).cloned()
        else {
            return Err(refuse("the second curve is not an arc".into()));
        };
        let pt = |id: PointId| self.point_xy(id).ok_or(SketchError::MissingPoint(id));
        let (p, pa, o) = (pt(corner)?, pt(a)?, pt(center)?);
        let other = if start == corner { end } else { start };
        let po = pt(other)?;
        let sub = |x: [f64; 2], y: [f64; 2]| [x[0] - y[0], x[1] - y[1]];
        let dot = |x: [f64; 2], y: [f64; 2]| x[0] * y[0] + x[1] * y[1];
        let crs = |x: [f64; 2], y: [f64; 2]| x[0] * y[1] - x[1] * y[0];
        let norm = |v: [f64; 2]| v[0].hypot(v[1]);
        let big_r = norm(sub(p, o));
        let tol = solve_tolerance(self.scale().max(radius).max(big_r));
        let la = norm(sub(pa, p));
        if la <= tol || big_r <= tol {
            return Err(refuse("a curve at the corner is degenerate".into()));
        }
        let u = [(pa[0] - p[0]) / la, (pa[1] - p[1]) / la];
        // Arc tangent at P along the arc (arcs run counter-clockwise start→end).
        let rv = [(p[0] - o[0]) / big_r, (p[1] - o[1]) / big_r];
        let v = if start == corner {
            [-rv[1], rv[0]]
        } else {
            [rv[1], -rv[0]]
        };
        let sin = crs(u, v);
        if sin.abs() < 1e-6 {
            return Err(refuse(
                "the line and the arc are tangent (or reverse) at the corner: nothing to round"
                    .into(),
            ));
        }
        // Normal of the line toward the arc side.
        let n = if sin > 0.0 {
            [-u[1], u[0]]
        } else {
            [u[1], -u[0]]
        };
        // Interior of the corner: inside or outside the arc's circle? Moving
        // from P along the bisector u + v changes |q − O| at the rate
        // (u + v)·r̂ = u·r̂ (v ⟂ r̂): exact to first order (review run 5).
        let inside = dot(u, rv) < 0.0;
        let d = if inside {
            if radius >= big_r - tol {
                return Err(refuse(format!(
                    "radius {radius} mm does not fit inside the arc (radius {big_r:.4} mm)"
                )));
            }
            big_r - radius
        } else {
            big_r + radius
        };
        let w = [p[0] + radius * n[0] - o[0], p[1] + radius * n[1] - o[1]];
        let wu = dot(w, u);
        let disc = wu * wu - dot(w, w) + d * d;
        if disc < 0.0 {
            return Err(refuse(format!(
                "radius {radius} mm does not fit between the line and the arc"
            )));
        }
        let sq = disc.sqrt();
        let s_par = [-wu - sq, -wu + sq]
            .into_iter()
            .filter(|s| *s > tol)
            .fold(f64::INFINITY, f64::min);
        if !s_par.is_finite() || s_par >= la - tol {
            return Err(refuse(format!(
                "radius {radius} mm does not fit: the tangent point would leave the line \
                 ({la:.4} mm long)"
            )));
        }
        let t1 = [p[0] + s_par * u[0], p[1] + s_par * u[1]];
        let f = [t1[0] + radius * n[0], t1[1] + radius * n[1]];
        let fo = sub(f, o);
        let fol = norm(fo);
        if fol <= tol {
            return Err(refuse(
                "degenerate fillet (centre on the arc's centre)".into(),
            ));
        }
        let t2 = [o[0] + big_r * fo[0] / fol, o[1] + big_r * fo[1] / fol];
        // T2 strictly inside the arc's span (counter-clockwise from start).
        let ang = |x: [f64; 2]| (x[1] - o[1]).atan2(x[0] - o[0]);
        let (ps, pe) = if start == corner { (p, po) } else { (po, p) };
        let ccw =
            |from: [f64; 2], to: [f64; 2]| (ang(to) - ang(from)).rem_euclid(std::f64::consts::TAU);
        let span = ccw(ps, pe);
        let at = ccw(ps, t2);
        let eps = tol / big_r;
        if !(at > eps && at < span - eps) {
            return Err(refuse(format!(
                "radius {radius} mm does not fit: the tangent point would leave the arc"
            )));
        }
        let dim = self.fillet_dim_name(name);
        let mut s = self.clone();
        let (q1, q2, qc) = (
            s.add_point(t1[0], t1[1]),
            s.add_point(t2[0], t2[1]),
            s.add_point(f[0], f[1]),
        );
        for e in &mut s.entities {
            match e {
                Entity::Line { id, a, b } if *id == l => {
                    if *a == corner {
                        *a = q1;
                    } else {
                        *b = q1;
                    }
                }
                Entity::Arc { id, start, end, .. } if *id == arc_id => {
                    if *start == corner {
                        *start = q2;
                    } else {
                        *end = q2;
                    }
                }
                _ => {}
            }
        }
        // Fillet arc: the one through the corner side (towards P), counter-
        // clockwise from start to end. Decided by angles, not by the sign of
        // (T1 − F) × (T2 − F), which vanishes at the largest radius that fits
        // (half circle; review run 5).
        let ang_f = |x: [f64; 2]| (x[1] - f[1]).atan2(x[0] - f[0]);
        let tau = std::f64::consts::TAU;
        let sweep12 = (ang_f(t2) - ang_f(t1)).rem_euclid(tau);
        let via_p = (ang_f(p) - ang_f(t1)).rem_euclid(tau);
        let (fs, fe) = if via_p < sweep12 { (q1, q2) } else { (q2, q1) };
        let fillet = s.next_entity_id();
        s.entities.push(Entity::Arc {
            id: fillet,
            center: qc,
            start: fs,
            end: fe,
        });
        if s.is_construction(l) {
            s.construction.insert(fillet);
        }
        s.constraints.extend([
            Constraint::PointOnLine {
                point: corner,
                line: l,
            },
            Constraint::PointOnCircle {
                point: corner,
                circle: arc_id,
            },
            Constraint::Tangent {
                line: l,
                arc: fillet,
            },
            Constraint::TangentArcs {
                a: fillet,
                b: arc_id,
            },
            Constraint::Radius {
                entity: fillet,
                dim: Dimension::new(dim.clone(), radius),
            },
        ]);
        self.accept_fillet(s, fillet, dim, tol)
    }
}
