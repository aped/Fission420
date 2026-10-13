//! Print export dialog driven through the real widgets (docs/PRINT_EXPORT.md):
//! the parametric enclosure (printables::enclosure_workflow) is exported as one
//! 3MF with both bodies placed on the P1S bed, the lid turned over onto the
//! plate's top face (selected in the app) so the lip points up, laid out side
//! by side. The written file is re-read with the 3MF reader and checked against
//! the analytic geometry; the dialog shows the per-body report; the document
//! is unchanged.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::printables::{self, ENCLOSURE_BASELINE, enclosure_ids};
use rcad_document::{FaceRef, FaceRole, FeatureId};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1600.0, 1100.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(300), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("rcad-print-ui-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn bounds(p: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for q in p {
        for i in 0..3 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    (lo, hi)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn enclosure_3mf_with_lid_face_down_and_layout() {
    let dir = out_dir();
    let mut h = harness();
    for step in printables::enclosure_workflow(&dir.join("enclosure.rcad.json")) {
        h.state_mut().request(step.cmd.clone());
        settle(&mut h);
        let last = h.state().diagnostics().last().cloned();
        assert!(
            last.as_ref().is_none_or(|d| !d.error),
            "{:?}: {last:?}",
            step.cmd
        );
    }
    let before = h.state().summary().clone();
    assert_eq!(before.bodies.len(), 2);
    let (base, lid) = (before.bodies[0].clone(), before.bodies[1].clone());
    // Select the lid plate's top face (end cap of the plate extrusion).
    let top = FaceRef {
        feature: FeatureId(enclosure_ids::LID),
        role: FaceRole::ExtrudeEndCap,
    };
    let group = h
        .state()
        .faces()
        .iter()
        .find(|f| f.reference == Some(top))
        .map(|f| f.group)
        .expect("lid top face is selectable");
    h.state_mut().select_face(Some(group));
    let _ = h.try_run();

    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export 3MF…");
    click(&mut h, "All visible bodies (2)");
    let out = dir.join("enclosure_print.3mf");
    type_into(&mut h, "3MF path", out.to_str().unwrap());
    // Units are stated; placement controls are disabled until placement is on.
    assert!(
        h.query_by_label_contains("Units: declared in the file as millimeter")
            .is_some()
    );
    click(&mut h, "Place on bed (export only)");
    // Base row first, lid row second (export order).
    let face_down = h.get_all_by_label("Selected face down").collect::<Vec<_>>();
    assert_eq!(face_down.len(), 2);
    assert!(
        face_down[0].accesskit_node().is_disabled(),
        "face is on the lid, not the base"
    );
    assert!(!face_down[1].accesskit_node().is_disabled());
    face_down[1].click();
    settle(&mut h);
    assert!(h.query_by_label_contains("on bed: ").is_some());
    click(&mut h, "Lay out side by side along X");
    click(&mut h, "Bambu Lab P1S");
    click(&mut h, "OK");
    settle(&mut h);
    let last = h.state().diagnostics().last().cloned();
    assert!(last.as_ref().is_none_or(|d| !d.error), "{last:?}");
    // The dialog stays open with the per-body report.
    assert!(h.query_by_label_contains("Exported revision").is_some());
    assert_eq!(
        h.query_all_by_label_contains("fits bambu_p1s at the placed position")
            .count(),
        2,
        "fit per body"
    );
    assert_eq!(
        h.query_all_by_label_contains("valid solid mesh: closed yes")
            .count(),
        2,
        "mesh checks per body"
    );

    // Re-read the written file (independent of the export report).
    let bytes = std::fs::read(&out).expect("3MF written");
    let pkg = rcad_exchange::threemf::read(&bytes, &Default::default()).unwrap();
    assert_eq!(pkg.objects.len(), 2);
    let p = ENCLOSURE_BASELINE;
    let ((ol, ow), _) = p.lip();
    let base_obj = &pkg.objects[0];
    let lid_obj = &pkg.objects[1];
    assert_eq!(base_obj.name.as_deref(), Some(base.name.as_str()));
    assert_eq!(lid_obj.name.as_deref(), Some(lid.name.as_str()));
    let (blo, bhi) = bounds(&base_obj.positions);
    let (llo, lhi) = bounds(&lid_obj.positions);
    // Base rests on its floor with its modeled size; lid on z = 0, 5 mm tall.
    assert!(close(blo[2], 0.0) && close(bhi[2], p.outer_height));
    assert!(close(bhi[0] - blo[0], p.outer_length) && close(bhi[1] - blo[1], p.outer_width));
    assert!(close(llo[2], 0.0), "{llo:?}");
    assert!(close(lhi[2], p.lid_thickness + p.lip_depth), "{lhi:?}");
    assert!(close(lhi[0] - llo[0], p.outer_length) && close(lhi[1] - llo[1], p.outer_width));
    // Lip up: everything above the plate (z > t) is the lip, of the lip's outer size.
    let above: Vec<[f64; 3]> = lid_obj
        .positions
        .iter()
        .copied()
        .filter(|q| q[2] > p.lid_thickness + 1e-6)
        .collect();
    assert!(!above.is_empty(), "lip points up");
    let (alo, ahi) = bounds(&above);
    assert!(
        close(ahi[0] - alo[0], ol) && close(ahi[1] - alo[1], ow),
        "{alo:?} {ahi:?}"
    );
    // Side by side along X with a 10 mm gap, both closed with the analytic volumes.
    assert!(close(llo[0] - bhi[0], 10.0), "{bhi:?} {llo:?}");
    for (o, v) in [(base_obj, p.base_volume()), (lid_obj, p.lid_volume())] {
        let d = &o.diagnostics;
        assert!(d.closed, "{d:?}");
        assert!((d.signed_volume.unwrap() - v).abs() < 1e-3, "{d:?} vs {v}");
    }
    // Export-only: the document is unchanged.
    let after = h.state().summary().clone();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.bodies, before.bodies);
}
