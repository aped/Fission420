use truck_meshalgo::prelude::*;
use truck_modeling::*;
use truck_topology::shell::ShellCondition;

#[test]
fn punched_cube() {
    let v = builder::vertex(Point3::origin());
    let e = builder::tsweep(&v, Vector3::unit_x());
    let f = builder::tsweep(&e, Vector3::unit_y());
    let cube = builder::tsweep(&f, Vector3::unit_z());

    let v = builder::vertex(Point3::new(0.5, 0.25, -0.5));
    let w = builder::rsweep(&v, Point3::new(0.5, 0.5, 0.0), Vector3::unit_z(), Rad(7.0));
    let f = builder::try_attach_plane(&[w]).unwrap();
    let mut cylinder = builder::tsweep(&f, Vector3::unit_z() * 2.0);
    cylinder.not();
    let and = crate::and(&cube, &cylinder, 0.05).unwrap();

    let poly = and.triangulation(0.01).to_polygon();
    let file = std::fs::File::create("punched-cube.obj").unwrap();
    obj::write(&poly, file).unwrap();
}

// R-CAD fork: the two tests below are from ricosjp/truck PR #134.

fn unit_cube() -> Solid {
    let v = builder::vertex(Point3::origin());
    let e = builder::tsweep(&v, Vector3::unit_x());
    let f = builder::tsweep(&e, Vector3::unit_y());
    builder::tsweep(&f, Vector3::unit_z())
}

/// A vertical cylinder with axis through `(cx, cy)`, radius `r`, spanning `z0 .. z0 + h`.
fn cylinder(cx: f64, cy: f64, r: f64, z0: f64, h: f64) -> Solid {
    let v = builder::vertex(Point3::new(cx + r, cy, z0));
    let w = builder::rsweep(&v, Point3::new(cx, cy, z0), Vector3::unit_z(), Rad(7.0));
    let f = builder::try_attach_plane(&[w]).unwrap();
    builder::tsweep(&f, Vector3::unit_z() * h)
}

fn volume(solid: &Solid) -> f64 {
    // R-CAD fork: 0.4's tessellation does not weld every seam, so closedness is asserted on the
    // topology, not on the mesh as in the upstream PR.
    for shell in solid.boundaries() {
        assert_eq!(shell.shell_condition(), ShellCondition::Closed);
    }
    let mut poly = solid.triangulation(0.01).to_polygon();
    poly.put_together_same_attrs(TOLERANCE)
        .remove_degenerate_faces()
        .remove_unused_attrs();
    let ps = poly.positions();
    poly.faces()
        .triangle_iter()
        .map(|t| {
            let [a, b, c] = [ps[t[0].pos], ps[t[1].pos], ps[t[2].pos]];
            a.to_vec().dot(b.to_vec().cross(c.to_vec())) / 6.0
        })
        .sum()
}

/// Difference with a tool that does not touch the target returns the target unchanged.
#[test]
fn difference_with_disjoint_tool() {
    let cube = unit_cube();
    let mut tool = cylinder(5.0, 5.0, 0.25, -0.5, 2.0);
    tool.not();
    let res = crate::and(&cube, &tool, 0.05).unwrap();
    assert_eq!(res.boundaries().len(), 1);
    assert_eq!(res.boundaries()[0].len(), 6);
    assert_near!(volume(&res), 1.0);
}

/// Difference with a tool lying inside an existing hole (touching no material) returns the target unchanged.
#[test]
fn difference_with_tool_inside_hole() {
    let mut hole = cylinder(0.5, 0.5, 0.25, -0.5, 2.0);
    hole.not();
    let punched = crate::and(&unit_cube(), &hole, 0.05).unwrap();
    let before = volume(&punched);
    let mut tool = cylinder(0.5, 0.5, 0.1, -0.5, 2.0);
    tool.not();
    let res = crate::and(&punched, &tool, 0.05).unwrap();
    assert_eq!(res.boundaries().len(), 1);
    assert_eq!(res.boundaries()[0].len(), punched.boundaries()[0].len());
    assert!((volume(&res) - before).abs() < 1.0e-6);
}

/// R-CAD fork: a ring of revolution cut from a coaxial cylinder (a groove). The tool's faces are surfaces
/// of revolution (v period 2π); a boundary crossing the seam used to jump a full period in parameter
/// space, so face division failed and `and` returned `None` for every seam angle.
#[test]
fn groove_cut_by_a_revolved_ring() {
    use std::f64::consts::PI;
    for seam in [0.0, PI / 4.0, PI / 2.0, PI] {
        let cyl = cylinder(0.0, 0.0, 10.0, 0.0, 30.0);
        let v: Vec<Vertex> = [(9.0, 10.0), (11.0, 10.0), (11.0, 12.0), (9.0, 12.0)]
            .iter()
            .map(|&(x, z)| builder::vertex(Point3::new(x, 0.0, z)))
            .collect();
        let w: Wire = (0..4).map(|i| builder::line(&v[i], &v[(i + 1) % 4])).collect();
        let face = builder::try_attach_plane(&[w]).unwrap();
        let face = builder::rotated(&face, Point3::origin(), Vector3::unit_z(), Rad(seam));
        let mut ring = builder::rsweep(&face, Point3::origin(), Vector3::unit_z(), Rad(2.0 * PI));
        if volume(&ring) < 0.0 {
            ring.not();
        }
        ring.not();
        let res = crate::and(&cyl, &ring, 0.05).unwrap_or_else(|| panic!("seam {seam}: None"));
        assert_eq!(res.boundaries().len(), 1);
        // Tessellated volume (chordal error ~3e-4 at this tolerance); exact: 3000π - 38π.
        let want = 2962.0 * PI;
        assert!(((volume(&res) - want) / want).abs() < 1e-3, "seam {seam}: {}", volume(&res));
    }
}

/// R-CAD fork: a ring boss joined through a block's top face cuts two concentric circles into that
/// face. Each inner loop must become a hole of the smallest loop containing it; taking the first
/// containing loop (the face's outer boundary) gave overlapping pieces and an open shell (panic in
/// `Solid::new`).
#[test]
fn ring_boss_through_a_face_nests_loops() {
    use std::f64::consts::PI;
    let v: Vec<Vertex> = [(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0)]
        .iter()
        .map(|&(x, y)| builder::vertex(Point3::new(x, y, 0.0)))
        .collect();
    let w: Wire = (0..4).map(|i| builder::line(&v[i], &v[(i + 1) % 4])).collect();
    let block = builder::tsweep(&builder::try_attach_plane(&[w]).unwrap(), Vector3::unit_z() * 30.0);
    let v: Vec<Vertex> = [(8.0, 28.0), (12.0, 28.0), (12.0, 34.0), (8.0, 34.0)]
        .iter()
        .map(|&(x, z)| builder::vertex(Point3::new(x, 0.0, z)))
        .collect();
    let w: Wire = (0..4).map(|i| builder::line(&v[i], &v[(i + 1) % 4])).collect();
    let face = builder::try_attach_plane(&[w]).unwrap();
    let mut ring = builder::rsweep(&face, Point3::origin(), Vector3::unit_z(), Rad(2.0 * PI));
    if volume(&ring) < 0.0 {
        ring.not();
    }
    let res = crate::or(&block, &ring, 0.05).expect("union");
    assert_eq!(res.boundaries().len(), 1);
    let want = 48000.0 + 320.0 * PI;
    assert!(((volume(&res) - want) / want).abs() < 1e-3, "{}", volume(&res));
}
