//! Sharp B-rep edges as exact 3-D curves, for drawings (run 6, docs/DRAWINGS.md).
//!
//! Every edge of the body is returned once as a straight segment, a circle or
//! circular arc (recognised from samples lying on one circle within
//! [`EDGE_FIT_TOL`]; the rational quadratic arcs the kernel builds and the
//! exactified boolean edges are exact circles), or, for any other curve, a
//! polyline of [`POLY_SAMPLES`] + 1 points (stated as sampled). Smooth edges
//! (seam edges of a closed face, both sides on the same face, and edges where
//! the two faces' outward normals agree within [`SMOOTH_COS`] at the start,
//! middle and end: tangent edges, split cylinders) are flagged by
//! [`drawing_edges`] with their surface normals and omitted by
//! [`sharp_edges`].
use crate::{Body, Result, guarded};
use serde::{Deserialize, Serialize};
use truck_modeling::*;
use truck_topology::compress::CompressedFace;

/// Distance (mm) within which samples must lie on the fitted line/circle.
pub const EDGE_FIT_TOL: f64 = 1.0e-6;
/// Samples used to recognise a curve (and the polyline resolution otherwise).
pub const POLY_SAMPLES: usize = 32;
/// Two faces meeting with outward normals whose cosine exceeds this are smooth.
pub const SMOOTH_COS: f64 = 1.0 - 1.0e-9;

/// One sharp edge, exact where recognised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeCurve {
    Line {
        a: [f64; 3],
        b: [f64; 3],
    },
    /// Arc about `center` in the plane with unit `normal`, starting at
    /// `start` and turning `sweep` rad counter-clockwise about `normal`
    /// (`sweep` = 2π: a full circle).
    Arc {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
        start: [f64; 3],
        sweep: f64,
    },
    /// Any other curve, sampled (not exact).
    Polyline(Vec<[f64; 3]>),
}

impl EdgeCurve {
    /// Point at fraction `s` in [0, 1] along the curve's parameter.
    pub fn point(&self, s: f64) -> [f64; 3] {
        match self {
            EdgeCurve::Line { a, b } => std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s),
            EdgeCurve::Arc {
                center,
                normal,
                start,
                sweep,
                ..
            } => {
                let c = v(*center);
                let n = v(*normal);
                let x = v(*start) - c;
                let y = n.cross(x);
                let t = sweep * s;
                let p = c + x * t.cos() + y * t.sin();
                [p.x, p.y, p.z]
            }
            EdgeCurve::Polyline(p) => {
                if p.len() < 2 {
                    return p.first().copied().unwrap_or([0.0; 3]);
                }
                let f = s.clamp(0.0, 1.0) * (p.len() - 1) as f64;
                let i = (f.floor() as usize).min(p.len() - 2);
                let t = f - i as f64;
                std::array::from_fn(|k| p[i][k] + (p[i + 1][k] - p[i][k]) * t)
            }
        }
    }

    /// Length (exact for lines and arcs).
    pub fn length(&self) -> f64 {
        match self {
            EdgeCurve::Line { a, b } => (v(*b) - v(*a)).magnitude(),
            EdgeCurve::Arc { radius, sweep, .. } => radius * sweep.abs(),
            EdgeCurve::Polyline(p) => p.windows(2).map(|w| (v(w[1]) - v(w[0])).magnitude()).sum(),
        }
    }
}

fn v(p: [f64; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}

fn a3(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// Circumcentre of three points in 3-D (None when collinear).
fn circumcenter(a: Point3, b: Point3, c: Point3) -> Option<(Point3, Vector3)> {
    let (u, w) = (a - c, b - c);
    let n = u.cross(w);
    let n2 = n.magnitude2();
    if n2 < 1e-24 {
        return None;
    }
    let k = (w * u.magnitude2() - u * w.magnitude2()).cross(n) / (2.0 * n2);
    Some((c + k, n / n2.sqrt()))
}

fn outward_normal_at(f: &CompressedFace<Surface>, p: Point3) -> Option<Vector3> {
    let (u, w) = f.surface.search_parameter(p, None, 100)?;
    let n = f.surface.normal(u, w);
    Some(if f.orientation { n } else { -n })
}

/// Classify one curve from its samples.
fn classify(pts: &[Point3], straight: bool) -> EdgeCurve {
    let n = pts.len() - 1;
    let (a, b) = (pts[0], pts[n]);
    let scale = pts
        .iter()
        .map(|p| p.to_vec().magnitude())
        .fold(1.0, f64::max);
    let tol = EDGE_FIT_TOL.max(1e-9 * scale);
    let chord = b - a;
    let collinear = chord.magnitude() > tol
        && pts.iter().all(|p| {
            let d = *p - a;
            (d - chord * (d.dot(chord) / chord.magnitude2())).magnitude() <= tol
        });
    if straight || collinear {
        return EdgeCurve::Line { a: a3(a), b: a3(b) };
    }
    let closed = (b - a).magnitude() <= tol;
    let (p, q, r) = if closed {
        (pts[0], pts[n / 3], pts[2 * n / 3])
    } else {
        (a, pts[n / 2], b)
    };
    if let Some((c, nrm)) = circumcenter(p, q, r) {
        let rad = (p - c).magnitude();
        let on = pts
            .iter()
            .all(|s| ((*s - c).magnitude() - rad).abs() <= tol && (*s - c).dot(nrm).abs() <= tol);
        if on && rad > tol {
            // Orient the normal so the curve turns counter-clockwise about it.
            let x = a - c;
            let m = pts[1] - c;
            let nrm = if x.cross(m).dot(nrm) >= 0.0 {
                nrm
            } else {
                -nrm
            };
            let sweep = if closed {
                std::f64::consts::TAU
            } else {
                let y = nrm.cross(x);
                let e = b - c;
                let ang = e.dot(y).atan2(e.dot(x));
                if ang <= 0.0 {
                    ang + std::f64::consts::TAU
                } else {
                    ang
                }
            };
            return EdgeCurve::Arc {
                center: a3(c),
                normal: [nrm.x, nrm.y, nrm.z],
                radius: rad,
                start: a3(a),
                sweep,
            };
        }
    }
    EdgeCurve::Polyline(pts.iter().map(|p| a3(*p)).collect())
}

/// One B-rep edge for drawings: its curve, whether it is smooth (a seam of a
/// closed face, or two faces meeting tangentially), and for a smooth edge the
/// shared outward surface normal at parameter fractions 0, ½ and 1 (a smooth
/// edge is drawn only where it is a silhouette: normal ⊥ view direction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingEdge {
    pub curve: EdgeCurve,
    pub smooth: bool,
    pub normals: Option<[[f64; 3]; 3]>,
}

/// Every edge of `body` once (see [`DrawingEdge`]), in compressed edge order.
pub fn drawing_edges(body: &Body) -> Result<Vec<DrawingEdge>> {
    let cs = guarded("compress body", || body.solid.compress())?;
    guarded("drawing edges", || {
        let mut out = Vec::new();
        for sh in &cs.boundaries {
            let mut faces_of: Vec<Vec<usize>> = vec![Vec::new(); sh.edges.len()];
            let mut seam = vec![false; sh.edges.len()];
            for (fi, f) in sh.faces.iter().enumerate() {
                for e in f.boundaries.iter().flatten() {
                    if let Some(v) = faces_of.get_mut(e.index) {
                        if v.contains(&fi) {
                            seam[e.index] = true;
                        } else {
                            v.push(fi);
                        }
                    }
                }
            }
            for (ei, e) in sh.edges.iter().enumerate() {
                let c = &e.curve;
                let (t0, t1) = c.range_tuple();
                let pts: Vec<Point3> = (0..=POLY_SAMPLES)
                    .map(|i| c.subs(t0 + (t1 - t0) * i as f64 / POLY_SAMPLES as f64))
                    .collect();
                let curve = classify(&pts, matches!(c, Curve::Line(_)));
                let at = [pts[0], pts[POLY_SAMPLES / 2], pts[POLY_SAMPLES]];
                let normals_of = |f: usize| -> Option<[[f64; 3]; 3]> {
                    let face = sh.faces.get(f)?;
                    let mut n = [[0.0; 3]; 3];
                    for (k, p) in at.iter().enumerate() {
                        let v = outward_normal_at(face, *p)?;
                        n[k] = [v.x, v.y, v.z];
                    }
                    Some(n)
                };
                let (smooth, normals) = if seam[ei] {
                    (true, faces_of[ei].first().and_then(|f| normals_of(*f)))
                } else if let [f0, f1] = faces_of[ei][..] {
                    match (normals_of(f0), normals_of(f1)) {
                        (Some(a), Some(b))
                            if (0..3).all(|k| {
                                a[k][0] * b[k][0] + a[k][1] * b[k][1] + a[k][2] * b[k][2]
                                    > SMOOTH_COS
                            }) =>
                        {
                            (true, Some(a))
                        }
                        _ => (false, None),
                    }
                } else {
                    (false, None)
                };
                out.push(DrawingEdge {
                    curve,
                    smooth,
                    normals,
                });
            }
        }
        out
    })
}

/// Sharp edges of `body` (see the module docs), in the body's compressed
/// edge order.
pub fn sharp_edges(body: &Body) -> Result<Vec<EdgeCurve>> {
    Ok(drawing_edges(body)?
        .into_iter()
        .filter(|e| !e.smooth)
        .map(|e| e.curve)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, Profile, cut_prism, extrude};

    #[test]
    fn holed_block_edges() {
        let b = extrude(
            &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
            &Frame::XY,
            10.0,
        )
        .unwrap();
        let b = cut_prism(
            &b,
            &Profile::Circle {
                center: [20.0, 15.0],
                radius: 4.0,
            },
            &Frame::XY.offset(-1.0),
            12.0,
        )
        .unwrap();
        let e = sharp_edges(&b).unwrap();
        let lines = e
            .iter()
            .filter(|c| matches!(c, EdgeCurve::Line { .. }))
            .count();
        assert_eq!(lines, 12, "{e:?}");
        // The hole's rims: arcs of radius 4 about the axis, 2π in total per rim.
        let mut sweep_top = 0.0;
        let mut sweep_bottom = 0.0;
        for c in &e {
            if let EdgeCurve::Arc {
                center,
                radius,
                sweep,
                normal,
                ..
            } = c
            {
                assert!((radius - 4.0).abs() < 1e-9);
                assert!((center[0] - 20.0).abs() < 1e-9 && (center[1] - 15.0).abs() < 1e-9);
                assert!(normal[2].abs() > 1.0 - 1e-9);
                if center[2] > 5.0 {
                    sweep_top += sweep;
                } else {
                    sweep_bottom += sweep;
                }
            }
        }
        assert!(
            (sweep_top - std::f64::consts::TAU).abs() < 1e-9,
            "{sweep_top}"
        );
        assert!((sweep_bottom - std::f64::consts::TAU).abs() < 1e-9);
        assert!(!e.iter().any(|c| matches!(c, EdgeCurve::Polyline(_))));
    }
}
