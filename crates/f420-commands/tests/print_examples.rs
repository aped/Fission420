//! Print exports of the real printable examples (`f420_commands::printables`):
//! enclosure baseline and variant (base + lid), clearance coupon (plate + peg),
//! mounting bracket. Writes placed STL / 3MF and per-body STEP plus the
//! expected-value specs consumed by `tools/print/validate_examples.sh`
//! (Python mesh checker, FreeCAD, Bambu Studio). Expected values are analytic
//! (parameters, volume formulas, the documented layout rule), never copied
//! from the export report.
//!
//! Run (the driver does this):
//! `F420_PRINT_EXAMPLES_OUT=/abs/dir cargo test -p f420-commands --release --test print_examples -- --ignored`
#![allow(clippy::needless_range_loop)]
use f420_commands::print::{
    BedPreset, BedSpec, BodyPlacement, Layout, Orient, Placement, PrintOptions,
};
use f420_commands::printables::{
    self, COUPON, COUPON_CLEARANCES, ENCLOSURE_BASELINE, ENCLOSURE_VARIANT, Enclosure,
};
use f420_commands::{Command, Response, Session};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId};
use serde_json::{Value, json};
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

/// Default export chord (`tol::MESH_CHORD`), mm: the sagitta bound of curved faces.
const CHORD: f64 = 0.01;
/// f32 coordinate rounding allowance on mesh volumes (mm^3) and lengths (mm).
const F32_VOL: f64 = 0.5;
const F32_LEN: f64 = 1e-4;
/// P1S bed centre (printable_area 0..256) and layout gap used here.
const BED_C: f64 = 128.0;
const GAP: f64 = 10.0;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?} {:?}", r.error, r.message);
    r
}

fn replay(s: &mut Session, steps: &[f420_commands::scenarios::Step]) {
    for (i, st) in steps.iter().enumerate() {
        let r = s.execute(&st.cmd);
        assert_eq!(r.ok, st.expect_ok, "step {i} {:?}: {:?}", st.cmd, r.error);
    }
}

fn opts(face_down: Option<(u32, u32)>, separate: bool) -> PrintOptions {
    PrintOptions {
        placement: Some(Placement {
            bodies: face_down
                .map(|(body, feature)| {
                    vec![BodyPlacement {
                        body: BodyId(body),
                        orient: Orient::FaceDown(FaceRef {
                            feature: FeatureId(feature),
                            role: FaceRole::ExtrudeEndCap,
                        }),
                    }]
                })
                .unwrap_or_default(),
            layout: Some(Layout { gap: GAP }),
        }),
        bed: Some(BedSpec::Preset {
            preset: BedPreset::BambuP1s,
        }),
        separate_files: separate,
        ..Default::default()
    }
}

/// STL (separate files) + 3MF + one STEP per body; returns the 3MF report.
fn export_all(
    s: &mut Session,
    out: &Path,
    stem: &str,
    bodies: &[u32],
    face_down: Option<(u32, u32)>,
    separate_stl: bool,
) -> Value {
    let ids: Vec<BodyId> = bodies.iter().map(|b| BodyId(*b)).collect();
    let r = ok(
        s,
        Command::ExportStl {
            path: out.join(format!("{stem}.stl")),
            chord: None,
            bodies: Some(ids.clone()),
            print: opts(face_down, separate_stl),
        },
    );
    let stl = serde_json::to_value(r.export.unwrap()).unwrap();
    let r = ok(
        s,
        Command::Export3mf {
            path: out.join(format!("{stem}.3mf")),
            chord: None,
            bodies: Some(ids.clone()),
            unit: None,
            print: opts(face_down, false),
        },
    );
    let tmf = serde_json::to_value(r.export.unwrap()).unwrap();
    let names: Vec<String> = s
        .summary()
        .bodies
        .iter()
        .filter(|b| ids.contains(&b.id))
        .map(|b| b.name.clone())
        .collect();
    for b in &ids {
        ok(
            s,
            Command::ExportStep {
                path: out.join(format!("{stem}-body{}.step", b.0)),
                bodies: Some(vec![*b]),
            },
        );
    }
    json!({"stl": stl, "3mf": tmf, "names": names})
}

fn sanitize(n: &str) -> String {
    f420_commands::print::sanitize_file_component(n)
}

/// Expected placed bounds of a row of boxes laid out along X (export order),
/// centred on the bed centre: `sizes[i] = [dx, dy, dz]`.
fn row(sizes: &[[f64; 3]]) -> Vec<[[f64; 3]; 2]> {
    let total: f64 = sizes.iter().map(|s| s[0]).sum::<f64>() + GAP * (sizes.len() - 1) as f64;
    let mut x = BED_C - total / 2.0;
    sizes
        .iter()
        .map(|s| {
            let b = [
                [x, BED_C - s[1] / 2.0, 0.0],
                [x + s[0], BED_C + s[1] / 2.0, s[2]],
            ];
            x += s[0] + GAP;
            b
        })
        .collect()
}

fn obj(volume: (f64, f64), dims: [f64; 3], dims_tol: f64, b: [[f64; 3]; 2], b_tol: f64) -> Value {
    json!({
        "closed": true,
        "volume": [volume.0, volume.1],
        "dims": [dims, dims_tol],
        "min_z": [0.0, 1e-4],
        "bounds": [b[0], b[1], b_tol],
    })
}

const BED: [[f64; 2]; 2] = [[0.0, 0.0], [256.0, 256.0]];

fn enclosure_spec(p: &Enclosure, stem: &str, names: &[String], spec: &mut Value, step: &mut Value) {
    let (l, w, h) = (p.outer_length, p.outer_width, p.outer_height);
    let lid_h = p.lid_thickness + p.lip_depth;
    let b = row(&[[l, w, h], [l, w, lid_h]]);
    let base = obj(
        (p.base_volume(), F32_VOL),
        [l, w, h],
        F32_LEN,
        b[0],
        F32_LEN,
    );
    let lid = obj(
        (p.lid_volume(), F32_VOL),
        [l, w, lid_h],
        F32_LEN,
        b[1],
        F32_LEN,
    );
    let f = spec["files"].as_object_mut().unwrap();
    for (i, o) in [&base, &lid].into_iter().enumerate() {
        let mut o = o.clone();
        o["objects"] = json!(1);
        o["within_bed"] = json!([BED[0], BED[1], 250.0]);
        f.insert(format!("{stem}-{}.stl", sanitize(&names[i])), o);
    }
    f.insert(
        format!("{stem}.3mf"),
        json!({"unit": "millimeter", "objects": 2, "names": names, "closed": true,
               "min_z": [0.0, 1e-4], "within_bed": [BED[0], BED[1], 250.0],
               "per_object": {names[0].clone(): base, names[1].clone(): lid}}),
    );
    // STEP (exact, as modeled): base x[0,L] y[0,W] z[0,H]; lid plate centred
    // at y = -(W/2 + lid_gap), z [-lip_depth, t].
    let ly = w / 2.0 + p.lid_gap;
    let s = step.as_object_mut().unwrap();
    s.insert(
        format!("{stem}-body1.step"),
        json!({"volume": p.base_volume(), "bounds": [0.0, 0.0, 0.0, l, w, h]}),
    );
    s.insert(
        format!("{stem}-body2.step"),
        json!({"volume": p.lid_volume(),
               "bounds": [0.0, -ly - w / 2.0, -p.lip_depth, l, -ly + w / 2.0, p.lid_thickness]}),
    );
}

/// Inscribed-polygon bounds of a tessellated circle of radius `r` with sagitta
/// <= CHORD: area in [pi (r - s)^2, pi r^2].
fn circle_area_range(r: f64) -> (f64, f64) {
    (PI * (r - CHORD).powi(2), PI * r * r)
}

/// `(centre, half-width)` of an interval, widened by the f32 allowance.
fn mid(lo: f64, hi: f64) -> (f64, f64) {
    ((lo + hi) / 2.0, (hi - lo) / 2.0 + F32_VOL)
}

#[test]
#[ignore = "writes example exports; run by tools/print/validate_examples.sh"]
fn export_printable_examples() {
    let out = PathBuf::from(
        std::env::var("F420_PRINT_EXAMPLES_OUT").expect("set F420_PRINT_EXAMPLES_OUT"),
    );
    std::fs::create_dir_all(&out).unwrap();
    let mut spec = json!({"files": {}});
    let mut step = json!({});
    let mut reports = json!({});
    use printables::enclosure_ids::{LID, LID_BODY};

    // Enclosure baseline, then the variant through its user parameters.
    let mut s = Session::new();
    replay(
        &mut s,
        &printables::enclosure_workflow(&out.join("enclosure.f420.json")),
    );
    let rev = s.model().unwrap().revision;
    let rep = export_all(
        &mut s,
        &out,
        "enclosure_baseline",
        &[1, 2],
        Some((LID_BODY, LID)),
        true,
    );
    assert_eq!(
        s.model().unwrap().revision,
        rev,
        "export must not change the document"
    );
    let names: Vec<String> = serde_json::from_value(rep["names"].clone()).unwrap();
    enclosure_spec(
        &ENCLOSURE_BASELINE,
        "enclosure_baseline",
        &names,
        &mut spec,
        &mut step,
    );
    reports["enclosure_baseline"] = rep;
    for c in ENCLOSURE_VARIANT.set_parameters() {
        ok(&mut s, c);
    }
    let rep = export_all(
        &mut s,
        &out,
        "enclosure_variant",
        &[1, 2],
        Some((LID_BODY, LID)),
        true,
    );
    enclosure_spec(
        &ENCLOSURE_VARIANT,
        "enclosure_variant",
        &names,
        &mut spec,
        &mut step,
    );
    reports["enclosure_variant"] = rep;

    // Clearance coupon: plate 70 x 20 x t with four through holes, peg d x h.
    let (d, t, h, sp) = COUPON;
    let len = sp * COUPON_CLEARANCES.len() as f64 + 10.0;
    let mut s = Session::new();
    replay(
        &mut s,
        &printables::coupon_workflow(&out.join("coupon.f420.json")),
    );
    let rep = export_all(&mut s, &out, "coupon", &[1, 2], None, true);
    let cn: Vec<String> = serde_json::from_value(rep["names"].clone()).unwrap();
    reports["coupon"] = rep;
    let holes_exact: f64 = COUPON_CLEARANCES
        .iter()
        .map(|c| PI * ((d + 2.0 * c) / 2.0).powi(2) * t)
        .sum();
    let holes_mesh = COUPON_CLEARANCES.iter().fold((0.0, 0.0), |acc, c| {
        let (lo, hi) = circle_area_range((d + 2.0 * c) / 2.0);
        (acc.0 + lo * t, acc.1 + hi * t)
    });
    let plate_exact = len * 20.0 * t - holes_exact;
    let plate_mesh = mid(len * 20.0 * t - holes_mesh.1, len * 20.0 * t - holes_mesh.0);
    let (pa_lo, pa_hi) = circle_area_range(d / 2.0);
    let peg_mesh = mid(pa_lo * h, pa_hi * h);
    // Peg footprint in the mesh: [d - 2s, d] per axis; the row is laid out
    // from mesh bounds, so positions carry that uncertainty (tolerance CHORD).
    let b = row(&[[len, 20.0, t], [d, d, h]]);
    let peg_dims_tol = CHORD + F32_LEN;
    let plate = obj(plate_mesh, [len, 20.0, t], F32_LEN, b[0], CHORD + F32_LEN);
    let mut peg = obj(peg_mesh, [d, d, h], peg_dims_tol, b[1], CHORD + F32_LEN);
    peg["dims"] = json!([[d - CHORD, d - CHORD, h], peg_dims_tol]);
    {
        let f = spec["files"].as_object_mut().unwrap();
        for (i, o) in [&plate, &peg].into_iter().enumerate() {
            let mut o = o.clone();
            o["objects"] = json!(1);
            o["within_bed"] = json!([BED[0], BED[1], 250.0]);
            f.insert(format!("coupon-{}.stl", sanitize(&cn[i])), o);
        }
        f.insert(
            "coupon.3mf".into(),
            json!({"unit": "millimeter", "objects": 2, "names": cn, "closed": true,
                   "min_z": [0.0, 1e-4], "within_bed": [BED[0], BED[1], 250.0],
                   "per_object": {cn[0].clone(): plate, cn[1].clone(): peg}}),
        );
        let st = step.as_object_mut().unwrap();
        st.insert(
            "coupon-body1.step".into(),
            json!({"volume": plate_exact, "bounds": [0.0, 0.0, 0.0, len, 20.0, t]}),
        );
        st.insert(
            "coupon-body2.step".into(),
            json!({"volume": PI * (d / 2.0).powi(2) * h,
                   "bounds": [len / 2.0 - d / 2.0, -15.0 - d / 2.0, 0.0, len / 2.0 + d / 2.0, -15.0 + d / 2.0, h]}),
        );
    }

    // Mounting bracket (one body, as modeled orientation, centred on the bed).
    let mut s = Session::new();
    replay(
        &mut s,
        &printables::bracket_workflow(&out.join("bracket.f420.json")),
    );
    let rep = export_all(&mut s, &out, "bracket", &[1], None, false);
    let bn: Vec<String> = serde_json::from_value(rep["names"].clone()).unwrap();
    reports["bracket"] = rep;
    let r = 5.5 / 2.0;
    let solid = 60.0 * 40.0 * 5.0 + 50.0 * 5.0 * 35.0 - 0.5 * 60.0;
    let bracket_exact = solid - 4.0 * PI * r * r * 5.0;
    let (ha_lo, ha_hi) = circle_area_range(r);
    let bracket_mesh = mid(solid - 4.0 * ha_hi * 5.0, solid - 4.0 * ha_lo * 5.0);
    let b = row(&[[60.0, 40.0, 40.0]]);
    let br = obj(bracket_mesh, [60.0, 40.0, 40.0], F32_LEN, b[0], F32_LEN);
    {
        let f = spec["files"].as_object_mut().unwrap();
        let mut o = br.clone();
        o["objects"] = json!(1);
        o["within_bed"] = json!([BED[0], BED[1], 250.0]);
        f.insert("bracket.stl".into(), o);
        f.insert(
            "bracket.3mf".into(),
            json!({"unit": "millimeter", "objects": 1, "names": bn, "closed": true,
                   "min_z": [0.0, 1e-4], "per_object": {bn[0].clone(): br}}),
        );
        step.as_object_mut().unwrap().insert(
            "bracket-body1.step".into(),
            json!({"volume": bracket_exact, "bounds": [0.0, 0.0, 0.0, 60.0, 40.0, 40.0]}),
        );
    }

    // In-test cross-check of the reports against the same analytic values.
    let check = |rep: &Value, i: usize, v: (f64, f64), name: &str| {
        let got = rep["3mf"]["bodies"][i]["mesh_check"]["mesh_volume"]
            .as_f64()
            .unwrap();
        assert!(
            (got - v.0).abs() <= v.1,
            "{name}: mesh volume {got} vs {v:?}"
        );
        let mc = &rep["3mf"]["bodies"][i]["mesh_check"];
        assert_eq!(mc["closed_edges"], json!(true), "{name}");
        assert_eq!(mc["nonmanifold_vertices"], json!(0), "{name}");
        assert_eq!(
            rep["3mf"]["bodies"][i]["fit"]["within_bed"],
            json!(true),
            "{name}"
        );
    };
    for (key, p) in [
        ("enclosure_baseline", ENCLOSURE_BASELINE),
        ("enclosure_variant", ENCLOSURE_VARIANT),
    ] {
        check(&reports[key], 0, (p.base_volume(), 1e-6), key);
        check(&reports[key], 1, (p.lid_volume(), 1e-6), key);
        let tf = &reports[key]["3mf"]["bodies"][1]["transform"]["rotation"];
        assert_eq!(
            tf[2][2].as_f64().unwrap(),
            -1.0,
            "{key}: lid turned over (lip up)"
        );
    }
    check(&reports["coupon"], 0, plate_mesh, "coupon plate");
    check(&reports["coupon"], 1, peg_mesh, "coupon peg");
    check(&reports["bracket"], 0, bracket_mesh, "bracket");

    let w = |name: &str, v: &Value| {
        std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap()).unwrap()
    };
    w("spec.json", &spec);
    w("step_expected.json", &step);
    w("reports.json", &reports);
}
