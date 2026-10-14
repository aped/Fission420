//! Direct edits of the independent FreeCAD fixture `direct_edit_block_mm.step`
//! (tools/fixtures/step/direct_edit.py): block [0,60]x[0,40]x[0,20], Ø8
//! through hole at (15,20), Ø6 x 8 blind hole at (30,12), Ø10 x 5 boss at
//! (45,25). Expected values are analytic.
use super::*;
use crate::{Body, StepImportOptions, VolumeResult, import_step, measure};
use std::f64::consts::PI;

const STEP: &str = include_str!("../../tests/fixtures/step/direct_edit_block_mm.step");
const V0: f64 = 48000.0 - 267.0 * PI;
/// Top face (z = 20): 60 x 40 minus the through hole, blind hole and boss base.
const TOP_AREA: f64 = 2400.0 - 50.0 * PI;

pub(crate) fn block() -> Body {
    let imp = import_step(STEP, &StepImportOptions::default()).expect("import fixture");
    assert_eq!(imp.solids.len(), 1);
    imp.solids.into_iter().next().unwrap().body
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        other => panic!("no volume: {other:?}"),
    }
}

fn assert_volume(b: &Body, want: f64) {
    let v = volume(b);
    assert!(
        (v - want).abs() <= 1e-6 * want.abs().max(1.0),
        "volume {v} vs {want} (diff {})",
        v - want
    );
}

/// Index of the planar face with outward normal `n` through a point at `level`
/// along it whose plane contains `p`.
fn plane_face(b: &Body, n: [f64; 3], p: [f64; 3]) -> usize {
    let count = crate::face_count(b);
    let hits: Vec<usize> = (0..count)
        .filter(|i| match classify_face(b, *i).unwrap() {
            DirectFace::Plane { origin, normal } => {
                let d: f64 = (0..3).map(|k| (p[k] - origin[k]) * normal[k]).sum();
                (0..3).all(|k| (normal[k] - n[k]).abs() < 1e-9) && d.abs() < 1e-9
            }
            _ => false,
        })
        .collect();
    assert_eq!(hits.len(), 1, "plane faces {n:?} through {p:?}: {hits:?}");
    hits[0]
}

/// A cylinder face with the given axis position (x, y) and radius.
fn cyl_face(b: &Body, x: f64, y: f64, r: f64) -> usize {
    let count = crate::face_count(b);
    (0..count)
        .find(|i| match classify_face(b, *i).unwrap() {
            DirectFace::Cylinder {
                origin,
                axis,
                radius,
                ..
            } => {
                axis[2].abs() > 1.0 - 1e-9
                    && (origin[0] - x).abs() < 1e-6
                    && (origin[1] - y).abs() < 1e-6
                    && (radius - r).abs() < 1e-6
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("no cylinder at ({x},{y}) r {r}"))
}

fn top(b: &Body) -> usize {
    plane_face(b, [0.0, 0.0, 1.0], [0.0, 0.0, 20.0])
}

fn bounds(b: &Body) -> ([f64; 3], [f64; 3]) {
    let m = measure(b).unwrap();
    (m.bounds.min, m.bounds.max)
}

#[test]
fn fixture_faces_classify() {
    let b = block();
    assert_volume(&b, V0);
    let n = crate::face_count(&b);
    let (mut planes, mut holes, mut bosses) = (0, 0, 0);
    for i in 0..n {
        match classify_face(&b, i).unwrap() {
            DirectFace::Plane { .. } => planes += 1,
            DirectFace::Cylinder { hole: true, .. } => holes += 1,
            DirectFace::Cylinder { hole: false, .. } => bosses += 1,
            DirectFace::Other => panic!("face {i} unclassified"),
        }
    }
    // STEP cylinders are split in two at import (seam).
    assert_eq!((planes, holes, bosses), (8, 4, 2));
    let t = top(&b);
    assert!((planar_face_area(&b, t).unwrap() - TOP_AREA).abs() < 1e-9);
    let through = cylinder_feature(&b, cyl_face(&b, 15.0, 20.0, 4.0)).unwrap();
    assert_eq!(through.describe(), "through hole");
    assert_eq!(through.faces.len(), 2);
    assert!((through.length - 20.0).abs() < 1e-9);
    let blind = cylinder_feature(&b, cyl_face(&b, 30.0, 12.0, 3.0)).unwrap();
    assert_eq!(blind.describe(), "blind hole");
    assert!((blind.length - 8.0).abs() < 1e-9);
    let boss = cylinder_feature(&b, cyl_face(&b, 45.0, 25.0, 5.0)).unwrap();
    assert_eq!(boss.describe(), "boss");
    assert!((boss.length - 5.0).abs() < 1e-9);
}

#[test]
fn offset_top_face_out_and_in() {
    let b = block();
    let t = top(&b);
    let up = offset_face(&b, t, 3.0).unwrap();
    assert_volume(&up.body, V0 + 3.0 * TOP_AREA);
    assert_eq!(crate::face_count(&up.body), crate::face_count(&b));
    // The top face keeps its index and is now at z = 23; the boss top stays.
    assert_eq!(top_at(&up.body, 23.0), t);
    assert!((bounds(&up.body).1[2] - 25.0).abs() < 1e-9);
    let down = offset_face(&b, t, -3.0).unwrap();
    assert_volume(&down.body, V0 - 3.0 * TOP_AREA);
    assert_eq!(top_at(&down.body, 17.0), t);
    // The boss top is not adjacent to the top face: it stays at 25 (boss now 8 tall).
    assert!((bounds(&down.body).1[2] - 25.0).abs() < 1e-9);
}

fn top_at(b: &Body, z: f64) -> usize {
    plane_face(b, [0.0, 0.0, 1.0], [0.0, 0.0, z])
}

#[test]
fn offset_side_face_and_floor() {
    let b = block();
    let side = plane_face(&b, [1.0, 0.0, 0.0], [60.0, 0.0, 0.0]);
    let e = offset_face(&b, side, 2.0).unwrap();
    assert_volume(&e.body, V0 + 2.0 * 40.0 * 20.0);
    assert!((bounds(&e.body).1[0] - 62.0).abs() < 1e-9);
    // Blind hole floor (outward normal +z at z = 12): 2 mm deeper.
    let floor = plane_face(&b, [0.0, 0.0, 1.0], [0.0, 0.0, 12.0]);
    let e = offset_face(&b, floor, -2.0).unwrap();
    assert_volume(&e.body, V0 - 2.0 * 9.0 * PI);
}

#[test]
fn offset_refusals() {
    let b = block();
    let t = top(&b);
    // Up 5: the boss (5 tall) would vanish into the top face.
    let e = offset_face(&b, t, 5.0).unwrap_err().to_string();
    assert!(e.contains("collapse or invert"), "{e}");
    // Down 8: the blind hole (8 deep) would lose its wall.
    let e = offset_face(&b, t, -8.0).unwrap_err().to_string();
    assert!(e.contains("collapse or invert"), "{e}");
    // Down 25: past the bottom.
    assert!(offset_face(&b, t, -25.0).is_err());
    // Blind floor up 8: reaches the top face.
    let floor = plane_face(&b, [0.0, 0.0, 1.0], [0.0, 0.0, 12.0]);
    assert!(offset_face(&b, floor, 8.0).is_err());
    // A cylinder is not planar.
    let c = cyl_face(&b, 15.0, 20.0, 4.0);
    assert!(
        offset_face(&b, c, 1.0)
            .unwrap_err()
            .to_string()
            .contains("not planar")
    );
    assert!(offset_face(&b, t, f64::NAN).is_err());
    assert!(offset_face(&b, t, 0.0).is_err());
    assert!(offset_face(&b, 999, 1.0).is_err());
}

#[test]
fn resize_holes_and_boss() {
    let b = block();
    let c = cyl_face(&b, 15.0, 20.0, 4.0);
    let e = resize_cylinder(&b, c, 12.0).unwrap();
    assert_volume(&e.body, V0 - PI * (36.0 - 16.0) * 20.0);
    assert_eq!(crate::face_count(&e.body), crate::face_count(&b));
    assert!(matches!(
        classify_face(&e.body, c).unwrap(),
        DirectFace::Cylinder { radius, hole: true, .. } if (radius - 6.0).abs() < 1e-9
    ));
    let e = resize_cylinder(&b, c, 4.0).unwrap();
    assert_volume(&e.body, V0 + PI * (16.0 - 4.0) * 20.0);
    // Blind hole Ø6 -> Ø10 (8 deep).
    let bl = cyl_face(&b, 30.0, 12.0, 3.0);
    let e = resize_cylinder(&b, bl, 10.0).unwrap();
    assert_volume(&e.body, V0 - PI * (25.0 - 9.0) * 8.0);
    // Boss Ø10 -> Ø14 (5 tall), and smaller.
    let bo = cyl_face(&b, 45.0, 25.0, 5.0);
    let e = resize_cylinder(&b, bo, 14.0).unwrap();
    assert_volume(&e.body, V0 + PI * (49.0 - 25.0) * 5.0);
    let e = resize_cylinder(&b, bo, 6.0).unwrap();
    assert_volume(&e.body, V0 - PI * (25.0 - 9.0) * 5.0);
    // Through hole up to just below the blind hole's clearance:
    // |(15,20)-(30,12)| = 17, minus r 3 -> radius < 14.
    let e = resize_cylinder(&b, c, 27.0).unwrap();
    assert_volume(&e.body, V0 - PI * (13.5 * 13.5 - 16.0) * 20.0);
}

#[test]
fn resize_refusals() {
    let b = block();
    let c = cyl_face(&b, 15.0, 20.0, 4.0);
    // Ø29: radius 14.5 reaches the blind hole (14 mm away).
    let e = resize_cylinder(&b, c, 29.0).unwrap_err().to_string();
    assert!(e.contains("would reach another edge"), "{e}");
    // Ø31 (radius 15.5) would also break the x = 0 wall.
    assert!(resize_cylinder(&b, c, 31.0).is_err());
    assert!(resize_cylinder(&b, c, 8.0).is_err(), "same size");
    assert!(resize_cylinder(&b, c, 0.0).is_err());
    assert!(resize_cylinder(&b, c, f64::INFINITY).is_err());
    let t = top(&b);
    assert!(
        resize_cylinder(&b, t, 5.0)
            .unwrap_err()
            .to_string()
            .contains("not a circular cylinder")
    );
}

#[test]
fn remove_holes_and_boss() {
    let b = block();
    let n = crate::face_count(&b);
    let c = cyl_face(&b, 15.0, 20.0, 4.0);
    let e = remove_cylinder(&b, c).unwrap();
    assert_volume(&e.body, V0 + PI * 16.0 * 20.0);
    assert_eq!(e.removed_faces.len(), 2);
    assert_eq!(crate::face_count(&e.body), n - 2);
    let bl = cyl_face(&b, 30.0, 12.0, 3.0);
    let e = remove_cylinder(&b, bl).unwrap();
    assert_volume(&e.body, V0 + PI * 9.0 * 8.0);
    assert_eq!(e.removed_faces.len(), 3, "two walls and the floor");
    let bo = cyl_face(&b, 45.0, 25.0, 5.0);
    let e = remove_cylinder(&b, bo).unwrap();
    assert_volume(&e.body, V0 - PI * 25.0 * 5.0);
    assert!((bounds(&e.body).1[2] - 20.0).abs() < 1e-9);
    // Surviving faces keep their identity under `renumber`.
    let t = top(&b);
    assert_eq!(renumber(t, &e.removed_faces), Some(top(&e.body)));
    // Remove everything in turn: a plain block.
    let mut body = b.clone();
    for (x, y, r) in [(15.0, 20.0, 4.0), (30.0, 12.0, 3.0), (45.0, 25.0, 5.0)] {
        let f = cyl_face(&body, x, y, r);
        body = remove_cylinder(&body, f).unwrap().body;
    }
    assert_volume(&body, 48000.0);
    assert_eq!(crate::face_count(&body), 6);
}

#[test]
fn edits_compose_and_export() {
    let b = block();
    let t = top(&b);
    let e1 = offset_face(&b, t, 3.0).unwrap().body;
    let c = cyl_face(&e1, 15.0, 20.0, 4.0);
    let e2 = resize_cylinder(&e1, c, 10.0).unwrap().body;
    let bl = cyl_face(&e2, 30.0, 12.0, 3.0);
    let e3 = remove_cylinder(&e2, bl).unwrap().body;
    // 3 mm on top; hole Ø10 over 23 mm; blind hole filled (now 11 deep).
    let want = V0 + 3.0 * TOP_AREA - PI * (25.0 - 16.0) * 23.0 + PI * 9.0 * 11.0;
    assert_volume(&e3, want);
    // Exported exactly (re-read independently by tools/check_direct_edit.sh in
    // FreeCAD; our own importer also reads the SURFACE_OF_REVOLUTION
    // the writer uses for imported cylinders, within the limits of
    // docs/formats/step.md).
    let step = crate::export_step(&e3, "R-CAD test").unwrap();
    assert!(step.contains("MANIFOLD_SOLID_BREP"));
    if let Ok(dir) = std::env::var("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, b: &Body| {
            let s = crate::export_step(b, "R-CAD direct edit evidence").unwrap();
            std::fs::write(std::path::Path::new(&dir).join(name), s).unwrap();
        };
        write("direct_composed.step", &e3);
        write("direct_unedited.step", &b);
        write(
            "direct_offset_top_up3.step",
            &offset_face(&b, t, 3.0).unwrap().body,
        );
        let c0 = cyl_face(&b, 15.0, 20.0, 4.0);
        write(
            "direct_resize_through_d12.step",
            &resize_cylinder(&b, c0, 12.0).unwrap().body,
        );
        let bo = cyl_face(&b, 45.0, 25.0, 5.0);
        write(
            "direct_remove_boss.step",
            &remove_cylinder(&b, bo).unwrap().body,
        );
    }
}

#[test]
fn renumber_maps_indices() {
    assert_eq!(renumber(0, &[1, 3]), Some(0));
    assert_eq!(renumber(1, &[1, 3]), None);
    assert_eq!(renumber(2, &[1, 3]), Some(1));
    assert_eq!(renumber(5, &[1, 3]), Some(3));
}

#[test]
fn oblique_neighbour_is_refused() {
    // Loft frustum 20x20 (z = 0) to 10x10 (z = 10): the side faces are inclined.
    let sq = |h: f64| Profile::Polygon(vec![[-h, -h], [h, -h], [h, h], [-h, h]]);
    let b = crate::loft(&sq(10.0), &Frame::XY, &sq(5.0), &Frame::XY.offset(10.0)).unwrap();
    let t = plane_face(&b, [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    let e = offset_face(&b, t, 1.0).unwrap_err().to_string();
    assert!(e.contains("not perpendicular"), "{e}");
}

#[test]
fn result_check_rejects_a_wrong_expectation() {
    // The independent check is not vacuous: the +3 offset is not a cut.
    let b = block();
    let t = top(&b);
    let up = offset_face(&b, t, 3.0).unwrap();
    let sh = shell_of(&b).unwrap();
    let (fr, outer, holes, _) = face_region(&sh, t, geo_tol(&sh)).unwrap();
    let prism = Prism {
        profile: &outer,
        holes: &holes,
        start: &fr,
        length: 3.0,
    };
    assert!(verify(&b, &up.body, PrismOp::Join, &prism).is_ok());
    let shifted = fr.offset(-3.0);
    let wrong = Prism {
        start: &shifted,
        ..prism
    };
    assert!(verify(&b, &up.body, PrismOp::Cut, &wrong).is_err());
}

/// Volume removed by a countersink of diameter 2R, angle `ang`, on a hole of
/// radius r: the cone frustum minus the cylinder it replaces.
fn csink_removed(r: f64, big: f64, ang: f64) -> f64 {
    let h = (big - r) / (ang / 2.0).tan();
    PI * h / 3.0 * (big * big + big * r + r * r) - PI * r * r * h
}

/// Volume of `b` (exact or a labelled estimate) agrees with `want`: an
/// estimate must contain it within its stated error.
fn assert_volume_any(b: &Body, want: f64) {
    match measure(b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => {
            let slack = error_estimate.max(1e-6 * want);
            assert!(
                (volume - want).abs() <= slack,
                "{method:?}: {volume} ± {error_estimate} vs {want}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn countersink_on_imported_through_hole() {
    let b = block();
    let out = countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 8.0, 16.0, PI / 2.0).unwrap();
    let removed = csink_removed(4.0, 8.0, PI / 2.0);
    assert!((removed - (256.0 / 3.0) * PI).abs() < 1e-9);
    assert_volume_any(&out, V0 - removed);
    // Probes: just inside the cone surface is void, just outside is material
    // (depth 1: cone radius 7).
    let m = crate::point_membership(
        &out,
        &[
            [15.0 + 6.8, 20.0, 19.0],
            [15.0 + 7.2, 20.0, 19.0],
            [15.0, 20.0 + 4.2, 15.0],
        ],
    )
    .unwrap();
    assert_eq!(m, vec![Some(false), Some(true), Some(true)]);
    // The cone faces are the exact cone: every surface point at depth t
    // below z = 20 is 8 - t from the axis (R = 8, r = 4, h = 4).
    let mut cones = 0;
    for f in out.solid.face_iter() {
        if let truck_modeling::Surface::NurbsSurface(srf) = f.surface() {
            cones += 1;
            let ((u0, u1), (v0, v1)) = srf.parameter_range();
            let b = |x: std::ops::Bound<f64>| match x {
                std::ops::Bound::Included(v) | std::ops::Bound::Excluded(v) => v,
                std::ops::Bound::Unbounded => panic!("unbounded"),
            };
            for i in 0..=20 {
                for j in 0..=20 {
                    let u = b(u0) + (b(u1) - b(u0)) * i as f64 / 20.0;
                    let v = b(v0) + (b(v1) - b(v0)) * j as f64 / 20.0;
                    let p = srf.subs(u, v);
                    let rho = (p.x - 15.0).hypot(p.y - 20.0);
                    let t = 20.0 - p.z;
                    assert!((rho - (8.0 - t)).abs() < 1e-9, "off cone at {p:?}");
                    assert!((-1e-9..=4.0 + 1e-9).contains(&t));
                }
            }
        }
    }
    assert_eq!(cones, 2);
    // The direct-edit view still sees one hole wall and the new cone faces.
    let kinds = classify_faces(&out).unwrap();
    assert_eq!(kinds.len(), crate::face_count(&b) + 2, "two cone faces");
    // 82 degrees (common inch screws), and the axis given from below.
    let out = countersink(
        &b,
        [15.0, 20.0, 20.0],
        [0.0, 0.0, -1.0],
        8.0,
        12.0,
        82f64.to_radians(),
    )
    .unwrap();
    assert_volume_any(&out, V0 - csink_removed(4.0, 6.0, 82f64.to_radians()));
    if let Ok(dir) = std::env::var("RCAD_EVIDENCE_DIR") {
        let s = crate::export_step(
            &countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 8.0, 16.0, PI / 2.0).unwrap(),
            "R-CAD countersink evidence",
        )
        .unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("csink_block_d16.step"), s).unwrap();
    }
}

#[test]
fn countersink_on_both_ends_of_a_plate_hole() {
    // FreeCAD plate 60x40x8 with a Ø10 hole at (30,20).
    let imp = import_step(
        include_str!("../../tests/fixtures/step/plate_hole_mm.step"),
        &StepImportOptions::default(),
    )
    .unwrap();
    let b = imp.solids.into_iter().next().unwrap().body;
    let v0 = 19200.0 - 200.0 * PI;
    let top = countersink(&b, [30.0, 20.0, 8.0], [0.0, 0.0, 1.0], 10.0, 16.0, PI / 2.0).unwrap();
    let both = countersink(
        &top,
        [30.0, 20.0, 0.0],
        [0.0, 0.0, 1.0],
        10.0,
        14.0,
        PI / 2.0,
    )
    .unwrap();
    let want = v0 - csink_removed(5.0, 8.0, PI / 2.0) - csink_removed(5.0, 7.0, PI / 2.0);
    assert_volume_any(&both, want);
}

#[test]
fn countersink_refusals() {
    let b = block();
    let e = |r: Result<Body>| r.unwrap_err().to_string();
    // Ø30 on the Ø8 hole at x = 15: reaches the blind hole / the x = 0 wall.
    let m = e(countersink(
        &b,
        [15.0, 20.0, 20.0],
        [0.0, 0.0, 1.0],
        8.0,
        30.0,
        PI / 2.0,
    ));
    assert!(m.contains("does not fit"), "{m}");
    // Ø6 x 8 blind hole: Ø23 at 90 degrees is 8.5 deep.
    let m = e(countersink(
        &b,
        [30.0, 12.0, 20.0],
        [0.0, 0.0, 1.0],
        6.0,
        23.0,
        PI / 2.0,
    ));
    assert!(m.contains("reaches the end of the hole wall"), "{m}");
    // Not larger than the hole, bad angles, no hole there, wrong diameter.
    assert!(countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 8.0, 8.0, PI / 2.0).is_err());
    assert!(countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 8.0, 12.0, PI).is_err());
    assert!(countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 8.0, 12.0, 0.0).is_err());
    assert!(countersink(&b, [5.0, 5.0, 20.0], [0.0, 0.0, 1.0], 8.0, 12.0, PI / 2.0).is_err());
    assert!(countersink(&b, [15.0, 20.0, 20.0], [0.0, 0.0, 1.0], 6.0, 12.0, PI / 2.0).is_err());
    // Not at a face holding the hole's opening (mid-depth plane).
    assert!(countersink(&b, [15.0, 20.0, 10.0], [0.0, 0.0, 1.0], 8.0, 12.0, PI / 2.0).is_err());
}

/// Truck-built block [0,40]x[0,30]x[0,10] (single-loop faces).
fn plain_block() -> Body {
    crate::extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
        &Frame::XY,
        10.0,
    )
    .unwrap()
}

/// Top face frame as the importer would record it: centroid (20,15,10),
/// x = world X (least aligned with +Z), y = +Y.
const TOP_FRAME: Frame = Frame {
    origin: [20.0, 15.0, 10.0],
    x: [1.0, 0.0, 0.0],
    y: [0.0, 1.0, 0.0],
    z: [0.0, 0.0, 1.0],
};

#[test]
fn replace_face_by_tilted_and_offset_planes() {
    let b = plain_block();
    let t = plane_face(&b, [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    // Tilt 20 degrees about the centroid's x axis: the top becomes
    // z = 10 + (y − 15) tan 20; the volume is unchanged (symmetric).
    let a = 20f64.to_radians();
    let f = tilt_offset_frame(&TOP_FRAME, 0.0, a);
    let e = replace_face(&b, t, f.origin, f.z).unwrap();
    assert_volume(&e.body, 12000.0);
    let (lo, hi) = bounds(&e.body);
    assert!((hi[2] - (10.0 + 15.0 * a.tan())).abs() < 1e-9, "{hi:?}");
    assert!((lo[2]).abs() < 1e-9);
    assert_eq!(crate::face_count(&e.body), 6);
    // Plus 2 mm along the tilted normal: the height at the centroid grows by
    // 2 / cos 20, so the volume by 1200 · 2 / cos 20.
    let f = tilt_offset_frame(&TOP_FRAME, 2.0, a);
    let e = replace_face(&b, t, f.origin, f.z).unwrap();
    assert_volume(&e.body, 12000.0 + 1200.0 * 2.0 / a.cos());
    // A side face tilted outward 10 degrees about its own x (world Y... its
    // frame: centroid (40,15,5), x = Y, z = +X, y = z × x = +Z).
    let side = plane_face(&b, [1.0, 0.0, 0.0], [40.0, 0.0, 0.0]);
    let sf = Frame {
        origin: [40.0, 15.0, 5.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    let f = tilt_offset_frame(&sf, 0.0, 10f64.to_radians());
    let e = replace_face(&b, side, f.origin, f.z).unwrap();
    assert_volume(&e.body, 12000.0);
    if let Ok(dir) = std::env::var("RCAD_EVIDENCE_DIR") {
        let f = tilt_offset_frame(&TOP_FRAME, 2.0, a);
        let s =
            crate::export_step(&replace_face(&b, t, f.origin, f.z).unwrap().body, "R-CAD").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("replace_top_tilt20_up2.step"),
            s,
        )
        .unwrap();
    }
}

#[test]
fn replace_face_refusals() {
    let b = plain_block();
    let t = plane_face(&b, [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    // 35 degrees: at y = 0 the top would drop below the bottom (15 tan 35 > 10).
    let f = tilt_offset_frame(&TOP_FRAME, 0.0, 35f64.to_radians());
    let e = replace_face(&b, t, f.origin, f.z).unwrap_err().to_string();
    assert!(e.contains("collapses or inverts"), "{e}");
    // Facing the other way.
    assert!(replace_face(&b, t, [0.0, 0.0, 10.0], [0.0, 0.0, -1.0]).is_err());
    // The imported block's top has holes (inner loops); its blind-hole floor
    // has cylindrical neighbours.
    let blk = block();
    let e = replace_face(&blk, top(&blk), [0.0, 0.0, 21.0], [0.0, 0.0, 1.0])
        .unwrap_err()
        .to_string();
    assert!(e.contains("single-loop"), "{e}");
    let floor = plane_face(&blk, [0.0, 0.0, 1.0], [0.0, 0.0, 12.0]);
    let e = replace_face(&blk, floor, [0.0, 0.0, 11.0], [0.0, 0.0, 1.0])
        .unwrap_err()
        .to_string();
    assert!(e.contains("not planar") || e.contains("curved edge"), "{e}");
}
