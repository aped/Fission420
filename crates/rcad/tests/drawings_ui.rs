//! Browser "Drawings" section through the real widgets (egui_kittest, run 6;
//! docs/DRAWINGS.md): select a body, create its drawing, export it, switch
//! the projection, add an iso view, delete the hole the drawing shows and see
//! the dangling-reference report, delete the drawing.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::drawing::{Projection, ViewId};
use rcad_document::{FaceRef, FaceRole, SketchPlane};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
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
        assert!(start.elapsed() < Duration::from_secs(180), "not idle");
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

fn request(h: &mut Harness<'_, RcadApp>, c: Command) {
    h.state_mut().request(c);
    settle(h);
}

fn out(name: &str) -> PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/run6/drawings");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn create_export_and_flag_a_drawing_from_the_browser() {
    let mut h = harness();
    click(&mut h, "New");
    request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        },
    );
    let sk = h.state().summary().features[0].id;
    request(&mut h, Command::extrude(None, sk, 10.0));
    let e = h.state().summary().features[1].id;
    request(
        &mut h,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: e,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [20.0, 15.0],
            diameter: 8.0,
            depth: None,
            counterbore: None,
            target: None,
        },
    );
    let hole = h.state().summary().features[2].id;
    click(&mut h, "Drawings (0)");
    // Disabled until a body is selected.
    let name = h.state().summary().bodies[0].name.clone();
    click(&mut h, &name);
    click(&mut h, "New drawing of selected body");
    let ds = h.state().summary().drawings.clone();
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].views.len(), 3);
    let dname = ds[0].name.clone();

    let pdf = out("ui_drawing.pdf");
    let _ = std::fs::remove_file(&pdf);
    type_into(&mut h, "Drawing export path", pdf.to_str().unwrap());
    click(&mut h, &format!("Export {dname}"));
    let bytes = std::fs::read(&pdf).expect("exported");
    assert!(bytes.starts_with(b"%PDF-1.4"));
    assert!(
        String::from_utf8_lossy(&bytes).contains("(\\3308) Tj"),
        "Ø8 in the PDF"
    );

    click(&mut h, &format!("Projection of {dname}"));
    assert_eq!(
        h.state().summary().drawings[0].projection,
        Projection::FirstAngle
    );
    click(&mut h, &format!("Add iso view to {dname}"));
    assert_eq!(h.state().summary().drawings[0].views.len(), 4);
    click(&mut h, &format!("Add section to {dname}"));
    let views = h.state().summary().drawings[0].views.clone();
    assert_eq!(views.len(), 5);
    match &views[4].kind {
        rcad_document::drawing::ViewKind::Section { offset, label, .. } => {
            assert!((offset - 20.0).abs() < 1e-9, "through the middle: {offset}");
            assert_eq!(label, "A");
        }
        k => panic!("{k:?}"),
    }

    // A hole callout on the top view, then delete the hole: the next export
    // reports the dangling callout in the section (red), nothing silent.
    request(
        &mut h,
        Command::AddHoleCallout {
            drawing: ds[0].id,
            view: ViewId(2),
            feature: hole,
        },
    );
    request(&mut h, Command::DeleteFeature { feature: hole });
    let svg = out("ui_drawing.svg");
    type_into(&mut h, "Drawing export path", svg.to_str().unwrap());
    click(&mut h, &format!("Export {dname}"));
    let text = std::fs::read_to_string(&svg).expect("exported svg");
    assert!(text.contains("Ø?") && text.contains("class=\"flag\""));
    assert!(
        h.query_by_label_contains("dangling").is_some(),
        "the issue is listed in the Drawings section"
    );
    // Undo the deletion (the drawing survives), then delete the drawing.
    request(&mut h, Command::Undo);
    click(&mut h, &format!("Delete drawing {dname}"));
    assert!(h.state().summary().drawings.is_empty());
}
