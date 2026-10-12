#![allow(clippy::needless_range_loop)]
//! Print export (docs/PRINT_EXPORT.md): tessellation accuracy measured on the
//! exported file bytes, strict mesh policy, export-only placement, layout, bed
//! fit, separate files, hidden-body policy, JSON compatibility. Expected values
//! are analytic; exported files are parsed here with an independent reader.
use f420_commands::print::{
    BedPreset, BedSpec, BodyPlacement, Layout, Orient, Placement, PrintOptions,
};
use f420_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

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
        .join("../../artifacts/run4/print/tests")
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

/// Independent binary STL reader: triangles as f32 corner triples.
fn read_stl(path: &Path) -> Vec<[[f64; 3]; 3]> {
    let b = std::fs::read(path).unwrap();
    assert!(b.len() >= 84);
    assert!(
        !b.starts_with(b"solid"),
        "binary STL header must not start with 'solid'"
    );
    let n = u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize;
    assert_eq!(b.len(), 84 + 50 * n);
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as f64;
    (0..n)
        .map(|i| {
            let o = 84 + 50 * i + 12;
            std::array::from_fn(|v| std::array::from_fn(|c| f(o + 12 * v + 4 * c)))
        })
        .collect()
}

fn signed_volume(tris: &[[[f64; 3]; 3]]) -> f64 {
    tris.iter()
        .map(|[p, q, r]| {
            p[0] * (q[1] * r[2] - q[2] * r[1]) - p[1] * (q[0] * r[2] - q[2] * r[0])
                + p[2] * (q[0] * r[1] - q[1] * r[0])
        })
        .sum::<f64>()
        / 6.0
}

fn tri_bounds(tris: &[[[f64; 3]; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in tris.iter().flatten() {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

/// Closed edge incidence with opposite directions, by exact corner equality.
fn closed_and_oriented(tris: &[[[f64; 3]; 3]]) -> bool {
    let key = |p: [f64; 3]| p.map(|c| (c as f32).to_bits());
    let mut e: HashMap<([u32; 3], [u32; 3]), i32> = HashMap::new();
    for t in tris {
        for k in 0..3 {
            *e.entry((key(t[k]), key(t[(k + 1) % 3]))).or_default() += 1;
        }
    }
    e.iter()
        .all(|(&(a, b), &n)| n == 1 && e.get(&(b, a)) == Some(&1))
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Exported cylinder Ø20 x 10: every vertex on the lateral surface lies on the
/// circle (f32 rounding), every lateral edge's midpoint sagitta is within the
/// chord target, bounds/scale are exact, and a tighter chord improves accuracy
/// with a triangle count bounded by the sagitta formula.
#[test]
fn cylinder_tessellation_accuracy_from_exported_bytes() {
    let r = 10.0;
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch(
        &mut s,
        SketchPlane::WorldXY,
        SketchTemplate::CenteredCircle {
            diameter: 2.0 * r,
            center: None,
        },
    );
    extrude(&mut s, sk, 10.0, ExtrudeOp::NewBody, Some("Cylinder"));
    let dir = out_dir("cylinder");
    let mut results = Vec::new();
    for chord in [0.02, 0.005] {
        let p = dir.join(format!("cylinder_chord_{chord}.stl"));
        let resp = ok(
            &mut s,
            Command::ExportStl {
                path: p.clone(),
                chord: Some(chord),
                bodies: None,
                print: PrintOptions::default(),
            },
        );
        let tris = read_stl(&p);
        let (lo, hi) = tri_bounds(&tris);
        // The polygon's extremes lie within one sagitta inside the circle.
        for i in 0..2 {
            assert!(
                lo[i] >= -r - 1e-5 && lo[i] <= -r + chord + 1e-5,
                "{lo:?} {hi:?}"
            );
            assert!(
                hi[i] <= r + 1e-5 && hi[i] >= r - chord - 1e-5,
                "{lo:?} {hi:?}"
            );
        }
        assert!(approx(lo[2], 0.0, 1e-6) && approx(hi[2], 10.0, 1e-5));
        assert!(closed_and_oriented(&tris));
        let rad = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt();
        let (mut max_vertex_err, mut max_sagitta) = (0.0f64, 0.0f64);
        let mut angles = std::collections::BTreeSet::new();
        for t in &tris {
            let n = {
                let u = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
                let v = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
                let c = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                c.map(|x| x / l)
            };
            if n[2].abs() > 1e-6 {
                continue; // cap triangle (planar, exact)
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                max_vertex_err = max_vertex_err.max((rad(a) - r).abs());
                if a[2] == 0.0 {
                    angles.insert(((a[1].atan2(a[0])) * 1e6).round() as i64);
                }
                let m = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1]), 0.0];
                max_sagitta = max_sagitta.max(r - rad(m));
            }
        }
        // f32 storage: ~1e-6 relative at 10 mm.
        assert!(max_vertex_err <= 2e-5, "vertex error {max_vertex_err}");
        assert!(
            max_sagitta <= chord + 2e-5,
            "sagitta {max_sagitta} > chord {chord}"
        );
        // Minimum segment count for this sagitta: N >= pi / acos(1 - s/r).
        let n_min = (PI / (1.0 - chord / r).acos()).ceil() as usize;
        let segments = angles.len();
        assert!(segments >= n_min, "{segments} < {n_min}");
        // Not pathological: at most twice the minimum segments, and a triangle
        // count linear in them (2 per lateral segment + 2 caps of ~1 each).
        assert!(
            segments <= 2 * n_min,
            "{segments} segments vs minimum {n_min}"
        );
        let lateral = tris
            .iter()
            .filter(|t| t[0][2] != t[1][2] || t[1][2] != t[2][2])
            .count();
        eprintln!(
            "chord {chord}: n_min {n_min} segments {segments} triangles {} lateral {lateral}",
            tris.len()
        );
        // Truck meshes the lateral face on an isotropic parameter grid (rows along
        // the straight rulings too: observed ~segments/2.7 rows), so the count is
        // ~ segments^2, not ~ segments. Bound it by that grid: 2 triangles per
        // cell of a (2 n_min) x (2 n_min) grid, i.e. 8 n_min^2.
        assert!(tris.len() <= 8 * n_min * n_min, "{} triangles", tris.len());
        // An inscribed polygon whose sagitta is <= chord contains the circle of
        // radius r - chord: pi (r - s)^2 h <= V <= pi r^2 h (f32 slack 1e-3).
        let exact = PI * r * r * 10.0;
        let v = signed_volume(&tris);
        let inner = PI * (r - chord).powi(2) * 10.0;
        assert!(
            v <= exact + 1e-3 && v >= inner - 1e-3,
            "{v} vs [{inner}, {exact}]"
        );
        let rep = resp.export.unwrap();
        assert_eq!(rep.bodies[0].triangles, tris.len());
        assert!(rep.units.starts_with("mm"));
        results.push((chord, max_sagitta, segments, tris.len(), v));
    }
    let (a, b) = (results[0], results[1]);
    assert!(
        b.1 < a.1,
        "tighter chord must reduce the sagitta: {results:?}"
    );
    assert!(b.4 > a.4, "and the inscribed volume error: {results:?}");
    // Segments scale ~ 1/sqrt(chord): 4x tighter -> ~2x the segments (allow 3x).
    assert!(b.2 as f64 <= 3.0 * a.2 as f64, "{results:?}");
    // Triangles: no worse than linear in 1/chord (4x tighter -> at most 6x).
    assert!(b.3 as f64 <= 6.0 * a.3 as f64, "{results:?}");
    std::fs::write(
        dir.join("accuracy.json"),
        format!(
            "{{\"results\":[{}]}}\n",
            results
                .iter()
                .map(|(c, sg, n, t, v)| format!(
                    "{{\"chord\":{c},\"max_sagitta\":{sg},\"segments\":{n},\"triangles\":{t},\"volume\":{v}}}"
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    )
    .unwrap();
}

#[test]
fn angle_limit_refines_the_chord_or_refuses() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch(
        &mut s,
        SketchPlane::WorldXY,
        SketchTemplate::CenteredCircle {
            diameter: 20.0,
            center: None,
        },
    );
    extrude(&mut s, sk, 10.0, ExtrudeOp::NewBody, None);
    let dir = out_dir("angle");
    let p = dir.join("cyl_angle.stl");
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: p.clone(),
            chord: Some(0.5),
            bodies: None,
            print: PrintOptions {
                angle_deg: Some(5.0),
                ..Default::default()
            },
        },
    );
    let b = &r.export.unwrap().bodies[0];
    assert!(b.max_facet_angle_deg.unwrap() <= 5.0, "{b:?}");
    assert!(b.chord.unwrap() < 0.5);
    // Without the angle the chord 0.5 gives coarser facets (angle > 5 degrees).
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: p.clone(),
            chord: Some(0.5),
            bodies: None,
            print: PrintOptions::default(),
        },
    );
    assert!(r.export.unwrap().bodies[0].max_facet_angle_deg.unwrap() > 5.0);
    // Unreachable / invalid limits are refused and nothing is written.
    let q = dir.join("never.stl");
    for a in [1e-4, 0.0, 95.0, f64::NAN] {
        let r = s.execute(&Command::ExportStl {
            path: q.clone(),
            chord: Some(0.5),
            bodies: None,
            print: PrintOptions {
                angle_deg: Some(a),
                ..Default::default()
            },
        });
        assert!(!r.ok, "angle {a}");
        assert!(!q.exists());
    }
}

#[test]
fn enclosure_lid_placement_layout_and_fit() {
    let mut s = Session::new();
    let (base, lid, plate) = enclosure(&mut s);
    let before = s.summary();
    let dir = out_dir("enclosure");
    // As modeled: lid spans z = [-3, 2].
    let p = dir.join("as_modeled.3mf");
    let r = ok(
        &mut s,
        Command::Export3mf {
            path: p.clone(),
            chord: None,
            bodies: None,
            unit: None,
            print: PrintOptions::default(),
        },
    );
    let rep = r.export.unwrap();
    let lid_rep = &rep.bodies[1];
    assert!(approx(lid_rep.bounds_min[2], -3.0, 1e-9) && approx(lid_rep.bounds_max[2], 2.0, 1e-9));
    let unplaced_vol = [
        rep.bodies[0].mesh_check.mesh_volume,
        lid_rep.mesh_check.mesh_volume,
    ];
    assert!(approx(unplaced_vol[0], 19592.0, 1e-6), "{unplaced_vol:?}");
    assert!(approx(unplaced_vol[1], 9404.0, 1e-6), "{unplaced_vol:?}");

    // Placed: base on its floor, lid plate-down (lip up), side by side on the P1S bed.
    let placement = Placement {
        bodies: vec![BodyPlacement {
            body: lid,
            orient: Orient::FaceDown(face(plate, FaceRole::ExtrudeEndCap)),
        }],
        layout: Some(Layout { gap: 10.0 }),
    };
    let opts = PrintOptions {
        placement: Some(placement.clone()),
        bed: Some(BedSpec::Preset {
            preset: BedPreset::BambuP1s,
        }),
        ..Default::default()
    };
    let p = dir.join("placed.3mf");
    let r = ok(
        &mut s,
        Command::Export3mf {
            path: p.clone(),
            chord: None,
            bodies: Some(vec![base, lid]),
            unit: None,
            print: opts.clone(),
        },
    );
    let rep = r.export.unwrap();
    let (b, l) = (&rep.bodies[0], &rep.bodies[1]);
    for (x, dims, vol) in [
        (b, [80.0, 50.0, 25.0], 19592.0),
        (l, [80.0, 50.0, 5.0], 9404.0),
    ] {
        assert!(approx(x.bounds_min[2], 0.0, 1e-12), "{x:?}");
        for i in 0..3 {
            assert!(approx(x.dims[i], dims[i], 1e-9), "{x:?}");
        }
        assert!(approx(x.mesh_check.mesh_volume, vol, 1e-6));
        assert!(x.mesh_check.is_valid_solid());
        let f = x.fit.as_ref().unwrap();
        assert!(f.fits_size && f.within_bed == Some(true), "{f:?}");
        let rot = x.transform.unwrap().rotation;
        let det = rot[0][0] * (rot[1][1] * rot[2][2] - rot[1][2] * rot[2][1])
            - rot[0][1] * (rot[1][0] * rot[2][2] - rot[1][2] * rot[2][0])
            + rot[0][2] * (rot[1][0] * rot[2][1] - rot[1][1] * rot[2][0]);
        assert!(approx(det, 1.0, 1e-12));
    }
    // Lid upside down: the lip is now above the plate (z in [2, 5]), proper rotation.
    assert!(approx(l.transform.unwrap().rotation[2][2], -1.0, 1e-12));
    // Layout: base x = [83, 163]... centred on 128: total 80 + 10 + 80 = 170.
    assert!(
        approx(b.bounds_min[0], 128.0 - 85.0, 1e-9) && approx(l.bounds_min[0], 128.0 + 5.0, 1e-9)
    );
    assert!(approx(b.bounds_min[1], 128.0 - 25.0, 1e-9));
    assert!(rep.layout_fit.as_ref().unwrap().within_bed == Some(true));
    assert!(r.message.as_deref().unwrap().contains("document unchanged"));
    // The document is untouched: same revision, same bodies and measurements.
    let after = s.summary();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.bodies, after.bodies);
    assert!(!after.dirty || before.dirty);

    // Same placement as separate STL files: one per body, sanitized names, and
    // the file bytes (read independently) carry the placed geometry.
    let p = dir.join("enclosure.stl");
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: p.clone(),
            chord: None,
            bodies: Some(vec![base, lid]),
            print: PrintOptions {
                separate_files: true,
                ..opts.clone()
            },
        },
    );
    let rep = r.export.unwrap();
    let files: Vec<PathBuf> = rep.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        files,
        vec![
            dir.join("enclosure-Base.stl"),
            dir.join("enclosure-Lid.stl")
        ]
    );
    for (f, vol, dims) in [
        (&files[0], 19592.0, [80.0, 50.0, 25.0]),
        (&files[1], 9404.0, [80.0, 50.0, 5.0]),
    ] {
        let t = read_stl(f);
        assert!(closed_and_oriented(&t));
        // f32 coordinates near 128..213 mm: volume within 1e-3 relative.
        assert!(approx(signed_volume(&t), vol, vol * 1e-5), "{f:?}");
        let (lo, hi) = tri_bounds(&t);
        assert!(approx(lo[2], 0.0, 1e-6));
        for i in 0..3 {
            assert!(approx(hi[i] - lo[i], dims[i], 1e-4));
        }
        assert_eq!(
            std::fs::metadata(f).unwrap().len(),
            rep.files.iter().find(|x| &x.path == f).unwrap().bytes
        );
    }
    assert!(!p.exists(), "separate_files writes only the per-body files");

    // A face that is not lowest after rotation is refused: the cavity floor
    // (z = 2, normal +Z) turned down would leave the walls 23 mm below it.
    let cavity = s
        .summary()
        .features
        .iter()
        .find(|f| matches!(f.operation, Some(f420_document::Operation::Cut { target }) if target == base))
        .unwrap()
        .id;
    let q = dir.join("bad_face.3mf");
    let bad = |body: BodyId, f: FaceRef| Command::Export3mf {
        path: q.clone(),
        chord: None,
        bodies: Some(vec![body]),
        unit: None,
        print: PrintOptions {
            placement: Some(Placement {
                bodies: vec![BodyPlacement {
                    body,
                    orient: Orient::FaceDown(f),
                }],
                layout: None,
            }),
            ..Default::default()
        },
    };
    let r = s.execute(&bad(base, face(cavity, FaceRole::ExtrudeEndCap)));
    assert!(!r.ok && !q.exists(), "{r:?}");
    let e = format!("{:?}", r.error);
    assert!(
        e.contains("would not rest on the bed") && e.contains("23.0000"),
        "{e}"
    );
    // The lid's start cap is no longer a single referenceable face once the lip
    // is joined on it: refused by name, not resolved to some other face.
    let r = s.execute(&bad(lid, face(plate, FaceRole::ExtrudeStartCap)));
    assert!(!r.ok && !q.exists(), "{r:?}");
    assert!(format!("{:?}", r.error).contains("not a referenceable planar face"));
    // A face of another body is refused.
    let r = s.execute(&Command::Export3mf {
        path: q.clone(),
        chord: None,
        bodies: Some(vec![base]),
        unit: None,
        print: PrintOptions {
            placement: Some(Placement {
                bodies: vec![BodyPlacement {
                    body: base,
                    orient: Orient::FaceDown(face(plate, FaceRole::ExtrudeEndCap)),
                }],
                layout: None,
            }),
            ..Default::default()
        },
    });
    assert!(!r.ok && !q.exists(), "{r:?}");

    // A bed too small: export succeeds, fit reported and warned.
    let r = ok(
        &mut s,
        Command::Export3mf {
            path: q.clone(),
            chord: None,
            bodies: Some(vec![base]),
            unit: None,
            print: PrintOptions {
                bed: Some(BedSpec::Manual {
                    width: 60.0,
                    depth: 60.0,
                    height: 20.0,
                }),
                ..Default::default()
            },
        },
    );
    let f = r.export.as_ref().unwrap().bodies[0].fit.clone().unwrap();
    assert!(!f.fits_size && f.within_bed.is_none(), "{f:?}");
    assert!(f.detail.contains("x 80.000 > 60.000") && f.detail.contains("z 25.000 > 20.000"));
    assert!(r.message.unwrap().contains("too large"));
}

#[test]
fn hidden_bodies_follow_the_stated_policy() {
    let mut s = Session::new();
    let (base, lid, _) = enclosure(&mut s);
    ok(
        &mut s,
        Command::SetBodyVisible {
            body: lid,
            visible: false,
        },
    );
    let dir = out_dir("hidden");
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: dir.join("default.stl"),
            chord: None,
            bodies: None,
            print: PrintOptions::default(),
        },
    );
    let rep = r.export.unwrap();
    assert_eq!(
        rep.bodies.iter().map(|b| b.id).collect::<Vec<_>>(),
        vec![base]
    );
    assert_eq!(
        rep.selection,
        "default: all visible bodies; hidden not exported: 'Lid'"
    );
    assert!(r.message.unwrap().contains("hidden not exported: 'Lid'"));
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: dir.join("explicit.stl"),
            chord: None,
            bodies: Some(vec![lid]),
            print: PrintOptions::default(),
        },
    );
    let rep = r.export.unwrap();
    assert_eq!(rep.selection, "explicit selection, includes hidden: 'Lid'");
    assert!(!rep.bodies[0].visible);
}

fn write_binary_stl(path: &Path, tris: &[[[f32; 3]; 3]]) {
    let mut b = vec![0u8; 80];
    b.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        b.extend_from_slice(&[0u8; 12]);
        for p in t {
            for c in p {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0u8; 2]);
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn open_mesh_bodies_need_explicit_reference_mode() {
    let dir = out_dir("open_mesh");
    // Tetrahedron with one face missing: an open reference mesh.
    let p0 = [0.0f32, 0.0, 0.0];
    let (p1, p2, p3) = (
        [10.0f32, 0.0, 0.0],
        [0.0f32, 10.0, 0.0],
        [0.0f32, 0.0, 10.0],
    );
    let src = dir.join("open_tet.stl");
    write_binary_stl(&src, &[[p0, p2, p1], [p0, p1, p3], [p0, p3, p2]]);
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::ImportStl {
            path: src,
            units_to_mm: Some(1.0),
        },
    );
    let out = dir.join("out.stl");
    for (format, path) in [("stl", out.clone()), ("3mf", dir.join("out.3mf"))] {
        let cmd = if format == "stl" {
            Command::ExportStl {
                path: path.clone(),
                chord: None,
                bodies: None,
                print: PrintOptions::default(),
            }
        } else {
            Command::Export3mf {
                path: path.clone(),
                chord: None,
                bodies: None,
                unit: None,
                print: PrintOptions::default(),
            }
        };
        let r = s.execute(&cmd);
        assert!(!r.ok, "{format}");
        let e = format!("{:?}", r.error.unwrap());
        assert!(
            e.contains("3 boundary (open) edges") && e.contains("allow_open_meshes"),
            "{e}"
        );
        assert_eq!(r.mesh_check.unwrap().boundary_edges, 3);
        assert!(!path.exists(), "nothing written");
    }
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: out.clone(),
            chord: None,
            bodies: None,
            print: PrintOptions {
                allow_open_meshes: true,
                ..Default::default()
            },
        },
    );
    let rep = r.export.unwrap();
    assert_eq!(rep.bodies[0].kind, "mesh");
    assert_eq!(
        rep.bodies[0].label,
        "open reference mesh (not a printable solid)"
    );
    assert!(rep.warnings[0].contains("open reference mesh"));
    assert_eq!(read_stl(&out).len(), 3);
}

#[test]
fn json_shape_is_backward_compatible() {
    // Run-3 JSON (no print fields) parses with defaults and re-serializes unchanged.
    let old = r#"{"cmd":"export_stl","path":"a.stl","chord":0.05,"bodies":[1]}"#;
    let c: Command = serde_json::from_str(old).unwrap();
    assert!(matches!(&c, Command::ExportStl { print, .. } if *print == PrintOptions::default()));
    assert_eq!(
        serde_json::to_value(&c).unwrap(),
        serde_json::from_str::<serde_json::Value>(old).unwrap()
    );
    let old3 = r#"{"cmd":"export_3mf","path":"a.3mf","chord":null,"unit":"inch"}"#;
    let c: Command = serde_json::from_str(old3).unwrap();
    assert!(matches!(&c, Command::Export3mf { print, .. } if *print == PrintOptions::default()));
    // New fields: documented JSON shape.
    let new = r#"{"cmd":"export_3mf","path":"p.3mf","chord":0.01,"bodies":[1,2],"unit":"millimeter",
        "angle_deg":10.0,
        "placement":{"bodies":[{"body":2,"orient":{"face_down":{"feature":5,"role":"ExtrudeEndCap"}}},
                                {"body":1,"orient":"as_modeled"}],
                     "layout":{"gap":10.0}},
        "bed":{"preset":"bambu_p1s"},"allow_open_meshes":true}"#;
    let c: Command = serde_json::from_str(new).unwrap();
    let Command::Export3mf { print, .. } = &c else {
        panic!()
    };
    assert_eq!(print.angle_deg, Some(10.0));
    assert_eq!(
        print.placement.as_ref().unwrap().bodies[0].orient,
        Orient::FaceDown(FaceRef {
            feature: FeatureId(5),
            role: FaceRole::ExtrudeEndCap
        })
    );
    assert_eq!(
        print.bed,
        Some(BedSpec::Preset {
            preset: BedPreset::BambuP1s
        })
    );
    let back: Command = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
    assert_eq!(back, c);
    let manual = r#"{"cmd":"export_stl","path":"a.stl","separate_files":true,
        "bed":{"width":200,"depth":180,"height":150},"placement":{}}"#;
    let c: Command = serde_json::from_str(manual).unwrap();
    let Command::ExportStl { print, .. } = &c else {
        panic!()
    };
    assert!(print.separate_files && print.placement == Some(Placement::default()));
    assert_eq!(
        print.bed,
        Some(BedSpec::Manual {
            width: 200.0,
            depth: 180.0,
            height: 150.0
        })
    );
}

#[test]
fn separate_file_names_are_sanitized_and_unique() {
    use f420_commands::print::sanitize_file_component;
    assert_eq!(sanitize_file_component("Lid (2)"), "Lid_2");
    assert_eq!(sanitize_file_component("../../etc/passwd"), "etc_passwd");
    assert_eq!(sanitize_file_component("Lid v1.2"), "Lid_v1_2");
    assert_eq!(sanitize_file_component("  "), "body");
    assert_eq!(sanitize_file_component("Käse/Box"), "K_se_Box");
    let mut s = Session::new();
    let (base, lid, _) = enclosure(&mut s);
    ok(
        &mut s,
        Command::RenameBody {
            body: lid,
            name: "base".into(),
        },
    );
    let dir = out_dir("names");
    let r = ok(
        &mut s,
        Command::ExportStl {
            path: dir.join("Part A.stl"),
            chord: None,
            bodies: Some(vec![base, lid]),
            print: PrintOptions {
                separate_files: true,
                ..Default::default()
            },
        },
    );
    let names: Vec<String> = r
        .export
        .unwrap()
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "Part_A-Base.stl".to_string(),
            format!("Part_A-base-{lid}.stl")
        ]
    );
    // 3MF refuses separate_files (it already keeps objects apart).
    let r = s.execute(&Command::Export3mf {
        path: dir.join("x.3mf"),
        chord: None,
        bodies: None,
        unit: None,
        print: PrintOptions {
            separate_files: true,
            ..Default::default()
        },
    });
    assert!(!r.ok);
}
