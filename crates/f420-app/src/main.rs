//! Fission420 native app entry point.
//!
//! Flags:
//! - `--demo`: run the GOAL.md acceptance sequence through the UI request path.
//! - `--screenshot <path.png>`: once idle, save a window screenshot and exit
//!   (non-zero on failure).
//! - `--demo-place`: build a plate, then place an off-centre hole with
//!   synthetic pointer events through the placement tool, commit, and cut.
//! - `--demo-place-preview`: as above but stop mid-drag (preview visible).
//! - `--sync`: execute commands on the UI thread (debugging; UI blocks).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use eframe::egui;
use f420_app::{ExecMode, F420App};
use f420_viewport::Viewport;

/// Idle frames to render before requesting the screenshot (lets layout settle).
const SCREENSHOT_AFTER_IDLE_FRAMES: u32 = 5;
/// Give up if the app never becomes idle or the screenshot never arrives.
const SCREENSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(50);

struct Args {
    screenshot: Option<PathBuf>,
    demo: bool,
    /// `Some(stop_mid_drag)`.
    demo_place: Option<bool>,
    sync: bool,
    /// Save a screenshot to this directory each time F12 is pressed (live-window evidence).
    shot_dir: Option<PathBuf>,
    /// Start maximized (window coordinates = monitor coordinates in a headless compositor).
    maximized: bool,
    scene: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args_os().skip(1);
    let mut out = Args {
        screenshot: None,
        demo: false,
        demo_place: None,
        sync: false,
        shot_dir: None,
        maximized: false,
        scene: None,
    };
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--screenshot") => {
                let p = args
                    .next()
                    .ok_or_else(|| "--screenshot requires a path".to_string())?;
                out.screenshot = Some(PathBuf::from(p));
            }
            Some("--demo") => out.demo = true,
            Some("--demo-place") => out.demo_place = Some(false),
            Some("--demo-place-preview") => out.demo_place = Some(true),
            Some("--sync") => out.sync = true,
            Some("--maximized") => out.maximized = true,
            Some("--scene") => {
                let s = args
                    .next()
                    .ok_or_else(|| "--scene requires a name".to_string())?;
                out.scene = Some(s.to_string_lossy().into_owned());
            }
            Some("--shot-dir") => {
                let p = args
                    .next()
                    .ok_or_else(|| "--shot-dir requires a directory".to_string())?;
                out.shot_dir = Some(PathBuf::from(p));
            }
            Some("-h" | "--help") => {
                return Err(
                    "usage: f420-app [--demo | --demo-place | --demo-place-preview] [--sync] [--maximized] \
                     [--screenshot <path.png>] [--shot-dir <dir> (F12 saves a screenshot)] \
                     [--scene multibody|extrude|step|sideface|import|export]"
                        .to_string(),
                );
            }
            _ => return Err(format!("unknown argument: {}", a.to_string_lossy())),
        }
    }
    Ok(out)
}

struct Shot {
    path: PathBuf,
    idle_frames: u32,
    requested: bool,
    started: std::time::Instant,
}

struct Shell {
    app: F420App,
    shot: Option<Shot>,
    shot_dir: Option<PathBuf>,
    shots_taken: u32,
    f12_pending: bool,
    title: String,
    trace_input: bool,
    closing: bool,
    probe: Option<PathBuf>,
    probe_last: String,
    result: Arc<Mutex<Result<(), String>>>,
}

impl Shell {
    fn set_result(&self, r: Result<(), String>) {
        if let Ok(mut g) = self.result.lock() {
            *g = r;
        }
    }

    fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let idle = self.app.idle();
        let Some(shot) = self.shot.as_mut() else {
            return;
        };
        ctx.request_repaint();
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let path = shot.path.clone();
            let r = save_png(&path, &image);
            match &r {
                Ok(()) => eprintln!("screenshot saved to {}", path.display()),
                Err(e) => eprintln!("screenshot failed: {e}"),
            }
            self.shot = None;
            self.set_result(r);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if shot.started.elapsed() > SCREENSHOT_TIMEOUT {
            self.shot = None;
            self.set_result(Err("timed out waiting for idle state / screenshot".into()));
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        shot.idle_frames = if idle { shot.idle_frames + 1 } else { 0 };
        if !shot.requested && shot.idle_frames >= SCREENSHOT_AFTER_IDLE_FRAMES {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

impl Shell {
    /// F12: save `shot_NNN.png` into `--shot-dir` without exiting.
    fn drive_f12(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.shot_dir.clone() else {
            return;
        };
        if self.f12_pending {
            let image = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                self.shots_taken += 1;
                let path = dir.join(format!("shot_{:03}.png", self.shots_taken));
                match save_png(&path, &image) {
                    Ok(()) => eprintln!("screenshot saved to {}", path.display()),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                self.f12_pending = false;
            }
            ctx.request_repaint();
        } else if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
            self.f12_pending = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let w = u32::try_from(w).map_err(|e| e.to_string())?;
    let h = u32::try_from(h).map_err(|e| e.to_string())?;
    let buf = image::RgbaImage::from_raw(w, h, bytes)
        .ok_or_else(|| "screenshot buffer size mismatch".to_string())?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    buf.save(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `F420_TRACE_INPUT=1`: log pointer-button and key events as egui receives them
/// from the windowing system (evidence that real input reaches the window).
fn trace_input(ctx: &egui::Context) {
    ctx.input(|i| {
        for e in &i.events {
            match e {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => eprintln!(
                    "input: pointer {button:?} {} at ({:.1}, {:.1}) mods {modifiers:?}",
                    if *pressed { "down" } else { "up" },
                    pos.x,
                    pos.y
                ),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => eprintln!("input: key {key:?} mods {modifiers:?}"),
                egui::Event::Text(t) => eprintln!("input: text {t:?}"),
                _ => {}
            }
        }
    });
}

/// `F420_PROBE=<file>`: after each frame whose projection changed, write the view
/// rect, camera and open sketch frame as JSON, so an external input driver
/// (tools/live_input.py) can target sketch-plane and world coordinates with
/// the same projection the app uses.
fn write_probe(path: &std::path::Path, app: &F420App, last: &mut String) {
    let (cam, _) = app.view_camera();
    let Some(r) = app.view_rect() else { return };
    let f = |v: [f64; 3]| format!("[{},{},{}]", v[0], v[1], v[2]);
    let sketch = app.sketch_session().map_or("null".to_string(), |s| {
        format!(
            "{{\"origin\":{},\"x\":{},\"y\":{}}}",
            f(s.frame.origin),
            f(s.frame.x),
            f(s.frame.y)
        )
    });
    let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
    let widgets: Vec<String> = f420_app::probe::rects()
        .iter()
        .map(|(k, r)| {
            format!(
                "\"{}\":[{},{},{},{}]",
                esc(k),
                r.min.x,
                r.min.y,
                r.max.x,
                r.max.y
            )
        })
        .collect();
    let s = app.summary();
    let volume = app.volume().map_or("null".to_string(), |v| v.to_string());
    let editing = app.sketch_session().map_or("null".to_string(), |ss| {
        format!(
            "{{\"status\":\"{:?}\",\"dof\":{},\"entities\":{},\"dims\":{}}}",
            ss.editor.report.status,
            ss.editor.report.dof,
            ss.editor.sketch.entities.len(),
            ss.editor.sketch.dimensions().len()
        )
    });
    // Run 4: per-body volumes, parameter values and the last reply.
    let body_volume: Vec<String> = s
        .bodies
        .iter()
        .filter_map(|b| match b.measurements.as_ref()?.volume {
            f420_kernel::VolumeResult::Value { volume, .. } => {
                Some(format!("\"{}\":{volume}", b.id))
            }
            _ => None,
        })
        .collect();
    let params: Vec<String> = s
        .parameters
        .iter()
        .filter_map(|p| Some(format!("\"{}\":{}", esc(&p.name), p.value?)))
        .collect();
    let last_reply = app.last_reply().map_or("null".to_string(), |(c, ok)| {
        format!("{{\"command\":\"{}\",\"ok\":{ok}}}", esc(c))
    });
    static FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let frame = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let doc = format!(
        "{{\"frame\":{frame},\"open\":{},\"revision\":{},\"features\":{},\"dirty\":{},\"busy\":{},\"measuring\":{},\"volume\":{volume},\"confirm\":{},\"editing\":{editing},\"body_volume\":{{{}}},\"params\":{{{}}},\"last_reply\":{last_reply}}}",
        s.open,
        s.revision,
        s.features.len(),
        s.dirty,
        app.busy(),
        s.measuring,
        app.pending_confirmation().is_some(),
        body_volume.join(","),
        params.join(",")
    );
    let json = format!(
        "{{\"rect\":[{},{},{},{}],\"camera\":{{\"target\":[{},{},{}],\"distance\":{},\"yaw\":{},\"pitch\":{},\"fov_y\":{}}},\"sketch\":{sketch},\"doc\":{doc},\"widgets\":{{{}}}}}",
        r.min.x,
        r.min.y,
        r.max.x,
        r.max.y,
        cam.target[0],
        cam.target[1],
        cam.target[2],
        cam.distance,
        cam.yaw,
        cam.pitch,
        cam.fov_y,
        widgets.join(",")
    );
    if json != *last {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            *last = json;
        }
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.trace_input {
            trace_input(&ctx);
        }
        // Unsaved changes: intercept the window-manager close and ask first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.app.allow_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let title = self.app.window_title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if self.probe.is_some() {
            f420_app::probe::begin_frame();
        }
        self.app.ui(ui);
        // A confirmed exit (Discard, or Save followed by a successful save).
        if self.app.exit_confirmed() && !self.closing {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(p) = self.probe.clone() {
            write_probe(&p, &self.app, &mut self.probe_last);
        }
        self.drive_screenshot(&ctx);
        self.drive_f12(&ctx);
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    let result = Arc::new(Mutex::new(Ok(())));
    let result_app = result.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Fission420")
            .with_app_id("fission420")
            .with_inner_size([1400.0, 860.0])
            .with_maximized(args.maximized),
        // The viewport renders to its own offscreen depth buffer.
        depth_buffer: 0,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let run = eframe::run_native(
        "Fission420",
        options,
        Box::new(move |cc| {
            let mode = if args.sync {
                ExecMode::Sync
            } else {
                ExecMode::Threaded
            };
            let ctx = cc.egui_ctx.clone();
            let mut app = F420App::new(mode, Some(Box::new(move || ctx.request_repaint())));
            if std::env::var_os("F420_PROBE").is_some() {
                f420_app::probe::enable();
            }
            app.viewport = Viewport::new(cc);
            // Autosave/recovery for interactive use; scripted runs never touch
            // the recovery directory.
            if args.scene.is_none() && !args.demo && args.demo_place.is_none() {
                app.enable_autosave();
            }
            if args.demo {
                app.start_demo(std::env::temp_dir().join("fission420-demo"));
            } else if let Some(stop) = args.demo_place {
                app.start_demo_place(stop);
            } else if let Some(s) = &args.scene
                && !app.start_scene(s)
            {
                eprintln!("unknown scene '{s}'; known: {:?}", f420_app::scenes::SCENES);
            }
            Ok(Box::new(Shell {
                app,
                shot: args.screenshot.map(|path| Shot {
                    path,
                    idle_frames: 0,
                    requested: false,
                    started: std::time::Instant::now(),
                }),
                shot_dir: args.shot_dir,
                shots_taken: 0,
                f12_pending: false,
                title: String::new(),
                trace_input: std::env::var_os("F420_TRACE_INPUT").is_some_and(|v| v == "1"),
                closing: false,
                probe: std::env::var_os("F420_PROBE").map(PathBuf::from),
                probe_last: String::new(),
                result: result_app,
            }))
        }),
    );
    if let Err(e) = run {
        eprintln!("eframe error: {e}");
        return ExitCode::FAILURE;
    }
    match result.lock().map(|g| g.clone()) {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
        Err(_) => ExitCode::FAILURE,
    }
}
