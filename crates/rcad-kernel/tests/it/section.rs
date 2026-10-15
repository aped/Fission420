//! Plane sections of bodies (`section_curves`): exact lines, arcs and circles
//! where the plane crosses planar faces, cylinders about its normal and
//! cylinders parallel to it, exact ellipses where it cuts a cylinder obliquely;
//! refusals by name otherwise. Expected curves are
//! closed-form.
use rcad_kernel::*;

fn block_with_hole() -> Body {
    let b = extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
        &Frame::XY,
        10.0,
    )
    .unwrap();
    cut_prism(
        &b,
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 4.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap()
}

#[test]
fn horizontal_section_of_a_holed_block_is_its_outline_and_hole() {
    let loops = section_curves(&block_with_hole(), &Frame::XY.offset(4.0)).unwrap();
    assert_eq!(loops.len(), 2, "{loops:?}");
    assert_eq!(loops[0].len(), 4);
    for c in &loops[0] {
        let Curve2::Line { a, b } = c else {
            panic!("{c:?}")
        };
        for p in [a, b] {
            assert!(
                [0.0, 40.0].iter().any(|x| (p[0] - x).abs() < 1e-9)
                    && [0.0, 30.0].iter().any(|y| (p[1] - y).abs() < 1e-9),
                "{p:?}"
            );
        }
    }
    for c in &loops[1] {
        let (cc, r) = match c {
            Curve2::Circle { c, r } => (*c, *r),
            Curve2::Arc { c, a, .. } => (*c, (a[0] - c[0]).hypot(a[1] - c[1])),
            other => panic!("{other:?}"),
        };
        assert!((cc[0] - 20.0).abs() < 1e-9 && (cc[1] - 15.0).abs() < 1e-9);
        assert!((r - 4.0).abs() < 1e-9);
    }
}

#[test]
fn section_of_a_cylinder_parallel_to_the_plane_is_two_lines_wide() {
    // Cylinder r 5 along -Y (circle on the XZ plane, extruded 20 mm),
    // cut by z = 3: lines at x = ±4 (√(25 − 9)) closed by the end caps.
    let cyl = extrude(
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        },
        &Frame::XZ,
        20.0,
    )
    .unwrap();
    let loops = section_curves(&cyl, &Frame::XY.offset(3.0)).unwrap();
    assert_eq!(loops.len(), 1, "{loops:?}");
    let xs: Vec<f64> = loops[0]
        .iter()
        .flat_map(|c| match c {
            Curve2::Line { a, b } => vec![a[0], b[0]],
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(xs.iter().all(|x| (x.abs() - 4.0).abs() < 1e-6), "{xs:?}");
}

#[test]
fn section_through_an_oblique_hole_is_the_exact_ellipse() {
    // Ø8 hole whose axis is tilted 30° from Z (about the x axis) through
    // the block centre (20, 15, 5); the plane z = 5 cuts its wall in an
    // ellipse with semi-axes 4 (along x) and 4 / cos 30° (along y).
    let b = extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
        &Frame::XY,
        10.0,
    )
    .unwrap();
    let t = 30f64.to_radians();
    let tilted = Frame {
        origin: [20.0, 15.0, 5.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, t.cos(), t.sin()],
        z: [0.0, -t.sin(), t.cos()],
    };
    let holed = cut_prism(
        &b,
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        },
        &tilted.offset(-15.0),
        30.0,
    )
    .unwrap();
    let loops = section_curves(&holed, &Frame::XY.offset(5.0)).unwrap();
    assert_eq!(loops.len(), 2, "{loops:?}");
    let [Curve2::Ellipse { c, major, minor }] = loops[1].as_slice() else {
        panic!("{:?}", loops[1])
    };
    assert!(
        (c[0] - 20.0).abs() < 1e-6 && (c[1] - 15.0).abs() < 1e-6,
        "{c:?}"
    );
    let a = (major[0] - c[0]).hypot(major[1] - c[1]);
    assert!((a - 4.0 / t.cos()).abs() < 1e-6, "{a}");
    assert!((minor - 4.0).abs() < 1e-6, "{minor}");
    assert!((major[0] - c[0]).abs() < 1e-6, "major axis along y");
}

#[test]
fn refusals_name_the_reason() {
    let b = block_with_hole();
    let e = section_curves(&b, &Frame::XY.offset(10.0)).unwrap_err();
    assert!(e.to_string().contains("contains a face"), "{e}");
    let e = section_curves(&b, &Frame::XY.offset(25.0)).unwrap_err();
    assert!(e.to_string().contains("does not cross"), "{e}");
    // A tilted plane through the hole wall: the hole's section is an exact
    // ellipse (semi-axes 4 / cos 0.4 and 4).
    let tilt = 0.4f64;
    let f = Frame {
        origin: [20.0, 15.0, 5.0],
        x: [tilt.cos(), 0.0, tilt.sin()],
        y: [0.0, 1.0, 0.0],
        z: [-tilt.sin(), 0.0, tilt.cos()],
    };
    let loops = section_curves(&b, &f).unwrap();
    let el = loops
        .iter()
        .flatten()
        .find_map(|c| match c {
            Curve2::Ellipse { c, major, minor } => {
                Some(((major[0] - c[0]).hypot(major[1] - c[1]), *minor))
            }
            _ => None,
        })
        .expect("ellipse");
    assert!(
        (el.0 - 4.0 / tilt.cos()).abs() < 1e-6 && (el.1 - 4.0).abs() < 1e-6,
        "{el:?}"
    );
}
