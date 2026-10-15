//! Organizer tray regression: the committed tray workflows
//! (`examples/organizer_tray_workflow.json`, `organizer_tray_finished_workflow.json`)
//! replayed as `rcad-cli run` would, the saved example documents reopened, and the
//! accepted parameter variants (160 × 90 × 35 with 2.4 walls/floor/dividers and 3
//! dividers; no dividers; 3 mm walls) applied through user parameters.
//!
//! The oracle sums the material slabs of the implemented definitions instead of
//! subtracting cavities (a second derivation, so the overlap conventions are
//! checked): floor `L·W·f` under everything; two long walls `L·t·(H−f)` over the
//! full length; two short walls `(W−2t)·t·(H−f)` between them; `n` dividers
//! `d·(W−2t)·(H−f)` spanning the cavity width; minus the rim notch (rectangle, or
//! finger notch with two bottom corner fillets of radius `r_f`) cut `t` deep
//! through the front wall only, and (finished tray) four 45° corner chamfers
//! `c²/2·H` inside the wall corners. The parameter values are read from the
//! workflow's own `set_user_parameter` steps. The accepted historical figures
//! (baseline 49664 plain / 49408 notched; variant 92927.04 / 92619.84) are
//! compared at the end as a cross-check, not used as the oracle.
use rcad_commands::{Command, Session};
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::f64::consts::PI;

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn exec(s: &mut Session, v: &Value) -> rcad_commands::Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    s.execute(&c)
}

fn volume(s: &Session) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(1))
        .expect("tray body");
    match b.measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Replay a committed workflow (CLI-only steps such as `inspect` skipped; the
/// save goes to a temporary directory). Returns the session and the numeric user
/// parameters the workflow sets.
fn replay(file: &str) -> (Session, HashMap<String, f64>) {
    let steps: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(repo().join("examples").join(file)).unwrap())
            .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    let mut params = HashMap::new();
    for (i, v) in steps.iter().enumerate() {
        let mut v = v.clone();
        match v["cmd"].as_str() {
            Some("inspect") => continue,
            Some("save") => v["path"] = json!(dir.path().join("t.rcad.json")),
            Some("set_user_parameter") => {
                if let Ok(x) = v["expression"].as_str().unwrap().parse::<f64>() {
                    params.insert(v["name"].as_str().unwrap().to_string(), x);
                }
            }
            _ => {}
        }
        let r = exec(&mut s, &v);
        assert!(r.ok, "{file} step {i} {v}: {:?}", r.error);
    }
    (s, params)
}

#[derive(Clone, Copy, Debug)]
struct T {
    l: f64,
    w: f64,
    h: f64,
    t: f64,
    f: f64,
    d: f64,
    n: f64,
    nw: f64,
    nd: f64,
}

impl T {
    fn from(p: &HashMap<String, f64>) -> T {
        let g = |k: &str| *p.get(k).unwrap_or_else(|| panic!("parameter {k}"));
        T {
            l: g("tray_length"),
            w: g("tray_width"),
            h: g("tray_height"),
            t: g("wall"),
            f: g("floor"),
            d: g("divider_thickness"),
            n: g("divider_count"),
            nw: g("notch_width"),
            nd: g("notch_depth"),
        }
    }

    fn compartment(&self) -> f64 {
        (self.l - 2.0 * self.t - self.n * self.d) / (self.n + 1.0)
    }

    /// Material slabs (module docs).
    fn slabs(&self) -> f64 {
        let up = self.h - self.f;
        let inner = self.w - 2.0 * self.t;
        self.l * self.w * self.f
            + 2.0 * self.l * self.t * up
            + 2.0 * inner * self.t * up
            + self.n * self.d * inner * up
    }

    /// The notch lies in the front wall over compartment 1, below the rim: it
    /// removes exactly its area × wall when it fits (asserted).
    fn notch_fits(&self) {
        let c = self.compartment();
        assert!(self.nw < c, "notch wider than the compartment: {self:?}");
        assert!(
            self.nd < self.h - self.f,
            "notch reaches the floor: {self:?}"
        );
    }
}

#[test]
fn committed_tray_workflows_and_documents_match_the_slab_oracle() {
    // Plain tray with the rectangular rim notch.
    let (mut s, p) = replay("organizer_tray_workflow.json");
    let t = T::from(&p);
    t.notch_fits();
    let notched = t.slabs() - t.nw * t.nd * t.t;
    assert!(
        (volume(&s) - notched).abs() < 1e-6,
        "{} vs {notched}",
        volume(&s)
    );
    let mut doc = Session::new();
    assert!(
        exec(
            &mut doc,
            &json!({"cmd": "open", "path": repo().join("examples/organizer_tray.rcad.json")})
        )
        .ok
    );
    assert!((volume(&doc) - notched).abs() < 1e-6, "saved document");
    assert_eq!(
        doc.model().unwrap().document(),
        s.model().unwrap().document()
    );

    // Accepted variants through the parameters.
    let variants = [
        T {
            l: 160.0,
            w: 90.0,
            h: 35.0,
            t: 2.4,
            f: 2.4,
            d: 2.4,
            n: 3.0,
            ..t
        },
        T {
            l: 160.0,
            w: 90.0,
            h: 35.0,
            t: 2.4,
            f: 2.4,
            d: 2.4,
            n: 0.0,
            ..t
        },
        T {
            t: 3.0,
            f: 3.0,
            d: 3.0,
            n: 3.0,
            ..t
        },
    ];
    let mut got = vec![];
    for v in variants {
        v.notch_fits();
        let r = exec(
            &mut s,
            &json!({"cmd": "set_user_parameters", "parameters": [
                {"name": "tray_length", "expression": format!("{}", v.l), "unit": "mm"},
                {"name": "tray_width", "expression": format!("{}", v.w), "unit": "mm"},
                {"name": "tray_height", "expression": format!("{}", v.h), "unit": "mm"},
                {"name": "wall", "expression": format!("{}", v.t), "unit": "mm"},
                {"name": "floor", "expression": format!("{}", v.f), "unit": "mm"},
                {"name": "divider_thickness", "expression": format!("{}", v.d), "unit": "mm"},
                {"name": "divider_count", "expression": format!("{}", v.n), "unit": "unitless"},
            ]}),
        );
        assert!(r.ok, "{v:?}: {:?}", r.error);
        let want = v.slabs() - v.nw * v.nd * v.t;
        assert!(
            (volume(&s) - want).abs() < 1e-6,
            "{v:?}: {} vs {want}",
            volume(&s)
        );
        got.push(volume(&s));
    }
    // Historical acceptance figures (cross-check only).
    assert!((t.slabs() - 49664.0).abs() < 1e-9 && (notched - 49408.0).abs() < 1e-9);
    assert!((variants[0].slabs() - 92927.04).abs() < 1e-6);
    assert!((got[0] - 92619.84).abs() < 1e-6);
}

#[test]
fn finished_tray_workflow_and_document_match_the_slab_oracle() {
    let (s, p) = replay("organizer_tray_finished_workflow.json");
    let t = T::from(&p);
    t.notch_fits();
    let rf = p["finger_radius"];
    let c = p["corner_chamfer"];
    assert!(
        rf <= t.nd && 2.0 * rf <= t.nw,
        "finger fillets fit the notch"
    );
    assert!(c < t.t, "corner chamfer stays inside the wall corner");
    // Finger notch: rectangle minus the two filleted bottom corners' spandrels.
    let finger = t.nw * t.nd - 2.0 * (rf * rf - PI * rf * rf / 4.0);
    let want = t.slabs() - finger * t.t - 4.0 * c * c / 2.0 * t.h;
    assert!((volume(&s) - want).abs() < 1e-6, "{} vs {want}", volume(&s));
    let mut doc = Session::new();
    assert!(
        exec(
            &mut doc,
            &json!({"cmd": "open",
                    "path": repo().join("examples/organizer_tray_finished.rcad.json")})
        )
        .ok
    );
    assert!((volume(&doc) - want).abs() < 1e-6, "saved document");
    assert_eq!(
        doc.model().unwrap().document(),
        s.model().unwrap().document()
    );
}
