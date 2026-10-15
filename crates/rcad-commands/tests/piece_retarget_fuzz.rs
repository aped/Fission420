//! Seeded randomized check that split pieces keep their body ids (ADR 0003
//! piece identity): a plate with a tab, split by a through slot into a top
//! and a bottom piece, under 200 random upstream edits of the slot
//! position, the slot length (a slot that stops short of the tab or the
//! plate edge rejoins the pieces; a later edit separates them again), the
//! tab position (which flips the pieces' bounding-box order) and the plate
//! width. Invariant after every edit, accepted or refused: Body1 is the
//! piece above the slot and Body2 the one below (or absent while the
//! pieces are joined); no other body appears. An edit may be refused, never
//! applied with the ids swapped.
mod support;
use rcad_commands::Session;
use serde_json::json;
use support::tray;

/// xorshift64*, deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

fn y_range(s: &Session, id: u32) -> Option<(f64, f64)> {
    let b = s.summary().bodies.into_iter().find(|b| b.id.0 == id)?;
    let bb = b.bounds?;
    Some((bb.min[1], bb.max[1]))
}

#[test]
fn split_pieces_never_swap_ids_under_random_edits() {
    let mut s = Session::new();
    // Plate y 0..30 (x centred on 20, width w), tab x -10..2 (z -5..5)
    // centred on y = ty (F3), slot y c-1..c+1 cut through (F5/F6).
    let (mut c, mut ty) = (15.0, 25.0);
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 12.0, "height": 10.0, "center": [-4.0, ty]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 10.0, "op": "join", "target": 1,
               "options": {"symmetric": true}}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 2.0, "center": [20.0, c]}),
        json!({"cmd": "extrude", "sketch": 5, "op": "cut", "target": 1, "through_all": true}),
    ] {
        tray::ok(&mut s, &v);
    }
    let mut rng = Rng(0x5eed_0f_91ec_e5);
    let (mut accepted, mut refused, mut joined) = (0, 0, 0);
    for step in 0..200 {
        let (feature, name, value) = match (rng.next() * 4.0) as u32 {
            0 => (5, "center_y", rng.range(8.0, 22.0)),
            1 => (3, "center_y", rng.range(5.0, 25.0)),
            2 => (5, "width", rng.range(40.0, 64.0)),
            _ => (1, "width", rng.range(36.0, 50.0)),
        };
        let r = tray::exec(
            &mut s,
            &json!({"cmd": "set_parameter", "feature": feature, "name": name, "value": value}),
        );
        if r.ok {
            accepted += 1;
            match feature {
                5 => c = value,
                3 => ty = value,
                _ => {}
            }
        } else {
            refused += 1;
        }
        let ids: Vec<u32> = s.summary().bodies.iter().map(|b| b.id.0).collect();
        let ctx = format!(
            "step {step}: {name} = {value:.3} (c {c:.3}, tab {ty:.3}), ok {}",
            r.ok
        );
        assert!(
            ids.iter().all(|i| *i == 1 || *i == 2),
            "{ctx}: bodies {ids:?}"
        );
        let top = y_range(&s, 1).unwrap_or_else(|| panic!("{ctx}: Body1 missing"));
        match y_range(&s, 2) {
            Some(bottom) => {
                assert!(
                    (top.1 - 30.0).abs() < 1e-9 && top.0 > 1.0,
                    "{ctx}: Body1 is not the top piece: {top:?}"
                );
                assert!(
                    bottom.0.abs() < 1e-9 && bottom.1 < 29.0,
                    "{ctx}: Body2 is not the bottom piece: {bottom:?}"
                );
            }
            None => {
                // Joined: the slot no longer crosses the part.
                assert!(
                    top.0.abs() < 1e-9 && (top.1 - 30.0).abs() < 1e-9,
                    "{ctx}: Body2 vanished but Body1 is {top:?}"
                );
                joined += 1;
            }
        }
    }
    eprintln!("accepted {accepted}, refused {refused}, joined {joined}");
    assert!(accepted >= 150, "too many refusals: {refused}");
    assert!(
        joined > 10 && joined < 150,
        "rejoin not exercised: {joined}"
    );
}
