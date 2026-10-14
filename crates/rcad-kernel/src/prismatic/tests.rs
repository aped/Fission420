//! Prismatic slab booleans. Expected volumes are analytic; every result is also
//! checked against target and tool by the independent result gate
//! (`verify_prism_boolean`) and must export to STEP.
use super::*;
use crate::{
    Prism, PrismOp, VolumeResult, cut_prism, export_step, extrude, measure, planar_faces_on,
    verify_prism_boolean,
};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

/// 40 x 30 x 10 block on XY.
fn block() -> Body {
    extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap()
}

/// Pocket of `depth` from the top (z = 10) down, built here from z = 10 - depth
/// up to exactly z = 10 (the tool's top cap coplanar with the block's top).
fn pocket(t: &Body, p: &Profile, depth: f64) -> Result<Body> {
    prism_boolean(t, p, &Frame::XY.offset(10.0 - depth), depth, SlabOp::Cut)
}

fn checked(t: &Body, r: &Body, p: &Profile, start: &Frame, len: f64, op: PrismOp) {
    let m = measure(r).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    let prism = Prism {
        profile: p,
        holes: &[],
        start,
        length: len,
    };
    verify_prism_boolean(t, r, op, &prism, (0.0, len)).unwrap();
    export_step(r, "test").unwrap();
}

#[test]
fn overlapping_pockets_with_a_shared_floor_and_walls() {
    let a = pocket(&block(), &rect(5.0, 5.0, 20.0, 20.0), 4.0).unwrap();
    assert!((volume(&a) - (12000.0 - 900.0)).abs() < 1e-6);
    // Same depth, walls y = 5 and y = 20 continue those of the first pocket and
    // the floors coincide over [15, 20] x [5, 20].
    let b_prof = rect(15.0, 5.0, 30.0, 20.0);
    let start = Frame::XY.offset(6.0);
    let b = prism_boolean(&a, &b_prof, &start, 4.0, SlabOp::Cut).unwrap();
    assert!((volume(&b) - (12000.0 - 25.0 * 15.0 * 4.0)).abs() < 1e-6);
    checked(&a, &b, &b_prof, &start, 4.0, PrismOp::Cut);
    // One face per wall plane and one floor: references stay unambiguous.
    let wall_y5 = Frame {
        origin: [0.0, 5.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, -1.0],
        z: [0.0, 1.0, 0.0],
    };
    assert_eq!(planar_faces_on(&b, &wall_y5), 1);
    let floor = Frame::XY.offset(6.0);
    assert_eq!(planar_faces_on(&b, &floor), 1);
    // 6 block faces (top now has a hole) + floor + 4 walls.
    assert_eq!(measure(&b).unwrap().face_count, 11);
}

#[test]
fn pocket_flush_with_outer_walls_and_a_nested_deeper_pocket() {
    // Flush with x = 0, y = 0 and y = 30: a step along the whole -X edge.
    let s = pocket(&block(), &rect(0.0, 0.0, 10.0, 30.0), 3.0).unwrap();
    assert!((volume(&s) - (12000.0 - 900.0)).abs() < 1e-6);
    checked(
        &block(),
        &s,
        &rect(0.0, 0.0, 10.0, 30.0),
        &Frame::XY.offset(7.0),
        3.0,
        PrismOp::Cut,
    );
    // A deeper pocket whose wall x = 10 continues the step's riser.
    let p = rect(4.0, 10.0, 10.0, 20.0);
    let start = Frame::XY.offset(2.0);
    let d = prism_boolean(&s, &p, &start, 8.0, SlabOp::Cut).unwrap();
    // Removed below the step floor (z 2..7): 6 x 10 x 5.
    assert!((volume(&d) - (11100.0 - 300.0)).abs() < 1e-6);
    checked(&s, &d, &p, &start, 8.0, PrismOp::Cut);
}

#[test]
fn join_flush_with_an_edge_and_coplanar_on_the_top() {
    // A boss standing exactly on the top face (no overlap into the support)
    // and flush with the +X wall: Truck has no answer for either contact.
    let p = rect(30.0, 10.0, 40.0, 20.0);
    let start = Frame::XY.offset(10.0);
    let j = prism_boolean(&block(), &p, &start, 5.0, SlabOp::Join).unwrap();
    assert!((volume(&j) - (12000.0 + 500.0)).abs() < 1e-6);
    checked(&block(), &j, &p, &start, 5.0, PrismOp::Join);
    let wall_x40 = Frame {
        origin: [40.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    assert_eq!(planar_faces_on(&j, &wall_x40), 1, "+X wall is one face");
}

#[test]
fn refusals() {
    // Cutting the block in two.
    let e = pocket(&block(), &rect(18.0, -1.0, 22.0, 31.0), 10.0).unwrap_err();
    assert!(e.to_string().contains("disconnected"), "{e}");
    // Two blocks meeting along an edge only.
    let e = prism_boolean(
        &block(),
        &rect(40.0, 30.0, 50.0, 40.0),
        &Frame::XY,
        10.0,
        SlabOp::Join,
    )
    .unwrap_err();
    assert!(
        e.to_string().contains("manifold") || e.to_string().contains("disconnected"),
        "{e}"
    );
    // Removes nothing.
    let e = pocket(&block(), &rect(50.0, 0.0, 60.0, 10.0), 3.0).unwrap_err();
    assert!(e.to_string().contains("removes no material"), "{e}");
    // A cylindrical hole parallel to the axis and an arc profile are
    // prismatic (ADR 0007); a hole across the axis is not.
    let holed = cut_prism(
        &block(),
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 3.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    let p = rect(5.0, 5.0, 10.0, 10.0);
    assert!(applicable(&holed, &p, &Frame::XY));
    let circle = Profile::Circle {
        center: [5.0, 5.0],
        radius: 2.0,
    };
    assert!(applicable(&block(), &circle, &Frame::XY));
    assert!(applicable(&block(), &p, &Frame::XY));
    let side = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let cross = cut_prism(
        &block(),
        &Profile::Circle {
            center: [20.0, 5.0],
            radius: 2.0,
        },
        &side.offset(-31.0),
        32.0,
    )
    .unwrap();
    let why = not_applicable(&cross, &p, &Frame::XY).unwrap();
    assert!(why.contains("not parallel to the tool axis"), "{why}");
    assert!(matches!(
        pocket(&cross, &p, 2.0),
        Err(KernelError::InvalidInput(_))
    ));
    // Oblique axis.
    let tilted = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.6, -0.8],
        z: [0.0, 0.8, 0.6],
    };
    assert!(!applicable(&block(), &p, &tilted));
}

#[test]
fn side_axis_cut_and_non_axis_aligned_profile() {
    // Cut along -Y from the front face (y = 0) with a rotated (45°) square
    // whose corner touches the top face's plane: slab axis is world Y.
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let h = 2.0;
    let p = Profile::Polygon(vec![
        [20.0, 10.0 - h],
        [20.0 + h, 10.0],
        [20.0, 10.0 + h],
        [20.0 - h, 10.0],
    ]);
    // Tool from y = 0 to y = 30 (through the block along +Y): start = front
    // moved by -30 along its normal (-Y), so it spans y in [0, 30].
    let start = front.offset(-30.0);
    let c = prism_boolean(&block(), &p, &start, 30.0, SlabOp::Cut).unwrap();
    // Half of the 2h-diagonal square lies below z = 10: area h^2, times 30.
    assert!((volume(&c) - (12000.0 - h * h * 30.0)).abs() < 1e-6);
    checked(&block(), &c, &p, &start, 30.0, PrismOp::Cut);
}

#[test]
fn truck_alone_refuses_these_cases() {
    // The reason the slab construction exists (observed with Truck 0.6): each
    // case above fails or is refused when handed to the 3-D boolean directly.
    let a = pocket(&block(), &rect(5.0, 5.0, 20.0, 20.0), 4.0).unwrap();
    let shared = cut_prism(
        &a,
        &rect(15.0, 5.0, 30.0, 20.0),
        &Frame::XY.offset(6.0),
        5.0,
    );
    let flush = cut_prism(
        &block(),
        &rect(0.0, 0.0, 10.0, 30.0),
        &Frame::XY.offset(7.0),
        4.0,
    );
    let on_top = crate::join_prism(
        &block(),
        &rect(30.0, 10.0, 40.0, 20.0),
        &Frame::XY.offset(10.0),
        5.0,
    );
    eprintln!(
        "truck: shared floor/walls {:?}\n flush {:?}\n coplanar join {:?}",
        shared.as_ref().map(volume),
        flush.as_ref().map(volume),
        on_top.as_ref().map(volume)
    );
    assert!(shared.is_err() && flush.is_err() && on_top.is_err());
}

#[test]
fn grid_snapping_is_consistent_across_calls() {
    let r = |x0: f64, y0: f64, x1: f64, y1: f64| -> Region {
        vec![vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]]]
    };
    // The same rectangle with ~1e-13 mm of noise (as Truck's B-rep carries),
    // alone and next to far-away geometry that moves each call's bounds: no
    // sliver may survive the difference.
    let a = r(1.0, 2.0, 23.9999999999997, 10.0);
    let b = r(1.0 - 1e-13, 2.0, 23.999999999999783, 10.000000000000002);
    let far = r(400.0, -300.0, 401.0, -299.0);
    assert!(boolean(&a, &b, OverlayRule::Difference).unwrap().is_empty());
    let mut b_far = b.clone();
    b_far.extend(far.clone());
    assert!(
        boolean(&a, &b_far, OverlayRule::Difference)
            .unwrap()
            .is_empty()
    );
    // The output of one call, fed to another, stays on the same grid.
    let u = boolean(&a, &far, OverlayRule::Union).unwrap();
    let back = boolean(&u, &far, OverlayRule::Difference).unwrap();
    assert!(boolean(&back, &b, OverlayRule::Xor).unwrap().is_empty());
    for p in back.iter().flatten().flatten() {
        assert_eq!(p.map(snap), *p, "off the grid: {p:?}");
    }
}

/// A pocket covered by a joined plate encloses a void. Not
/// supported (one shell per body); the refusal says so instead of reporting
/// disconnected solids.
#[test]
fn enclosed_void_is_refused_by_name() {
    let p = pocket(&block(), &rect(10.0, 10.0, 20.0, 20.0), 4.0).unwrap();
    let plate = rect(5.0, 5.0, 25.0, 25.0);
    let Err(e) = prism_boolean(&p, &plate, &Frame::XY.offset(10.0), 2.0, SlabOp::Join) else {
        panic!("a closed cavity was accepted");
    };
    assert!(e.to_string().contains("internal void"), "{e}");
    // Separate bodies keep their message.
    let Err(e) = pocket(&block(), &rect(18.0, -1.0, 22.0, 31.0), 10.0) else {
        panic!("a split body was accepted");
    };
    assert!(e.to_string().contains("disconnected"), "{e}");
}
