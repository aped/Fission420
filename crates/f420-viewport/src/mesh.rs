//! Display mesh, bounds, and CPU ray picking.

use crate::camera::Ray;
use crate::math::{self, Vec3};

/// Triangle mesh for display. Indexed; `tri_group[i]` is the group id
/// (e.g. B-rep face id) of triangle `i` (`indices[3i..3i+3]`).
/// `normals` may be empty, in which case flat normals are computed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub tri_group: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    IndexCountNotMultipleOf3(usize),
    TriGroupLenMismatch { triangles: usize, groups: usize },
    NormalLenMismatch { positions: usize, normals: usize },
    IndexOutOfRange { index: u32, vertices: usize },
    NonFinitePosition(usize),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IndexCountNotMultipleOf3(n) => {
                write!(f, "index count {n} is not a multiple of 3")
            }
            Self::TriGroupLenMismatch { triangles, groups } => {
                write!(f, "{groups} triangle groups for {triangles} triangles")
            }
            Self::NormalLenMismatch { positions, normals } => {
                write!(f, "{normals} normals for {positions} positions")
            }
            Self::IndexOutOfRange { index, vertices } => {
                write!(f, "index {index} out of range for {vertices} vertices")
            }
            Self::NonFinitePosition(i) => write!(f, "position {i} is not finite"),
        }
    }
}

impl std::error::Error for MeshError {}

impl RenderMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn validate(&self) -> Result<(), MeshError> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshError::IndexCountNotMultipleOf3(self.indices.len()));
        }
        if self.tri_group.len() != self.triangle_count() {
            return Err(MeshError::TriGroupLenMismatch {
                triangles: self.triangle_count(),
                groups: self.tri_group.len(),
            });
        }
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(MeshError::NormalLenMismatch {
                positions: self.positions.len(),
                normals: self.normals.len(),
            });
        }
        if let Some(&index) = self
            .indices
            .iter()
            .find(|&&i| i as usize >= self.positions.len())
        {
            return Err(MeshError::IndexOutOfRange {
                index,
                vertices: self.positions.len(),
            });
        }
        if let Some(i) = self
            .positions
            .iter()
            .position(|p| !p.iter().all(|c| c.is_finite()))
        {
            return Err(MeshError::NonFinitePosition(i));
        }
        Ok(())
    }

    /// Triangle `t`'s corner positions. Caller must have validated the mesh.
    fn tri(&self, t: usize) -> Option<[Vec3; 3]> {
        let i = self.indices.get(3 * t..3 * t + 3)?;
        Some([
            *self.positions.get(i[0] as usize)?,
            *self.positions.get(i[1] as usize)?,
            *self.positions.get(i[2] as usize)?,
        ])
    }

    pub fn bounds(&self) -> Option<Aabb> {
        Aabb::from_points(self.positions.iter().copied())
    }

    /// Closest hit along `ray`: returns (distance, triangle index, group).
    pub fn pick(&self, ray: &Ray) -> Option<(f32, usize, u32)> {
        let mut best: Option<(f32, usize, u32)> = None;
        for t in 0..self.triangle_count() {
            let Some(tri) = self.tri(t) else { continue };
            if let Some(d) = ray_triangle(ray, tri)
                && best.is_none_or(|(bd, _, _)| d < bd)
            {
                best = Some((d, t, self.tri_group.get(t).copied().unwrap_or(u32::MAX)));
            }
        }
        best
    }
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut b = Self {
            min: first,
            max: first,
        };
        for p in it {
            for ((lo, hi), v) in b.min.iter_mut().zip(b.max.iter_mut()).zip(p) {
                *lo = lo.min(v);
                *hi = hi.max(v);
            }
        }
        Some(b)
    }

    pub fn center(&self) -> Vec3 {
        math::scale(math::add(self.min, self.max), 0.5)
    }

    /// Radius of the bounding sphere centred at [`Self::center`].
    pub fn radius(&self) -> f32 {
        0.5 * math::length(math::sub(self.max, self.min))
    }
}

/// Möller–Trumbore, two-sided. Returns hit distance along the (unit) ray.
pub fn ray_triangle(ray: &Ray, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let e1 = math::sub(b, a);
    let e2 = math::sub(c, a);
    let p = math::cross(ray.dir, e2);
    let det = math::dot(e1, p);
    // Relative degeneracy test: scale by edge magnitudes so mm and m
    // models behave the same.
    let scale = math::length(e1) * math::length(e2);
    if det.abs().partial_cmp(&(1.0e-7 * scale)) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let inv = 1.0 / det;
    let s = math::sub(ray.origin, a);
    let u = math::dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = math::cross(s, e1);
    let v = math::dot(ray.dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = math::dot(e2, q) * inv;
    (t > 0.0).then_some(t)
}

/// Axis-aligned box mesh with one group per face:
/// 0:-X 1:+X 2:-Y 3:+Y 4:-Z 5:+Z. Useful for demos and tests.
pub fn box_mesh(min: Vec3, max: Vec3) -> RenderMesh {
    let mut m = RenderMesh::default();
    // (axis, sign) per face; corners ordered CCW seen from outside.
    for (group, (axis, positive)) in [
        (0, false),
        (0, true),
        (1, false),
        (1, true),
        (2, false),
        (2, true),
    ]
    .into_iter()
    .enumerate()
    {
        let u_ax = (axis + 1) % 3;
        let v_ax = (axis + 2) % 3;
        let w = if positive { max[axis] } else { min[axis] };
        let mut normal = [0.0f32; 3];
        normal[axis] = if positive { 1.0 } else { -1.0 };
        let corner = |uu: f32, vv: f32| {
            let mut p = [0.0f32; 3];
            p[axis] = w;
            p[u_ax] = uu;
            p[v_ax] = vv;
            p
        };
        let quad = [
            corner(min[u_ax], min[v_ax]),
            corner(max[u_ax], min[v_ax]),
            corner(max[u_ax], max[v_ax]),
            corner(min[u_ax], max[v_ax]),
        ];
        let base = m.positions.len() as u32;
        m.positions.extend_from_slice(&quad);
        m.normals.extend_from_slice(&[normal; 4]);
        let order: [u32; 6] = if positive {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        };
        m.indices.extend(order.iter().map(|i| base + i));
        m.tri_group.extend_from_slice(&[group as u32; 2]);
    }
    m
}
