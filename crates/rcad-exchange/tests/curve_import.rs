//! DXF ELLIPSE / SPLINE and SVG `<ellipse>` import as exact sketch curves,
//! and the explicit refusal of curves that are not exactly representable
//! (elliptical arcs, non-uniform or rational splines, skewed ellipses).
//! Expected areas are closed-form (πab) or computed independently below.

use rcad_exchange::ExchangeError;
use rcad_exchange::dxf::{self, DxfImportOptions};
use rcad_exchange::svg::{self, SvgImportOptions};
use rcad_sketch::Entity;
use std::f64::consts::PI;

/// A minimal mm DXF with the given ENTITIES body (group code / value lines).
fn dxf_file(body: &str) -> Vec<u8> {
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{body}0\nENDSEC\n0\nEOF\n"
    )
    .into_bytes()
}

fn ellipse(cx: f64, cy: f64, mx: f64, my: f64, ratio: f64, t0: f64, t1: f64) -> String {
    format!(
        "0\nELLIPSE\n8\n0\n10\n{cx}\n20\n{cy}\n30\n0.0\n11\n{mx}\n21\n{my}\n31\n0.0\n\
         40\n{ratio}\n41\n{t0}\n42\n{t1}\n"
    )
}

fn spline(knots: &[f64], weights: &[f64], ctrl: &[[f64; 2]]) -> String {
    let mut s = format!(
        "0\nSPLINE\n8\n0\n70\n{}\n71\n3\n72\n{}\n73\n{}\n",
        if weights.is_empty() { 8 } else { 12 },
        knots.len(),
        ctrl.len()
    );
    for k in knots {
        s += &format!("40\n{k}\n");
    }
    for w in weights {
        s += &format!("41\n{w}\n");
    }
    for p in ctrl {
        s += &format!("10\n{}\n20\n{}\n30\n0.0\n", p[0], p[1]);
    }
    s
}

fn line(a: [f64; 2], b: [f64; 2]) -> String {
    format!(
        "0\nLINE\n8\n0\n10\n{}\n20\n{}\n30\n0.0\n11\n{}\n21\n{}\n31\n0.0\n",
        a[0], a[1], b[0], b[1]
    )
}

#[test]
fn full_dxf_ellipse_imports_exactly() {
    // Major axis vector (12, 16): a = 20, ratio 0.4: b = 8.
    let body = ellipse(5.0, 5.0, 12.0, 16.0, 0.4, 0.0, 2.0 * PI);
    let imp = dxf::import(&dxf_file(&body), &DxfImportOptions::default()).unwrap();
    let e = imp
        .sketch
        .entities
        .iter()
        .find(|e| matches!(e, Entity::Ellipse { .. }))
        .expect("ellipse");
    let Entity::Ellipse {
        center,
        major,
        minor,
        ..
    } = e
    else {
        unreachable!()
    };
    assert_eq!(imp.sketch.point_xy(*center), Some([5.0, 5.0]));
    assert_eq!(imp.sketch.point_xy(*major), Some([17.0, 21.0]));
    assert!((minor - 8.0).abs() < 1e-12);
    let area = imp.sketch.regions().unwrap()[0].area;
    assert!((area - PI * 160.0).abs() < 1e-8 * area, "{area}");
}

#[test]
fn elliptical_arcs_and_inexact_splines_are_refused_by_name() {
    let arc = ellipse(0.0, 0.0, 10.0, 0.0, 0.5, 0.0, PI);
    let e = dxf::import(&dxf_file(&arc), &DxfImportOptions::default()).unwrap_err();
    assert!(
        matches!(&e, ExchangeError::Unsupported { .. })
            && e.to_string().contains("elliptical arcs"),
        "{e}"
    );
    let ctrl = [
        [0.0, 0.0],
        [5.0, 10.0],
        [15.0, 10.0],
        [20.0, 0.0],
        [25.0, -5.0],
    ];
    // Non-uniform interior knot.
    let nonuni = spline(&[0.0, 0.0, 0.0, 0.0, 0.3, 1.0, 1.0, 1.0, 1.0], &[], &ctrl);
    let e = dxf::import(&dxf_file(&nonuni), &DxfImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("non-uniform"), "{e}");
    // Rational.
    let rational = spline(
        &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        &[1.0, 2.0, 2.0, 1.0],
        &ctrl[..4],
    );
    let e = dxf::import(&dxf_file(&rational), &DxfImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("rational"), "{e}");
    // Partial import keeps the rest and lists the refusal.
    let both = format!("{rational}{}", line([0.0, 0.0], [9.0, 0.0]));
    let imp = dxf::import(
        &dxf_file(&both),
        &DxfImportOptions {
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(imp.sketch.entities.len(), 1);
    assert!(imp.skipped.iter().any(|s| s.entity_type == "SPLINE"));
}

#[test]
fn uniform_dxf_spline_joins_a_line_into_a_profile() {
    // Knots scaled by 10 (any affine knot parametrisation is the same curve).
    let ctrl = [
        [0.0, 0.0],
        [2.0, 15.0],
        [15.0, 22.0],
        [28.0, 15.0],
        [30.0, 0.0],
    ];
    let body = format!(
        "{}{}",
        spline(
            &[0.0, 0.0, 0.0, 0.0, 10.0, 20.0, 20.0, 20.0, 20.0],
            &[],
            &ctrl
        ),
        line([30.0, 0.0], [0.0, 0.0])
    );
    let imp = dxf::import(&dxf_file(&body), &DxfImportOptions::default()).unwrap();
    let Some(Entity::Spline { points, closed, .. }) = imp
        .sketch
        .entities
        .iter()
        .find(|e| matches!(e, Entity::Spline { .. }))
    else {
        panic!("no spline")
    };
    assert!(!closed);
    assert_eq!(points.len(), 5);
    // Ends shared with the line: one closed region.
    let regions = imp.sketch.regions().unwrap();
    assert_eq!(regions.len(), 1);
    // Enclosed area: shoelace over dense points of the spline evaluated by the
    // Cox–de Boor recursion (independent of the importer and of the Bézier
    // decomposition), closed by the line along y = 0. Chord error O(h²): with
    // 40 000 steps it is far below the 1e-5 mm² tolerance.
    let knots = [0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 2.0];
    fn basis(k: &[f64], i: usize, p: usize, t: f64) -> f64 {
        if p == 0 {
            let end = t == 2.0 && k[i] < k[i + 1] && k[i + 1] == 2.0;
            return f64::from(u8::from((k[i] <= t && t < k[i + 1]) || end));
        }
        let mut v = 0.0;
        if k[i + p] > k[i] {
            v += (t - k[i]) / (k[i + p] - k[i]) * basis(k, i, p - 1, t);
        }
        if k[i + p + 1] > k[i + 1] {
            v += (k[i + p + 1] - t) / (k[i + p + 1] - k[i + 1]) * basis(k, i + 1, p - 1, t);
        }
        v
    }
    let at = |t: f64| {
        let mut q = [0.0; 2];
        for (i, c) in ctrl.iter().enumerate() {
            let b = basis(&knots, i, 3, t);
            q[0] += b * c[0];
            q[1] += b * c[1];
        }
        q
    };
    let n = 40_000;
    let mut area = 0.0;
    for k in 0..n {
        let (a, b) = (
            at(2.0 * k as f64 / n as f64),
            at(2.0 * (k + 1) as f64 / n as f64),
        );
        area += 0.5 * (a[0] * b[1] - b[0] * a[1]);
    }
    let got = regions[0].area;
    assert!((got - area.abs()).abs() < 1e-5, "{got} vs {}", area.abs());
}

#[test]
fn svg_ellipse_imports_exactly_and_skew_is_refused() {
    let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
<ellipse cx="50" cy="40" rx="5" ry="20"/></svg>"#;
    let imp = svg::import(doc, &SvgImportOptions::default()).unwrap();
    let Some(Entity::Ellipse { minor, .. }) = imp.sketch.entities.first() else {
        panic!("{:?}", imp.sketch.entities)
    };
    assert!((minor - 5.0).abs() < 1e-9);
    let area = imp.sketch.regions().unwrap()[0].area;
    assert!((area - PI * 100.0).abs() < 1e-8 * area, "{area}");
    let skew = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
<ellipse cx="50" cy="40" rx="5" ry="20" transform="skewX(20)"/></svg>"#;
    let e = svg::import(skew, &SvgImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("non-uniform scale or skew"), "{e}");
}

#[test]
fn closed_dxf_splines_are_refused_not_dropped() {
    let ctrl = [
        [0.0, 0.0],
        [5.0, 10.0],
        [15.0, 10.0],
        [20.0, 0.0],
        [0.0, 0.0],
    ];
    let knots = [0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 2.0];
    // Ends coinciding: refused at classification (all-or-nothing by default).
    let body = spline(&knots, &[], &ctrl);
    let e = dxf::import(&dxf_file(&body), &DxfImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("closed spline"), "{e}");
    // The closed flag (70 bit 1) alone is refused too.
    let flagged = spline(
        &knots,
        &[],
        &[
            [0.0, 0.0],
            [5.0, 10.0],
            [15.0, 10.0],
            [20.0, 0.0],
            [25.0, -5.0],
        ],
    )
    .replace("70\n8\n", "70\n9\n");
    let e = dxf::import(&dxf_file(&flagged), &DxfImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("closed splines"), "{e}");
    // Ends within the join tolerance (not equal): the import fails, it never
    // succeeds without the outline.
    let mut near = ctrl;
    near[4] = [1e-7, 0.0];
    let body = spline(&knots, &[], &near);
    let r = dxf::import(&dxf_file(&body), &DxfImportOptions::default());
    assert!(r.is_err(), "{:?}", r.map(|i| i.sketch.entities.len()));
}
