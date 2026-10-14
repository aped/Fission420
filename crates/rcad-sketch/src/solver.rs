//! Levenberg-Marquardt solver over the sketch equation vector with an analytic
//! Jacobian, plus SVD-based DOF / redundancy diagnostics.

use crate::{
    Constraint, Entity, MAX_DIAGNOSED_CONSTRAINTS, RANK_REL_TOL, Sketch, SolveReport, SolveStatus,
    solve_tolerance,
};
use nalgebra::{DMatrix, DVector};
use std::collections::HashMap;

/// Hard iteration cap for one LM run.
const MAX_ITERATIONS: usize = 200;
/// Initial LM damping (Jacobian entries are O(1), dimensionless).
const LAMBDA_INIT: f64 = 1e-3;
/// Damping above which the iteration is considered stalled.
const LAMBDA_MAX: f64 = 1e16;
/// Relative gradient threshold: `|J^T r|_inf <= GRAD_REL_TOL * max(1, |r|_2)`
/// means we sit at a least-squares stationary point.
const GRAD_REL_TOL: f64 = 1e-12;

/// One scalar equation; column indices into the unknown vector.
#[derive(Clone, Copy, Debug)]
enum Eq {
    /// `x[c] - v`
    Fix { c: usize, v: f64 },
    /// `x[b] - x[a] - v`
    Diff { a: usize, b: usize, v: f64 },
    /// `(x[a] + x[b]) / 2 - x[m]`
    Mid { a: usize, b: usize, m: usize },
    /// `k * x[c] - v`
    Scaled { c: usize, k: f64, v: f64 },
    /// `|(x[b], x[b+1]) - (x[a], x[a+1])| - v` (a, b are x columns of points)
    Dist { a: usize, b: usize, v: f64 },
    /// `|a - c| - |b - d|` (x columns of points): equal distances. Used for the
    /// equation an `Arc` implies (`c = d = centre`).
    SameDist {
        c: usize,
        a: usize,
        d: usize,
        b: usize,
    },
    /// `(p - c)·(q - p) / |q - p|`: component of the radius vector `p - c` along
    /// the line `p -> q` (mm); zero when the line is tangent at `p`.
    Tangent { p: usize, c: usize, q: usize },
    /// `(p - c1) × (p - c2) / |p - c2|` (mm): zero when the radius vectors of
    /// two arcs at their shared point `p` are collinear (tangent arcs).
    TangentArcs { p: usize, c1: usize, c2: usize },
    /// `|a - c| - x[r]` (point x columns `c`, `a`; radius column `r`).
    DistVar { c: usize, a: usize, r: usize },
    /// Line-line angle: `k · (u_a × u_b)` (`perp == false`, parallel) or
    /// `k · (u_a · u_b)` (`perp == true`, perpendicular), where `u_a` is the
    /// unit direction `a0 -> a1`, `u_b` the unit direction `b0 -> b1` and `k`
    /// the sketch scale (mm, fixed when the system is built). The residual is
    /// `k sin θ` / `k cos θ` in mm and every Jacobian entry is `k / length`,
    /// dimensionless.
    /// Fixed signed angle: `k · wrap(atan2(u_a × u_b, u_a · u_b) − θ0)` (rad
    /// times the sketch scale `k`, so mm), wrap to (−π, π].
    FixedAngle {
        a0: usize,
        a1: usize,
        b0: usize,
        b1: usize,
        k: f64,
        theta: f64,
    },
    LineAngle {
        a0: usize,
        a1: usize,
        b0: usize,
        b1: usize,
        k: f64,
        perp: bool,
    },
    /// `u × (p - a)` with `u` the unit direction `a -> b`: signed distance (mm)
    /// of point `p` from the infinite line through `a`, `b`.
    PointLine { p: usize, a: usize, b: usize },
    /// `PointLine − v` (offset copy vertex on the offset of a source line).
    PointLineOff {
        p: usize,
        a: usize,
        b: usize,
        v: f64,
    },
    /// `|a − c| − |b − d| − v` (offset copy vertex on the offset of a source arc).
    SameDistOff {
        c: usize,
        a: usize,
        d: usize,
        b: usize,
        v: f64,
    },
    /// `x[p+k] − x[q+k] − v · n_k`, `n` the left unit normal of `a -> b`
    /// (tangent-joint copy vertex `p` of source vertex `q`, axis `k`).
    VertLine {
        p: usize,
        q: usize,
        a: usize,
        b: usize,
        v: f64,
        k: usize,
    },
    /// `x[to] − x[from] − k (x[b] − x[a])` (one axis; pattern step).
    Step {
        to: usize,
        from: usize,
        a: usize,
        b: usize,
        k: f64,
    },
    /// Axis `axis` of `to − c − R (from − c)`, R the rotation by (cs, sn)
    /// (point x columns; circular pattern copy).
    Rot {
        to: usize,
        from: usize,
        c: usize,
        cs: f64,
        sn: f64,
        axis: usize,
    },
    /// `x[p+k] − x[q+k] − v · w_k`, `w` the unit direction `c -> q`.
    VertRadial {
        p: usize,
        q: usize,
        c: usize,
        v: f64,
        k: usize,
    },
}

/// Adds the partial derivatives of `f(u)` (with `u = (q - p)/|q - p|`, the
/// unit direction from point column `p` to `q`) into `row`, given the
/// gradient `g = df/du`: `df/d(q - p) = (g - (g·u) u) / |q - p|`. Zero below
/// [`DIST_GRAD_FLOOR`], where [`unit`] returns a fixed direction.
fn add_unit_grad(x: &[f64], j: &mut DMatrix<f64>, row: usize, p: usize, q: usize, g: (f64, f64)) {
    let (ux, uy, l) = unit(x, p, q);
    if l <= DIST_GRAD_FLOOR {
        return;
    }
    let gu = g.0 * ux + g.1 * uy;
    let (wx, wy) = ((g.0 - gu * ux) / l, (g.1 - gu * uy) / l);
    j[(row, q)] += wx;
    j[(row, q + 1)] += wy;
    j[(row, p)] -= wx;
    j[(row, p + 1)] -= wy;
}

/// Unit vector from `a` to `b` (x columns) and the distance; a fixed unit
/// direction below [`DIST_GRAD_FLOOR`].
fn unit(x: &[f64], a: usize, b: usize) -> (f64, f64, f64) {
    let (dx, dy) = (x[b] - x[a], x[b + 1] - x[a + 1]);
    let l = dx.hypot(dy);
    if l > DIST_GRAD_FLOOR {
        (dx / l, dy / l, l)
    } else {
        (1.0, 0.0, l)
    }
}

/// Below this length (mm) the distance gradient is undefined; the Jacobian row
/// uses a fixed unit direction so LM can still move the points apart.
const DIST_GRAD_FLOOR: f64 = 1e-12;

impl Eq {
    fn eval(&self, x: &[f64]) -> f64 {
        match *self {
            Eq::Fix { c, v } => x[c] - v,
            Eq::Diff { a, b, v } => x[b] - x[a] - v,
            Eq::Mid { a, b, m } => 0.5 * (x[a] + x[b]) - x[m],
            Eq::Scaled { c, k, v } => k * x[c] - v,
            Eq::Dist { a, b, v } => (x[b] - x[a]).hypot(x[b + 1] - x[a + 1]) - v,
            Eq::SameDist { c, a, d, b } => {
                (x[a] - x[c]).hypot(x[a + 1] - x[c + 1]) - (x[b] - x[d]).hypot(x[b + 1] - x[d + 1])
            }
            Eq::Tangent { p, c, q } => {
                let (ux, uy, _) = unit(x, p, q);
                (x[p] - x[c]) * ux + (x[p + 1] - x[c + 1]) * uy
            }
            Eq::TangentArcs { p, c1, c2 } => {
                let (ux, uy, _) = unit(x, c2, p);
                (x[p] - x[c1]) * uy - (x[p + 1] - x[c1 + 1]) * ux
            }
            Eq::DistVar { c, a, r } => (x[a] - x[c]).hypot(x[a + 1] - x[c + 1]) - x[r],
            Eq::LineAngle {
                a0,
                a1,
                b0,
                b1,
                k,
                perp,
            } => {
                let (ax, ay, _) = unit(x, a0, a1);
                let (bx, by, _) = unit(x, b0, b1);
                if perp {
                    k * (ax * bx + ay * by)
                } else {
                    k * (ax * by - ay * bx)
                }
            }
            Eq::PointLine { p, a, b } => {
                let (ux, uy, _) = unit(x, a, b);
                ux * (x[p + 1] - x[a + 1]) - uy * (x[p] - x[a])
            }
            Eq::PointLineOff { p, a, b, v } => Eq::PointLine { p, a, b }.eval(x) - v,
            Eq::SameDistOff { c, a, d, b, v } => Eq::SameDist { c, a, d, b }.eval(x) - v,
            Eq::VertLine { p, q, a, b, v, k } => {
                let (ux, uy, _) = unit(x, a, b);
                let n = [-uy, ux];
                x[p + k] - x[q + k] - v * n[k]
            }
            Eq::Step { to, from, a, b, k } => x[to] - x[from] - k * (x[b] - x[a]),
            Eq::Rot {
                to,
                from,
                c,
                cs,
                sn,
                axis,
            } => {
                let (dx, dy) = (x[from] - x[c], x[from + 1] - x[c + 1]);
                if axis == 0 {
                    x[to] - x[c] - (cs * dx - sn * dy)
                } else {
                    x[to + 1] - x[c + 1] - (sn * dx + cs * dy)
                }
            }
            Eq::VertRadial { p, q, c, v, k } => {
                let (wx, wy, _) = unit(x, c, q);
                let w = [wx, wy];
                x[p + k] - x[q + k] - v * w[k]
            }
            Eq::FixedAngle {
                a0,
                a1,
                b0,
                b1,
                k,
                theta,
            } => {
                let (ax, ay, _) = unit(x, a0, a1);
                let (bx, by, _) = unit(x, b0, b1);
                let t = (ax * by - ay * bx).atan2(ax * bx + ay * by) - theta;
                k * (t + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - k * std::f64::consts::PI
            }
        }
    }

    /// Adds this row's partial derivatives into `row` of `j`.
    fn jac(&self, x: &[f64], j: &mut DMatrix<f64>, row: usize) {
        match *self {
            Eq::Fix { c, .. } => j[(row, c)] += 1.0,
            Eq::Diff { a, b, .. } => {
                j[(row, b)] += 1.0;
                j[(row, a)] -= 1.0;
            }
            Eq::Mid { a, b, m } => {
                j[(row, a)] += 0.5;
                j[(row, b)] += 0.5;
                j[(row, m)] -= 1.0;
            }
            Eq::Scaled { c, k, .. } => j[(row, c)] += k,
            Eq::Dist { a, b, .. } => {
                let (ux, uy, _) = unit(x, a, b);
                j[(row, b)] += ux;
                j[(row, b + 1)] += uy;
                j[(row, a)] -= ux;
                j[(row, a + 1)] -= uy;
            }
            Eq::SameDist { c, a, d, b } => {
                let (ux, uy, _) = unit(x, c, a);
                j[(row, a)] += ux;
                j[(row, a + 1)] += uy;
                j[(row, c)] -= ux;
                j[(row, c + 1)] -= uy;
                let (vx, vy, _) = unit(x, d, b);
                j[(row, b)] -= vx;
                j[(row, b + 1)] -= vy;
                j[(row, d)] += vx;
                j[(row, d + 1)] += vy;
            }
            Eq::Tangent { p, c, q } => {
                // f = v·u with v = p - c, u = (q - p)/L. df/dv = u;
                // df/d(q - p) = (v - (v·u) u) / L (zero below the length floor,
                // where u is a fixed direction).
                let (ux, uy, l) = unit(x, p, q);
                let (vx, vy) = (x[p] - x[c], x[p + 1] - x[c + 1]);
                let vu = vx * ux + vy * uy;
                let (wx, wy) = if l > DIST_GRAD_FLOOR {
                    ((vx - vu * ux) / l, (vy - vu * uy) / l)
                } else {
                    (0.0, 0.0)
                };
                j[(row, p)] += ux - wx;
                j[(row, p + 1)] += uy - wy;
                j[(row, c)] -= ux;
                j[(row, c + 1)] -= uy;
                j[(row, q)] += wx;
                j[(row, q + 1)] += wy;
            }
            Eq::TangentArcs { p, c1, c2 } => {
                // f = v × u, v = p - c1, u = (p - c2)/L. df/dv = (uy, -ux);
                // df/du = g = (-vy, vx); df/dw = (g - (g·u) u) / L for w = p - c2
                // (zero below the length floor, where u is fixed).
                let (ux, uy, l) = unit(x, c2, p);
                let (vx, vy) = (x[p] - x[c1], x[p + 1] - x[c1 + 1]);
                let (gx, gy) = (-vy, vx);
                let gu = gx * ux + gy * uy;
                let (wx, wy) = if l > DIST_GRAD_FLOOR {
                    ((gx - gu * ux) / l, (gy - gu * uy) / l)
                } else {
                    (0.0, 0.0)
                };
                j[(row, p)] += uy + wx;
                j[(row, p + 1)] += -ux + wy;
                j[(row, c1)] -= uy;
                j[(row, c1 + 1)] += ux;
                j[(row, c2)] -= wx;
                j[(row, c2 + 1)] -= wy;
            }
            Eq::DistVar { c, a, r } => {
                let (ux, uy, _) = unit(x, c, a);
                j[(row, a)] += ux;
                j[(row, a + 1)] += uy;
                j[(row, c)] -= ux;
                j[(row, c + 1)] -= uy;
                j[(row, r)] -= 1.0;
            }
            Eq::LineAngle {
                a0,
                a1,
                b0,
                b1,
                k,
                perp,
            } => {
                let (ax, ay, _) = unit(x, a0, a1);
                let (bx, by, _) = unit(x, b0, b1);
                // perp: f = k u_a·u_b, df/du_a = k u_b, df/du_b = k u_a.
                // parallel: f = k (ax by − ay bx), df/du_a = k (by, −bx),
                // df/du_b = k (−ay, ax).
                let (ga, gb) = if perp {
                    ((k * bx, k * by), (k * ax, k * ay))
                } else {
                    ((k * by, -k * bx), (-k * ay, k * ax))
                };
                add_unit_grad(x, j, row, a0, a1, ga);
                add_unit_grad(x, j, row, b0, b1, gb);
            }
            Eq::FixedAngle {
                a0, a1, b0, b1, k, ..
            } => {
                // θ = atan2(cr, dt) of unit vectors (cr² + dt² = 1):
                // dθ = dt·d(cr) − cr·d(dt); d(cr)/du_a = (by, −bx),
                // d(dt)/du_a = (bx, by); d(cr)/du_b = (−ay, ax), d(dt)/du_b = (ax, ay).
                let (ax, ay, _) = unit(x, a0, a1);
                let (bx, by, _) = unit(x, b0, b1);
                let (cr, dt) = (ax * by - ay * bx, ax * bx + ay * by);
                let ga = (k * (dt * by - cr * bx), k * (-dt * bx - cr * by));
                let gb = (k * (-dt * ay - cr * ax), k * (dt * ax - cr * ay));
                add_unit_grad(x, j, row, a0, a1, ga);
                add_unit_grad(x, j, row, b0, b1, gb);
            }
            Eq::PointLine { p, a, b } => {
                // f = u × v with v = p − a: df/dv = (−uy, ux) (moves p and a);
                // df/du = (vy, −vx) (through u = (b − a)/|b − a|).
                let (ux, uy, _) = unit(x, a, b);
                let (vx, vy) = (x[p] - x[a], x[p + 1] - x[a + 1]);
                j[(row, p)] -= uy;
                j[(row, p + 1)] += ux;
                j[(row, a)] += uy;
                j[(row, a + 1)] -= ux;
                add_unit_grad(x, j, row, a, b, (vy, -vx));
            }
            Eq::PointLineOff { p, a, b, .. } => Eq::PointLine { p, a, b }.jac(x, j, row),
            Eq::SameDistOff { c, a, d, b, .. } => Eq::SameDist { c, a, d, b }.jac(x, j, row),
            Eq::VertLine { p, q, a, b, v, k } => {
                // f = x[p+k] − x[q+k] − v n_k with n = (−u_y, u_x):
                // df/du = (0, −v)·… : k = 0 → (0, v); k = 1 → (−v, 0).
                j[(row, p + k)] += 1.0;
                j[(row, q + k)] -= 1.0;
                let g = if k == 0 { (0.0, v) } else { (-v, 0.0) };
                add_unit_grad(x, j, row, a, b, g);
            }
            Eq::Step { to, from, a, b, k } => {
                j[(row, to)] += 1.0;
                j[(row, from)] -= 1.0;
                j[(row, b)] -= k;
                j[(row, a)] += k;
            }
            Eq::Rot {
                to,
                from,
                c,
                cs,
                sn,
                axis,
            } => {
                // d/d(from) of R(from − c) row `axis`, and of −c + R c.
                let (rx, ry) = if axis == 0 { (cs, -sn) } else { (sn, cs) };
                j[(row, to + axis)] += 1.0;
                j[(row, c + axis)] -= 1.0;
                j[(row, from)] -= rx;
                j[(row, from + 1)] -= ry;
                j[(row, c)] += rx;
                j[(row, c + 1)] += ry;
            }
            Eq::VertRadial { p, q, c, v, k } => {
                j[(row, p + k)] += 1.0;
                j[(row, q + k)] -= 1.0;
                let g = if k == 0 { (-v, 0.0) } else { (0.0, -v) };
                add_unit_grad(x, j, row, c, q, g);
            }
        }
    }
}

/// Compiled equation system. Assumes the sketch passed `validate()`.
pub(crate) struct System {
    eqs: Vec<Eq>,
    /// Constraint index owning each equation row.
    owner: Vec<usize>,
    pub(crate) initial: Vec<f64>,
    n: usize,
    point_col: HashMap<crate::PointId, usize>,
    radius_col: Vec<(usize, usize)>, // (entity index, column)
}

/// Temporary equation used for interactive drags; not part of the sketch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pin {
    /// `axis` 0 = x, 1 = y.
    Coord {
        p: crate::PointId,
        axis: usize,
        value: f64,
    },
    Radius {
        circle: crate::EntityId,
        value: f64,
    },
}

impl System {
    pub(crate) fn build(s: &Sketch) -> System {
        Self::build_with(s, &[])
    }

    /// Build with extra pin equations. Pins are owned by the pseudo-constraint
    /// index `s.constraints.len()`, so conflict diagnosis never reports them.
    pub(crate) fn build_with(s: &Sketch, pins: &[Pin]) -> System {
        let mut initial = Vec::with_capacity(s.points.len() * 2);
        let mut point_col = HashMap::new();
        for p in &s.points {
            point_col.insert(p.id, initial.len());
            initial.push(p.x);
            initial.push(p.y);
        }
        let mut radius_col = Vec::new();
        let mut lines = HashMap::new();
        let mut circles = HashMap::new();
        let mut arcs = HashMap::new();
        for (ei, e) in s.entities.iter().enumerate() {
            match e {
                Entity::Line { id, a, b } => {
                    lines.insert(*id, (point_col[a], point_col[b]));
                }
                Entity::Circle { id, radius, .. } => {
                    circles.insert(*id, initial.len());
                    radius_col.push((ei, initial.len()));
                    initial.push(*radius);
                }
                Entity::Arc {
                    id,
                    center,
                    start,
                    end,
                } => {
                    arcs.insert(*id, (point_col[center], point_col[start], point_col[end]));
                }
            }
        }
        let entity = |id: &crate::EntityId| s.entities.iter().find(|e| e.id() == *id);
        // Fixed per build: scales the unit-vector line-angle rows to mm.
        let angle_scale = s.scale();
        let mut eqs = Vec::new();
        let mut owner = Vec::new();
        for (ci, c) in s.constraints.iter().enumerate() {
            let mut push = |e: Eq| {
                eqs.push(e);
                owner.push(ci);
            };
            match c {
                Constraint::FixedPoint { p, x, y } => {
                    let c = point_col[p];
                    push(Eq::Fix { c, v: *x });
                    push(Eq::Fix { c: c + 1, v: *y });
                }
                Constraint::FixedX { p, x } => {
                    push(Eq::Fix {
                        c: point_col[p],
                        v: *x,
                    });
                }
                Constraint::FixedY { p, y } => {
                    push(Eq::Fix {
                        c: point_col[p] + 1,
                        v: *y,
                    });
                }
                Constraint::Horizontal { line } => {
                    let (a, b) = lines[line];
                    push(Eq::Diff {
                        a: a + 1,
                        b: b + 1,
                        v: 0.0,
                    });
                }
                Constraint::Vertical { line } => {
                    let (a, b) = lines[line];
                    push(Eq::Diff { a, b, v: 0.0 });
                }
                Constraint::HorizontalDistance { a, b, dim } => push(Eq::Diff {
                    a: point_col[a],
                    b: point_col[b],
                    v: dim.value,
                }),
                Constraint::VerticalDistance { a, b, dim } => push(Eq::Diff {
                    a: point_col[a] + 1,
                    b: point_col[b] + 1,
                    v: dim.value,
                }),
                Constraint::SymmetricAboutPoint { a, b, center } => {
                    let (a, b, m) = (point_col[a], point_col[b], point_col[center]);
                    push(Eq::Mid { a, b, m });
                    push(Eq::Mid {
                        a: a + 1,
                        b: b + 1,
                        m: m + 1,
                    });
                }
                Constraint::Diameter { circle, dim } => push(Eq::Scaled {
                    c: circles[circle],
                    k: 2.0,
                    v: dim.value,
                }),
                Constraint::Distance { a, b, dim } => push(Eq::Dist {
                    a: point_col[a],
                    b: point_col[b],
                    v: dim.value,
                }),
                Constraint::Coincident { a, b } => {
                    let (a, b) = (point_col[a], point_col[b]);
                    push(Eq::Diff { a, b, v: 0.0 });
                    push(Eq::Diff {
                        a: a + 1,
                        b: b + 1,
                        v: 0.0,
                    });
                }
                Constraint::Radius { entity, dim } => {
                    if let Some(&(c, a, _)) = arcs.get(entity) {
                        push(Eq::Dist {
                            a: c,
                            b: a,
                            v: dim.value,
                        });
                    } else if let Some(&c) = circles.get(entity) {
                        push(Eq::Fix { c, v: dim.value });
                    }
                }
                Constraint::EqualRadius { a, b } => {
                    let e = match (arcs.get(a), arcs.get(b), circles.get(a), circles.get(b)) {
                        (Some(&(c1, s1, _)), Some(&(c2, s2, _)), _, _) => Some(Eq::SameDist {
                            c: c1,
                            a: s1,
                            d: c2,
                            b: s2,
                        }),
                        (Some(&(c, s, _)), None, _, Some(&r))
                        | (None, Some(&(c, s, _)), Some(&r), _) => Some(Eq::DistVar { c, a: s, r }),
                        (None, None, Some(&r1), Some(&r2)) => Some(Eq::Diff {
                            a: r1,
                            b: r2,
                            v: 0.0,
                        }),
                        _ => None,
                    };
                    if let Some(e) = e {
                        push(e);
                    }
                }
                Constraint::Tangent { line, arc } => {
                    if let (Some(l), Some(a)) = (entity(line), entity(arc))
                        && let Ok((p, c, q)) = crate::tangent_shared_point(l, a)
                    {
                        push(Eq::Tangent {
                            p: point_col[&p],
                            c: point_col[&c],
                            q: point_col[&q],
                        });
                    }
                }
                Constraint::TangentArcs { a, b } => {
                    if let (Some(ea), Some(eb)) = (entity(a), entity(b))
                        && let Ok((p, c1, c2)) = crate::arcs_shared_point(ea, eb)
                    {
                        push(Eq::TangentArcs {
                            p: point_col[&p],
                            c1: point_col[&c1],
                            c2: point_col[&c2],
                        });
                    }
                }
                Constraint::Parallel { a, b } | Constraint::Perpendicular { a, b } => {
                    if let (Some(&(a0, a1)), Some(&(b0, b1))) = (lines.get(a), lines.get(b)) {
                        push(Eq::LineAngle {
                            a0,
                            a1,
                            b0,
                            b1,
                            k: angle_scale,
                            perp: matches!(c, Constraint::Perpendicular { .. }),
                        });
                    }
                }
                Constraint::PatternStep {
                    from,
                    to,
                    a,
                    b,
                    k,
                    axis,
                } => {
                    let (from, to, a, b) =
                        (point_col[from], point_col[to], point_col[a], point_col[b]);
                    for ax in 0..2usize {
                        if axis.is_none_or(|x| usize::from(x) == ax) {
                            push(Eq::Step {
                                to: to + ax,
                                from: from + ax,
                                a: a + ax,
                                b: b + ax,
                                k: f64::from(*k),
                            });
                        }
                    }
                }
                Constraint::PatternRotate {
                    from,
                    to,
                    center,
                    k,
                    dim,
                    axis,
                } => {
                    let (sn, cs) = (f64::from(*k) * dim.value.to_radians()).sin_cos();
                    for ax in 0..2usize {
                        if axis.is_none_or(|x| usize::from(x) == ax) {
                            push(Eq::Rot {
                                to: point_col[to],
                                from: point_col[from],
                                c: point_col[center],
                                cs,
                                sn,
                                axis: ax,
                            });
                        }
                    }
                }
                Constraint::Angle { a, b, dim } => {
                    if let (Some(&(a0, a1)), Some(&(b0, b1))) = (lines.get(a), lines.get(b)) {
                        push(Eq::FixedAngle {
                            a0,
                            a1,
                            b0,
                            b1,
                            k: angle_scale,
                            theta: dim.value.to_radians(),
                        });
                    }
                }
                Constraint::EqualLength { a, b } => {
                    if let (Some(&(a0, a1)), Some(&(b0, b1))) = (lines.get(a), lines.get(b)) {
                        push(Eq::SameDist {
                            c: a0,
                            a: a1,
                            d: b0,
                            b: b1,
                        });
                    }
                }
                Constraint::PointOnLine { point, line } => {
                    if let (Some(&p), Some(&(a, b))) = (point_col.get(point), lines.get(line)) {
                        push(Eq::PointLine { p, a, b });
                    }
                }
                Constraint::Offset { pairs, dim } => {
                    let v = dim.value;
                    if let [one] = pairs.as_slice()
                        && let (Some(&r0), Some(&r1)) =
                            (circles.get(&one.source), circles.get(&one.copy))
                    {
                        push(Eq::Diff {
                            a: r0,
                            b: r1,
                            v: f64::from(one.sign) * v,
                        });
                        continue;
                    }
                    let n = pairs.len();
                    for i in 0..n {
                        let (pp, qq) = (&pairs[i], &pairs[(i + 1) % n]);
                        let (Some(src), Some(cp)) = (
                            crate::offset::shared_point(s, pp.source, qq.source),
                            crate::offset::shared_point(s, pp.copy, qq.copy),
                        ) else {
                            continue;
                        };
                        let (q, p) = (point_col[&src], point_col[&cp]);
                        // Row of pair `x` holding copy vertex `p`.
                        let row = |x: &crate::OffsetPair| -> Option<Eq> {
                            let sv = f64::from(x.sign) * v;
                            if let Some(&(a, b)) = lines.get(&x.source) {
                                Some(Eq::PointLineOff { p, a, b, v: sv })
                            } else {
                                arcs.get(&x.source).map(|&(c, st, _)| Eq::SameDistOff {
                                    c,
                                    a: p,
                                    d: c,
                                    b: st,
                                    v: sv,
                                })
                            }
                        };
                        if pp.tangent_next {
                            let line = [pp, qq].into_iter().find_map(|x| {
                                lines
                                    .get(&x.source)
                                    .map(|&(a, b)| (a, b, f64::from(x.sign) * v))
                            });
                            for k in 0..2 {
                                match line {
                                    Some((a, b, sv)) => push(Eq::VertLine {
                                        p,
                                        q,
                                        a,
                                        b,
                                        v: sv,
                                        k,
                                    }),
                                    None => {
                                        if let Some(&(c, _, _)) = arcs.get(&pp.source) {
                                            push(Eq::VertRadial {
                                                p,
                                                q,
                                                c,
                                                v: f64::from(pp.sign) * v,
                                                k,
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            for x in [pp, qq] {
                                if let Some(e) = row(x) {
                                    push(e);
                                }
                            }
                        }
                    }
                }
                Constraint::PointOnCircle { point, circle } => {
                    let Some(&p) = point_col.get(point) else {
                        continue;
                    };
                    if let Some(&(c, start, _)) = arcs.get(circle) {
                        // |p − c| − |start − c|
                        push(Eq::SameDist {
                            c,
                            a: p,
                            d: c,
                            b: start,
                        });
                    } else if let (Some(&r), Some(Entity::Circle { center, .. })) =
                        (circles.get(circle), entity(circle))
                    {
                        // |p − c| − r
                        push(Eq::DistVar {
                            c: point_col[center],
                            a: p,
                            r,
                        });
                    }
                }
            }
        }
        // Pseudo-owner for equations that are not user constraints (arc-implied
        // equal radius, drag pins): never removed or reported by diagnosis.
        let pin_owner = s.constraints.len();
        // Offset copy arcs: both ends are fixed by the offset vertex rows, which
        // imply the equal radius (from the source arc's own row).
        let offset_arcs: std::collections::HashSet<crate::EntityId> = s
            .constraints
            .iter()
            .flat_map(|c| match c {
                Constraint::Offset { pairs, .. } => pairs.iter().map(|p| p.copy).collect(),
                _ => vec![],
            })
            .collect();
        for e in &s.entities {
            if let Entity::Arc { id, .. } = e
                && !offset_arcs.contains(id)
                && let Some(&(c, a, b)) = arcs.get(id)
            {
                eqs.push(Eq::SameDist { c, a, d: c, b });
                owner.push(pin_owner);
            }
        }
        for pin in pins {
            let e = match *pin {
                Pin::Coord { p, axis, value } => point_col.get(&p).map(|&c| Eq::Fix {
                    c: c + axis.min(1),
                    v: value,
                }),
                Pin::Radius { circle, value } => {
                    circles.get(&circle).map(|&c| Eq::Fix { c, v: value })
                }
            };
            if let Some(e) = e {
                eqs.push(e);
                owner.push(pin_owner);
            }
        }
        let n = initial.len();
        System {
            eqs,
            owner,
            initial,
            n,
            point_col,
            radius_col,
        }
    }

    fn residuals(&self, x: &[f64], rows: &[usize]) -> DVector<f64> {
        DVector::from_iterator(rows.len(), rows.iter().map(|&r| self.eqs[r].eval(x)))
    }

    fn jacobian(&self, x: &[f64], rows: &[usize]) -> DMatrix<f64> {
        let mut j = DMatrix::zeros(rows.len(), self.n);
        for (i, &r) in rows.iter().enumerate() {
            self.eqs[r].jac(x, &mut j, i);
        }
        j
    }

    fn all_rows(&self) -> Vec<usize> {
        (0..self.eqs.len()).collect()
    }

    fn rows_without(&self, constraint: usize) -> Vec<usize> {
        (0..self.eqs.len())
            .filter(|&r| self.owner[r] != constraint)
            .collect()
    }

    pub(crate) fn max_residual(&self, x: &[f64]) -> f64 {
        self.eqs.iter().map(|e| e.eval(x).abs()).fold(0.0, f64::max)
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Converged,
    Stalled,
    MaxIterations,
}

struct LmResult {
    x: Vec<f64>,
    max_res: f64,
    iterations: usize,
    outcome: Outcome,
}

fn inf_norm(v: &DVector<f64>) -> f64 {
    v.iter().fold(0.0, |m, e| m.max(e.abs()))
}

/// Damped Gauss-Newton (Levenberg-Marquardt) on the selected rows.
fn levenberg_marquardt(sys: &System, rows: &[usize], x0: &[f64], tol: f64) -> LmResult {
    let mut x = x0.to_vec();
    let mut r = sys.residuals(&x, rows);
    let mut lambda = LAMBDA_INIT;
    let mut iterations = 0;
    let outcome = loop {
        let max_res = inf_norm(&r);
        if max_res <= tol {
            break Outcome::Converged;
        }
        if iterations >= MAX_ITERATIONS {
            break Outcome::MaxIterations;
        }
        iterations += 1;
        let j = sys.jacobian(&x, rows);
        let g = j.tr_mul(&r);
        if inf_norm(&g) <= GRAD_REL_TOL * r.norm().max(1.0) {
            break Outcome::Stalled;
        }
        let jtj = j.tr_mul(&j);
        let cost = r.norm_squared();
        let mut accepted = false;
        while lambda <= LAMBDA_MAX {
            let mut a = jtj.clone();
            for i in 0..sys.n {
                a[(i, i)] += lambda;
            }
            let Some(chol) = a.cholesky() else {
                lambda *= 10.0;
                continue;
            };
            let step = chol.solve(&(-&g));
            let xn: Vec<f64> = x.iter().zip(step.iter()).map(|(a, b)| a + b).collect();
            let rn = sys.residuals(&xn, rows);
            let cost_n = rn.norm_squared();
            if cost_n.is_finite() && cost_n < cost {
                let gain = cost - cost_n;
                x = xn;
                r = rn;
                lambda = (lambda / 10.0).max(1e-12);
                accepted = true;
                // Negligible relative improvement while still above tolerance:
                // we are at the least-squares minimum.
                if gain <= 1e-15 * cost && inf_norm(&r) > tol {
                    lambda = LAMBDA_MAX * 10.0;
                }
                break;
            }
            lambda *= 10.0;
        }
        if !accepted || lambda > LAMBDA_MAX {
            if inf_norm(&r) <= tol {
                break Outcome::Converged;
            }
            break Outcome::Stalled;
        }
    };
    LmResult {
        max_res: inf_norm(&r),
        x,
        iterations,
        outcome,
    }
}

fn rank(j: &DMatrix<f64>) -> usize {
    if j.nrows() == 0 || j.ncols() == 0 {
        return 0;
    }
    let sv = j.singular_values();
    let smax = sv.iter().fold(0.0_f64, |m, &s| m.max(s));
    if smax <= 0.0 || !smax.is_finite() {
        return 0;
    }
    sv.iter().filter(|&&s| s > RANK_REL_TOL * smax).count()
}

pub(crate) fn solve(sketch: &mut Sketch) -> SolveReport {
    solve_with(sketch, &[])
}

/// Solve with temporary pins (see [`Pin`]). Same write-back rule as [`solve`].
pub(crate) fn solve_with(sketch: &mut Sketch, pins: &[Pin]) -> SolveReport {
    if sketch.validate().is_err() {
        return SolveReport {
            status: SolveStatus::Invalid,
            residual: f64::NAN,
            dof: 0,
            iterations: 0,
            conflicting: Vec::new(),
        };
    }
    let sys = System::build_with(sketch, pins);
    let tol = solve_tolerance(sketch.scale());
    let rows = sys.all_rows();
    let lm = levenberg_marquardt(&sys, &rows, &sys.initial, tol);

    let j = sys.jacobian(&lm.x, &rows);
    let full_rank = rank(&j);
    let dof = sys.n - full_rank;
    let m = rows.len();
    let n_constraints = sketch.constraints.len();
    let diagnose = n_constraints <= MAX_DIAGNOSED_CONSTRAINTS;

    let (status, conflicting) = match lm.outcome {
        Outcome::Converged => {
            if m > full_rank {
                // Redundant equations: a constraint is part of a redundant set
                // when removing it leaves the rank unchanged.
                let conflicting = if diagnose {
                    (0..n_constraints)
                        .filter(|&c| {
                            let sub = sys.rows_without(c);
                            sub.len() < m && rank(&sys.jacobian(&lm.x, &sub)) == full_rank
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                (SolveStatus::OverConstrained, conflicting)
            } else if dof > 0 {
                (SolveStatus::UnderConstrained, Vec::new())
            } else {
                (SolveStatus::FullyConstrained, Vec::new())
            }
        }
        Outcome::Stalled => {
            // Candidate culprits: constraints whose removal makes the rest solvable.
            let conflicting = if diagnose {
                (0..n_constraints)
                    .filter(|&c| {
                        let sub = sys.rows_without(c);
                        sub.len() < m
                            && levenberg_marquardt(&sys, &sub, &sys.initial, tol).outcome
                                == Outcome::Converged
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (SolveStatus::Inconsistent, conflicting)
        }
        Outcome::MaxIterations => (SolveStatus::DidNotConverge, Vec::new()),
    };

    let report = SolveReport {
        status,
        residual: lm.max_res,
        dof,
        iterations: lm.iterations,
        conflicting,
    };
    if report.is_success() {
        for p in &mut sketch.points {
            let c = sys.point_col[&p.id];
            p.x = lm.x[c];
            p.y = lm.x[c + 1];
        }
        for &(ei, col) in &sys.radius_col {
            if let Some(Entity::Circle { radius, .. }) = sketch.entities.get_mut(ei) {
                *radius = lm.x[col];
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dimension, EntityId, PointId, SketchPoint};

    /// The analytic Jacobian must match central finite differences for every
    /// equation kind, including the nonlinear Distance row.
    #[test]
    fn analytic_jacobian_matches_finite_differences() {
        let pt = |id: u32, x: f64, y: f64| SketchPoint {
            id: PointId(id),
            x,
            y,
        };
        let s = Sketch {
            origin: None,
            construction: Default::default(),
            retired_entity_ids: 0,
            texts: vec![],
            points: vec![
                pt(0, 0.3, -0.2),
                pt(1, 4.1, 1.7),
                pt(2, -2.5, 3.3),
                pt(3, 7.0, -4.0),
            ],
            entities: vec![
                Entity::Line {
                    id: EntityId(1),
                    a: PointId(0),
                    b: PointId(1),
                },
                Entity::Line {
                    id: EntityId(2),
                    a: PointId(1),
                    b: PointId(2),
                },
                Entity::Circle {
                    id: EntityId(3),
                    center: PointId(3),
                    radius: 2.2,
                },
            ],
            constraints: vec![
                Constraint::FixedPoint {
                    p: PointId(0),
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Horizontal { line: EntityId(1) },
                Constraint::Vertical { line: EntityId(2) },
                Constraint::HorizontalDistance {
                    a: PointId(0),
                    b: PointId(1),
                    dim: Dimension::new("w", 5.0),
                },
                Constraint::VerticalDistance {
                    a: PointId(1),
                    b: PointId(2),
                    dim: Dimension::new("h", 3.0),
                },
                Constraint::SymmetricAboutPoint {
                    a: PointId(0),
                    b: PointId(2),
                    center: PointId(3),
                },
                Constraint::Diameter {
                    circle: EntityId(3),
                    dim: Dimension::new("d", 4.0),
                },
                Constraint::Coincident {
                    a: PointId(2),
                    b: PointId(3),
                },
                Constraint::Distance {
                    a: PointId(0),
                    b: PointId(2),
                    dim: Dimension::new("len", 6.0),
                },
            ],
        };
        s.validate().unwrap();
        let sys = System::build(&s);
        let rows = sys.all_rows();
        let x0 = sys.initial.clone();
        let j = sys.jacobian(&x0, &rows);
        let h = 1e-6;
        let mut max_err: f64 = 0.0;
        for c in 0..sys.n {
            let (mut xp, mut xm) = (x0.clone(), x0.clone());
            xp[c] += h;
            xm[c] -= h;
            let (rp, rm) = (sys.residuals(&xp, &rows), sys.residuals(&xm, &rows));
            for r in 0..rows.len() {
                let fd = (rp[r] - rm[r]) / (2.0 * h);
                max_err = max_err.max((fd - j[(r, c)]).abs());
            }
        }
        assert!(
            max_err < 1e-7,
            "Jacobian vs finite differences: {max_err:e}"
        );
        // Distance row against the hand-derived gradient d|b-a|/db = (b-a)/|b-a|
        // with a = (0.3, -0.2), b = (-2.5, 3.3).
        let (dx, dy): (f64, f64) = (-2.5 - 0.3, 3.3 + 0.2);
        let l = (dx * dx + dy * dy).sqrt();
        let last = rows.len() - 1;
        let bcol = sys.point_col[&PointId(2)];
        assert!((j[(last, bcol)] - dx / l).abs() < 1e-12);
        assert!((j[(last, bcol + 1)] - dy / l).abs() < 1e-12);
    }

    /// Arc rows (implied equal radius, Radius, Tangent, EqualRadius arc-arc and
    /// circle-arc) against central finite
    /// differences at a generic (unsolved) configuration.
    #[test]
    fn arc_rows_jacobian_matches_finite_differences() {
        let pt = |id: u32, x: f64, y: f64| SketchPoint {
            id: PointId(id),
            x,
            y,
        };
        let s = Sketch {
            origin: None,
            construction: Default::default(),
            retired_entity_ids: 0,
            texts: vec![],
            points: vec![
                pt(0, 0.4, -0.3),
                pt(1, 9.1, 1.2),
                pt(2, -1.5, 8.7),
                pt(3, 13.0, -6.0),
                pt(4, 2.0, 3.0),
                pt(5, 6.5, 2.5),
                pt(6, 3.0, 9.0),
                pt(7, -6.0, 12.0),
                pt(8, -9.0, 4.0),
            ],
            entities: vec![
                Entity::Arc {
                    id: EntityId(1),
                    center: PointId(0),
                    start: PointId(1),
                    end: PointId(2),
                },
                Entity::Line {
                    id: EntityId(2),
                    a: PointId(3),
                    b: PointId(1),
                },
                Entity::Arc {
                    id: EntityId(3),
                    center: PointId(4),
                    start: PointId(5),
                    end: PointId(6),
                },
                Entity::Circle {
                    id: EntityId(4),
                    center: PointId(4),
                    radius: 3.3,
                },
                // An arc sharing arc 1's end point (TangentArcs row).
                Entity::Arc {
                    id: EntityId(5),
                    center: PointId(7),
                    start: PointId(2),
                    end: PointId(8),
                },
            ],
            constraints: vec![
                Constraint::TangentArcs {
                    a: EntityId(1),
                    b: EntityId(5),
                },
                Constraint::Radius {
                    entity: EntityId(1),
                    dim: Dimension::new("r", 9.0),
                },
                Constraint::Tangent {
                    line: EntityId(2),
                    arc: EntityId(1),
                },
                Constraint::EqualRadius {
                    a: EntityId(1),
                    b: EntityId(3),
                },
                Constraint::EqualRadius {
                    a: EntityId(4),
                    b: EntityId(3),
                },
            ],
        };
        s.validate().unwrap();
        let sys = System::build(&s);
        assert_eq!(
            sys.eqs.len(),
            8,
            "tangent arcs, radius, tangent, 2 equal radius, 3 implied"
        );
        assert_eq!(
            sys.owner[5],
            s.constraints.len(),
            "implied rows: pseudo-owner"
        );
        assert_eq!(sys.owner[6], s.constraints.len());
        assert_eq!(sys.owner[7], s.constraints.len());
        let rows = sys.all_rows();
        let x0 = sys.initial.clone();
        let j = sys.jacobian(&x0, &rows);
        let h = 1e-6;
        let mut max_err: f64 = 0.0;
        for c in 0..sys.n {
            let (mut xp, mut xm) = (x0.clone(), x0.clone());
            xp[c] += h;
            xm[c] -= h;
            let (rp, rm) = (sys.residuals(&xp, &rows), sys.residuals(&xm, &rows));
            for r in 0..rows.len() {
                let fd = (rp[r] - rm[r]) / (2.0 * h);
                max_err = max_err.max((fd - j[(r, c)]).abs());
            }
        }
        assert!(
            max_err < 1e-7,
            "Jacobian vs finite differences: {max_err:e}"
        );
        // Tangent residual by hand: P = (9.1, 1.2), C = (0.4, -0.3), Q = (13, -6):
        // (P - C)·(Q - P)/|Q - P| = (8.7*3.9 + 1.5*(-7.2)) / sqrt(3.9^2 + 7.2^2).
        let want = (8.7 * 3.9 - 1.5 * 7.2) / (3.9f64 * 3.9 + 7.2 * 7.2).sqrt();
        assert!((sys.eqs[2].eval(&x0) - want).abs() < 1e-12);
        // TangentArcs by hand: P = (-1.5, 8.7), C1 = (0.4, -0.3), C2 = (-6, 12):
        // (P - C1) × (P - C2) / |P - C2|.
        let want = (-1.9 * -3.3 - 9.0 * 4.5) / (4.5f64 * 4.5 + 3.3 * 3.3).sqrt();
        assert!((sys.eqs[0].eval(&x0) - want).abs() < 1e-12);
    }

    /// Coincident endpoints: the Distance gradient falls back to a unit
    /// direction instead of NaN, so the row still has rank 1.
    #[test]
    fn distance_row_is_finite_at_zero_length() {
        let s = Sketch {
            origin: None,
            construction: Default::default(),
            retired_entity_ids: 0,
            texts: vec![],
            points: vec![
                SketchPoint {
                    id: PointId(0),
                    x: 1.0,
                    y: 1.0,
                },
                SketchPoint {
                    id: PointId(1),
                    x: 1.0,
                    y: 1.0,
                },
            ],
            entities: vec![],
            constraints: vec![Constraint::Distance {
                a: PointId(0),
                b: PointId(1),
                dim: Dimension::new("d", 5.0),
            }],
        };
        let sys = System::build(&s);
        let j = sys.jacobian(&sys.initial, &sys.all_rows());
        assert!(j.iter().all(|v| v.is_finite()));
        assert_eq!(rank(&j), 1);
    }

    /// Central-difference check of selected rows; step `h` (mm), max absolute
    /// error returned.
    fn fd_error(sys: &System, x0: &[f64], h: f64) -> f64 {
        let rows = sys.all_rows();
        let j = sys.jacobian(x0, &rows);
        let mut max_err: f64 = 0.0;
        for c in 0..sys.n {
            let (mut xp, mut xm) = (x0.to_vec(), x0.to_vec());
            xp[c] += h;
            xm[c] -= h;
            let (rp, rm) = (sys.residuals(&xp, &rows), sys.residuals(&xm, &rows));
            for r in 0..rows.len() {
                let fd = (rp[r] - rm[r]) / (2.0 * h);
                max_err = max_err.max((fd - j[(r, c)]).abs());
            }
        }
        max_err
    }

    /// Two lines L1 = P0->P1, L2 = P2->P3, point P4; every line relation.
    fn line_relations(pts: [(f64, f64); 5]) -> Sketch {
        let mut s = Sketch::default();
        for (x, y) in pts {
            s.add_point(x, y);
        }
        let l1 = s.add_line(PointId(0), PointId(1));
        let l2 = s.add_line(PointId(2), PointId(3));
        s.constraints = vec![
            Constraint::Parallel { a: l1, b: l2 },
            Constraint::Perpendicular { a: l1, b: l2 },
            Constraint::EqualLength { a: l1, b: l2 },
            Constraint::PointOnLine {
                point: PointId(4),
                line: l1,
            },
            Constraint::PointOnLine {
                point: PointId(4),
                line: l2,
            },
        ];
        s.validate().unwrap();
        s
    }

    /// Parallel / Perpendicular / EqualLength / PointOnLine rows against
    /// central finite differences at a generic configuration, and the
    /// residuals against hand-computed values.
    #[test]
    fn line_relation_rows_match_finite_differences() {
        let s = line_relations([
            (0.4, -0.3),
            (9.1, 1.2),
            (-1.5, 8.7),
            (3.0, 13.0),
            (2.0, 3.0),
        ]);
        let sys = System::build(&s);
        assert_eq!(sys.eqs.len(), 5);
        let x0 = sys.initial.clone();
        let err = fd_error(&sys, &x0, 1e-6);
        assert!(err < 1e-7, "Jacobian vs finite differences: {err:e}");

        // Hand values. d1 = (8.7, 1.5), d2 = (4.5, 4.3); k = sketch scale = 13.
        let (d1, d2) = ((8.7f64, 1.5f64), (4.5f64, 4.3f64));
        let (n1, n2) = (d1.0.hypot(d1.1), d2.0.hypot(d2.1));
        let k = 13.0;
        let cross = (d1.0 * d2.1 - d1.1 * d2.0) / (n1 * n2);
        let dot = (d1.0 * d2.0 + d1.1 * d2.1) / (n1 * n2);
        let want = [
            k * cross,
            k * dot,
            n1 - n2,
            // P4 − P0 = (1.6, 3.3) against u1 = d1/|d1|.
            (d1.0 * 3.3 - d1.1 * 1.6) / n1,
            // P4 − P2 = (3.5, −5.7) against u2.
            (d2.0 * -5.7 - d2.1 * 3.5) / n2,
        ];
        for (i, w) in want.iter().enumerate() {
            let got = sys.eqs[i].eval(&x0);
            assert!((got - w).abs() < 1e-12, "row {i}: {got} vs {w}");
        }
    }

    /// Near-degenerate short lines (1e-3 mm and 1e-5 mm in a 13 mm sketch):
    /// the rows stay finite and the analytic Jacobian still matches finite
    /// differences (relative to the largest entry; steps well below the
    /// line length).
    #[test]
    fn line_relation_rows_near_degenerate() {
        for len in [1e-3, 1e-5] {
            let s = line_relations([
                (0.4, -0.3),
                (0.4 + 0.6 * len, -0.3 + 0.8 * len),
                (-1.5, 8.7),
                (3.0, 13.0),
                (2.0, 3.0),
            ]);
            let sys = System::build(&s);
            let x0 = sys.initial.clone();
            let j = sys.jacobian(&x0, &sys.all_rows());
            assert!(j.iter().all(|v| v.is_finite()));
            let jmax = j.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            // Entries scale like k / len (k = 13 mm).
            assert!(jmax <= 2.0 * 13.0 / len, "jmax {jmax}");
            let err = fd_error(&sys, &x0, len * 1e-4);
            assert!(err <= 1e-5 * jmax, "len {len}: {err:e} vs {jmax:e}");
        }
        // Exactly zero-length line: finite rows (fixed fallback direction).
        let s = line_relations([(1.0, 1.0), (1.0, 1.0), (-1.5, 8.7), (3.0, 13.0), (2.0, 3.0)]);
        let sys = System::build(&s);
        let j = sys.jacobian(&sys.initial, &sys.all_rows());
        assert!(j.iter().all(|v| v.is_finite()));
    }

    /// PointOnCircle rows (circle: `|p − c| − r`, arc: `|p − c| − |start − c|`)
    /// against central finite differences and hand values at a generic
    /// (unsolved) configuration, including the radius column of the circle.
    #[test]
    fn point_on_circle_rows_match_finite_differences() {
        let mut s = Sketch::default();
        let c0 = s.add_point(1.0, -2.0);
        let circle = s.add_circle(c0, 7.5);
        let a0 = s.add_point(0.0, 0.0);
        let a1 = s.add_point(10.0, 0.0);
        let arc = s.add_arc_3pt(a1, a0, [5.0, 5.0]).unwrap();
        let p = s.add_point(9.3, 4.1);
        s.constraints = vec![
            Constraint::PointOnCircle { point: p, circle },
            Constraint::PointOnCircle {
                point: p,
                circle: arc,
            },
        ];
        s.validate().unwrap();
        let sys = System::build(&s);
        // Two constraint rows plus the arc's implied equal-radius row.
        assert_eq!(sys.eqs.len(), 3);
        let x0 = sys.initial.clone();
        let err = fd_error(&sys, &x0, 1e-6);
        assert!(err < 1e-7, "Jacobian vs finite differences: {err:e}");
        // Hand values: |(9.3, 4.1) − (1, −2)| − 7.5; the arc's centre is the
        // circumcentre of (0,0), (10,0), (5,5): (5, 0), radius 5.
        let want0 = (8.3f64).hypot(6.1) - 7.5;
        let want1 = (4.3f64).hypot(4.1) - 5.0;
        assert!((sys.eqs[0].eval(&x0) - want0).abs() < 1e-12);
        assert!((sys.eqs[1].eval(&x0) - want1).abs() < 1e-12);
        // Circle row depends on the radius unknown (d/dr = −1).
        let j = sys.jacobian(&x0, &sys.all_rows());
        let rcol = sys.radius_col[0].1;
        assert_eq!(j[(0, rcol)], -1.0);
    }

    /// Offset rows (sharp line joints, tangent line/arc joints, a circle)
    /// against central finite differences at a perturbed (unsolved)
    /// configuration, and the square-system property: the copy adds exactly as
    /// many rows as unknowns.
    #[test]
    fn offset_rows_match_finite_differences() {
        // Rounded slot: lines + arcs with tangent joints, offset outward.
        let mut s = Sketch::default();
        let bl = s.add_point(-15.0, -5.0);
        let br = s.add_point(15.0, -5.0);
        let tr = s.add_point(15.0, 5.0);
        let tl = s.add_point(-15.0, 5.0);
        let c1 = s.add_point(-15.0, 0.0);
        let c2 = s.add_point(15.0, 0.0);
        let bottom = s.add_line(bl, br);
        s.add_line(tr, tl);
        for (c, a, b) in [(c2, br, tr), (c1, tl, bl)] {
            let id = s.next_entity_id();
            s.entities.push(Entity::Arc {
                id,
                center: c,
                start: a,
                end: b,
            });
        }
        // A sharp-cornered triangle and a circle elsewhere.
        let t0 = s.add_point(40.0, -5.0);
        let t1 = s.add_point(60.0, -5.0);
        let t2 = s.add_point(50.0, 12.0);
        let tri = s.add_line(t0, t1);
        s.add_line(t1, t2);
        s.add_line(t2, t0);
        let cc = s.add_point(-50.0, 0.0);
        let circle = s.add_circle(cc, 4.0);
        s.offset_loop(bottom, 1.5, Some("a")).unwrap();
        s.offset_loop(tri, -1.0, Some("b")).unwrap();
        s.offset_loop(circle, 2.0, Some("c")).unwrap();
        let sys = System::build(&s);
        // Rows: slot copy 4 vertices x 2 + triangle copy 3 x 2 + circle 1, plus
        // the 2 source arcs' implied rows (copy arcs' are dropped).
        assert_eq!(sys.eqs.len(), 8 + 6 + 1 + 2);
        let mut x0 = sys.initial.clone();
        for (i, v) in x0.iter_mut().enumerate() {
            *v += 0.05 * ((i * 7 % 11) as f64 - 5.0) / 5.0;
        }
        let err = fd_error(&sys, &x0, 1e-6);
        assert!(err < 1e-6, "Jacobian vs finite differences: {err:e}");
        // At the constructed geometry every row is satisfied.
        assert!(sys.max_residual(&sys.initial) < 1e-9);
    }
}
