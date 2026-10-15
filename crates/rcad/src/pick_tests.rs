//! Click picking (`face_at` / `face_hit`) goes through the scene's picking
//! hierarchy: it must return exactly what the plain scan of the display mesh
//! returns, with and without a section plane.
use crate::app::RcadApp;
use crate::engine::ExecMode;
use crate::section::{SectionAxis, SectionState};
use rcad_commands::Command;
use rcad_viewport::{Camera, Ray};
use std::path::Path;

fn open_example(name: &str) -> RcadApp {
    let mut app = RcadApp::new(ExecMode::Sync, None);
    app.request(Command::Open {
        path: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name),
        safe_mode: false,
    });
    while let Some(r) = app.engine.poll() {
        app.handle_reply(r);
    }
    assert!(
        app.scene.mesh.triangle_count() > 0,
        "{name}: no display mesh"
    );
    app
}

/// Rays of a 40 x 30 pixel grid from 12 view directions around the model.
fn view_rays(app: &RcadApp) -> Vec<Ray> {
    let b = app.scene.mesh.bounds().unwrap();
    let c = [0, 1, 2].map(|k| 0.5 * (b.min[k] + b.max[k]));
    let r = (0..3)
        .map(|k| (b.max[k] - b.min[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    let mut rays = Vec::new();
    for v in 0..12 {
        let cam = Camera {
            target: c,
            distance: 2.0 * r,
            yaw: v as f32 * 0.55,
            pitch: (v as f32 * 0.37).sin() * 1.2,
            ..Camera::default()
        };
        for i in 0..40 {
            for j in 0..30 {
                let (x, y) = (i as f32 * 20.0 + 3.0, j as f32 * 20.0 + 7.0);
                rays.extend(cam.ray(x, y, 800.0, 600.0));
            }
        }
    }
    rays
}

#[test]
fn click_picks_match_the_plain_scan() {
    for name in ["rounded_enclosure.rcad.json", "threaded_pair.rcad.json"] {
        let mut app = open_example(name);
        assert!(app.scene_picker.is_some(), "{name}: no picking hierarchy");
        let b = app.scene.mesh.bounds().unwrap();
        let c = [0, 1, 2].map(|k| f64::from(0.5 * (b.min[k] + b.max[k])));
        let rays = view_rays(&app);
        let mut hits = 0;
        for state in [
            None,
            Some(SectionState {
                axis: SectionAxis::Z,
                offset: format!("{}", c[2]),
                flip: false,
            }),
            Some(SectionState {
                axis: SectionAxis::Face {
                    point: c,
                    normal: [0.6, 0.8, 0.0],
                },
                offset: "0".into(),
                flip: true,
            }),
        ] {
            let on = state.is_some();
            app.section = state;
            let section = app.section_plane();
            assert_eq!(section.is_some(), on);
            for ray in &rays {
                let plain = app.scene.mesh.pick_section(ray, section);
                let tree = app.pick_scene_face(ray);
                assert_eq!(
                    plain.map(|(d, t, g)| (d.to_bits(), t, g)),
                    tree.map(|(d, t, g)| (d.to_bits(), t, g)),
                    "{name}: ray {ray:?} section {section:?}"
                );
                hits += usize::from(plain.is_some());
            }
        }
        assert!(hits > 1000, "{name}: only {hits} hits");
    }
}

/// Per-click pick time on the largest example scene: plain scan vs hierarchy.
/// `cargo test -p rcad --release --lib click_pick_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing report; run explicitly"]
fn click_pick_timing() {
    let app = open_example("threaded_pair.rcad.json");
    let rays = view_rays(&app);
    let t = std::time::Instant::now();
    let a: Vec<_> = rays
        .iter()
        .map(|r| app.scene.mesh.pick_section(r, None))
        .collect();
    let scan = t.elapsed().as_secs_f64() / rays.len() as f64;
    let t = std::time::Instant::now();
    let b: Vec<_> = rays.iter().map(|r| app.pick_scene_face(r)).collect();
    let tree = t.elapsed().as_secs_f64() / rays.len() as f64;
    assert_eq!(a, b);
    eprintln!(
        "BENCH click pick, {} triangles: plain scan {:.4} ms, hierarchy {:.4} ms",
        app.scene.mesh.triangle_count(),
        scan * 1e3,
        tree * 1e3
    );
}
