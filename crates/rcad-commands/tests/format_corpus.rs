//! Run 8 §3 qualification corpus (tools/fixtures/corpus/MANIFEST.md): every
//! file of `manifest.json` is imported through the typed command API and
//! checked against independent expectations (closed form or FreeCAD's own
//! read-back); malformed files must fail with an error and leave nothing in
//! the document.
use rcad_commands::{Command, Session};
use rcad_kernel::VolumeResult;
use serde_json::{Value, json};
use std::path::PathBuf;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/fixtures/corpus")
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

#[test]
fn corpus_imports_match_their_references() {
    let manifest: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(corpus().join("manifest.json")).unwrap())
            .unwrap();
    let mut failures = Vec::new();
    for e in &manifest {
        let file = e["file"].as_str().unwrap();
        let mut cmd = e["import"].clone();
        cmd["path"] = json!(corpus().join(file));
        let label = format!("{file} ({})", cmd["cmd"]);
        let mut s = Session::new();
        assert!(s.execute(&Command::NewDocument).ok);
        let c: Command =
            serde_json::from_value(cmd.clone()).unwrap_or_else(|x| panic!("{label}: {x}"));
        let r = s.execute(&c);
        let exp = &e["expect"];
        let sum = s.summary();
        let mut bad = |m: String| failures.push(format!("{label}: {m}"));
        if let Some(want) = exp.get("error").and_then(Value::as_str) {
            if r.ok {
                bad("accepted, expected an error".into());
                continue;
            }
            let msg = format!("{:?}", r.error);
            if !msg.contains(want) {
                bad(format!("error {msg} lacks '{want}'"));
            }
            if msg.len() > 2000 {
                bad(format!("unbounded error message ({} bytes)", msg.len()));
            }
            if !sum.features.is_empty() || !sum.bodies.is_empty() {
                bad("a failed import left features/bodies".into());
            }
            continue;
        }
        if !r.ok {
            bad(format!("import failed: {:?}", r.error));
            continue;
        }
        if let Some(n) = exp.get("bodies").and_then(Value::as_u64)
            && sum.bodies.len() as u64 != n
        {
            bad(format!("{} bodies, expected {n}", sum.bodies.len()));
        }
        let mesh = exp.get("mesh").and_then(Value::as_bool).unwrap_or(false);
        let (mut vol, mut lo, mut hi) = (0.0, [f64::MAX; 3], [f64::MIN; 3]);
        // Stated error of estimated volumes (labelled, e.g. a sphere's
        // tessellation estimate): the comparison allows it.
        let mut vol_err = 0.0;
        let mut grow = |a: [f64; 3], b: [f64; 3]| {
            for i in 0..3 {
                lo[i] = lo[i].min(a[i]);
                hi[i] = hi[i].max(b[i]);
            }
        };
        let mut comps = 0;
        for b in &sum.bodies {
            if mesh {
                let d = b.mesh.as_ref().expect("mesh diagnostics");
                comps += d.components;
                vol += d.signed_volume.unwrap_or(f64::NAN);
                if let Some((a, c)) = d.bounds {
                    grow(a, c);
                }
                if let Some(cl) = exp.get("closed").and_then(Value::as_bool)
                    && d.closed != cl
                {
                    bad(format!("mesh closed = {}", d.closed));
                }
            } else {
                let m = b.measurements.as_ref().expect("measured");
                grow(m.bounds.min, m.bounds.max);
                if let Some(cl) = exp.get("closed").and_then(Value::as_bool)
                    && m.all_shells_closed != cl
                {
                    bad(format!("closed = {}", m.all_shells_closed));
                }
                match &m.volume {
                    VolumeResult::Value {
                        volume,
                        error_estimate,
                        ..
                    } => {
                        vol += volume;
                        vol_err += error_estimate;
                    }
                    VolumeResult::Unavailable { .. } => {
                        if exp.get("volume").is_some() {
                            bad("no volume".into());
                        }
                    }
                }
            }
        }
        if let (Some(v), Some(t)) = (f(exp, "volume"), f(exp, "vol_tol"))
            && (vol - v).abs() > t + vol_err
        {
            bad(format!(
                "volume {vol} vs {v} (tol {t} + stated error {vol_err})"
            ));
        }
        if let Some(n) = exp.get("components").and_then(Value::as_u64)
            && comps as u64 != n
        {
            bad(format!("{comps} mesh components, expected {n}"));
        }
        if let (Some(bb), Some(t)) = (
            exp.get("bbox").and_then(Value::as_array),
            f(exp, "bbox_tol"),
        ) {
            let got = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
            let err = bb
                .iter()
                .zip(got)
                .map(|(w, g)| (w.as_f64().unwrap() - g).abs())
                .fold(0.0, f64::max);
            if err > t {
                bad(format!("bbox {got:?} (err {err:.3e} > {t})"));
            }
        }
        if let Some(n) = exp.get("sketch_entities").and_then(Value::as_u64) {
            let sk = sum
                .features
                .iter()
                .find(|x| x.kind == "sketch")
                .and_then(|x| x.sketch.clone());
            match sk {
                None => bad("no sketch".into()),
                Some(sk) => {
                    let ents = sk
                        .entities
                        .iter()
                        .filter(|x| !sk.construction.contains(&x.id()))
                        .count();
                    if ents as u64 != n {
                        bad(format!("{ents} sketch entities, expected {n}"));
                    }
                    if let Some(mx) = exp.get("sketch_max").and_then(Value::as_array) {
                        let gx = sk.points.iter().map(|q| q.x).fold(f64::MIN, f64::max);
                        let gy = sk.points.iter().map(|q| q.y).fold(f64::MIN, f64::max);
                        let (wx, wy) = (mx[0].as_f64().unwrap(), mx[1].as_f64().unwrap());
                        if (gx - wx).abs() > 1e-6 || (gy - wy).abs() > 1e-6 {
                            bad(format!("sketch max ({gx}, {gy}) vs ({wx}, {wy})"));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(manifest.len() >= 20);
}
