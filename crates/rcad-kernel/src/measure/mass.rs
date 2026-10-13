//! Volume, centre of mass and second moments of a solid (mass properties without
//! the density), with the same boundary-integral machinery as the exact volume.
//!
//! The ten moments `∫ m dV` for m in (1, x, y, z, xx, yy, zz, xy, yz, xz) are taken in
//! a local orthonormal frame (o; e1, e2, a) with the divergence theorem and the field
//! F = G(x, y, z) a, where dG/dz = m and G = 0 at z = 0 (so div F = m):
//! `∫ m dV = Σ_faces ∫ G (a · n) dA`.
//!
//! * Axis methods (Z, X, Y; `a` the axis): curved faces ruled along `a` have
//!   `a · n = 0` and contribute nothing; planar regions (planar faces of any
//!   orientation and curved faces lying in a plane perpendicular to `a`) are fanned
//!   into triangles over their exact boundary chords, each integrated with a cubature
//!   rule exact for cubic G (G is at most cubic), so a polygonal face is exact and a
//!   curved boundary converges like the chord sums of the volume.
//! * Revolution method (`a` the common axis): faces of revolution and circular
//!   cylinders about other axes reduce to line integrals over their exact boundary
//!   edges via Stokes (see `revolution.rs`).
//! * Fallback: the same triangle integral over the welded watertight tessellation at
//!   two chord tolerances (exact for that polyhedron; the difference between the two
//!   levels is the labelled error estimate, as for the volume).
//!
//! Three chord levels and Richardson extrapolation, as for the volume; the error
//! estimates are the differences between the two extrapolants.
use super::{Axis, EVAL_FAIL, VOLUME_CHORDS, VolumeMethod, axis_supported, eval_curve};
use crate::{Body, KernelError, Result, TriMesh, guarded};
use serde::{Deserialize, Serialize};
use truck_modeling::*;

/// Moments (1, x, y, z, xx, yy, zz, xy, yz, xz) in a [`MomFrame`].
pub(super) type Mom = [f64; 10];

/// Local frame of the moment integrals: origin `o`, right-handed orthonormal axes
/// (e1, e2, a); local coordinates (x, y, z) = ((p - o)·e1, (p - o)·e2, (p - o)·a).
#[derive(Clone, Copy, Debug)]
pub(super) struct MomFrame {
    pub o: Point3,
    pub e1: Vector3,
    pub e2: Vector3,
    pub a: Vector3,
}

impl MomFrame {
    fn local(&self, p: Point3) -> [f64; 3] {
        let d = p - self.o;
        [d.dot(self.e1), d.dot(self.e2), d.dot(self.a)]
    }

    /// G of every monomial at world point `p` (dG/dz = m, G(z = 0) = 0).
    pub(super) fn g(&self, p: Point3) -> Mom {
        let [x, y, z] = self.local(p);
        [
            z,
            x * z,
            y * z,
            0.5 * z * z,
            x * x * z,
            y * y * z,
            z * z * z / 3.0,
            x * y * z,
            0.5 * y * z * z,
            0.5 * x * z * z,
        ]
    }
}

pub(super) fn add_scaled(acc: &mut Mom, v: &Mom, s: f64) {
    for i in 0..10 {
        acc[i] += s * v[i];
    }
}

/// Flux of F = G a through the flat triangle (p0, p1, p2) (orientation from the
/// vertex order): (A_vec · a) times the mean of G, by the 7-point rule with weights
/// 1/20 (vertices), 2/15 (edge midpoints), 9/20 (centroid), exact for cubic G.
pub(super) fn tri_flux(fr: &MomFrame, p0: Point3, p1: Point3, p2: Point3, acc: &mut Mom) {
    let w = 0.5 * (p1 - p0).cross(p2 - p0).dot(fr.a);
    if w == 0.0 {
        return;
    }
    let mid = |p: Point3, q: Point3| p.midpoint(q);
    let c = Point3::from_vec((p0.to_vec() + p1.to_vec() + p2.to_vec()) / 3.0);
    for p in [p0, p1, p2] {
        add_scaled(acc, &fr.g(p), w / 20.0);
    }
    for p in [mid(p0, p1), mid(p1, p2), mid(p2, p0)] {
        add_scaled(acc, &fr.g(p), w * 2.0 / 15.0);
    }
    add_scaled(acc, &fr.g(c), w * 9.0 / 20.0);
}

/// Flux of F = G a through a planar region bounded by `face`'s wires (all in one
/// plane): fan of signed triangles from the first boundary point over every chord
/// (`n` per edge) of every wire, in wire orientation.
pub(super) fn fan_flux(face: &Face, fr: &MomFrame, n: usize) -> std::result::Result<Mom, String> {
    let mut acc = [0.0; 10];
    let mut apex = None;
    for w in face.boundaries() {
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let mut prev = eval_curve(&c, t0).ok_or(EVAL_FAIL)?;
            let o = *apex.get_or_insert(prev);
            for i in 1..=n {
                let p = eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64).ok_or(EVAL_FAIL)?;
                tri_flux(fr, o, prev, p, &mut acc);
                prev = p;
            }
        }
    }
    Ok(acc)
}

/// Moments about an axis method's axis at `n` chords per edge (faces as in
/// `volume_axis`: ruled faces contribute nothing, planar regions are fanned).
fn axis_moments(
    solid: &Solid,
    fr: &MomFrame,
    axis: Axis,
    n: usize,
) -> std::result::Result<Mom, String> {
    let (_, _, k) = axis.indices();
    let mut acc = [0.0; 10];
    for f in solid.face_iter() {
        let s = f.surface();
        let planar = match &s {
            Surface::Plane(pl) => pl.normal()[k].abs() > 1e-12,
            other => matches!(
                super::classify_by_sampling(other, axis)?,
                super::AxisClass::Perpendicular(_)
            ),
        };
        if planar {
            add_scaled(&mut acc, &fan_flux(f, fr, n)?, 1.0);
        }
    }
    Ok(acc)
}

/// Evaluate `f` at n/2, n, 2n concurrently; returns the two Richardson extrapolants
/// (finer first).
pub(super) fn richardson<F>(n: usize, f: F) -> std::result::Result<(Mom, Mom), String>
where
    F: Fn(usize) -> std::result::Result<Mom, String> + Sync,
{
    let f = &f;
    let [a, b, c] = std::thread::scope(|sc| {
        let hs = [n / 2, n, 2 * n].map(|k| sc.spawn(move || f(k)));
        hs.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("moment worker panicked".into()))
        })
    });
    let (a, b, c) = (a?, b?, c?);
    let mut r1 = [0.0; 10];
    let mut r2 = [0.0; 10];
    for i in 0..10 {
        r1[i] = (4.0 * b[i] - a[i]) / 3.0;
        r2[i] = (4.0 * c[i] - b[i]) / 3.0;
    }
    Ok((r2, r1))
}

/// Volume, centroid and central second moments of a solid (no density).
///
/// Units: mm, mm³, mm⁵; world coordinates and axes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeProperties {
    /// mm³.
    pub volume: f64,
    /// Centre of volume (= centre of mass at uniform density), world mm.
    pub centroid: [f64; 3],
    /// Second moments about the centroid, world axes: S_ij = ∫ (p − c)_i (p − c)_j dV
    /// (mm⁵). The inertia tensor at density ρ is ρ (tr(S) I − S), see
    /// [`VolumeProperties::inertia`].
    pub second_moments: [[f64; 3]; 3],
    /// How the integrals were computed: an exact boundary-integral method (same
    /// selection as the volume), [`VolumeMethod::TessellationEstimate`] or
    /// [`VolumeMethod::TriangleMesh`].
    pub method: VolumeMethod,
    /// Error estimates (mm³; mm, distance; mm⁵, largest component). For the exact
    /// methods the Richardson difference (near rounding); for the tessellation
    /// estimate the difference between two chord levels (an estimate, not a bound).
    pub volume_error: f64,
    pub centroid_error: f64,
    pub second_moments_error: f64,
}

impl VolumeProperties {
    /// Inertia tensor about the centroid, world axes, for a uniform density in
    /// mass per mm³ (e.g. g/mm³ gives g·mm²): ρ (tr(S) I − S).
    pub fn inertia(&self, density_per_mm3: f64) -> [[f64; 3]; 3] {
        let s = &self.second_moments;
        let tr = s[0][0] + s[1][1] + s[2][2];
        let mut out = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                out[i][j] = density_per_mm3 * (if i == j { tr } else { 0.0 } - s[i][j]);
            }
        }
        out
    }

    /// Whether the values come from an exact boundary integral (not a
    /// tessellation estimate or a triangle mesh).
    pub fn is_exact(&self) -> bool {
        !matches!(
            self.method,
            VolumeMethod::TessellationEstimate | VolumeMethod::TriangleMesh
        )
    }
}

/// (volume, centroid, central second moments) in world terms.
type Finished = (f64, [f64; 3], [[f64; 3]; 3]);

/// (V, centroid, central S) in world terms from local moments.
fn finish(fr: &MomFrame, m: &Mom) -> std::result::Result<Finished, String> {
    let v = m[0];
    if !(v.is_finite() && v > 0.0) {
        return Err(format!("non-positive or non-finite volume {v}"));
    }
    let cl = [m[1] / v, m[2] / v, m[3] / v];
    let sl = [[m[4], m[7], m[9]], [m[7], m[5], m[8]], [m[9], m[8], m[6]]];
    let mut sc = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            sc[i][j] = sl[i][j] - v * cl[i] * cl[j];
        }
    }
    let r = [fr.e1, fr.e2, fr.a]; // columns: local axis i in world
    let mut c = [fr.o.x, fr.o.y, fr.o.z];
    for (k, ax) in r.iter().enumerate() {
        for i in 0..3 {
            c[i] += ax[i] * cl[k];
        }
    }
    let mut sw = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut x = 0.0;
            for (k, rk) in r.iter().enumerate() {
                for (l, rl) in r.iter().enumerate() {
                    x += rk[i] * sc[k][l] * rl[j];
                }
            }
            sw[i][j] = x;
        }
    }
    if c.iter().chain(sw.iter().flatten()).any(|x| !x.is_finite()) {
        return Err("non-finite moments".into());
    }
    Ok((v, c, sw))
}

/// Properties from the best and second-best moment estimates (errors = differences).
fn properties(
    fr: &MomFrame,
    best: &Mom,
    other: &Mom,
    method: VolumeMethod,
) -> std::result::Result<VolumeProperties, String> {
    let (v, c, s) = finish(fr, best)?;
    let (v1, c1, s1) = finish(fr, other)?;
    let dc = (0..3).map(|i| (c[i] - c1[i]).powi(2)).sum::<f64>().sqrt();
    let ds = (0..3)
        .flat_map(|i| (0..3).map(move |j| (i, j)))
        .map(|(i, j)| (s[i][j] - s1[i][j]).abs())
        .fold(0.0, f64::max);
    Ok(VolumeProperties {
        volume: v,
        centroid: c,
        second_moments: s,
        method,
        volume_error: (v - v1).abs(),
        centroid_error: dc,
        second_moments_error: ds,
    })
}

fn bbox_center(solid: &Solid) -> Result<Point3> {
    let b = super::raw_bounds(solid)?;
    Ok(Point3::new(
        0.5 * (b.min[0] + b.max[0]),
        0.5 * (b.min[1] + b.max[1]),
        0.5 * (b.min[2] + b.max[2]),
    ))
}

fn unit(i: usize) -> Vector3 {
    let mut v = Vector3::new(0.0, 0.0, 0.0);
    v[i] = 1.0;
    v
}

/// Tessellation fallback: the polyhedron's moments at the two chord levels of the
/// volume fallback (coarse, fine); the fine one is reported.
fn tessellated(body: &Body, center: Point3) -> std::result::Result<VolumeProperties, String> {
    let fr = world_frame(center);
    let [coarse, fine] = super::fallback_chords(body)?;
    let level = |chord: f64| -> std::result::Result<Mom, String> {
        let m = crate::mesh::tessellate(body, chord).map_err(|e| e.to_string())?;
        let c = crate::mesh::check_mesh(&m);
        if !c.watertight {
            return Err(format!(
                "tessellation at chord {chord:.3e} mm is not watertight"
            ));
        }
        Ok(mesh_moments(&m, &fr))
    };
    let [mc, mf] = std::thread::scope(|sc| {
        let hs = [coarse, fine].map(|c| sc.spawn(move || level(c)));
        hs.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("tessellation worker panicked".into()))
        })
    });
    properties(&fr, &mf?, &mc?, VolumeMethod::TessellationEstimate)
}

fn world_frame(center: Point3) -> MomFrame {
    MomFrame {
        o: center,
        e1: unit(0),
        e2: unit(1),
        a: unit(2),
    }
}

fn mesh_moments(m: &TriMesh, fr: &MomFrame) -> Mom {
    let mut acc = [0.0; 10];
    let p = |i: u32| {
        let q = m.positions[i as usize];
        Point3::new(q[0], q[1], q[2])
    };
    for t in &m.triangles {
        tri_flux(fr, p(t[0]), p(t[1]), p(t[2]), &mut acc);
    }
    acc
}

/// Volume properties of `body`: exact by the same method the volume uses (Z, X, Y
/// axis integrals, then the surface-of-revolution integral), else the labelled
/// tessellation estimate. Err when none applies.
pub fn volume_properties(body: &Body) -> Result<VolumeProperties> {
    let solid = &body.solid;
    guarded("volume properties", || -> Result<VolumeProperties> {
        let center = bbox_center(solid)?;
        let mut reasons = Vec::new();
        let mut eval_failed = false;
        for axis in Axis::ORDER {
            if let Err(e) = axis_supported(solid, axis) {
                reasons.push(format!("{axis:?}: {e}"));
                continue;
            }
            let (u, v, k) = axis.indices();
            let fr = MomFrame {
                o: center,
                e1: unit(u),
                e2: unit(v),
                a: unit(k),
            };
            match richardson(VOLUME_CHORDS, |n| axis_moments(solid, &fr, axis, n))
                .and_then(|(b, o)| properties(&fr, &b, &o, axis.method()))
            {
                Ok(p) => return Ok(p),
                Err(e) => {
                    reasons.push(format!("{axis:?}: {e}"));
                    eval_failed = true;
                    break;
                }
            }
        }
        if !eval_failed {
            match super::revolution::revolution_moments(solid, VOLUME_CHORDS, center).and_then(
                |(fr, b, o)| properties(&fr, &b, &o, VolumeMethod::BrepBoundaryIntegralRevolution),
            ) {
                Ok(p) => return Ok(p),
                Err(e) => reasons.push(format!("revolution: {e}")),
            }
        }
        tessellated(body, center).map_err(|e| {
            reasons.push(format!("tessellation fallback: {e}"));
            KernelError::InvalidResult(format!(
                "volume properties unavailable: {}",
                reasons.join("; ")
            ))
        })
    })?
}

/// Volume properties of a closed, consistently oriented triangle mesh (a mesh body):
/// exact for the mesh ([`VolumeMethod::TriangleMesh`], error estimates zero). The
/// caller is responsible for the mesh being closed; a negative or zero volume is
/// refused.
pub fn mesh_volume_properties(m: &TriMesh) -> Result<VolumeProperties> {
    if m.positions.is_empty() || m.triangles.is_empty() {
        return Err(KernelError::InvalidInput("empty mesh".into()));
    }
    if m.triangles
        .iter()
        .flatten()
        .any(|&i| i as usize >= m.positions.len())
    {
        return Err(KernelError::InvalidInput(
            "triangle index out of range".into(),
        ));
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &m.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let fr = world_frame(Point3::new(
        0.5 * (lo[0] + hi[0]),
        0.5 * (lo[1] + hi[1]),
        0.5 * (lo[2] + hi[2]),
    ));
    let mm = mesh_moments(m, &fr);
    properties(&fr, &mm, &mm, VolumeMethod::TriangleMesh).map_err(KernelError::InvalidResult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_rule_is_exact_on_a_tetrahedron() {
        // Unit tetrahedron (outward triangles; the slanted face x + y + z = 1 makes G
        // a full cubic on a triangle). ∫ x^a y^b z^c dV = a! b! c! / (a + b + c + 3)!.
        let m = TriMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            normals: vec![],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            tri_face: vec![],
            chord_tolerance: 0.0,
        };
        // Frame off the origin and rotated about Z, so every monomial is exercised.
        let (s, c) = 0.3f64.sin_cos();
        let fr = MomFrame {
            o: Point3::new(0.0, 0.0, 0.0),
            e1: Vector3::new(c, s, 0.0),
            e2: Vector3::new(-s, c, 0.0),
            a: unit(2),
        };
        let got = mesh_moments(&m, &fr);
        let (v, cen, sc) = finish(&fr, &got).unwrap();
        let fact = |n: u32| (1..=n).map(f64::from).product::<f64>();
        let mono = |a: u32, b: u32, cc: u32| fact(a) * fact(b) * fact(cc) / fact(a + b + cc + 3);
        assert!((v - 1.0 / 6.0).abs() < 1e-15);
        for i in 0..3 {
            assert!((cen[i] - 0.25).abs() < 1e-14, "{cen:?}");
        }
        // Central second moments: ∫x² − V c², ∫xy − V c c.
        let xx = mono(2, 0, 0) - v * 0.0625;
        let xy = mono(1, 1, 0) - v * 0.0625;
        for (i, row) in sc.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                let want = if i == j { xx } else { xy };
                assert!((x - want).abs() < 1e-15, "S{i}{j} {x} vs {want}");
            }
        }
    }
}
