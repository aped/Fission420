//! Autosave/recovery through the real app: a dirty document is
//! copied to the recovery directory, a "crashed" session's copy is offered and
//! opens as an unsaved document, and saving removes the copy; a copy older
//! than the saved file it came from is flagged and the saved file offered.
//! Own test binary:
//! it sets process environment variables.
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

fn harness() -> Harness<'static, RcadApp> {
    let mut app = RcadApp::new(ExecMode::Sync, None);
    app.enable_autosave();
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut RcadApp| app.ui(ui), app)
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn copies(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    rcad::recovery::find_copies(dir)
}

#[test]
fn autosave_crash_recover_and_save() {
    let base = std::env::temp_dir().join(format!("rcad-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    struct D(std::path::PathBuf);
    impl D {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    let dir = D(base);
    // SAFETY: the only test in this binary; set before any app (or other
    // thread) reads the environment.
    unsafe {
        std::env::set_var("RCAD_RECOVERY_DIR", dir.path());
        std::env::set_var("RCAD_AUTOSAVE_SECS", "0");
    }
    // Session 1: edit, autosave, then "crash" (no Quit).
    {
        let mut h = harness();
        h.state_mut().request(Command::NewDocument);
        h.state_mut().request(Command::CreateSketch {
            name: Some("Plate".into()),
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 30.0,
                height: 20.0,
                center: None,
            },
        });
        h.state_mut()
            .request(Command::extrude(None, rcad_document::FeatureId(1), 5.0));
        settle(&mut h);
        let c = copies(dir.path());
        assert_eq!(c.len(), 1, "{c:?}");
        let nf = rcad_document::load_file(&c[0]).unwrap();
        assert_eq!(
            nf.document.features.len(),
            2,
            "the copy has the latest edit"
        );
        assert!(
            h.state().summary().dirty,
            "the document itself stays unsaved"
        );
        assert!(h.state().summary().path.is_none());
    }
    // Session 2: the copy is offered; open it as an unsaved document.
    let mut h = harness();
    let _ = h.try_run();
    h.get_by_label_contains("had unsaved changes");
    h.get_by_label("Open copy").click();
    settle(&mut h);
    let s = h.state().summary().clone();
    assert_eq!(s.features.len(), 2);
    assert!(s.dirty && s.path.is_none());
    assert!((h.state().volume().unwrap() - 3000.0).abs() < 1e-9);
    // Saving removes the recovery copy.
    let saved = dir.path().join("saved.rcad.json");
    h.state_mut().request(Command::Save {
        path: saved.clone(),
    });
    settle(&mut h);
    assert!(saved.exists());
    assert!(copies(dir.path()).is_empty(), "{:?}", copies(dir.path()));
    drop(h);
    // Session 3: edit the saved document, autosave, "crash". The copy records
    // the saved file it came from.
    {
        let mut h = harness();
        h.state_mut().request(Command::Open {
            path: saved.clone(),
        });
        settle(&mut h);
        h.state_mut().request(Command::SetParameter {
            feature: rcad_document::FeatureId(2),
            name: "distance".into(),
            value: 7.0,
        });
        settle(&mut h);
        let c = copies(dir.path());
        assert_eq!(c.len(), 1, "{c:?}");
        let origin = rcad::recovery::copy_origin(&c[0]).expect("origin recorded");
        assert_eq!(origin, saved.canonicalize().unwrap());
        assert!(!rcad::recovery::origin_is_newer(&c[0], &origin));
    }
    // The saved file is then written again later (another instance saved
    // newer work): the copy is now older than the file it came from.
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&saved)
        .unwrap()
        .set_modified(later)
        .unwrap();
    // Session 4: the offer says so; "Open saved document" opens the saved
    // file (5 mm plate, its own path) and never the older copy's state.
    let mut h = harness();
    let _ = h.try_run();
    h.get_by_label_contains("was saved after this copy");
    h.get_by_label("Open saved document").click();
    settle(&mut h);
    let s = h.state().summary().clone();
    assert_eq!(s.path.as_deref(), Some(saved.as_path()));
    assert!(!s.dirty);
    assert!((h.state().volume().unwrap() - 3000.0).abs() < 1e-9);
    assert_eq!(
        copies(dir.path()).len(),
        1,
        "the copy stays until discarded"
    );
    let _ = std::fs::remove_dir_all(dir.path());
}
