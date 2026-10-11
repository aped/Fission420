//! Mesh diagnostics (closed edges, manifold vertices, orientation, degenerate
//! triangles), facet angles, and rigid print placements, against hand-built
//! meshes and analytic values.
use f420_kernel::*;

fn mesh(positions: Vec<[f64; 3]>, triangles: Vec<[u32; 3]>) -> TriMesh {
    let n = triangles.len();
    TriMesh {
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        triangles,
        tri_face: vec![0; n],
        chord_tolerance: 0.0,
    }
}

/// Right tetrahedron with legs `s` at `o`, outward winding; volume s^3/6.
fn tetra(o: [f64; 3], s: f64) -> TriMesh {
    let p = vec![
        o,
        [o[0] + s, o[1], o[2]],
        [o[0], o[1] + s, o[2]],
        [o[0], o[1], o[2] + s],
    ];
    mesh(p, vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]])
}

#[test]
fn closed_tetra_is_a_valid_solid() {
    let c = check_mesh(&tetra([0.0; 3], 6.0));
    assert!(c.is_valid_solid(), "{c:?}");
    assert!(c.closed_edges && c.orientation_consistent && c.outward);
    assert_eq!(
        (c.nonmanifold_vertices, c.degenerate_triangles, c.shells),
        (0, 0, 1)
    );
    assert!((c.mesh_volume - 36.0).abs() < 1e-12);
    assert_eq!(c.self_intersection, "not checked");
    assert!(c.problems().is_empty());
}

#[test]
fn open_flipped_inverted_and_touching_meshes_are_diagnosed() {
    // One triangle removed: three boundary edges.
    let mut open = tetra([0.0; 3], 1.0);
    open.triangles.pop();
    open.tri_face.pop();
    let c = check_mesh(&open);
    assert!(!c.is_valid_solid() && !c.closed_edges && c.boundary_edges == 3);
    assert!(c.problems().iter().any(|p| p.contains("boundary")));

    // One triangle flipped: closed edge incidence, but three misoriented edges.
    let mut flipped = tetra([0.0; 3], 1.0);
    flipped.triangles[3].swap(1, 2);
    let c = check_mesh(&flipped);
    assert!(c.closed_edges && !c.orientation_consistent && c.misoriented_edges == 3);
    assert!(!c.watertight && !c.is_valid_solid());

    // Every triangle flipped: consistent but inside out (negative volume).
    let mut inverted = tetra([0.0; 3], 1.0);
    inverted.triangles.iter_mut().for_each(|t| t.swap(1, 2));
    let c = check_mesh(&inverted);
    assert!(c.watertight && c.orientation_consistent && !c.outward);
    assert!(!c.is_valid_solid());
    assert!(c.problems().iter().any(|p| p.contains("not positive")));

    // Two tetrahedra touching at one shared vertex index: every edge is fine,
    // the vertex is not. b's apex (0,0,0) is a's vertex 0; b lies below z = 0.
    let a = tetra([0.0; 3], 1.0);
    let b = tetra([0.0, 0.0, -1.0], 1.0);
    let mut p = a.positions.clone();
    let off = p.len() as u32;
    p.extend(&b.positions);
    let mut t = a.triangles.clone();
    t.extend(
        b.triangles
            .iter()
            .map(|t| t.map(|i| if i == 3 { 0 } else { i + off })),
    );
    let c = check_mesh(&mesh(p, t));
    assert!(c.watertight && c.closed_edges, "{c:?}");
    assert_eq!(c.nonmanifold_vertices, 1, "{c:?}");
    assert_eq!(c.shells, 2, "shells are edge-connected components");
    assert!(!c.is_valid_solid());
}

#[test]
fn degenerate_and_invalid_triangles_are_counted() {
    let mut m = tetra([0.0; 3], 1.0);
    // A zero-area sliver with three distinct (collinear) vertices.
    m.positions.push([0.5, 0.0, 0.0]);
    m.normals.push([0.0, 0.0, 1.0]);
    m.triangles.push([0, 1, 4]);
    m.tri_face.push(0);
    // A repeated-index triangle.
    m.triangles.push([1, 1, 2]);
    m.tri_face.push(0);
    let c = check_mesh(&m);
    assert_eq!(
        (c.degenerate_triangles, c.invalid_triangles),
        (1, 1),
        "{c:?}"
    );
    assert!(!c.watertight && !c.is_valid_solid());
}

#[test]
fn weld_exact_merges_unshared_corners_only_when_bitwise_equal() {
    let t = tetra([0.0; 3], 2.0);
    // Unshare every corner (like an imported mesh body).
    let mut un = TriMesh::default();
    for tri in &t.triangles {
        let base = un.positions.len() as u32;
        for &i in tri {
            un.positions.push(t.positions[i as usize]);
            un.normals.push([0.0, 0.0, 1.0]);
        }
        un.triangles.push([base, base + 1, base + 2]);
        un.tri_face.push(0);
    }
    assert_eq!(check_mesh(&un).boundary_edges, 12);
    let w = weld_exact(&un);
    assert_eq!(w.positions.len(), 4);
    assert!(check_mesh(&w).is_valid_solid());
    // A corner moved by one ulp is not welded.
    un.positions[0][0] = f64::from_bits(un.positions[0][0].to_bits() + 1);
    assert!(!check_mesh(&weld_exact(&un)).closed_edges);
}

fn det(r: [[f64; 3]; 3]) -> f64 {
    r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0])
}

#[test]
fn placement_rotations_are_proper_and_reflections_refused() {
    let normals = [
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.3, -0.4, 0.866],
        [-1e-13, 0.0, 1.0],
        [0.0, 1e-13, -1.0],
    ];
    for n in normals {
        let r = RigidTransform::rotation_to_minus_z(n).unwrap();
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let d = r.rotate(n.map(|c| c / l));
        assert!(
            d[0].abs() < 1e-12 && d[1].abs() < 1e-12 && (d[2] + 1.0).abs() < 1e-12,
            "{n:?} -> {d:?}"
        );
        assert!((det(r.rotation) - 1.0).abs() < 1e-12);
    }
    let mirror = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
    assert!(RigidTransform::new(mirror, [0.0; 3]).is_err());
    let scale = [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    assert!(RigidTransform::new(scale, [0.0; 3]).is_err());
    assert!(RigidTransform::rotation_to_minus_z([0.0; 3]).is_err());
}

#[test]
fn placement_preserves_volume_distances_and_orientation() {
    let body = extrude(
        &Profile::Circle {
            center: [3.0, -2.0],
            radius: 10.0,
        },
        &Frame::XY,
        10.0,
    )
    .unwrap();
    let m = tessellate(&body, 0.02).unwrap();
    let before = check_mesh(&m);
    assert!(before.is_valid_solid(), "{before:?}");
    let place = RigidTransform::rotation_to_minus_z([0.3, -0.4, 0.866])
        .unwrap()
        .then_translate([5.0, 7.0, 11.0]);
    let p = place.apply_mesh(&m);
    let after = check_mesh(&p);
    assert!(after.is_valid_solid(), "{after:?}");
    assert!((after.mesh_volume - before.mesh_volume).abs() <= 1e-9 * before.mesh_volume);
    let dist = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let n = m.positions.len();
    for k in 0..200 {
        let (i, j) = ((k * 7919) % n, (k * 104_729 + 13) % n);
        let (d0, d1) = (
            dist(m.positions[i], m.positions[j]),
            dist(p.positions[i], p.positions[j]),
        );
        assert!((d0 - d1).abs() <= 1e-12 * d0.max(1.0), "{d0} vs {d1}");
    }
}

#[test]
fn facet_angle_matches_cylinder_segmentation() {
    let r = 10.0;
    let body = extrude(
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: r,
        },
        &Frame::XY,
        10.0,
    )
    .unwrap();
    let planar = extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]),
        &Frame::XY,
        3.0,
    )
    .unwrap();
    assert_eq!(
        max_facet_angle_deg(&tessellate(&planar, 0.02).unwrap()),
        0.0
    );
    let mut last = f64::INFINITY;
    for chord in [0.1, 0.02, 0.005] {
        let a = max_facet_angle_deg(&tessellate(&body, chord).unwrap());
        // A circle with sagitta <= chord needs a segment angle <= 2 acos(1 - chord/r);
        // adjacent facet normals differ by the segment angle.
        let bound = 2.0 * (1.0 - chord / r).acos().to_degrees();
        assert!(a > 0.0 && a <= bound + 1e-9, "chord {chord}: {a} > {bound}");
        assert!(a < last);
        last = a;
    }
}
