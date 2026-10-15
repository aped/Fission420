//! Application bar and ribbon: SOLID groups in Fusion order with promoted
//! icon buttons (accessible name = command label), every former
//! inspector-only action reachable from the ribbon with an enabled rule that
//! follows the selection, the contextual SKETCH tab with FINISH SKETCH, the
//! 1280 px layout (the ribbon does not wrap into the view), and preferences
//! saved to their file.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::shell::registry::{self, Group, Place, Tab};
use rcad::shell::{CmdId, KeymapLayer};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

fn harness(w: f32, h: f32) -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([w, h])
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
        assert!(start.elapsed() < Duration::from_secs(120), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn disabled(h: &Harness<'_, RcadApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// A 40 × 30 × 10 plate; returns the top face's display group.
fn plate(h: &mut Harness<'_, RcadApp>) -> u32 {
    click(h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: Some([20.0, 15.0]),
        },
    });
    h.state_mut()
        .request(Command::extrude(None, rcad_document::FeatureId(1), 10.0));
    settle(h);
    h.state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && (p.point[2] - 10.0).abs() < 1e-9)
        })
        .map(|f| f.group)
        .expect("top face")
}

#[test]
fn solid_tab_groups_are_in_fusion_order() {
    assert_eq!(
        registry::groups(Tab::Solid),
        [
            Group::Create,
            Group::Modify,
            Group::Assemble,
            Group::Configure,
            Group::Construct,
            Group::Inspect,
            Group::Insert,
            Group::Select,
        ]
    );
    // Former inspector-only actions all have a SOLID ribbon placement.
    for id in [
        CmdId::Hole,
        CmdId::Fillet,
        CmdId::Chamfer,
        CmdId::Shell,
        CmdId::Align,
        CmdId::DirectEdit,
        CmdId::MoveCopy,
        CmdId::Scale,
        CmdId::SplitBody,
        CmdId::Combine,
        CmdId::Material,
        CmdId::Pattern,
        CmdId::Mirror,
    ] {
        let d = registry::def(id);
        assert!(
            d.places.iter().any(|p| matches!(
                p,
                Place::Ribbon {
                    tab: Tab::Solid,
                    ..
                }
            )),
            "{} is not on the SOLID ribbon",
            d.label
        );
    }
}

#[test]
fn ribbon_commands_follow_the_selection() {
    let mut h = harness(1280.0, 800.0);
    // No document: modelling commands are disabled, document commands not.
    for l in ["Extrude…", "Hole", "Fillet", "Shell", "Move/Copy"] {
        assert!(disabled(&h, l), "{l} enabled without a document");
    }
    assert!(!disabled(&h, "New"));
    let top = plate(&mut h);
    // No face selected: face commands stay disabled with the reason.
    assert!(disabled(&h, "Hole"));
    assert_eq!(
        h.state().command_enabled(CmdId::Hole),
        Err("Select a planar face in the view first".into())
    );
    h.state_mut().select_face(Some(top));
    let _ = h.try_run();
    for l in ["Hole", "Fillet", "Chamfer", "Shell"] {
        assert!(!disabled(&h, l), "{l} disabled with a planar face selected");
    }
    // Hole from the ribbon opens the hole dialog on that face; OK cuts it.
    click(&mut h, "Hole");
    h.get_by_label("Hole centre X (face mm)");
    click(&mut h, "OK");
    let want = 12000.0 - PI * 2.75 * 2.75 * 10.0;
    let v = h.state().volume().unwrap();
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    // Shell from the ribbon with the top face open: 2 mm walls and floor.
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && (p.point[2] - 10.0).abs() < 1e-9)
        })
        .map(|f| f.group)
        .expect("top face");
    h.state_mut().select_face(Some(top));
    let _ = h.try_run();
    click(&mut h, "Shell");
    h.get_by_label_contains("Open face: Extrude1 end cap");
    click(&mut h, "Cancel");
    assert!(h.query_by_label_contains("Open face:").is_none());
    // A body selection enables the body commands.
    let body = h.state().summary().bodies[0].id;
    h.state_mut().select_face(None);
    h.state_mut().select_body(Some(body));
    let _ = h.try_run();
    assert!(!disabled(&h, "Move/Copy"));
    click(&mut h, "Move/Copy");
    h.get_by_label("ΔX (mm)");
}

#[test]
fn ribbon_fits_1280_and_sketch_tab_finishes() {
    let mut h = harness(1272.0, 768.0);
    click(&mut h, "New");
    let view = h.state().view_rect().expect("viewport");
    let promoted: Vec<&str> = registry::groups(Tab::Solid)
        .into_iter()
        .flat_map(|g| registry::in_group(Tab::Solid, g, true).map(|d| d.label))
        .collect();
    let y0 = h.get_by_label("Create Sketch").rect().center().y;
    for l in promoted {
        let r = h.get_by_label(l).rect();
        assert!(r.max.x <= 1272.0, "{l} is cut off at {r:?}");
        assert!(
            (r.center().y - y0).abs() < 1.0,
            "{l} wrapped to another row"
        );
        assert!(r.max.y <= view.min.y, "{l} overlaps the viewport");
    }
    for l in [
        "CREATE",
        "MODIFY",
        "ASSEMBLE",
        "CONFIGURE",
        "CONSTRUCT",
        "INSPECT",
        "INSERT",
        "SELECT",
    ] {
        let r = h.get_by_label(l).rect();
        assert!(r.max.x <= 1272.0 && r.max.y <= view.min.y, "{l} at {r:?}");
    }
    // Group dropdown lists the non-promoted commands.
    click(&mut h, "MODIFY");
    h.get_by_label("Split Body");
    h.get_by_label("Combine…");
    h.get_by_label("Press Pull");
    h.key_press(eframe::egui::Key::Escape);
    settle(&mut h);
    // Contextual SKETCH tab: the SOLID ribbon gives way to the sketch bar.
    click(&mut h, "Create Sketch");
    assert!(h.state().sketch_session().is_some());
    h.get_by_label("SKETCH tab");
    assert!(h.query_by_label("Extrude…").is_none());
    h.get_by_label("Line");
    let fin = h.get_by_label("FINISH SKETCH").rect();
    let help = h.get_by_label("Export").rect();
    assert!(
        fin.min.x > help.min.x - 400.0,
        "FINISH SKETCH is not at the right end"
    );
    click(&mut h, "FINISH SKETCH");
    assert!(h.state().sketch_session().is_none());
    h.get_by_label("Extrude…");
    // Other workspace tabs carry their commands.
    click(&mut h, "MESH tab");
    h.get_by_label("Repair");
    click(&mut h, "SURFACE tab");
    h.get_by_label("Patch");
    click(&mut h, "SHEET METAL tab");
    h.get_by_label("Flange");
    click(&mut h, "UTILITIES tab");
    h.get_by_label("Change Parameters");
    click(&mut h, "SOLID tab");
    h.get_by_label("Extrude…");
}

#[test]
fn preferences_are_saved_atomically_and_reloaded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r-cad").join("prefs.json");
    let mut h = harness(1280.0, 800.0);
    h.state_mut().load_prefs(path.clone());
    click(&mut h, "Preferences");
    click(&mut h, "Keys R-CAD classic");
    assert_eq!(h.state().keymap(), KeymapLayer::Classic);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"keymap\": \"classic\""), "{text}");
    assert!(text.contains("\"version\": 1"), "{text}");
    // No temporary file is left behind.
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    // A new window reads the layer back.
    let mut h2 = harness(1280.0, 800.0);
    h2.state_mut().load_prefs(path);
    assert_eq!(h2.state().keymap(), KeymapLayer::Classic);
    let _ = h2.try_run();
}

/// A dialog built from a face pick runs only at the model revision of the
/// pick: an edit applied in between makes OK a refused stale pick, and the
/// document keeps the edit and nothing else.
#[test]
fn a_pick_made_before_an_edit_is_refused_as_stale() {
    let mut h = harness(1280.0, 800.0);
    let top = plate(&mut h);
    h.state_mut().select_face(Some(top));
    let _ = h.try_run();
    click(&mut h, "Hole");
    h.get_by_label("Hole centre X (face mm)");
    // An edit lands while the dialog is open (the plate gets thicker).
    let plate_id = h.state().feature_by_name("Extrude1").unwrap().id;
    h.state_mut().request(Command::SetParameter {
        feature: plate_id,
        name: "distance".into(),
        value: 12.0,
    });
    settle(&mut h);
    let n = h.state().summary().features.len();
    click(&mut h, "OK");
    assert_eq!(h.state().summary().features.len(), n, "stale hole was added");
    assert!((h.state().volume().unwrap() - 14400.0).abs() < 1e-6);
    let last = h.state().diagnostics().last().cloned().unwrap();
    assert!(last.error, "{last:?}");
}
