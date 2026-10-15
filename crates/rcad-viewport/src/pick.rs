//! Picking of faces, edges and vertices through bounding-volume hierarchies.
//!
//! Faces are hit by the ray against the display triangles (closest hit).
//! Edges (polylines) and vertices are hit when they pass within a *pixel*
//! tolerance of the ray: the tolerance is given in screen pixels and turned
//! into a world distance at each depth ([`PixelTolerance`]). It is a display
//! quantity and is independent of the model tolerances of the geometry
//! kernel. An edge or vertex hidden behind the face hit is not picked.
//! Priority, as in Fusion: a vertex within tolerance wins over an edge, an
//! edge over a face.
//!
//! The ids carried here are whatever the caller supplies (provisional ids of
//! the current tessellation); mapping them to persistent references is the
//! document's job.

use crate::camera::Ray;
use crate::math::{self, Vec3};
use crate::mesh::{MeshError, RenderMesh, ray_triangle};

/// An edge of the model as a polyline, with the caller's id.
#[derive(Clone, Debug, PartialEq)]
pub struct PickEdge {
    pub id: u32,
    pub points: Vec<Vec3>,
}

/// A vertex of the model, with the caller's id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickVertex {
    pub id: u32,
    pub pos: Vec3,
}

/// Screen-space pick tolerance: `px` pixels, converted to a world distance at
/// ray depth `t` as `px * (ortho_mm_per_px + t * rad_per_px)` (perspective:
/// `rad_per_px = 2 tan(fov_y / 2) / viewport height`; orthographic: the
/// constant term).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelTolerance {
    pub px: f32,
    pub rad_per_px: f32,
    pub ortho_mm_per_px: f32,
}

impl PixelTolerance {
    /// Perspective camera with vertical field of view `fov_y` (radians) in a
    /// viewport `height_px` pixels high.
    pub fn perspective(px: f32, fov_y: f32, height_px: f32) -> Self {
        Self {
            px,
            rad_per_px: 2.0 * (0.5 * fov_y).tan() / height_px.max(1.0),
            ortho_mm_per_px: 0.0,
        }
    }

    /// World distance of the tolerance at depth `t` along a unit ray.
    pub fn world_at(&self, t: f32) -> f32 {
        self.px * (self.ortho_mm_per_px + t.max(0.0) * self.rad_per_px)
    }
}

/// What a pick hit; `depth` is the distance along the (unit) ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Vertex {
        id: u32,
        depth: f32,
    },
    Edge {
        id: u32,
        depth: f32,
    },
    Face {
        group: u32,
        triangle: usize,
        depth: f32,
    },
}

/// Flat BVH over axis-aligned boxes (median split on the longest axis).
#[derive(Clone, Debug, Default)]
struct Bvh {
    nodes: Vec<Node>,
    /// Item indices, leaves reference contiguous ranges.
    items: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: items `start..start + count` (count > 0). Inner (count = 0):
    /// the left child is the next node, the right child is `right`.
    start: u32,
    count: u32,
    right: u32,
}

const LEAF: usize = 4;

impl Bvh {
    fn build(boxes: &[(Vec3, Vec3)]) -> Self {
        let mut bvh = Bvh {
            nodes: Vec::with_capacity(2 * boxes.len() / LEAF + 1),
            items: (0..boxes.len() as u32).collect(),
        };
        if !boxes.is_empty() {
            let centres: Vec<Vec3> = boxes
                .iter()
                .map(|(a, b)| {
                    [
                        0.5 * (a[0] + b[0]),
                        0.5 * (a[1] + b[1]),
                        0.5 * (a[2] + b[2]),
                    ]
                })
                .collect();
            let n = boxes.len();
            bvh.split(boxes, &centres, 0, n);
        }
        bvh
    }

    /// Builds the node for items[lo..hi]; returns its index.
    fn split(&mut self, boxes: &[(Vec3, Vec3)], centres: &[Vec3], lo: usize, hi: usize) -> u32 {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut cmin = [f32::INFINITY; 3];
        let mut cmax = [f32::NEG_INFINITY; 3];
        for &i in &self.items[lo..hi] {
            let (a, b) = boxes[i as usize];
            let c = centres[i as usize];
            for k in 0..3 {
                min[k] = min[k].min(a[k]);
                max[k] = max[k].max(b[k]);
                cmin[k] = cmin[k].min(c[k]);
                cmax[k] = cmax[k].max(c[k]);
            }
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            min,
            max,
            start: lo as u32,
            count: (hi - lo) as u32,
            right: 0,
        });
        let ext = [cmax[0] - cmin[0], cmax[1] - cmin[1], cmax[2] - cmin[2]];
        let axis = (0..3)
            .max_by(|&a, &b| ext[a].total_cmp(&ext[b]))
            .unwrap_or(0);
        if hi - lo <= LEAF || ext[axis] <= 0.0 {
            return id;
        }
        let mid = lo + (hi - lo) / 2;
        self.items[lo..hi].select_nth_unstable_by(mid - lo, |&a, &b| {
            centres[a as usize][axis].total_cmp(&centres[b as usize][axis])
        });
        let left = self.split(boxes, centres, lo, mid);
        let right = self.split(boxes, centres, mid, hi);
        debug_assert_eq!(left, id + 1);
        let node = &mut self.nodes[id as usize];
        node.count = 0;
        node.right = right;
        id
    }

    /// Visits the items of every leaf whose box, grown by `margin(t_far)`,
    /// the ray enters before `t_max()` (re-read during traversal so the
    /// visitor can shrink it).
    fn visit(
        &self,
        ray: &Ray,
        margin: impl Fn(f32) -> f32,
        mut t_max: impl FnMut() -> f32,
        mut f: impl FnMut(u32),
    ) {
        if self.nodes.is_empty() {
            return;
        }
        let inv = ray.dir.map(|d| 1.0 / d);
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            // The margin at the farthest point of the box bounds the margin
            // anywhere in it (the tolerance grows with depth).
            let far = (0..3)
                .map(|k| {
                    let d = (n.min[k] - ray.origin[k])
                        .abs()
                        .max((n.max[k] - ray.origin[k]).abs());
                    d * d
                })
                .sum::<f32>()
                .sqrt();
            let Some((t0, _)) = slab(ray, &inv, n.min, n.max, margin(far)) else {
                continue;
            };
            if t0 > t_max() {
                continue;
            }
            if n.count > 0 {
                for &it in &self.items[n.start as usize..(n.start + n.count) as usize] {
                    f(it);
                }
            } else {
                stack.push(n.right);
                stack.push(i + 1);
            }
        }
    }
}

/// Ray / box (grown by `m`) entry and exit parameters, if the ray meets it at
/// t >= 0.
fn slab(ray: &Ray, inv: &Vec3, min: Vec3, max: Vec3, m: f32) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    for k in 0..3 {
        let (lo, hi) = (min[k] - m, max[k] + m);
        if inv[k].is_finite() {
            let a = (lo - ray.origin[k]) * inv[k];
            let b = (hi - ray.origin[k]) * inv[k];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        } else if ray.origin[k] < lo || ray.origin[k] > hi {
            return None;
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// Closest approach of the ray and segment `a`-`b`: (ray parameter, distance).
fn ray_segment(ray: &Ray, a: Vec3, b: Vec3) -> (f32, f32) {
    let d = math::sub(b, a);
    let w = math::sub(ray.origin, a);
    let (dd, rd, rw, dw) = (
        math::dot(d, d),
        math::dot(ray.dir, d),
        math::dot(ray.dir, w),
        math::dot(d, w),
    );
    // Ray dir is unit: minimise |o + t r - (a + s d)|.
    let den = dd - rd * rd;
    let mut s = if den > 1.0e-12 * dd.max(1.0e-30) {
        ((dw - rd * rw) / den).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = (rd * s - rw).max(0.0);
    // Re-project s for the clamped t.
    if dd > 0.0 {
        s = ((math::dot(math::add(w, math::scale(ray.dir, t)), d)) / dd).clamp(0.0, 1.0);
        t = (math::dot(
            math::sub(math::add(a, math::scale(d, s)), ray.origin),
            ray.dir,
        ))
        .max(0.0);
    }
    let p = math::add(ray.origin, math::scale(ray.dir, t));
    let q = math::add(a, math::scale(d, s));
    (t, math::length(math::sub(p, q)))
}

/// Faces, edges and vertices of one display scene, ready for picking.
#[derive(Clone, Debug, Default)]
pub struct PickScene {
    tris: Vec<[Vec3; 3]>,
    tri_group: Vec<u32>,
    tri_bvh: Bvh,
    /// (a, b, edge id)
    segs: Vec<(Vec3, Vec3, u32)>,
    seg_bvh: Bvh,
    verts: Vec<PickVertex>,
    vert_bvh: Bvh,
}

impl PickScene {
    /// Builds the hierarchies. The mesh is validated first.
    pub fn new(
        mesh: &RenderMesh,
        edges: &[PickEdge],
        vertices: &[PickVertex],
    ) -> Result<Self, MeshError> {
        mesh.validate()?;
        let tris: Vec<[Vec3; 3]> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|i| i.map(|k| mesh.positions[k as usize]))
            .collect();
        let bx = |pts: &[Vec3]| {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for p in pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            (lo, hi)
        };
        let tri_bvh = Bvh::build(&tris.iter().map(|t| bx(t)).collect::<Vec<_>>());
        let segs: Vec<(Vec3, Vec3, u32)> = edges
            .iter()
            .flat_map(|e| e.points.windows(2).map(move |w| (w[0], w[1], e.id)))
            .filter(|(a, b, _)| a.iter().chain(b).all(|c| c.is_finite()))
            .collect();
        let seg_bvh = Bvh::build(&segs.iter().map(|s| bx(&[s.0, s.1])).collect::<Vec<_>>());
        let verts: Vec<PickVertex> = vertices
            .iter()
            .copied()
            .filter(|v| v.pos.iter().all(|c| c.is_finite()))
            .collect();
        let vert_bvh = Bvh::build(&verts.iter().map(|v| (v.pos, v.pos)).collect::<Vec<_>>());
        Ok(Self {
            tri_group: mesh.tri_group.clone(),
            tris,
            tri_bvh,
            segs,
            seg_bvh,
            verts,
            vert_bvh,
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Closest face hit: (depth, triangle, group).
    pub fn pick_face(&self, ray: &Ray) -> Option<(f32, usize, u32)> {
        let best = std::cell::Cell::new(None::<(f32, usize, u32)>);
        self.tri_bvh.visit(
            ray,
            |_| 0.0,
            || best.get().map_or(f32::INFINITY, |b| b.0),
            |i| {
                let i = i as usize;
                if let Some(d) = ray_triangle(ray, self.tris[i])
                    && best.get().is_none_or(|b| d < b.0)
                {
                    best.set(Some((d, i, self.tri_group[i])));
                }
            },
        );
        best.get()
    }

    /// Exactly [`RenderMesh::pick_section`] of the mesh this scene was built
    /// from (the nearest hit on the visible side of `section`, the lowest
    /// triangle index among equally near hits), through the hierarchy instead
    /// of every triangle. Boxes are grown by a small margin so rounding in the
    /// box test cannot drop a hit the plain scan finds.
    pub fn pick_face_section(
        &self,
        ray: &Ray,
        section: Option<crate::SectionPlane>,
    ) -> Option<(f32, usize, u32)> {
        let best = std::cell::Cell::new(None::<(f32, usize, u32)>);
        self.tri_bvh.visit(
            ray,
            |far| 1.0e-4 * (1.0 + far),
            || best.get().map_or(f32::INFINITY, |b| b.0),
            |i| {
                let i = i as usize;
                if let Some(d) = ray_triangle(ray, self.tris[i])
                    && best.get().is_none_or(|b| (d, i) < (b.0, b.1))
                    && section
                        .is_none_or(|s| s.keeps(math::add(ray.origin, math::scale(ray.dir, d))))
                {
                    best.set(Some((d, i, self.tri_group[i])));
                }
            },
        );
        best.get()
    }

    /// Vertex, else edge, else face under the ray (see the module docs).
    pub fn pick(&self, ray: &Ray, tol: PixelTolerance) -> Option<Pick> {
        let face = self.pick_face(ray);
        // Things behind the visible surface are hidden; allow the tolerance
        // so an edge on the silhouette of the hit face still counts.
        let limit = face.map_or(f32::INFINITY, |f| f.0 + tol.world_at(f.0));
        let mut vbest: Option<(f32, f32, u32)> = None; // (normalised distance, depth, id)
        self.vert_bvh.visit(
            ray,
            |t| tol.world_at(t),
            || limit,
            |i| {
                let v = self.verts[i as usize];
                let w = math::sub(v.pos, ray.origin);
                let t = math::dot(w, ray.dir);
                if t < 0.0 || t > limit {
                    return;
                }
                let d = math::length(math::sub(w, math::scale(ray.dir, t)));
                let r = d / tol.world_at(t).max(f32::MIN_POSITIVE);
                if r <= 1.0 && vbest.is_none_or(|b| (r, t) < (b.0, b.1)) {
                    vbest = Some((r, t, v.id));
                }
            },
        );
        if let Some((_, depth, id)) = vbest {
            return Some(Pick::Vertex { id, depth });
        }
        let mut ebest: Option<(f32, f32, u32)> = None;
        self.seg_bvh.visit(
            ray,
            |t| tol.world_at(t),
            || limit,
            |i| {
                let (a, b, id) = self.segs[i as usize];
                let (t, d) = ray_segment(ray, a, b);
                if t > limit {
                    return;
                }
                let r = d / tol.world_at(t).max(f32::MIN_POSITIVE);
                if r <= 1.0 && ebest.is_none_or(|e| (r, t) < (e.0, e.1)) {
                    ebest = Some((r, t, id));
                }
            },
        );
        if let Some((_, depth, id)) = ebest {
            return Some(Pick::Edge { id, depth });
        }
        face.map(|(depth, triangle, group)| Pick::Face {
            group,
            triangle,
            depth,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_mesh;

    fn down_at(x: f32, y: f32) -> Ray {
        Ray {
            origin: [x, y, 100.0],
            dir: [0.0, 0.0, -1.0],
        }
    }

    #[test]
    fn bvh_face_pick_matches_brute_force() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 20.0, 5.0]);
        let s = PickScene::new(&m, &[], &[]).unwrap();
        for (x, y) in [
            (1.0, 1.0),
            (9.5, 19.0),
            (5.0, 10.0),
            (-1.0, 3.0),
            (3.0, 25.0),
        ] {
            let r = down_at(x, y);
            let a = s.pick_face(&r).map(|h| (h.0, h.2));
            let b = m.pick(&r).map(|h| (h.0, h.2));
            assert_eq!(a, b, "at {x},{y}");
        }
    }

    #[test]
    fn vertex_beats_edge_beats_face_within_pixels() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let edge = PickEdge {
            id: 7,
            points: vec![[0.0, 0.0, 10.0], [10.0, 0.0, 10.0]],
        };
        let vert = PickVertex {
            id: 3,
            pos: [10.0, 0.0, 10.0],
        };
        let s = PickScene::new(&m, &[edge], &[vert]).unwrap();
        // 1 mm per pixel at depth 90, 5 px.
        let tol = PixelTolerance {
            px: 5.0,
            rad_per_px: 1.0 / 90.0,
            ortho_mm_per_px: 0.0,
        };
        assert!(matches!(
            s.pick(&down_at(5.0, 3.0), tol),
            Some(Pick::Edge { id: 7, .. })
        ));
        assert!(matches!(
            s.pick(&down_at(9.0, 1.0), tol),
            Some(Pick::Vertex { id: 3, .. })
        ));
        assert!(matches!(
            s.pick(&down_at(5.0, 6.0), tol),
            Some(Pick::Face { .. })
        ));
        // A model-tolerance-sized pixel tolerance picks only the face.
        let tight = PixelTolerance { px: 1.0e-3, ..tol };
        assert!(matches!(
            s.pick(&down_at(5.0, 3.0), tight),
            Some(Pick::Face { .. })
        ));
    }

    /// Pick budget: 1M triangles (a finely tessellated wavy sheet over
    /// 100 x 100 mm, 707 x 707 quads) plus 20k edge segments, 1000 rays.
    /// Release build:
    /// `cargo test -p rcad-viewport --release --lib pick_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "timing benchmark; run in release"]
    fn pick_bench_one_million_triangles() {
        let n = 707usize;
        let h = 100.0 / n as f32;
        let z = |i: usize, j: usize| ((i as f32) * 0.05).sin() * ((j as f32) * 0.07).cos();
        let mut m = RenderMesh::default();
        for i in 0..=n {
            for j in 0..=n {
                m.positions.push([i as f32 * h, j as f32 * h, z(i, j)]);
            }
        }
        let at = |i: usize, j: usize| (i * (n + 1) + j) as u32;
        for i in 0..n {
            for j in 0..n {
                m.indices.extend([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                m.indices.extend([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                m.tri_group.extend([(i / 50 * 15 + j / 50) as u32; 2]);
            }
        }
        let edges: Vec<PickEdge> = (0..n)
            .step_by(50)
            .map(|i| PickEdge {
                id: i as u32,
                points: (0..=n)
                    .map(|j| [i as f32 * h, j as f32 * h, z(i, j)])
                    .collect(),
            })
            .collect();
        let t = std::time::Instant::now();
        let s = PickScene::new(&m, &edges, &[]).unwrap();
        let build = t.elapsed();
        let tol = PixelTolerance::perspective(5.0, 0.8, 800.0);
        let rays: Vec<Ray> = (0..1000)
            .map(|k| {
                let (x, y) = ((k * 37 % 1000) as f32 * 0.1, (k * 91 % 1000) as f32 * 0.1);
                let o = [x - 40.0, y - 30.0, 150.0];
                let d = math::normalize(math::sub([x, y, 0.0], o)).unwrap();
                Ray { origin: o, dir: d }
            })
            .collect();
        let t = std::time::Instant::now();
        let hits = rays.iter().filter(|r| s.pick(r, tol).is_some()).count();
        let per = t.elapsed() / rays.len() as u32;
        eprintln!(
            "PICK BENCH: triangles={} segments={} build={build:?} per_pick={per:?} hits={hits}",
            s.triangle_count(),
            s.segs.len()
        );
        assert!(hits > 900);
    }

    #[test]
    fn hidden_edges_are_not_picked() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        // An edge of the bottom face, straight below the ray through the top.
        let edge = PickEdge {
            id: 1,
            points: vec![[5.0, 0.0, 0.0], [5.0, 10.0, 0.0]],
        };
        let s = PickScene::new(&m, &[edge], &[]).unwrap();
        let tol = PixelTolerance {
            px: 5.0,
            rad_per_px: 1.0 / 90.0,
            ortho_mm_per_px: 0.0,
        };
        assert!(matches!(
            s.pick(&down_at(5.0, 5.0), tol),
            Some(Pick::Face { .. })
        ));
    }
}
