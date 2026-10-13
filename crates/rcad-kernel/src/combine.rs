//! Body Combine (run 6, SESSION_6 §B SO04): join, cut or intersect a target body
//! with one or more tool bodies, with an independent check of the result.
//!
//! The boolean is Truck's (`truck_shapeops::{or, and}`, the complement for a cut),
//! retried at the usual boolean tolerances and required to give one closed shell.
//! The result is then checked against the **inputs**, never against itself:
//!
//! * membership: on a deterministic grid over the bounding box of the inputs, each
//!   sample's expected state (`in target` op `in tools`) from the input bodies'
//!   tessellations (generalized winding number) must equal its state in the result.
//!   Samples closer than [`CombineCheck::skip_distance`] to any input surface are
//!   skipped (the result's surface lies on the inputs' surfaces, and a tessellation
//!   only classifies points farther than its chord reliably);
//! * volume: the result's measured volume must agree with the grid estimate of the
//!   expected region within the grid's stated resolution
//!   ([`CombineCheck::volume_tolerance`]).
//!
//! Either failing refuses the operation (`InvalidResult`); nothing is returned.
//!
//! Reusable pieces (stable names): [`boolean`] (one guarded Truck boolean with
//! tolerance retries) and [`verify_boolean_by_membership`] /
//! [`verify_combine_by_membership`] (the independent check as a report).
use crate::build::{BOOLEAN_TOLS, check_single_closed, with_exact_edges};
use crate::measure::raw_bounds;
use crate::{Body, Bounds, KernelError, Result, TriMesh, guarded, tessellate};
use serde::{Deserialize, Serialize};

/// What a combine does with the tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombineOp {
    /// Target ∪ tools.
    Join,
    /// Target − tools.
    Cut,
    /// Target ∩ tools (every tool).
    Intersect,
}

/// Alias used by other boolean features (Revolve Join/Cut tools).
pub type BoolOp = CombineOp;

/// Samples per axis of the check grid used by [`combine`].
pub const CHECK_GRID: usize = 14;

/// Evidence of the independent result check ([`verify_boolean_by_membership`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MembershipReport {
    /// Grid samples (samples_per_axis³).
    pub samples: usize,
    /// Samples compared (not near a surface, classified in every body).
    pub compared: usize,
    /// Samples whose result state differs from the expected one (0 on success).
    pub mismatches: usize,
    /// Samples skipped as closer than `skip_distance` to an input surface.
    pub skipped: usize,
    /// mm.
    pub skip_distance: f64,
    /// Grid estimate of the expected volume (skipped samples count half), mm³.
    pub grid_volume: f64,
    /// Stated resolution of `grid_volume`: half the skipped samples' cells plus one
    /// cell-diagonal-thick band over the result's surface area, mm³.
    pub volume_tolerance: f64,
    /// The result's measured volume, mm³ (None: not measurable).
    pub result_volume: Option<f64>,
    /// |result_volume − grid_volume| ≤ volume_tolerance (None: no volume).
    pub volume_ok: Option<bool>,
    /// First disagreeing sample and the state it should have (inside = true).
    pub first_mismatch: Option<([f64; 3], bool)>,
}

/// Former name of [`MembershipReport`].
pub type CombineCheck = MembershipReport;

impl MembershipReport {
    /// No sample disagrees and the volume (when measurable) is within tolerance.
    pub fn passed(&self) -> bool {
        self.mismatches == 0 && self.volume_ok != Some(false)
    }

    /// Why the check failed, if it did (text contains "independent check failed").
    pub fn failure(&self) -> Option<String> {
        if let Some((p, want)) = self.first_mismatch {
            return Some(format!(
                "independent check failed: {} of {} sample points disagree with the inputs \
                 (e.g. ({:.3}, {:.3}, {:.3}) should be {})",
                self.mismatches,
                self.compared,
                p[0],
                p[1],
                p[2],
                if want { "inside" } else { "outside" }
            ));
        }
        if self.volume_ok == Some(false) {
            return Some(format!(
                "independent check failed: result volume {:.3} mm³ vs grid estimate {:.3} ± {:.3} mm³",
                self.result_volume.unwrap_or(f64::NAN),
                self.grid_volume,
                self.volume_tolerance
            ));
        }
        None
    }
}

fn overlaps(a: &Bounds, b: &Bounds, eps: f64) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] + eps && b.min[i] <= a.max[i] + eps)
}

/// One guarded Truck boolean of `a` with `b` (`or`, `and`, `and` with the complement
/// of `b`), retried at the boolean tolerances; the result must be one closed shell
/// (several disconnected solids or an empty result are refused). Bodies whose bounds
/// do not overlap are refused (`InvalidInput`). No result check: see
/// [`verify_boolean_by_membership`].
pub fn boolean(a: &Body, b: &Body, op: CombineOp) -> Result<Body> {
    let (ab, bb) = (raw_bounds(&a.solid)?, raw_bounds(&b.solid)?);
    if !overlaps(&ab, &bb, crate::tol::KERNEL_LINEAR) {
        return Err(KernelError::InvalidInput(format!(
            "the tool does not {} the target",
            match op {
                CombineOp::Join => "touch",
                CombineOp::Cut | CombineOp::Intersect => "overlap",
            }
        )));
    }
    let what = match op {
        CombineOp::Join => "join",
        CombineOp::Cut => "cut",
        CombineOp::Intersect => "intersect",
    };
    let mut failures = Vec::new();
    for &btol in &BOOLEAN_TOLS {
        let r = guarded(what, || match op {
            CombineOp::Join => truck_shapeops::or(&a.solid, &b.solid, btol),
            CombineOp::Intersect => truck_shapeops::and(&a.solid, &b.solid, btol),
            CombineOp::Cut => {
                let mut t = b.solid.clone();
                t.not();
                truck_shapeops::and(&a.solid, &t, btol)
            }
        })?;
        let Some(solid) = r else {
            failures.push(format!("tol {btol}: Truck returned no solid"));
            continue;
        };
        let body = Body { solid };
        let shells = body.solid.boundaries().len();
        if shells > 1 {
            return Err(KernelError::InvalidResult(format!(
                "{what}: the result would be {shells} disconnected solids; one body must stay one connected solid"
            )));
        }
        if shells == 0 {
            return Err(KernelError::InvalidResult(format!(
                "{what}: the result is empty"
            )));
        }
        match check_single_closed(&body, what) {
            Ok(()) => return Ok(with_exact_edges(body)),
            Err(e) => failures.push(format!("tol {btol}: {e}")),
        }
    }
    Err(KernelError::BooleanFailed(format!(
        "{what}: {}",
        failures.join("; ")
    )))
}

/// Combine `target` with `tools` (in order) and check the result independently
/// ([`verify_combine_by_membership`] at [`CHECK_GRID`]); refused when it fails.
pub fn combine(target: &Body, tools: &[Body], op: CombineOp) -> Result<(Body, MembershipReport)> {
    if tools.is_empty() {
        return Err(KernelError::InvalidInput("combine: no tool bodies".into()));
    }
    let mut out = target.clone();
    for (i, t) in tools.iter().enumerate() {
        out = boolean(&out, t, op).map_err(|e| match e {
            KernelError::InvalidInput(m) => {
                KernelError::InvalidInput(format!("combine: tool {}: {m}", i + 1))
            }
            other => other,
        })?;
    }
    let check = verify_combine_by_membership(target, tools, &out, op, CHECK_GRID)?;
    if let Some(f) = check.failure() {
        return Err(KernelError::InvalidResult(format!("combine: {f}")));
    }
    Ok((out, check))
}

/// Independent check of `result` = `target` op `tool` against the inputs: point
/// membership on a deterministic `samples_per_axis`³ grid (cell centres, clamped to
/// 2..=64 per axis) over the inputs' union bounds, skipping samples near an input
/// surface or not classified, plus the volume comparison (module docs). Returns the
/// report (inspect [`MembershipReport::passed`]); Err when the inputs cannot be
/// tessellated closed or no sample could be decided.
pub fn verify_boolean_by_membership(
    target: &Body,
    tool: &Body,
    result: &Body,
    op: BoolOp,
    samples_per_axis: usize,
) -> Result<MembershipReport> {
    verify_combine_by_membership(
        target,
        std::slice::from_ref(tool),
        result,
        op,
        samples_per_axis,
    )
}

/// [`verify_boolean_by_membership`] for several tools applied in order (join: in
/// any tool; cut: in none; intersect: in every tool).
pub fn verify_combine_by_membership(
    target: &Body,
    tools: &[Body],
    result: &Body,
    op: BoolOp,
    samples_per_axis: usize,
) -> Result<MembershipReport> {
    let tb = raw_bounds(&target.solid)?;
    check_combine(
        target,
        tools,
        op,
        result,
        &tb,
        samples_per_axis.clamp(2, 64),
    )
}

/// Closest point on triangle (a, b, c) to p (Ericson, Real-Time Collision Detection
/// 5.1.5), returned as the squared distance.
fn dist2_tri(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let lerp =
        |x: [f64; 3], d: [f64; 3], t: f64| [x[0] + d[0] * t, x[1] + d[1] * t, x[2] + d[2] * t];
    let d2 = |q: [f64; 3]| {
        let d = sub(p, q);
        dot(d, d)
    };
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2v) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2v <= 0.0 {
        return d2(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return d2(b);
    }
    let vc = d1 * d4 - d3 * d2v;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return d2(lerp(a, ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return d2(c);
    }
    let vb = d5 * d2v - d1 * d6;
    if vb <= 0.0 && d2v >= 0.0 && d6 <= 0.0 {
        return d2(lerp(a, ac, d2v / (d2v - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let bc = sub(c, b);
        return d2(lerp(b, bc, (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    d2([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}

/// Generalized winding number of a closed triangle mesh at p (solid angles / 4π).
fn winding(m: &TriMesh, p: [f64; 3]) -> f64 {
    let mut sum = 0.0;
    for t in &m.triangles {
        let v = t.map(|i| {
            let q = m.positions[i as usize];
            [q[0] - p[0], q[1] - p[1], q[2] - p[2]]
        });
        let l = v.map(|x| (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt());
        let det = v[0][0] * (v[1][1] * v[2][2] - v[1][2] * v[2][1])
            - v[0][1] * (v[1][0] * v[2][2] - v[1][2] * v[2][0])
            + v[0][2] * (v[1][0] * v[2][1] - v[1][1] * v[2][0]);
        let d = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let den =
            l[0] * l[1] * l[2] + d(v[0], v[1]) * l[2] + d(v[1], v[2]) * l[0] + d(v[2], v[0]) * l[1];
        sum += 2.0 * det.atan2(den);
    }
    sum / (4.0 * std::f64::consts::PI)
}

/// Inside / outside / unclassifiable (winding number not near 0 or 1).
fn inside(m: &TriMesh, p: [f64; 3]) -> Option<bool> {
    let w = winding(m, p);
    if (w - 1.0).abs() < 0.1 {
        Some(true)
    } else if w.abs() < 0.1 {
        Some(false)
    } else {
        None
    }
}

fn near(m: &TriMesh, p: [f64; 3], d: f64) -> bool {
    let d2 = d * d;
    m.triangles.iter().any(|t| {
        let [a, b, c] = t.map(|i| m.positions[i as usize]);
        dist2_tri(p, a, b, c) < d2
    })
}

fn area(m: &TriMesh) -> f64 {
    m.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let x = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            0.5 * (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()
        })
        .sum()
}

fn check_combine(
    target: &Body,
    tools: &[Body],
    op: CombineOp,
    out: &Body,
    target_bounds: &Bounds,
    n: usize,
) -> Result<MembershipReport> {
    // Grid box: the inputs' union bounds, slightly enlarged so no sample sits on a
    // bounding face.
    let mut bb = *target_bounds;
    for t in tools {
        let b = raw_bounds(&t.solid)?;
        for i in 0..3 {
            bb.min[i] = bb.min[i].min(b.min[i]);
            bb.max[i] = bb.max[i].max(b.max[i]);
        }
    }
    let diag = (0..3)
        .map(|i| (bb.max[i] - bb.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    // Chord of the check meshes, as `point_membership` uses.
    let chord = (diag * 1.0e-4).clamp(1.0e-3, 0.02);
    let skip = 5.0 * chord + 1.0e-6 * diag;
    let mesh = |b: &Body| -> Result<TriMesh> {
        let m = tessellate(b, chord)?;
        if !crate::check_mesh(&m).watertight {
            return Err(KernelError::InvalidResult(
                "combine check: a tessellation is not closed".into(),
            ));
        }
        Ok(m)
    };
    let mt = mesh(target)?;
    let mtools: Vec<TriMesh> = tools.iter().map(mesh).collect::<Result<_>>()?;
    let mo = mesh(out)?;
    let step = [0, 1, 2].map(|i| (bb.max[i] - bb.min[i]) / n as f64);
    let cell = step[0] * step[1] * step[2];
    let pts: Vec<[f64; 3]> = (0..n * n * n)
        .map(|k| {
            let (i, j, l) = (k % n, (k / n) % n, k / (n * n));
            [
                bb.min[0] + (i as f64 + 0.5) * step[0],
                bb.min[1] + (j as f64 + 0.5) * step[1],
                bb.min[2] + (l as f64 + 0.5) * step[2],
            ]
        })
        .collect();
    // Review run 6 #5: a tool much smaller than the union (a small hole in a
    // large plate) can fall between grid points; also sample a grid over
    // each tool's own bounds so every tool is probed.
    let mut pts = pts;
    for t in tools {
        let tb = raw_bounds(&t.solid)?;
        let ts = [0, 1, 2].map(|i| (tb.max[i] - tb.min[i]) / n as f64);
        for k in 0..n * n * n {
            let (i, j, l) = (k % n, (k / n) % n, k / (n * n));
            pts.push([
                tb.min[0] + (i as f64 + 0.5) * ts[0],
                tb.min[1] + (j as f64 + 0.5) * ts[1],
                tb.min[2] + (l as f64 + 0.5) * ts[2],
            ]);
        }
    }
    // (expected, result) per sample; None = skipped.
    let classify = |p: [f64; 3]| -> Option<(bool, bool)> {
        if near(&mt, p, skip) || mtools.iter().any(|m| near(m, p, skip)) {
            return None;
        }
        let a = inside(&mt, p)?;
        let mut ins = Vec::with_capacity(mtools.len());
        for m in &mtools {
            ins.push(inside(m, p)?);
        }
        let want = match op {
            CombineOp::Join => a || ins.iter().any(|&x| x),
            CombineOp::Cut => a && !ins.iter().any(|&x| x),
            CombineOp::Intersect => a && ins.iter().all(|&x| x),
        };
        Some((want, inside(&mo, p)?))
    };
    let results: Vec<Option<(bool, bool)>> = guarded("combine check", || {
        let chunks = std::thread::available_parallelism()
            .map_or(1, |c| c.get())
            .clamp(1, 8);
        let per = pts.len().div_ceil(chunks);
        std::thread::scope(|sc| {
            let hs: Vec<_> = pts
                .chunks(per)
                .map(|c| sc.spawn(move || c.iter().map(|p| classify(*p)).collect::<Vec<_>>()))
                .collect();
            hs.into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    })?;
    if results.len() != pts.len() {
        return Err(KernelError::Internal("combine check worker failed".into()));
    }
    let mut c = MembershipReport {
        samples: pts.len(),
        compared: 0,
        mismatches: 0,
        skipped: 0,
        skip_distance: skip,
        grid_volume: 0.0,
        volume_tolerance: 0.0,
        result_volume: None,
        volume_ok: None,
        first_mismatch: None,
    };
    let global = n * n * n; // only the union grid estimates the volume
    let mut skipped_global = 0usize;
    for (k, (p, r)) in pts.iter().zip(&results).enumerate() {
        match r {
            None => {
                c.skipped += 1;
                if k < global {
                    skipped_global += 1;
                    c.grid_volume += 0.5 * cell;
                }
            }
            Some((want, got)) => {
                c.compared += 1;
                if *want && k < global {
                    c.grid_volume += cell;
                }
                if want != got {
                    c.mismatches += 1;
                    c.first_mismatch.get_or_insert((*p, *want));
                }
            }
        }
    }
    let h = (step[0] * step[0] + step[1] * step[1] + step[2] * step[2]).sqrt();
    c.volume_tolerance = 0.5 * skipped_global as f64 * cell + area(&mo) * h;
    if c.compared == 0 {
        return Err(KernelError::InvalidResult(
            "combine: independent check found no classifiable sample point".into(),
        ));
    }
    if let crate::VolumeResult::Value { volume, .. } = crate::measure(out)?.volume {
        c.result_volume = Some(volume);
        c.volume_ok = Some((volume - c.grid_volume).abs() <= c.volume_tolerance);
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_distance_regions() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let d = |p| dist2_tri(p, a, b, c).sqrt();
        assert!((d([0.2, 0.2, 3.0]) - 3.0).abs() < 1e-15); // face
        assert!((d([-1.0, -1.0, 0.0]) - 2f64.sqrt()).abs() < 1e-15); // vertex a
        assert!((d([0.5, -2.0, 0.0]) - 2.0).abs() < 1e-15); // edge ab
        assert!((d([1.0, 1.0, 0.0]) - 0.5f64.sqrt()).abs() < 1e-15); // edge bc
        assert!((d([3.0, 0.0, 4.0]) - 20f64.sqrt()).abs() < 1e-12); // vertex b
    }
}
