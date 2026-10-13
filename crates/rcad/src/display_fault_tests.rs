//! Fault injection at the display boundaries (SESSION_5 §10; run-3 app
//! findings #13 `set_mesh` failure and #16 failing display tessellation, both
//! untested until now). Test doubles stand in for the GPU upload and the
//! kernel's display tessellator; the model must stay valid and current, the
//! view must keep a mesh consistent with what is drawn, and the user must see
//! an actionable error.
use crate::app::RcadApp;
use crate::engine::{ExecMode, display_scene, display_scene_with};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

fn pump(app: &mut RcadApp) {
    while let Some(r) = app.engine.poll() {
        app.handle_reply(r);
    }
}

impl RcadApp {
    fn session_evaluation(&self) -> rcad_document::Evaluation {
        self.engine
            .sync_evaluation()
            .expect("sync session with a document")
    }
}

fn two_bodies() -> RcadApp {
    let mut app = RcadApp::new(ExecMode::Sync, None);
    app.request(Command::NewDocument);
    for (c, name) in [([0.0, 0.0], "A"), ([40.0, 0.0], "B")] {
        app.request(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 10.0,
                height: 10.0,
                center: Some(c),
            },
        });
        pump(&mut app);
        let sk = app.summary().features.last().unwrap().id;
        let mut e = Command::extrude(None, sk, 5.0);
        if let Command::Extrude { body_name, .. } = &mut e {
            *body_name = Some(name.into());
        }
        app.request(e);
    }
    pump(&mut app);
    assert_eq!(app.summary().bodies.len(), 2);
    app
}

#[test]
fn failing_display_tessellation_skips_one_body_and_reports_it() {
    let app = two_bodies();
    let ev = app.session_evaluation();
    let calls = std::cell::Cell::new(0);
    let scene = display_scene_with(&ev, &|b| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Err("injected tessellation failure".into())
        } else {
            rcad_kernel::tessellate_display(b, crate::engine::DISPLAY_CHORD)
                .map_err(|e| e.to_string())
        }
    })
    .expect("scene");
    assert_eq!(scene.skipped.len(), 1);
    assert!(scene.skipped[0].starts_with("A:"), "{:?}", scene.skipped);
    assert_eq!(scene.bodies.len(), 1, "the other body still displays");
    scene.mesh.validate().expect("consistent mesh");
    // Its groups are numbered from 0, as if alone.
    let full = display_scene(&ev).expect("full scene");
    assert!(full.mesh.triangle_count() > scene.mesh.triangle_count());
    assert_eq!(scene.mesh.tri_group.iter().min(), Some(&0));
}

#[test]
fn refused_mesh_upload_keeps_the_previous_view_and_says_so() {
    let mut app = two_bodies();
    let before = (app.scene.mesh.triangle_count(), app.mesh_triangles);
    assert!(before.0 > 0);
    let revision = app.summary().revision;
    // A new display scene (one body hidden) refused by the "GPU".
    let mut ev = app.session_evaluation();
    ev.bodies.truncate(1);
    let scene = display_scene(&ev).unwrap();
    let n_new = scene.mesh.triangle_count();
    assert_ne!(n_new, before.0);
    let ok = app.accept_scene(scene.clone(), |_, _| Err("injected upload failure".into()));
    assert!(!ok);
    assert_eq!(
        (app.scene.mesh.triangle_count(), app.mesh_triangles),
        before
    );
    let d = app.diagnostics().last().expect("diagnostic");
    assert!(
        d.error && d.text.contains("injected upload failure"),
        "{d:?}"
    );
    assert!(d.text.contains("previous model"), "actionable: {}", d.text);
    // The model is untouched by a display failure.
    assert_eq!(app.summary().revision, revision);
    assert_eq!(app.summary().bodies.len(), 2);
    // A later successful upload replaces the scene.
    assert!(app.accept_scene(scene, |_, _| Ok(())));
    assert_eq!(app.mesh_triangles, n_new);
}
