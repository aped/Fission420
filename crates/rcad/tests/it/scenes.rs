//! Every documentation scene (`--scene`) runs to completion headlessly (kittest),
//! so screenshot captures cannot stall on a scene that never finishes.
//!
//! The scenes are split over [`SHARDS`] tests by their index in `SCENES` (every
//! scene in exactly one shard), so the test runner spreads them over its workers.
use egui_kittest::Harness;
use rcad::{ExecMode, RcadApp};
use std::time::{Duration, Instant};

/// Number of scene tests; shard k runs the scenes whose index is k modulo this.
const SHARDS: usize = 4;

/// Run every scene of `SCENES` whose index is `shard` modulo [`SHARDS`].
fn scenes_complete(shard: usize) {
    for (i, scene) in rcad::scenes::SCENES.iter().enumerate() {
        if i % SHARDS != shard {
            continue;
        }
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

#[test]
fn every_scene_completes_shard_0() {
    scenes_complete(0);
}

#[test]
fn every_scene_completes_shard_1() {
    scenes_complete(1);
}

#[test]
fn every_scene_completes_shard_2() {
    scenes_complete(2);
}

#[test]
fn every_scene_completes_shard_3() {
    scenes_complete(3);
}
