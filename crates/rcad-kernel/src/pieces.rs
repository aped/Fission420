//! Separating a boolean result that fell apart into one body per piece.
//!
//! A cut through the whole width of a part leaves several separate solids. The
//! opt-in `*_pieces` operations return them as one `Solid` with one closed
//! shell per piece; [`split_pieces`] turns that into one [`Body`] per piece.
//! Pieces are ordered by the lowest corner of their bounding box
//! (lexicographically x, then y, then z, compared exactly), so the order is a
//! deterministic function of the geometry. A shell enclosing negative volume
//! is an internal void, not a piece, and is refused: a body is one connected
//! solid without cavities (ADR 0003).
use crate::measure::raw_bounds;
use crate::{Body, KernelError, Result, check_mesh, tessellate, tol};
use truck_modeling::*;

/// One body per closed shell of `body`, in the documented order. A body with
/// one shell is returned unchanged (as the only piece).
pub fn split_pieces(body: &Body) -> Result<Vec<Body>> {
    let shells = body.solid.boundaries();
    if shells.len() == 1 {
        return Ok(vec![body.clone()]);
    }
    if shells.is_empty() {
        return Err(KernelError::InvalidResult("the result has no shell".into()));
    }
    let mut pieces: Vec<(Bounds3, Body)> = Vec::with_capacity(shells.len());
    for (i, sh) in shells.iter().enumerate() {
        let cond = sh.shell_condition();
        if cond != ShellCondition::Closed {
            return Err(KernelError::InvalidResult(format!(
                "piece {} of the result is a {cond:?} shell, not Closed",
                i + 1
            )));
        }
        let piece = Body {
            solid: Solid::try_new(vec![sh.clone()])
                .map_err(|e| KernelError::Internal(format!("piece {}: {e:?}", i + 1)))?,
        };
        let b = raw_bounds(&piece.solid)?;
        let diag = (0..3)
            .map(|k| (b.max[k] - b.min[k]).powi(2))
            .sum::<f64>()
            .sqrt();
        // The sign of the enclosed volume tells a piece (outward shell) from
        // a void (inward shell); any chord that keeps the mesh closed gives
        // the sign, the size only bounds the cost.
        let chord = tol::MESH_CHORD.max(1e-3 * diag);
        let mesh = tessellate(&piece, chord)?;
        let check = check_mesh(&mesh);
        if !check.watertight {
            return Err(KernelError::InvalidResult(format!(
                "piece {} of the result does not tessellate to a closed mesh",
                i + 1
            )));
        }
        check_not_self_crossing(&mesh, chord, i)?;
        if check.mesh_volume <= 0.0 {
            return Err(KernelError::InvalidResult(
                "the result would enclose an internal void (a cavity closed on all sides); \
                 enclosed internal voids are not supported (leave the cavity open, e.g. with \
                 a vent hole)"
                    .into(),
            ));
        }
        pieces.push((Bounds3(b.min), piece));
    }
    pieces.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pieces.into_iter().map(|(_, p)| p).collect())
}

/// A closed piece shell can still pass through itself (a wrong boolean that
/// kept overlapping walls). Crossings within two chords are what the
/// tessellations of surfaces meeting tangentially can show, so they do not
/// count; past the test budget nothing is claimed. `i`: piece index.
fn check_not_self_crossing(mesh: &crate::TriMesh, chord: f64, i: usize) -> Result<()> {
    match crate::self_intersection(mesh, 2.0 * chord, 4_000_000).found {
        Some((_, _, p)) => Err(KernelError::InvalidResult(format!(
            "piece {} of the result passes through itself near ({:.3}, {:.3}, {:.3}) mm",
            i + 1,
            p[0],
            p[1],
            p[2]
        ))),
        None => Ok(()),
    }
}

/// Lowest bounding-box corner, compared lexicographically with `total_cmp`.
struct Bounds3([f64; 3]);

impl Bounds3 {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (0..3)
            .map(|i| self.0[i].total_cmp(&o.0[i]))
            .find(|c| c.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Points well inside `body`, for recognising the same piece after an
/// upstream edit (the piece that contains most of an id's points keeps the
/// id). The first is the candidate with the largest clearance from the
/// surface (deep inside). With `others` (the other pieces of the same
/// split) the second is a point just under the surface of `body` as far as
/// possible from them, i.e. away from the cut that separated them, so that
/// moving the cut does not move it into a neighbour. Candidates come from
/// the largest triangles of a fine tessellation; the result is deterministic
/// for a given geometry. Up to three further points are spread over the
/// piece (farthest-point sampling among the candidates). Each point is
/// farther than three chords from the surface and inside by the
/// winding-number check; refused when none is.
pub fn piece_seeds(body: &Body, others: &[&Body]) -> Result<Vec<[f64; 3]>> {
    let b = raw_bounds(&body.solid)?;
    let diag = (0..3)
        .map(|k| (b.max[k] - b.min[k]).powi(2))
        .sum::<f64>()
        .sqrt();
    let chord = crate::validate::check_chord(diag);
    let tris = triangles(body, chord)?;
    let other_tris: Vec<[[f64; 3]; 3]> = others
        .iter()
        .map(|o| triangles(o, chord))
        .collect::<Result<Vec<_>>>()?
        .concat();
    let nearest = |p: [f64; 3], ts: &[[[f64; 3]; 3]]| {
        ts.iter()
            .map(|u| point_triangle_distance(p, u))
            .min_by(f64::total_cmp)
            .unwrap_or(f64::INFINITY)
    };
    // Largest triangles first (ties by index): their centroids are far from
    // the face boundaries.
    let mut order: Vec<(f64, usize)> = tris
        .iter()
        .enumerate()
        .map(|(i, t)| (norm(cross(sub(t[1], t[0]), sub(t[2], t[0]))), i))
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let margin = 3.0 * chord;
    // (score, point); higher score wins, first wins ties.
    let mut deep: Option<(f64, [f64; 3])> = None;
    // Every candidate clear of the surface, for spreading further seeds.
    let mut pool: Vec<[f64; 3]> = vec![];
    let mut far: Option<(f64, [f64; 3])> = None;
    let better = |best: &Option<(f64, [f64; 3])>, score: f64| best.is_none_or(|(s0, _)| score > s0);
    for &(area2, i) in order.iter().take(256) {
        if area2 <= 0.0 {
            break;
        }
        let t = tris[i];
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let inward = scale(n, -1.0 / norm(n));
        let c = scale(add(add(t[0], t[1]), t[2]), 1.0 / 3.0);
        let hit = tris
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .filter_map(|(_, u)| ray_triangle(c, inward, u))
            .filter(|&d| d > 1e-9 * diag)
            .min_by(f64::total_cmp);
        let Some(depth) = hit else { continue };
        let p = add(c, scale(inward, depth / 2.0));
        let clearance = nearest(p, &tris);
        if clearance > margin {
            pool.push(p);
            if better(&deep, clearance) {
                deep = Some((clearance, p));
            }
        }
        if !other_tris.is_empty() {
            // Just under the surface, a few margins deep.
            let q = add(c, scale(inward, (depth / 2.0).min(4.0 * margin)));
            if nearest(q, &tris) > margin {
                let away = nearest(q, &other_tris);
                if better(&far, away) {
                    far = Some((away, q));
                }
            }
        }
    }
    let mut points: Vec<[f64; 3]> = deep.into_iter().chain(far).map(|(_, p)| p).collect();
    // Up to three more, spread over the piece (farthest-point sampling), so
    // that a moved appendage carries off only a minority of its seeds.
    while points.len() < 5 && !points.is_empty() {
        let spread = |q: [f64; 3]| {
            points
                .iter()
                .map(|p| norm(sub(*p, q)))
                .fold(f64::INFINITY, f64::min)
        };
        let next = pool
            .iter()
            .map(|q| (spread(*q), *q))
            .filter(|(d, _)| *d > margin)
            .fold(None::<(f64, [f64; 3])>, |b, c| match b {
                Some(b) if b.0 >= c.0 => Some(b),
                _ => Some(c),
            });
        let Some((_, q)) = next else { break };
        points.push(q);
    }
    let inside = if points.is_empty() {
        vec![]
    } else {
        crate::point_membership(body, &points)?
    };
    let kept: Vec<[f64; 3]> = points
        .into_iter()
        .zip(inside)
        .filter(|(_, m)| *m == Some(true))
        .map(|(p, _)| p)
        .collect();
    if kept.is_empty() {
        return Err(KernelError::InvalidResult(
            "no point clearly inside this piece was found (it is thinner than the \
             tessellation tolerance allows)"
                .into(),
        ));
    }
    Ok(kept)
}

fn triangles(body: &Body, chord: f64) -> Result<Vec<[[f64; 3]; 3]>> {
    let mesh = tessellate(body, chord)?;
    Ok(mesh
        .triangles
        .iter()
        .map(|t| t.map(|i| mesh.positions[i as usize]))
        .collect())
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Distance along unit `dir` from `o` to triangle `t` (Möller–Trumbore), if hit.
fn ray_triangle(o: [f64; 3], dir: [f64; 3], t: &[[f64; 3]; 3]) -> Option<f64> {
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(dot(e2, q) * inv)
}

/// Euclidean distance from `p` to triangle `t` (closest point by region,
/// Ericson, Real-Time Collision Detection §5.1.5).
fn point_triangle_distance(p: [f64; 3], t: &[[f64; 3]; 3]) -> f64 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let q = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else {
        let bp = sub(p, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        let cp = sub(p, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        let vc = d1 * d4 - d3 * d2;
        let vb = d5 * d2 - d1 * d6;
        let va = d3 * d6 - d5 * d4;
        if d3 >= 0.0 && d4 <= d3 {
            b
        } else if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            add(a, scale(ab, d1 / (d1 - d3)))
        } else if d6 >= 0.0 && d5 <= d6 {
            c
        } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            add(a, scale(ac, d2 / (d2 - d6)))
        } else if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))))
        } else {
            let denom = 1.0 / (va + vb + vc);
            add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)))
        }
    };
    norm(sub(p, q))
}

#[cfg(test)]
mod interior_tests {
    use super::*;

    /// Box [0, w] x [0, h] x [0, t] on the XY frame.
    fn block(w: f64, h: f64, t: f64) -> Body {
        let pts = vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        crate::extrude(&crate::Profile::Polygon(pts), &crate::Frame::XY, t).unwrap()
    }

    #[test]
    fn interior_point_of_a_box_is_inside_with_clearance() {
        let b = block(40.0, 20.0, 2.0);
        let p = piece_seeds(&b, &[]).unwrap()[0];
        for k in 0..3 {
            assert!(p[k] > 0.0, "{p:?}");
        }
        assert!(p[0] < 40.0 && p[1] < 20.0 && p[2] < 2.0, "{p:?}");
        // Halfway through the thinnest direction.
        assert!((p[2] - 1.0).abs() < 1e-9, "{p:?}");
        assert_eq!(piece_seeds(&b, &[]).unwrap()[0], p);
        assert_eq!(
            piece_seeds(&b, &[]).unwrap(),
            piece_seeds(&b, &[]).unwrap(),
            "deterministic"
        );
    }

    #[test]
    fn a_piece_point_stays_away_from_its_neighbour() {
        // Two pieces of a 40 mm bar cut at x in [10, 12]: the right piece's
        // point lies near x = 40, so it is still inside the right piece after
        // the cut moves to x = 30.
        let left = block(10.0, 20.0, 10.0);
        let right = {
            let pts = vec![[12.0, 0.0], [40.0, 0.0], [40.0, 20.0], [12.0, 20.0]];
            crate::extrude(&crate::Profile::Polygon(pts), &crate::Frame::XY, 10.0).unwrap()
        };
        let p = piece_seeds(&right, &[&left]).unwrap();
        assert!(p.len() >= 2);
        assert!(p[1][0] > 39.0 && p[1][0] < 40.0, "{p:?}");
        let q = piece_seeds(&left, &[&right]).unwrap();
        assert!(q[1][0] > 0.0 && q[1][0] < 1.0, "{q:?}");
    }
}

#[cfg(test)]
mod self_intersection_tests {
    use super::*;

    fn block(x0: f64, y0: f64, z0: f64, s: f64) -> Body {
        let pts = vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]];
        crate::extrude(
            &crate::Profile::Polygon(pts),
            &crate::Frame::XY.offset(z0),
            s,
        )
        .unwrap()
    }

    fn shell(b: &Body) -> Shell {
        b.solid.boundaries()[0].clone()
    }

    /// A closed mesh that passes through itself (two interpenetrating cube
    /// meshes) is refused by the piece check; each cube alone passes.
    #[test]
    fn self_crossing_piece_mesh_is_refused() {
        let chord = tol::MESH_CHORD;
        let mesh = |b: &Body| tessellate(b, chord).unwrap();
        let (a, b) = (
            mesh(&block(0.0, 0.0, 0.0, 10.0)),
            mesh(&block(5.0, 5.0, 5.0, 10.0)),
        );
        check_not_self_crossing(&a, chord, 0).unwrap();
        check_not_self_crossing(&b, chord, 0).unwrap();
        let mut both = a.clone();
        let off = both.positions.len() as u32;
        both.positions.extend(&b.positions);
        both.normals.extend(&b.normals);
        both.triangles
            .extend(b.triangles.iter().map(|t| t.map(|v| v + off)));
        both.tri_face.extend(&b.tri_face);
        assert!(
            check_mesh(&both).watertight,
            "closed, so only the crossing check sees it"
        );
        let e = check_not_self_crossing(&both, chord, 1)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("piece 2 of the result passes through itself"),
            "{e}"
        );
        // Touching cubes (a shared face plane, no overlap) do not cross.
        let mut touch = a.clone();
        let c = mesh(&block(10.0, 0.0, 0.0, 10.0));
        let off = touch.positions.len() as u32;
        touch.positions.extend(&c.positions);
        touch.normals.extend(&c.normals);
        touch
            .triangles
            .extend(c.triangles.iter().map(|t| t.map(|v| v + off)));
        touch.tri_face.extend(&c.tri_face);
        check_not_self_crossing(&touch, chord, 0).unwrap();
        // Split results still split.
        let far = block(100.0, 0.0, 0.0, 10.0);
        let good = Body {
            solid: Solid::new_unchecked(vec![shell(&far), shell(&block(30.0, 0.0, 0.0, 10.0))]),
        };
        assert_eq!(split_pieces(&good).unwrap().len(), 2);
    }

    /// Correct curved pieces (a holed block, a cylinder) show no crossing
    /// at the chord-based tolerance.
    #[test]
    fn curved_pieces_do_not_self_intersect() {
        let holed = crate::cut_prism(
            &block(0.0, 0.0, 0.0, 20.0),
            &crate::Profile::Circle {
                center: [10.0, 10.0],
                radius: 4.0,
            },
            &crate::Frame::XY.offset(-1.0),
            22.0,
        )
        .unwrap();
        let cyl = crate::extrude(
            &crate::Profile::Circle {
                center: [0.0, 0.0],
                radius: 3.0,
            },
            &crate::Frame::XY,
            7.0,
        )
        .unwrap();
        for b in [holed, cyl] {
            let chord = tol::MESH_CHORD;
            let m = tessellate(&b, chord).unwrap();
            let r = crate::self_intersection(&m, 2.0 * chord, 4_000_000);
            assert!(r.complete && r.found.is_none(), "{r:?}");
        }
    }
}
