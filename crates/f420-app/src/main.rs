//! Fission420 demo shell (temporary): viewport with a hard-coded test cube.
//!
//! `--screenshot <path.png>` renders a few frames, saves a window
//! screenshot as PNG, and exits (non-zero on failure). Used for automated
//! UI evidence.

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use f420_viewport::{RenderMesh, Viewport, box_mesh};

/// Frames to render before requesting the screenshot (lets layout settle).
const SCREENSHOT_AFTER_FRAMES: u32 = 5;
/// Give up if the screenshot never arrives.
const SCREENSHOT_TIMEOUT_FRAMES: u32 = 600;

struct Args {
    screenshot: Option<PathBuf>,
    /// Force-highlight a face group (for screenshot evidence of highlighting).
    highlight: Option<u32>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args_os().skip(1);
    let mut out = Args {
        screenshot: None,
        highlight: None,
    };
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--screenshot") => {
                let p = args
                    .next()
                    .ok_or_else(|| "--screenshot requires a path".to_string())?;
                out.screenshot = Some(PathBuf::from(p));
            }
            Some("--highlight") => {
                let g = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
                    .ok_or_else(|| "--highlight requires a group id (u32)".to_string())?;
                out.highlight = Some(g);
            }
            Some("-h" | "--help") => {
                return Err(
                    "usage: f420-app [--screenshot <path.png>] [--highlight <group>]".to_string(),
                );
            }
            _ => return Err(format!("unknown argument: {}", a.to_string_lossy())),
        }
    }
    Ok(out)
}

#[derive(Default)]
enum ScreenshotState {
    #[default]
    Off,
    Pending {
        path: PathBuf,
        frame: u32,
        requested: bool,
    },
}

struct DemoApp {
    viewport: Option<Viewport>,
    mesh_status: String,
    screenshot: ScreenshotState,
    forced_highlight: Option<u32>,
    /// Shared with `main` so screenshot failures produce a non-zero exit.
    result: std::sync::Arc<std::sync::Mutex<Result<(), String>>>,
}

fn demo_mesh() -> RenderMesh {
    // 100 mm cube sitting on the XY plane, centred on the Z axis.
    box_mesh([-50.0, -50.0, 0.0], [50.0, 50.0, 100.0])
}

impl DemoApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        screenshot: Option<PathBuf>,
        forced_highlight: Option<u32>,
        result: std::sync::Arc<std::sync::Mutex<Result<(), String>>>,
    ) -> Self {
        let mut viewport = Viewport::new(cc);
        let mesh_status = match viewport.as_mut() {
            Some(vp) => match vp.set_mesh(&demo_mesh()) {
                Ok(()) => {
                    vp.fit();
                    "test cube: 12 triangles, 6 faces".to_string()
                }
                Err(e) => format!("mesh rejected: {e}"),
            },
            None => "wgpu render state unavailable".to_string(),
        };
        Self {
            viewport,
            mesh_status,
            screenshot: screenshot
                .map(|path| ScreenshotState::Pending {
                    path,
                    frame: 0,
                    requested: false,
                })
                .unwrap_or_default(),
            forced_highlight,
            result,
        }
    }

    fn set_result(&self, r: Result<(), String>) {
        if let Ok(mut g) = self.result.lock() {
            *g = r;
        }
    }

    fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let ScreenshotState::Pending {
            path,
            frame,
            requested,
        } = &mut self.screenshot
        else {
            return;
        };
        *frame += 1;
        ctx.request_repaint();

        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let path = path.clone();
            let r = save_png(&path, &image);
            match &r {
                Ok(()) => eprintln!("screenshot saved to {}", path.display()),
                Err(e) => eprintln!("screenshot failed: {e}"),
            }
            self.set_result(r);
            self.screenshot = ScreenshotState::Off;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !*requested && *frame >= SCREENSHOT_AFTER_FRAMES {
            *requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        } else if *frame >= SCREENSHOT_TIMEOUT_FRAMES {
            self.set_result(Err("timed out waiting for screenshot event".to_string()));
            self.screenshot = ScreenshotState::Off;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drive_screenshot(&ctx);

        egui::Panel::left("f420_left_panel")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.heading("Fission420");
                ui.label("Browser (placeholder)");
                ui.separator();
                if ui.button("Fit").clicked()
                    && let Some(vp) = self.viewport.as_mut()
                {
                    vp.fit();
                }
                ui.separator();
                ui.label(&self.mesh_status);
                let hovered = self.viewport.as_ref().and_then(Viewport::hovered_group);
                ui.label(match hovered {
                    Some(g) => format!("hovered face group: {g}"),
                    None => "hovered face group: none".to_string(),
                });
                ui.separator();
                ui.small("LMB/MMB drag: orbit\nRMB or Shift+LMB drag: pan\nScroll: zoom");
            });

        egui::CentralPanel::no_frame().show(ui, |ui| match self.viewport.as_mut() {
            Some(vp) => {
                vp.ui(ui);
                let h = self.forced_highlight.or(vp.hovered_group());
                vp.set_highlight(h);
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label("3D viewport unavailable: eframe is not using the wgpu renderer");
                });
            }
        });
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
    let result = std::sync::Arc::new(std::sync::Mutex::new(Ok(())));
    let result_app = result.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Fission420")
            .with_app_id("fission420")
            .with_inner_size([1280.0, 800.0]),
        // The viewport renders to its own offscreen depth buffer.
        depth_buffer: 0,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let (screenshot, highlight) = (args.screenshot, args.highlight);
    let run = eframe::run_native(
        "Fission420",
        options,
        Box::new(move |cc| {
            Ok(Box::new(DemoApp::new(
                cc, screenshot, highlight, result_app,
            )))
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
