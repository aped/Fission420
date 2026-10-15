//! Size behaviour: the sketch solver bounds the importable drawing (rcad_sketch::MAX_UNKNOWNS).

use rcad_exchange::ExchangeError;
use rcad_exchange::dxf::{self, DxfImportOptions};
use std::fmt::Write as _;

fn squares(n: usize) -> String {
    let mut s = String::from(
        "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n",
    );
    for k in 0..n {
        let x = 20.0 * k as f64;
        let p = [(x, 0.0), (x + 10.0, 0.0), (x + 10.0, 10.0), (x, 10.0)];
        for i in 0..4 {
            let (a, b) = (p[i], p[(i + 1) % 4]);
            let _ = write!(
                s,
                "0\nLINE\n8\n0\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n",
                a.0, a.1, b.0, b.1
            );
        }
    }
    s + "0\nENDSEC\n0\nEOF\n"
}

/// 240 squares = 960 shared corners + origin = 1922 unknowns (< 2048): imports.
/// 300 squares = 2402 unknowns: a clear limit error, not a hang or panic.
#[test]
fn large_drawings_hit_the_sketch_limit_cleanly() {
    let t = std::time::Instant::now();
    let imp = dxf::import(squares(240).as_bytes(), &DxfImportOptions::default()).unwrap();
    assert_eq!(imp.sketch.entities.len(), 960);
    assert_eq!(imp.sketch.points.len(), 961);
    eprintln!("240 squares imported in {:?}", t.elapsed());
    match dxf::import(squares(300).as_bytes(), &DxfImportOptions::default()) {
        Err(ExchangeError::LimitExceeded { actual, .. }) => assert_eq!(actual, 2402),
        other => panic!("{other:?}"),
    }
}

/// 190 slots (3 LINE + ARC each; 5 points) = 951 points = 1902 unknowns with 190 arc
/// equations: the post-import solve runs the dense solver near its size limit.
#[test]
fn many_arcs_near_the_limit_solve() {
    let mut s = String::from(
        "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n",
    );
    let l = |a: (f64, f64), b: (f64, f64)| {
        format!(
            "0\nLINE\n8\n0\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n",
            a.0, a.1, b.0, b.1
        )
    };
    for k in 0..190 {
        let y = 30.0 * k as f64;
        s += &l((0.0, y), (30.0, y));
        s += &format!(
            "0\nARC\n8\n0\n10\n30\n20\n{}\n40\n10\n50\n-90\n51\n90\n",
            y + 10.0
        );
        s += &l((30.0, y + 20.0), (0.0, y + 20.0));
        s += &l((0.0, y + 20.0), (0.0, y));
    }
    s += "0\nENDSEC\n0\nEOF\n";
    let t = std::time::Instant::now();
    let imp = dxf::import(s.as_bytes(), &DxfImportOptions::default()).unwrap();
    eprintln!("190 slots imported in {:?} ({:?})", t.elapsed(), imp.solve);
    assert_eq!(imp.sketch.points.len(), 951);
    assert!(imp.solve.is_success());
}
