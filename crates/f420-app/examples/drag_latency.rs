//! Per-step cost of interactive sketch drags (solver on the UI thread):
//! `cargo run --release -p f420-app --example drag_latency`
use std::time::Instant;

use f420_sketch::{Constraint, Dimension, Sketch};

fn report(name: &str, mut v: Vec<f64>) {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    println!(
        "{name:<58} mean {mean:>7.3} ms  p50 {:>7.3}  p99 {:>7.3}  max {:>7.3}  (n={})",
        v[v.len() / 2],
        v[v.len() * 99 / 100],
        v[v.len() - 1],
        v.len()
    );
}

fn chamfer_plate() -> (Sketch, f420_sketch::PointId) {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let p: Vec<_> = [[60.0, 0.0], [60.0, 30.0], [50.0, 40.0], [0.0, 40.0]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    let ids = [o, p[0], p[1], p[2], p[3]];
    for i in 0..5 {
        s.add_line(ids[i], ids[(i + 1) % 5]);
    }
    (s, p[2])
}

fn main() {
    // Free vertex of the chamfer plate: every step reaches the target.
    let (mut s, v) = chamfer_plate();
    let mut t = vec![];
    for k in 0..2000 {
        let a = k as f64 * 0.01;
        let target = [50.0 + 5.0 * a.cos(), 40.0 + 5.0 * a.sin()];
        let t0 = Instant::now();
        let r = s.drag_point(v, target);
        t.push(t0.elapsed().as_secs_f64() * 1e3);
        assert!(r.report.is_success());
    }
    report("free vertex drag (5-line loop, 4 DOF)", t);

    // Fully dimensioned centred rectangle: every step is blocked (diagnosis path).
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c: Vec<_> = [[-40.0, -25.0], [40.0, -25.0], [40.0, 25.0], [-40.0, 25.0]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    let l: Vec<_> = (0..4).map(|i| s.add_line(c[i], c[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::SymmetricAboutPoint {
            a: c[0],
            b: c[2],
            center: o,
        },
        Constraint::HorizontalDistance {
            a: c[0],
            b: c[1],
            dim: Dimension::new("width", 80.0),
        },
        Constraint::VerticalDistance {
            a: c[1],
            b: c[2],
            dim: Dimension::new("height", 50.0),
        },
    ]);
    let mut t = vec![];
    for k in 0..500 {
        let target = [40.0 + (k % 7) as f64, 25.0 + (k % 5) as f64];
        let t0 = Instant::now();
        let _ = s.drag_point(c[2], target);
        t.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    report("blocked corner drag (dimensioned rectangle, conflict diagnosis)", t);

    // Under-constrained rectangle corner (no dimensions): resize drag.
    s.constraints.truncate(5);
    let mut t = vec![];
    for k in 0..2000 {
        let target = [40.0 + (k % 20) as f64, 25.0 + (k % 10) as f64];
        let t0 = Instant::now();
        let _ = s.drag_point(c[2], target);
        t.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    report("rectangle corner resize drag (H/V + symmetry, 2 DOF)", t);
}
