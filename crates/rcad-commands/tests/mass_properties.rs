//! Materials and mass properties ("property oracle"):
//! every expected value is computed here in closed form, independently of the
//! kernel's boundary integrals. Units: mm, g, g/cm³, g·mm²; world frame.
mod support;
use rcad_commands::mass::{BodyMass, MassReport};
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn report(s: &mut Session, bodies: Option<&[u32]>) -> MassReport {
    let v = match bodies {
        Some(b) => json!({"cmd": "mass_properties", "bodies": b}),
        None => json!({"cmd": "mass_properties"}),
    };
    ok(s, v).mass_properties.expect("mass report")
}

fn body(r: &MassReport, id: u32) -> &BodyMass {
    r.bodies
        .iter()
        .find(|b| b.body.0 == id)
        .expect("body in report")
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn assert_vec(label: &str, got: [f64; 3], want: [f64; 3], tol: f64) {
    for i in 0..3 {
        assert!(close(got[i], want[i], tol), "{label}: {got:?} vs {want:?}");
    }
}

/// Box 20 (X) × 30 (Y) × 40 (Z) with its base centred at (10, 15, 0).
fn box_doc(s: &mut Session) {
    ok(s, json!({"cmd": "new_document"}));
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 20.0, "height": 30.0, "center": [10.0, 15.0]}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": 1, "distance": 40.0}));
}

/// Box inertia about its centre: m (b² + c²) / 12 etc.
fn box_inertia(m: f64, a: f64, b: f64, c: f64) -> [f64; 3] {
    [
        m * (b * b + c * c) / 12.0,
        m * (a * a + c * c) / 12.0,
        m * (a * a + b * b) / 12.0,
    ]
}

#[test]
fn box_oracle_mass_centroid_and_translation() {
    let mut s = Session::new();
    box_doc(&mut s);
    // No material yet: volume and centroid, no mass, no total.
    let r = report(&mut s, None);
    let b = body(&r, 1);
    assert!(b.mass.is_none() && r.total.is_none());
    assert_eq!(r.total_missing.len(), 1, "{:?}", r.total_missing);
    assert_eq!(r.frame, "world");
    assert!(r.units.contains("g·mm²"));
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "density": 1.2, "name": "Resin"}),
    );
    let sb = s.summary().bodies[0].clone();
    assert_eq!(sb.material.as_ref().unwrap().density, 1.2);
    assert!(close(sb.mass.unwrap(), 28.8, 1e-9));
    let r = report(&mut s, None);
    let b = body(&r, 1);
    let p = b.properties.as_ref().unwrap();
    assert!(b.exact);
    // 24 cm³ = 24000 mm³; 24 cm³ × 1.2 g/cm³ = 28.8 g.
    assert!(close(p.volume, 24_000.0, 1e-6), "{}", p.volume);
    assert!(close(b.mass.unwrap(), 28.8, 1e-9), "{:?}", b.mass);
    assert_vec("centroid", p.centroid, [10.0, 15.0, 20.0], 1e-9);
    let i = b.inertia.unwrap();
    let want = box_inertia(28.8, 20.0, 30.0, 40.0);
    for k in 0..3 {
        assert!(close(i[k][k], want[k], 1e-9 * want[k]), "{i:?} vs {want:?}");
    }
    assert!(i[0][1].abs() < 1e-9 * want[0] && i[1][2].abs() < 1e-9 * want[0]);
    let t = r.total.unwrap();
    assert!(close(t.mass, 28.8, 1e-9) && t.exact);
    // Known translation: the centre of mass moves by exactly the translation; mass and
    // central inertia do not change.
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 1, "translate": [100.0, -50.0, 7.5]}),
    );
    let r = report(&mut s, None);
    let b = body(&r, 1);
    assert_vec(
        "moved centroid",
        b.center_of_mass().unwrap(),
        [110.0, -35.0, 27.5],
        1e-9,
    );
    assert!(close(b.mass.unwrap(), 28.8, 1e-9));
    // A 90° rotation about Z through the origin then: (x, y) -> (-y, x), and the
    // inertia's X and Y entries swap.
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 1, "angle_deg": 90.0}),
    );
    let r = report(&mut s, None);
    let b = body(&r, 1);
    assert_vec(
        "rotated centroid",
        b.center_of_mass().unwrap(),
        [35.0, 110.0, 27.5],
        1e-9,
    );
    let i = b.inertia.unwrap();
    assert!(close(i[0][0], want[1], 1e-9 * want[1]) && close(i[1][1], want[0], 1e-9 * want[0]));
    // Undo both moves: back at the original place.
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    let r = report(&mut s, None);
    assert_vec(
        "after undo",
        body(&r, 1).center_of_mass().unwrap(),
        [10.0, 15.0, 20.0],
        1e-9,
    );
}

#[test]
fn two_bodies_total_hidden_and_suppressed() {
    let mut s = Session::new();
    box_doc(&mut s);
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "density": 1.2}),
    );
    // A copy 60 mm along X (it inherits the material).
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 1, "translate": [60.0, 0.0, 0.0], "copy": true}),
    );
    let r = report(&mut s, None);
    assert_eq!(r.bodies.len(), 2);
    assert_eq!(body(&r, 2).material.as_ref().unwrap().density, 1.2);
    let t = r.total.as_ref().expect("total");
    assert!(close(t.mass, 57.6, 1e-9), "{}", t.mass);
    // Combined centre: midway, (40, 15, 20).
    assert_vec("combined", t.center_of_mass, [40.0, 15.0, 20.0], 1e-9);
    // Parallel axis: each box is 30 mm from the combined centre along X.
    let ib = box_inertia(28.8, 20.0, 30.0, 40.0);
    let d2 = 30.0 * 30.0;
    let want = [
        2.0 * ib[0],
        2.0 * (ib[1] + 28.8 * d2),
        2.0 * (ib[2] + 28.8 * d2),
    ];
    for k in 0..3 {
        assert!(
            close(t.inertia[k][k], want[k], 1e-9 * want[k]),
            "{:?} vs {want:?}",
            t.inertia
        );
    }
    // Hidden bodies are left out of the default set and named.
    ok(
        &mut s,
        json!({"cmd": "set_body_visible", "body": 2, "visible": false}),
    );
    let r = report(&mut s, None);
    assert_eq!(r.excluded_hidden.len(), 1);
    assert!(close(r.total.unwrap().mass, 28.8, 1e-9));
    // ... unless listed explicitly.
    let r = report(&mut s, Some(&[1, 2]));
    assert!(close(r.total.unwrap().mass, 57.6, 1e-9));
    // Suppressing the copy removes its body from the model and from the totals;
    // listing it is refused with a clear reason.
    let copy_feature = s.summary().features.last().unwrap().id.0;
    ok(
        &mut s,
        json!({"cmd": "set_suppressed", "feature": copy_feature, "suppressed": true}),
    );
    let r = report(&mut s, None);
    assert_eq!(r.bodies.len(), 1);
    assert!(close(r.total.unwrap().mass, 28.8, 1e-9));
    let e = tray::exec(&mut s, &json!({"cmd": "mass_properties", "bodies": [2]}));
    assert!(!e.ok);
    assert!(format!("{:?}", e.error).contains("not in the evaluated model"));
}

#[test]
fn density_units_catalog_undo_and_refusals() {
    let mut s = Session::new();
    box_doc(&mut s);
    // 1200 kg/m³ = 1.2 g/cm³.
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "density": 1200.0,
               "density_unit": "kg_per_m3"}),
    );
    let m = s.summary().bodies[0].mass.unwrap();
    assert!(close(m, 28.8, 1e-9), "{m}");
    // Catalog PLA (typical nominal 1.24): 24 × 1.24 = 29.76 g.
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "material": "pla"}),
    );
    let b = s.summary().bodies[0].clone();
    let mat = b.material.unwrap();
    assert_eq!(
        (mat.name.as_str(), mat.catalog.as_deref()),
        ("PLA", Some("pla"))
    );
    assert!(close(b.mass.unwrap(), 29.76, 1e-9));
    ok(&mut s, json!({"cmd": "undo"}));
    assert!(close(s.summary().bodies[0].mass.unwrap(), 28.8, 1e-9));
    ok(&mut s, json!({"cmd": "redo"}));
    // Refusals leave the document unchanged.
    let before = s.model().unwrap().document().clone();
    for bad in [
        json!({"cmd": "set_body_material", "body": 1, "material": "pla", "density": 2.0}),
        json!({"cmd": "set_body_material", "body": 1, "material": "unobtainium"}),
        json!({"cmd": "set_body_material", "body": 1, "density": 0.0}),
        json!({"cmd": "set_body_material", "body": 1, "density": 1200.0}),
        json!({"cmd": "set_body_material", "body": 9, "density": 1.0}),
    ] {
        let r = tray::exec(&mut s, &bad);
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad}");
    }
    // Clearing.
    ok(&mut s, json!({"cmd": "set_body_material", "body": 1}));
    assert!(s.summary().bodies[0].material.is_none());
    assert!(s.summary().bodies[0].mass.is_none());
}

#[test]
fn cylinder_and_hollow_part_centroids() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    // Cylinder Ø20 × 25 centred at (5, -3).
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
               "diameter": 20.0, "center": [5.0, -3.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 25.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "material": "al6061"}),
    );
    let r = report(&mut s, None);
    let b = body(&r, 1);
    let v = PI * 100.0 * 25.0;
    let m = v * 2.70e-3;
    assert!(close(b.mass.unwrap(), m, 1e-9 * m));
    assert_vec(
        "cylinder",
        b.center_of_mass().unwrap(),
        [5.0, -3.0, 12.5],
        1e-9,
    );
    let i = b.inertia.unwrap();
    // I_zz = m r²/2, I_xx = m (3 r² + h²)/12.
    assert!(close(i[2][2], m * 100.0 / 2.0, 1e-9 * i[2][2]));
    assert!(close(i[0][0], m * (300.0 + 625.0) / 12.0, 1e-9 * i[0][0]));
    // Hollow block: 60 × 40 × 20 with an off-centre Ø10 through hole at (15, 10).
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 40.0, "center": [30.0, 20.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 10.0, "center": [15.0, 10.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": 3}));
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "material": "steel"}),
    );
    let r = report(&mut s, None);
    let b = body(&r, 1);
    let p = b.properties.as_ref().unwrap();
    let (vb, vh) = (48_000.0, PI * 25.0 * 20.0);
    let v = vb - vh;
    // The end cap's sketch coordinates are world X/Y here: the hole is at (15, 10)
    // (membership: empty there, material at the mirrored point).
    let probe = tray::probe(&s, &[[15.0, 10.0, 10.0], [45.0, 30.0, 10.0]]);
    assert_eq!(probe, vec![Some(false), Some(true)]);
    let hole = [15.0, 10.0];
    let want = [
        (vb * 30.0 - vh * hole[0]) / v,
        (vb * 20.0 - vh * hole[1]) / v,
        10.0,
    ];
    assert!(close(p.volume, v, 1e-6));
    assert_vec("hollow", p.centroid, want, 1e-9);
    assert!(close(b.mass.unwrap(), v * 7.85e-3, 1e-9 * v));
}

#[test]
fn estimated_volume_is_labelled_and_mesh_bodies_are_exact_for_the_mesh() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let sphere = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rcad-kernel/tests/fixtures/step/sphere_r10_mm.step"
    );
    ok(&mut s, json!({"cmd": "import_step", "path": sphere}));
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "material": "pla"}),
    );
    let r = report(&mut s, None);
    let b = body(&r, 1);
    let p = b.properties.as_ref().unwrap();
    assert!(!b.exact);
    assert_eq!(p.method, rcad_kernel::VolumeMethod::TessellationEstimate);
    let v = 4.0 / 3.0 * PI * 1000.0;
    assert!(close(p.volume, v, p.volume_error), "{} vs {v}", p.volume);
    let m = v * 1.24e-3;
    assert!(
        close(b.mass.unwrap(), m, b.mass_error.unwrap()),
        "{:?}",
        b.mass
    );
    assert!(b.mass_error.unwrap() > 0.0);
    assert!(!r.total.as_ref().unwrap().exact);
    // Sphere centre (5, 5, 5).
    assert_vec("sphere", p.centroid, [5.0; 3], 1e-6);
}

#[test]
fn materials_survive_save_and_reopen() {
    let mut s = Session::new();
    box_doc(&mut s);
    let dir = tempfile::tempdir().unwrap();
    // Without a material the file stays at its previous version.
    let plain = dir.path().join("plain.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": plain}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&plain).unwrap()).unwrap();
    assert!(v["version"].as_u64().unwrap() < 6);
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "material": "petg"}),
    );
    let path = dir.path().join("m.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    assert_eq!(
        v["document"]["bodies"][0]["material"],
        json!({"name": "PETG", "density": 1.27, "catalog": "petg"})
    );
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(close(
        t.summary().bodies[0].mass.unwrap(),
        24.0 * 1.27,
        1e-9
    ));
    // A hand-edited invalid density is refused on load.
    let bad = dir.path().join("bad.rcad.json");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("1.27", "-1.0");
    std::fs::write(&bad, text).unwrap();
    let r = tray::exec(&mut t, &json!({"cmd": "open", "path": bad}));
    assert!(!r.ok);
}
