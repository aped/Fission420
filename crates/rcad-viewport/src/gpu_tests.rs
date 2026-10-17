//! Offscreen GPU smoke test (needs a wgpu adapter; ignored by default).
//!
//! ```text
//! RCAD_VP_SNAPSHOT_DIR=/tmp/vp cargo test -p rcad-viewport --release -- --ignored gpu
//! ```
//!
//! Renders a box with a selected face, a hovered face and feature edges over
//! the adaptive grid, reads the frame back and checks colours. With
//! `RCAD_VP_SNAPSHOT_DIR` set it also writes binary PPM images for eyeballing.

use super::*;
use eframe::egui_wgpu::{self, wgpu};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn render_state() -> Option<egui_wgpu::RenderState> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration::default(),
        &instance,
        None,
        egui_wgpu::RendererOptions::default(),
    ))
    .ok()
}

fn render(vp: &mut Viewport, w: u32, h: u32) -> Vec<u8> {
    vp.last_size = [w as f32, h as f32];
    vp.last_aspect = w as f32 / h as f32;
    vp.update_grid();
    let u = vp.uniforms(vp.last_aspect);
    vp.gpu.render([w, h], &u, CLEAR_COLOR).expect("render");
    let (px, size) = vp.gpu.read_back().expect("read back");
    assert_eq!(size, [w, h]);
    px
}

fn save_ppm(name: &str, px: &[u8], w: u32, h: u32) {
    let Some(dir) = std::env::var_os("RCAD_VP_SNAPSHOT_DIR") else {
        return;
    };
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for c in px.as_chunks::<4>().0 {
        out.extend_from_slice(&c[..3]);
    }
    let path = std::path::Path::new(&dir).join(name);
    std::fs::write(&path, out).expect("write ppm");
}

fn pixel(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2]]
}

#[test]
#[ignore = "needs a GPU adapter"]
fn gpu_renders_selection_hover_edges_and_grid() {
    let Some(rs) = render_state() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut vp = Viewport::with_render_state(&rs);
    let mesh = box_mesh([-40.0, -30.0, 0.0], [40.0, 30.0, 25.0]);
    vp.set_mesh(&mesh).expect("mesh");
    vp.set_selection(&[5]); // +Z top
    vp.set_highlight(Some(1)); // +X
    let (w, h) = (800, 600);
    vp.last_aspect = w as f32 / h as f32;
    vp.fit();
    let px = render(&mut vp, w, h);
    save_ppm("iso.ppm", &px, w, h);

    let count = |pred: &dyn Fn([u8; 3]) -> bool| {
        px.as_chunks::<4>()
            .0
            .iter()
            .filter(|c| pred([c[0], c[1], c[2]]))
            .count()
    };
    let blue = count(&|c| c[2] > 150 && c[2] > c[0] + 60);
    let orange = count(&|c| c[0] > c[1] && c[1] > c[2] && c[0] > c[2] + 60);
    assert!(blue > 1000, "selection pixels: {blue}");
    assert!(orange > 1000, "hover pixels: {orange}");
    // Edges: compare against the same frame without the edge buffer.
    vp.gpu.set_edges(&[]);
    let no_edges = render(&mut vp, w, h);
    vp.upload_edges();
    let darker = px
        .as_chunks::<4>()
        .0
        .iter()
        .zip(no_edges.as_chunks::<4>().0.iter())
        .filter(|(a, b)| u32::from(b[1]) > u32::from(a[1]) + 30)
        .count();
    // 9 visible box edges, each a few hundred px long.
    assert!(darker > 1000, "edge-darkened pixels: {darker}");
    // Hover wins over selection.
    vp.set_highlight(Some(5));
    let px2 = render(&mut vp, w, h);
    let target = vp.camera.project([0.0, 0.0, 25.0], w as f32, h as f32);
    let [tx, ty] = target.expect("top centre visible");
    let c = pixel(&px2, w, tx as u32, ty as u32);
    assert!(c[0] > c[2], "top face should be hover orange, got {c:?}");

    // Top view, dimmed, sketch grid on the top face.
    vp.set_highlight(None);
    vp.set_view(ViewPreset::Top);
    vp.set_dimmed(true);
    vp.set_grid_plane(Some(GridPlane {
        origin: [0.0, 0.0, 25.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    }))
    .expect("plane");
    let px3 = render(&mut vp, w, h);
    save_ppm("top_dimmed_grid.ppm", &px3, w, h);
    assert!(vp.grid_spacing() * vp.pixels_per_mm() >= MIN_CELL_PX);
}

#[test]
#[ignore = "needs a GPU adapter"]
fn gpu_thumbnail_is_clean_fitted_and_leaves_the_view_alone() {
    let Some(rs) = render_state() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut vp = Viewport::with_render_state(&rs);
    assert!(vp.render_thumbnail(64).is_none(), "no mesh, no thumbnail");
    let mesh = box_mesh([-40.0, -30.0, 0.0], [40.0, 30.0, 25.0]);
    vp.set_mesh(&mesh).expect("mesh");
    // Everything a thumbnail must not show: selection, hover, dimming, a section,
    // a zoomed-in top view of one corner.
    vp.set_selection(&[5]);
    vp.set_highlight(Some(1));
    vp.set_dimmed(true);
    vp.set_section(Some(SectionPlane {
        normal: [1.0, 0.0, 0.0],
        offset: 0.0,
    }));
    vp.set_view(ViewPreset::Top);
    vp.fit_to([40.0, 30.0, 25.0], 2.0);
    let before = vp.camera.clone();
    let px = vp.render_thumbnail(256).expect("thumbnail");
    assert_eq!(px.len(), 256 * 256 * 4);
    save_ppm("thumbnail.ppm", &px, 256, 256);
    assert_eq!(vp.camera, before, "the window's camera is untouched");
    let count = |pred: &dyn Fn([u8; 3]) -> bool| {
        px.as_chunks::<4>()
            .0
            .iter()
            .filter(|c| pred([c[0], c[1], c[2]]))
            .count()
    };
    // Identical to the thumbnail of an undecorated view of the same mesh.
    let mut plain = Viewport::with_render_state(&rs);
    plain.set_mesh(&mesh).expect("mesh");
    assert!(
        plain.render_thumbnail(256).expect("plain") == px,
        "selection, hover, dimming, section and camera must not show"
    );
    // Light backdrop in the corners, the body (whole, not sectioned) in the middle
    // and on both sides of the section plane.
    let bg = |c: [u8; 3]| c.iter().all(|&v| v > 200);
    for (x, y) in [(1, 1), (254, 1), (1, 254), (254, 254)] {
        assert!(bg(pixel(&px, 256, x, y)), "corner {x},{y}");
    }
    let cam = vp.thumbnail_camera().unwrap();
    for p in [[-20.0, 0.0, 25.0], [20.0, 0.0, 25.0]] {
        let [x, y] = cam.project(p, 256.0, 256.0).unwrap();
        assert!(!bg(pixel(&px, 256, x as u32, y as u32)), "body at {p:?}");
    }
    // The body fills a good part of the square.
    let body = count(&|c| !c.iter().all(|&v| v > 200));
    assert!(body > 256 * 256 / 5, "body pixels: {body}");
}
