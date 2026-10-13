//! Every documentation scene (`--scene`) runs to completion headlessly (kittest),
//! so screenshot captures cannot stall on a scene that never finishes.
use egui_kittest::Harness;
use rcad::{ExecMode, RcadApp};
use std::time::{Duration, Instant};

#[test]
fn every_scene_completes() {
    for scene in rcad::scenes::SCENES {
        let mut h = Harness::builder()
            .with_size([1280.0, 800.0])
            .with_max_steps(64)
            .build_ui_state(
                |ui, app: &mut RcadApp| app.ui(ui),
                RcadApp::new(ExecMode::Threaded, None),
            );
        assert!(h.state_mut().start_scene(scene), "{scene}");
        let t0 = Instant::now();
        while !(h.state().idle() && h.state().scene_fitted()) {
            let _ = h.try_run();
            assert!(
                t0.elapsed() < Duration::from_secs(60),
                "scene {scene} stalled; diagnostics {:?}",
                h.state().diagnostics().last()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
