//! Organizer tray acceptance (SESSION_5 §7) through the shared command API:
//! the spec's analytic oracle (tests/support/tray.rs, from the parameters
//! only), occupancy probes for walls/floor/dividers/compartments, one connected
//! closed solid, the user-controlled variants, an invalid edit refused with
//! the document unchanged, undo/redo + save/reopen, and a seeded sweep.
mod support;
use rcad_commands::Session;
use rcad_document::{BodyId, DocError};
use serde_json::json;
use support::tray::{self, Tray};

fn check(s: &Session, t: &Tray, notch: bool) {
    let want = if notch {
        t.notched_volume()
    } else {
        t.plain_volume()
    };
    let v = tray::volume(s);
    // Exact B-rep boundary integral: far below 1e-6 relative.
    assert!(
        (v - want).abs() <= 1e-9 * want.max(1.0) + 1e-6,
        "{t:?}: volume {v} vs formula {want}"
    );
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(1))
        .expect("body");
    let m = b.measurements.expect("measured");
    assert_eq!(m.shell_count, 1, "one connected solid");
    assert!(m.all_shells_closed);
    for (i, (lo, hi)) in [(0.0, t.l), (0.0, t.w), (0.0, t.h)].into_iter().enumerate() {
        assert!(
            (m.bounds.min[i] - lo).abs() < 1e-6 && (m.bounds.max[i] - hi).abs() < 1e-6,
            "bounds axis {i}: {:?}",
            m.bounds
        );
    }
    tray::assert_occupancy(s, t, notch);
}

fn set(s: &mut Session, t: &Tray) -> rcad_commands::Response {
    tray::exec(s, &serde_json::to_value(t.set_all()).unwrap())
}

#[test]
fn baseline_plain_tray_matches_the_formula() {
    let t = Tray::BASELINE;
    assert_eq!(t.plain_volume(), 49664.0);
    assert!((t.compartment() - 112.0 / 3.0).abs() < 1e-12);
    let s = tray::build(&t, false);
    check(&s, &t, false);
}

#[test]
fn variants_regenerate_from_parameters_only() {
    let mut s = tray::build(&Tray::BASELINE, false);
    let v1 = Tray {
        l: 160.0,
        w: 90.0,
        h: 35.0,
        wall: 2.4,
        floor: 2.4,
        dt: 2.4,
        n: 3,
        ..Tray::BASELINE
    };
    let r = set(&mut s, &v1);
    assert!(r.ok, "{:?}", r.error);
    check(&s, &v1, false);
    // divider_count = 0: one compartment, no pattern copies (count 1).
    let v0 = Tray { n: 0, ..v1 };
    assert!(set(&mut s, &v0).ok);
    check(&s, &v0, false);
    let pat = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id.0 == tray::COMPARTMENTS)
        .unwrap();
    assert!(pat.parameters.contains(&("count".to_string(), 1.0)));
}

#[test]
fn invalid_combinations_are_refused_unchanged() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    let before = s.model().unwrap().document().clone();
    for (bad, what) in [
        (Tray { wall: 45.0, ..t }, "cavity_width"),
        (Tray { floor: 30.0, ..t }, "cavity_depth"),
        (Tray { n: 60, ..t }, "compartment_width"),
        (Tray { dt: 60.0, ..t }, "compartment_width"),
    ] {
        let r = set(&mut s, &bad);
        assert!(!r.ok, "{bad:?} accepted");
        let e = format!("{:?}", r.error);
        assert!(e.contains(what) || e.contains("Compartment"), "{what}: {e}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad:?}");
    }
    // A non-integer divider count is refused by the count slot.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "divider_count", "expression": "1.5",
                "unit": "unitless"}),
    );
    assert!(!r.ok);
    match r.error {
        Some(DocError::Expression { detail, .. }) => {
            assert!(detail.contains("whole number"), "{detail}")
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);
    check(&s, &t, false);
}

#[test]
fn coordinated_change_undo_redo_and_reopen() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    let v = Tray {
        wall: 3.0,
        floor: 3.0,
        dt: 3.0,
        n: 3,
        ..t
    };
    assert!(set(&mut s, &v).ok);
    check(&s, &v, false);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    check(&s, &t, false);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    check(&s, &v, false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tray.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document(),
        "expressions and body identity preserved"
    );
    check(&s2, &v, false);
    // Expressions survive: one more edit regenerates the whole chain.
    assert!(set(&mut s2, &t).ok);
    check(&s2, &t, false);
}

#[test]
fn seeded_sweep_around_normal_values() {
    // xorshift64*, fixed seed: reproducible; bounds keep compartments >= 10 mm.
    let mut x: u64 = 0x5eed_7a7a_2026_1012;
    let mut r = |lo: f64, hi: f64| {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let u = (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64;
        // 0.1 mm steps, like typed dimensions.
        ((lo + u * (hi - lo)) * 10.0).round() / 10.0
    };
    let mut s = tray::build(&Tray::BASELINE, false);
    let mut done = 0;
    for _ in 0..12 {
        let t = Tray {
            l: r(90.0, 200.0),
            w: r(50.0, 120.0),
            h: r(15.0, 45.0),
            wall: r(1.2, 3.0),
            floor: r(1.2, 3.0),
            dt: r(1.2, 3.0),
            n: r(0.0, 4.0) as u32,
            ..Tray::BASELINE
        };
        assert!(t.compartment() >= 10.0, "{t:?}");
        let resp = set(&mut s, &t);
        assert!(resp.ok, "{t:?}: {:?}", resp.error);
        check(&s, &t, false);
        done += 1;
    }
    assert_eq!(done, 12);
}

/// The committed tray workflows (`printables::tray_workflow`) against
/// formulas derived here from the parameters: the notched tray, and the
/// finished variant (rectangular body, notch with two r-filleted bottom
/// corners: area w·d − 2(r² − πr²/4); four 90° corner chamfers c: c²/2 · H
/// each), then variant 1 through `set_user_parameters`.
#[test]
fn printable_tray_workflows_match_their_formulas() {
    use rcad_commands::printables::{
        self, TRAY_BASELINE, TRAY_CORNER_CHAMFER, TRAY_FINGER_RADIUS, TRAY_VARIANT,
    };
    let dir = tempfile::tempdir().unwrap();
    let as_test = |p: &printables::Tray| Tray {
        l: p.length,
        w: p.width,
        h: p.height,
        wall: p.wall,
        floor: p.floor,
        dt: p.divider_thickness,
        n: p.dividers,
        notch_w: p.notch_width,
        notch_d: p.notch_depth,
    };
    for finished in [false, true] {
        let mut s = Session::new();
        for st in printables::tray_workflow(&dir.path().join("t.rcad.json"), finished) {
            let r = s.execute(&st.cmd);
            assert_eq!(r.ok, st.expect_ok, "{:?}: {:?}", st.cmd, r.error);
        }
        for p in [TRAY_BASELINE, TRAY_VARIANT] {
            if p != TRAY_BASELINE {
                let r = s.execute(&p.set_parameters_batch());
                assert!(r.ok, "{:?}", r.error);
            }
            let t = as_test(&p);
            if !finished {
                check(&s, &t, true);
                continue;
            }
            let r = TRAY_FINGER_RADIUS;
            let notch = t.notch_w * t.notch_d - 2.0 * (r * r - std::f64::consts::PI * r * r / 4.0);
            let chamfers = 4.0 * TRAY_CORNER_CHAMFER.powi(2) / 2.0 * t.h;
            let want = t.plain_volume() - notch * t.wall - chamfers;
            let v = tray::volume(&s);
            assert!((v - want).abs() < 1e-6 * want, "{p:?}: {v} vs {want}");
            // Material and air where the finished features are.
            let c0 = t.compartment_center(0);
            let pts = [
                // finger notch: open in the middle and just above its rounded bottom
                [c0, t.wall / 2.0, t.h - 1.0],
                [c0, t.wall / 2.0, t.h - t.notch_d + 0.5],
                // a rounded corner of the notch keeps material in its corner
                [
                    c0 - t.notch_w / 2.0 + 0.3,
                    t.wall / 2.0,
                    t.h - t.notch_d + 0.3,
                ],
                // the chamfered outer corner (x, y < c/2 near the corner) is air
                [0.2, 0.2, t.h / 2.0],
                [0.2, 1.8, t.h / 2.0],
            ];
            assert_eq!(
                tray::probe(&s, &pts),
                vec![
                    Some(false),
                    Some(false),
                    Some(true),
                    Some(false),
                    Some(true)
                ],
                "{p:?}"
            );
        }
    }
}
