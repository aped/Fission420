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
                let (dx, dy) = (x[b] - x[a], x[b + 1] - x[a + 1]);
                let l = dx.hypot(dy);
                let (ux, uy) = if l > DIST_GRAD_FLOOR {
                    (dx / l, dy / l)
                } else {
                    (1.0, 0.0)
                };
                j[(row, b)] += ux;
                j[(row, b + 1)] += uy;
                j[(row, a)] -= ux;
                j[(row, a + 1)] -= uy;
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
            }
        }
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
            }
        }
        let pin_owner = s.constraints.len();
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
