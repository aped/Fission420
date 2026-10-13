//! Run 7 surfaces / open-shell repair (docs/SURFACES.md, ADR 0012): STEP
//! fixtures made independently by FreeCAD (tools/fixtures/step/open_shells.py,
//! FreeCAD 1.1.4 / OCC 7.8.1). Expected values are closed form.
use rcad_kernel::{
    Frame, LoopSegment, Profile, StepImportOptions, VolumeResult, analyze, extrude, fill_planar,
    glue, import_step, measure, stitch, thicken,
};
use std::f64::consts::PI;

fn surf(name: &str) -> rcad_kernel::ImportedSurfaces {
    let path = format!("{}/tests/fixtures/step/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let imp = import_step(&text, &StepImportOptions::default()).unwrap();
    assert!(imp.solids.is_empty(), "{name}: no solids expected");
    imp.surfaces.expect("surfaces")
}

fn vol(b: &rcad_kernel::Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("no volume: {reason}"),
    }
}

#[test]
fn open_box_is_open_and_fills_to_the_box() {
    let s = surf("open_box_mm.step");
    assert_eq!((s.shells, s.open_shells, s.faces), (1, 1, 5));
    assert!(s.body.is_open_shell());
    let m = measure(&s.body).unwrap();
    assert!(!m.all_shells_closed);
    assert!(matches!(m.volume, VolumeResult::Unavailable { .. }));
    let a = analyze(&s.body, 0.0).unwrap();
    assert!(!a.closed);
    assert_eq!(a.free_edges.len(), 4);
    assert!(a.gaps.is_empty());
    assert!(a.orientation_conflicts.is_empty());
    assert_eq!(a.boundary_loops.len(), 1);
    let l = &a.boundary_loops[0];
    assert!(l.planarity < 1e-12, "{}", l.planarity);
    assert!((l.normal[2].abs() - 1.0).abs() < 1e-12);
    // Every free edge lies on the top rim z = 20; total length = perimeter.
    assert!(
        a.free_edges
            .iter()
            .all(|e| e.from[2] == 20.0 && e.to[2] == 20.0)
    );
    let per: f64 = a.free_edges.iter().map(|e| e.length).sum();
    assert!((per - 140.0).abs() < 1e-9);
    let (solid, rep) = fill_planar(&s.body, 1e-6).unwrap();
    assert!(rep.closed && rep.filled == 1);
    assert!(!solid.is_open_shell());
    let v = vol(&solid);
    assert!((v - 24000.0).abs() < 1e-6, "{v}");
    let b = measure(&solid).unwrap().bounds;
    assert_eq!((b.min, b.max), ([0.0; 3], [40.0, 30.0, 20.0]));
    // The input stays open (fill builds a new body).
    assert!(s.body.is_open_shell());
}

#[test]
fn loose_faces_report_gaps_then_stitch_and_fill() {
    let s = surf("loose_faces_mm.step");
    assert_eq!((s.shells, s.open_shells, s.faces), (5, 5, 5));
    let a = analyze(&s.body, 1e-6).unwrap();
    assert_eq!(a.free_edges.len(), 20);
    // 8 coincident edge pairs: 4 floor edges, 4 vertical corners.
    assert_eq!(a.gaps.len(), 8);
    assert!(a.gaps.iter().all(|g| g.size < 1e-9));
    // Fill refuses before stitching (several shells / no loop).
    assert!(fill_planar(&s.body, 1e-6).is_err());
    let (st, rep) = stitch(&s.body, 1e-6).unwrap();
    assert!(!rep.closed);
    assert_eq!(rep.free_edges, 4);
    assert_eq!(rep.stats.edges_merged, 8);
    let a2 = analyze(&st, 0.0).unwrap();
    assert!(a2.orientation_conflicts.is_empty());
    assert_eq!(a2.boundary_loops.len(), 1);
    let (solid, _) = fill_planar(&st, 1e-6).unwrap();
    assert!((vol(&solid) - 24000.0).abs() < 1e-6);
}

#[test]
fn gap_box_heals_only_within_the_tolerance() {
    let s = surf("gap_box_mm.step");
    let a = analyze(&s.body, 0.1).unwrap();
    // The +X wall (x = 40.02) is 0.02 mm off its floor edge and two corners.
    let g: Vec<f64> = a.gaps.iter().map(|g| g.size).collect();
    assert_eq!(a.gaps.iter().filter(|g| g.size < 1e-9).count(), 5, "{g:?}");
    let off: Vec<_> = a.gaps.iter().filter(|g| g.size > 1e-9).collect();
    assert_eq!(off.len(), 3, "{g:?}");
    assert!(off.iter().all(|g| (g.size - 0.02).abs() < 1e-9));
    // Below the gap: stays open with the wall unattached.
    let (st, rep) = stitch(&s.body, 0.01).unwrap_or_else(|e| panic!("{e}"));
    assert!(!rep.closed);
    assert!(
        fill_planar(&st, 1e-6).is_err(),
        "unhealed gap must not fill"
    );
    // At 0.05 mm: merged; the wall is within 0.02 mm of its merged edges.
    let (st, rep) = stitch(&s.body, 0.05).unwrap();
    assert!((rep.stats.max_vertex_gap - 0.02).abs() < 1e-9);
    let (solid, _) = fill_planar(&st, 0.05).unwrap();
    // Volume within gap x wall area of the exact box.
    let v = vol(&solid);
    assert!((v - 24000.0).abs() <= 0.02 * 600.0 + 1e-6, "{v}");
}

#[test]
fn thicken_plate_and_half_tube() {
    let p = surf("plate_mm.step");
    let t = thicken(&p.body, 2.0, false).unwrap();
    assert!((vol(&t) - 2000.0).abs() < 1e-6);
    let b = measure(&t).unwrap().bounds;
    let up = b.max[2] == 2.0 && b.min[2] == 0.0;
    let t2 = thicken(&p.body, 2.0, true).unwrap();
    let b2 = measure(&t2).unwrap().bounds;
    let down = b2.min[2] == -2.0 && b2.max[2] == 0.0;
    // The two directions are opposite sides of the plane (one each).
    assert!(up ^ (b.min[2] == -2.0), "{b:?}");
    assert!(up == down, "{b:?} {b2:?}");
    let h = surf("half_tube_mm.step");
    let out = thicken(&h.body, 2.0, false).unwrap();
    let inn = thicken(&h.body, 2.0, true).unwrap();
    let (vo, vi) = (vol(&out), vol(&inn));
    let a = 0.5 * PI * (144.0 - 100.0) * 30.0;
    let b = 0.5 * PI * (100.0 - 64.0) * 30.0;
    assert!(
        ((vo - a).abs() < 1e-6 && (vi - b).abs() < 1e-6)
            || ((vo - b).abs() < 1e-6 && (vi - a).abs() < 1e-6),
        "{vo} {vi}"
    );
    // Toward the axis by >= the radius: refused (offset collapse).
    let inward = (vo - b).abs() >= 1e-6;
    let e = thicken(&h.body, 10.0, inward).unwrap_err().to_string();
    assert!(e.contains("collapses"), "{e}");
    // Solids are refused.
    assert!(thicken(&t, 1.0, false).is_err());
}

#[test]
fn glue_two_boxes_along_their_shared_face() {
    let sq = |x0: f64| {
        Profile::Loop(vec![
            LoopSegment::Line { start: [x0, 0.0] },
            LoopSegment::Line {
                start: [x0 + 10.0, 0.0],
            },
            LoopSegment::Line {
                start: [x0 + 10.0, 10.0],
            },
            LoopSegment::Line { start: [x0, 10.0] },
        ])
    };
    let a = extrude(&sq(0.0), &Frame::XY, 5.0).unwrap();
    let b = extrude(&sq(10.0), &Frame::XY, 5.0).unwrap();
    let (g, st) = glue(&[a, b], 1e-9).unwrap();
    assert_eq!(st.faces_glued, 1);
    let m = measure(&g).unwrap();
    assert!(m.all_shells_closed && m.shell_count == 1);
    assert_eq!(m.face_count, 10);
    assert!((vol(&g) - 1000.0).abs() < 1e-9);
    // Disjoint boxes cannot be glued into one solid.
    let c = extrude(&sq(30.0), &Frame::XY, 5.0).unwrap();
    let d = extrude(&sq(0.0), &Frame::XY, 5.0).unwrap();
    assert!(glue(&[d, c], 1e-9).is_err());
}
