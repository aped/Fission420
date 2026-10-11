//! SESSION_3 5.E frame pacing, the parts that are testable without a
//! compositor:
//! * the worker wakes the UI itself: every command reply and every deferred
//!   measurement follow-up calls the repaint callback (no pointer input, no
//!   polling by the UI);
//! * an idle UI does not keep requesting repaints (no busy loop): once the
//!   engine is idle, a frame ends with no repaint requested, and a bounded
//!   number of frames settles the harness;
//! * while work is pending the UI asks for a bounded timed repaint (50 ms),
//!   which stops once the work is done.
//!
//! Whether the compositor then delivers a frame (Wayland frame callbacks,
//! egui #8326) is outside the app; see the run-3 report.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use f420_app::F420App;
use f420_app::engine::{Engine, ExecMode};
use f420_commands::{Command, SketchTemplate};
use f420_document::{FeatureId, SketchPlane};

fn setup() -> Vec<Command> {
    vec![
        Command::NewDocument,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 80.0,
                height: 50.0,
                center: None,
            },
        },
        Command::extrude(None, FeatureId(1), 10.0),
    ]
}

#[test]
fn worker_replies_wake_the_ui_without_input() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let mut e = Engine::new(
        ExecMode::Threaded,
        Some(Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })),
    );
    assert!(e.submit_all(setup()));
    // Do not poll: the callback alone must report each reply (3 commands) and
    // the deferred measurement of the extrusion (one follow-up).
    let t0 = Instant::now();
    while wakes.load(Ordering::SeqCst) < 4 {
        assert!(
            t0.elapsed() < Duration::from_secs(120),
            "only {} wake-ups",
            wakes.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut replies = 0;
    let mut followups = 0;
    while let Some(r) = e.poll() {
        if r.followup {
            followups += 1;
        } else {
            replies += 1;
        }
    }
    assert_eq!(replies, 3);
    assert_eq!(followups, 1);
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        replies + followups,
        "one wake per reply"
    );
}

#[test]
fn idle_ui_requests_no_repaint_and_busy_ui_polls_bounded() {
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_max_steps(16)
        .build_ui_state(
            |ui, app: &mut F420App| app.ui(ui),
            F420App::new(ExecMode::Threaded, None),
        );
    h.get_by_label("New").click();
    let _ = h.try_run();
    for c in setup().into_iter().skip(1) {
        h.state_mut().request(c);
    }
    // Busy: the frame asks for a timed repaint (bounded polling while the
    // worker runs, in addition to the worker's own wake-up).
    h.step();
    let busy_seen = h.state().busy() || h.state().summary().measuring;
    let t0 = Instant::now();
    while !h.state().idle() {
        h.step();
        assert!(t0.elapsed() < Duration::from_secs(120), "never idle");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(busy_seen);
    assert_eq!(h.state().summary().bodies.len(), 1);
    // Idle: a couple of frames to settle layout, then no repaint is requested
    // and the harness settles well below its step limit.
    h.step();
    h.step();
    let steps = h
        .try_run()
        .expect("idle UI settles (no continuous repaint)");
    assert!(steps <= 4, "{steps} frames to settle");
    h.step();
    assert!(
        !h.ctx.has_requested_repaint(),
        "an idle frame must not request another repaint"
    );
}
