//! Closed-profile extraction from solved sketch geometry.

use crate::{Constraint, Entity, PointId, Sketch, SketchError, solve_tolerance};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Profile {
    /// Simple polygon, counter-clockwise, starting at the lowest (then leftmost)
    /// vertex. Not explicitly closed (last != first).
    Polygon(Vec<[f64; 2]>),
    Circle {
        center: [f64; 2],
        radius: f64,
    },
}

/// Union-find over point indices, merging `Coincident` points into one vertex.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

pub(crate) fn extract(s: &Sketch) -> Result<Profile, SketchError> {
    let residual = s.residual()?;
    let tol = solve_tolerance(s.scale());
    if residual > tol {
        return Err(SketchError::NotSolved {
            residual,
            tolerance: tol,
        });
    }

    let lines: Vec<(PointId, PointId)> = s
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Line { a, b, .. } => Some((*a, *b)),
            _ => None,
        })
        .collect();
    let circles: Vec<(PointId, f64)> = s
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Circle { center, radius, .. } => Some((*center, *radius)),
            _ => None,
        })
        .collect();

    let pidx: HashMap<PointId, usize> = s
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, i))
        .collect();
    let coord = |id: &PointId| -> [f64; 2] {
        let p = &s.points[pidx[id]];
        [p.x, p.y]
    };

    match (lines.len(), circles.len()) {
        (0, 1) => {
            let (c, r) = circles[0];
            if r <= tol {
                return Err(SketchError::DegenerateProfile(format!(
                    "circle radius {r} mm is not positive"
                )));
            }
            return Ok(Profile::Circle {
                center: coord(&c),
                radius: r,
            });
        }
        (0, 0) => {
            return Err(SketchError::UnsupportedProfile(
                "sketch has no curves".into(),
            ));
        }
        (_, 0) => {}
        _ => {
            return Err(SketchError::UnsupportedProfile(format!(
                "expected one closed loop of lines or a single circle; found {} lines and {} circles",
                lines.len(),
                circles.len()
            )));
        }
    }

    // Merge coincident points.
    let mut parent: Vec<usize> = (0..s.points.len()).collect();
    for c in &s.constraints {
        if let Constraint::Coincident { a, b } = c {
            let (ra, rb) = (find(&mut parent, pidx[a]), find(&mut parent, pidx[b]));
            parent[ra] = rb;
        }
    }
    let mut edges = Vec::with_capacity(lines.len());
    for (a, b) in &lines {
        let (ra, rb) = (find(&mut parent, pidx[a]), find(&mut parent, pidx[b]));
        if ra == rb {
            return Err(SketchError::DegenerateProfile(
                "line endpoints are coincident".into(),
            ));
        }
        edges.push((ra, rb));
    }
    if edges.len() < 3 {
        return Err(SketchError::UnsupportedProfile(format!(
            "{} line(s) cannot form a closed loop",
            edges.len()
        )));
    }
    // Every vertex must have degree exactly 2.
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ei, &(a, b)) in edges.iter().enumerate() {
        adj.entry(a).or_default().push(ei);
        adj.entry(b).or_default().push(ei);
    }
    if let Some((_, v)) = adj.iter().find(|(_, v)| v.len() != 2) {
        return Err(SketchError::UnsupportedProfile(format!(
            "lines do not form a single closed loop (a vertex joins {} lines)",
            v.len()
        )));
    }
    // Walk the cycle.
    let start = edges[0].0;
    let mut order = vec![start];
    let mut used = vec![false; edges.len()];
    let mut cur = start;
    let mut prev_edge = usize::MAX;
    while let Some(&e) = adj[&cur].iter().find(|&&e| e != prev_edge && !used[e]) {
        used[e] = true;
        prev_edge = e;
        let (a, b) = edges[e];
        cur = if a == cur { b } else { a };
        if cur == start {
            break;
        }
        order.push(cur);
    }
    if used.iter().any(|u| !u) || cur != start {
        return Err(SketchError::UnsupportedProfile(
            "lines form more than one loop".into(),
        ));
    }

    let rep_xy = |v: usize| {
        let p = &s.points[v];
        [p.x, p.y]
    };
    let mut poly: Vec<[f64; 2]> = order.iter().map(|&v| rep_xy(v)).collect();
    let k = poly.len();
    for i in 0..k {
        let (p, q) = (poly[i], poly[(i + 1) % k]);
        if (q[0] - p[0]).hypot(q[1] - p[1]) <= tol {
            return Err(SketchError::DegenerateProfile("zero-length edge".into()));
        }
    }
    if self_intersects(&poly, tol) {
        return Err(SketchError::UnsupportedProfile(
            "loop is self-intersecting".into(),
        ));
    }
    let area2: f64 = (0..k)
        .map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % k]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum();
    let perimeter: f64 = (0..k)
        .map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % k]);
            (q[0] - p[0]).hypot(q[1] - p[1])
        })
        .sum();
    if area2.abs() <= tol * perimeter {
        return Err(SketchError::DegenerateProfile(
            "loop encloses zero area".into(),
        ));
    }
    if area2 < 0.0 {
        poly.reverse();
    }
    // Start at the lowest vertex (within tol), then leftmost.
    let ymin = poly.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let start = (0..k)
        .filter(|&i| poly[i][1] <= ymin + tol)
        .min_by(|&i, &j| poly[i][0].total_cmp(&poly[j][0]))
        .unwrap_or(0);
    poly.rotate_left(start);
    Ok(Profile::Polygon(poly))
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// O(n^2) check for intersections between non-adjacent edges.
fn self_intersects(poly: &[[f64; 2]], tol: f64) -> bool {
    let k = poly.len();
    for i in 0..k {
        for j in (i + 1)..k {
            if j == i + 1 || (i == 0 && j == k - 1) {
                continue;
            }
            let (a, b) = (poly[i], poly[(i + 1) % k]);
            let (c, d) = (poly[j], poly[(j + 1) % k]);
            let la = (b[0] - a[0]).hypot(b[1] - a[1]);
            let lc = (d[0] - c[0]).hypot(d[1] - c[1]);
            // Orientation values are areas; compare against tol * length.
            let (d1, d2) = (orient(c, d, a), orient(c, d, b));
            let (d3, d4) = (orient(a, b, c), orient(a, b, d));
            let t1 = tol * lc;
            let t2 = tol * la;
            let straddle1 = (d1 > t1 && d2 < -t1) || (d1 < -t1 && d2 > t1);
            let straddle2 = (d3 > t2 && d4 < -t2) || (d3 < -t2 && d4 > t2);
            if straddle1 && straddle2 {
                return true;
            }
            // Touching / collinear overlap: a vertex lying on the other edge.
            let on = |p: [f64; 2], s0: [f64; 2], s1: [f64; 2], o: f64, t: f64| {
                o.abs() <= t
                    && p[0] >= s0[0].min(s1[0]) - tol
                    && p[0] <= s0[0].max(s1[0]) + tol
                    && p[1] >= s0[1].min(s1[1]) - tol
                    && p[1] <= s0[1].max(s1[1]) + tol
            };
            if on(a, c, d, d1, t1)
                || on(b, c, d, d2, t1)
                || on(c, a, b, d3, t2)
                || on(d, a, b, d4, t2)
            {
                return true;
            }
        }
    }
    false
}
