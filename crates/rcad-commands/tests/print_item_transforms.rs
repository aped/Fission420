//! 3MF build-item transforms (docs/PRINT_EXPORT.md, `item_transforms`): the
//! placed enclosure exported with each object in modeled coordinates and the
//! placement as its build item's transform, in inches. Read back with the 3MF
//! reader (which applies item transforms) the objects sit exactly where the
//! analytic placement puts them: on z = 0, side by side on the P1S bed centre
//! (128 mm) with a 10 mm gap, with the analytic volumes. The file is also left
//! under artifacts/print/tests/item_transforms for the external Python checker
//! (`tools/print/check_3mf_items.py`).
use rcad_commands::print::{
    BedPreset, BedSpec, BodyPlacement, Layout, Orient, Placement, PrintOptions,
};
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_exchange::mesh::diagnose;
use rcad_exchange::threemf::{ThreeMfLimits, ThreeMfUnit, read};
use std::path::PathBuf;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?} {:?}", r.error, r.message);
    r
}

fn sketch(s: &mut Session, plane: SketchPlane, t: SketchTemplate) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template: t,
        },
    )
    .feature
    .unwrap()
}

fn rect(w: f64, h: f64) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: None,
    }
}

fn extrude(s: &mut Session, sk: FeatureId, d: f64, op: ExtrudeOp, name: Option<&str>) -> FeatureId {
    ok(
        s,
        Command::Extrude {
            name: None,
            sketch: sk,
            distance: d,
            op,
            target: None,
            through_all: false,
            reverse: false,
            body_name: name.map(str::to_string),
            region: None,
            options: Default::default(),
        },
    )
    .feature
    .unwrap()
}

fn face(feature: FeatureId, role: FaceRole) -> FaceRef {
    FaceRef { feature, role }
}

fn out_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/print/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Enclosure base 80x50x25 (wall/floor 2, open top) centred on the origin, and a
/// lid: plate 80x50x2 on z = [0, 2] with a 75.5x45.5 lip ring (wall 2) on
/// z = [-3, 0]. Returns (base body, lid body, lid plate extrude feature).
fn enclosure(s: &mut Session) -> (BodyId, BodyId, FeatureId) {
    ok(s, Command::NewDocument);
    let sb = sketch(s, SketchPlane::WorldXY, rect(80.0, 50.0));
    let eb = extrude(s, sb, 25.0, ExtrudeOp::NewBody, Some("Base"));
    let top = SketchPlane::Face(face(eb, FaceRole::ExtrudeEndCap));
    let sc = sketch(s, top, rect(76.0, 46.0));
    extrude(s, sc, 23.0, ExtrudeOp::Cut, None);
    // Lid plate, then the lip joined on its start cap (pointing -Z), then the
    // lip's interior cut from the lip's end cap (z = -3) back up to z = 0.
    let sl = sketch(s, SketchPlane::WorldXY, rect(80.0, 50.0));
    let el = extrude(s, sl, 2.0, ExtrudeOp::NewBody, Some("Lid"));
    let start = SketchPlane::Face(face(el, FaceRole::ExtrudeStartCap));
    let slip = sketch(s, start, rect(75.5, 45.5));
    let elip = extrude(s, slip, 3.0, ExtrudeOp::Join, None);
    let lip_end = SketchPlane::Face(face(elip, FaceRole::ExtrudeEndCap));
    let sin = sketch(s, lip_end, rect(71.5, 41.5));
    extrude(s, sin, 3.0, ExtrudeOp::Cut, None);
    let bodies = s.summary().bodies;
    assert_eq!(bodies.len(), 2);
    (bodies[0].id, bodies[1].id, el)
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn placement_as_item_transforms_reads_back_placed() {
    let mut s = Session::new();
    let (base, lid, plate) = enclosure(&mut s);
    let dir = out_dir("item_transforms");
    let placement = Placement {
        bodies: vec![BodyPlacement {
            body: lid,
            orient: Orient::FaceDown(face(plate, FaceRole::ExtrudeEndCap)),
        }],
        layout: Some(Layout { gap: 10.0 }),
    };
    let mut files = vec![];
    for (name, item, unit) in [
        ("baked_mm.3mf", false, ThreeMfUnit::Millimeter),
        ("items_mm.3mf", true, ThreeMfUnit::Millimeter),
        ("items_inch.3mf", true, ThreeMfUnit::Inch),
        ("baked_inch.3mf", false, ThreeMfUnit::Inch),
    ] {
        let p = dir.join(name);
        let r = ok(
            &mut s,
            Command::Export3mf {
                path: p.clone(),
                chord: None,
                bodies: Some(vec![base, lid]),
                unit: Some(unit),
                print: PrintOptions {
                    placement: Some(placement.clone()),
                    bed: Some(BedSpec::Preset {
                        preset: BedPreset::BambuP1s,
                    }),
                    item_transforms: item,
                    ..Default::default()
                },
            },
        );
        let msg = r.message.unwrap();
        assert_eq!(msg.contains("build item's transform"), item, "{msg}");
        let warned = r
            .export
            .unwrap()
            .warnings
            .iter()
            .any(|w| w.contains("Bambu Studio"));
        assert_eq!(warned, item && unit != ThreeMfUnit::Millimeter);
        let imp = read(&std::fs::read(&p).unwrap(), &ThreeMfLimits::default()).unwrap();
        assert_eq!(imp.unit, unit);
        assert_eq!(imp.objects.len(), 2);
        // Analytic placement: total width 80 + 10 + 80 centred on x = 128.
        for (o, name, x0, dims, vol) in [
            (&imp.objects[0], "Base", 43.0, [80.0, 50.0, 25.0], 19592.0),
            (&imp.objects[1], "Lid", 133.0, [80.0, 50.0, 5.0], 9404.0),
        ] {
            assert_eq!(o.name.as_deref(), Some(name));
            let d = diagnose(&o.positions, &o.triangles);
            assert!(d.closed, "{name} {d:?}");
            let (lo, hi) = d.bounds.unwrap();
            let tol = 1e-6;
            assert!(approx(lo[2], 0.0, tol), "{name} {lo:?}");
            assert!(approx(lo[0], x0, tol), "{name} {lo:?}");
            assert!(
                approx(0.5 * (lo[1] + hi[1]), 128.0, tol),
                "{name} {lo:?} {hi:?}"
            );
            for i in 0..3 {
                assert!(approx(hi[i] - lo[i], dims[i], tol), "{name} {i}");
            }
            assert!(approx(d.signed_volume.unwrap(), vol, 1e-4), "{name}");
        }
        files.push(p);
    }
    // STL refuses the option; the document is unchanged by every export.
    let r = s.execute(&Command::ExportStl {
        path: dir.join("x.stl"),
        chord: None,
        bodies: None,
        print: PrintOptions {
            item_transforms: true,
            ..Default::default()
        },
    });
    assert!(!r.ok);
    assert!(!dir.join("x.stl").exists());
}
