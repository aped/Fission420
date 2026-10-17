//! Headless tests: camera math, projection/ray consistency, picking.

use super::*;
use crate::math::{Vec3, transform_point4};

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn approx3(a: Vec3, b: Vec3, tol: f32) -> bool {
    (0..3).all(|k| approx(a[k], b[k], tol))
}

fn cam(yaw: f32, pitch: f32) -> Camera {
    Camera {
        target: [10.0, -5.0, 20.0],
        distance: 250.0,
        yaw,
        pitch,
        fov_y: 40f32.to_radians(),
    }
}

#[test]
fn eye_position_follows_z_up_spherical_convention() {
    let c = cam(0.0, 0.0);
    assert!(approx3(c.eye(), [260.0, -5.0, 20.0], 1e-3));
    let c = cam(std::f32::consts::FRAC_PI_2, 0.0);
    assert!(approx3(c.eye(), [10.0, 245.0, 20.0], 1e-3));
    // Looking down from above: eye directly above target.
    let c = cam(0.0, 1.5);
    let e = c.eye();
    assert!(e[2] > 20.0 + 249.0 * 0.99);
}

#[test]
fn basis_is_orthonormal_and_up_has_positive_z() {
    for &(y, p) in &[(0.0, 0.0), (1.0, 0.5), (-2.0, -1.2), (3.0, 1.56)] {
        let (r, u, f) = cam(y, p).basis();
        for v in [r, u, f] {
            assert!(approx(math::length(v), 1.0, 1e-5));
        }
        assert!(approx(math::dot(r, u), 0.0, 1e-5));
        assert!(approx(math::dot(r, f), 0.0, 1e-5));
        assert!(approx(math::dot(u, f), 0.0, 1e-5));
        assert!(u[2] > 0.0, "screen-up must point toward +Z");
        // right-handed: right x up = -forward (camera looks down -Z_view)
        assert!(approx3(math::cross(r, u), math::scale(f, -1.0), 1e-5));
    }
}

#[test]
fn target_projects_to_screen_center_at_valid_depth() {
    let c = cam(0.7, 0.4);
    let m = c.view_proj(1.6, c.target, 100.0);
    let clip = transform_point4(&m, c.target);
    assert!(clip[3] > 0.0);
    assert!(approx(clip[0] / clip[3], 0.0, 1e-5));
    assert!(approx(clip[1] / clip[3], 0.0, 1e-5));
    let z = clip[2] / clip[3];
    assert!((0.0..=1.0).contains(&z), "depth {z}");
}

#[test]
fn ray_through_projected_pixel_passes_through_point() {
    let c = cam(-0.8, 0.6);
    let (w, h) = (1200.0, 700.0);
    let m = c.view_proj(w / h, c.target, 200.0);
    let p: Vec3 = [40.0, 30.0, 55.0];
    let clip = transform_point4(&m, p);
    let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
    let px = (nx + 1.0) * 0.5 * w;
    let py = (1.0 - ny) * 0.5 * h;
    let ray = c.ray(px, py, w, h).expect("valid ray");
    let to_p = math::sub(p, ray.origin);
    let along = math::dot(to_p, ray.dir);
    let closest = math::add(ray.origin, math::scale(ray.dir, along));
    assert!(approx3(closest, p, 1e-2), "{closest:?} vs {p:?}");
}

#[test]
fn clip_planes_bracket_scene_for_mm_scale_parts() {
    let mut c = Camera::default();
    let center = [0.0, 0.0, 50.0];
    c.fit_sphere(center, 87.0, 1.5);
    let (near, far) = c.clip_planes(center, 87.0);
    let d = math::length(math::sub(c.eye(), center));
    assert!(near > 0.0 && near < d - 87.0);
    assert!(far > d + 87.0);
    // Depth precision sanity: ratio far/near stays modest when framed.
    assert!(far / near < 1.0e3, "far/near = {}", far / near);
}

#[test]
fn fit_sphere_contains_sphere_in_frustum() {
    for aspect in [0.5f32, 1.0, 2.5] {
        let mut c = Camera::default();
        c.fit_sphere([5.0, 5.0, 5.0], 30.0, aspect);
        let half_v = 0.5 * c.fov_y;
        let half_h = (half_v.tan() * aspect).atan();
        let half = half_v.min(half_h);
        // Sphere subtends asin(r/d) which must be within the narrower half-FOV.
        assert!((30.0 / c.distance).asin() <= half + 1e-5);
    }
}

#[test]
fn orbit_clamps_pitch_and_zoom_clamps_distance() {
    let mut c = Camera::default();
    c.orbit(0.0, 1.0e6);
    assert!(c.pitch < std::f32::consts::FRAC_PI_2);
    c.orbit(0.0, -1.0e7);
    assert!(c.pitch > -std::f32::consts::FRAC_PI_2);
    c.zoom(1.0e9);
    assert!(c.distance > 0.0 && c.distance.is_finite());
    c.zoom(-1.0e9);
    assert!(c.distance.is_finite());
}

#[test]
fn pan_moves_target_in_view_plane() {
    let mut c = cam(0.3, 0.2);
    let before = c.target;
    let (_, _, f) = c.basis();
    c.pan(50.0, -20.0, 600.0);
    let delta = math::sub(c.target, before);
    assert!(math::length(delta) > 0.0);
    assert!(approx(math::dot(delta, f), 0.0, 1e-3));
}

#[test]
fn pick_box_faces_from_axis_rays() {
    let m = box_mesh([0.0; 3], [100.0; 3]);
    m.validate().expect("box mesh valid");
    assert_eq!(m.triangle_count(), 12);
    let cases: [(Vec3, Vec3, u32, f32); 4] = [
        ([50.0, 50.0, 500.0], [0.0, 0.0, -1.0], 5, 400.0), // +Z top
        ([50.0, 50.0, -10.0], [0.0, 0.0, 1.0], 4, 10.0),   // -Z bottom
        ([300.0, 20.0, 70.0], [-1.0, 0.0, 0.0], 1, 200.0), // +X
        ([30.0, -1.0, 60.0], [0.0, 1.0, 0.0], 2, 1.0),     // -Y
    ];
    for (origin, dir, group, dist) in cases {
        let (d, _, g) = m.pick(&Ray { origin, dir }).expect("hit");
        assert_eq!(g, group);
        assert!(approx(d, dist, 1e-3));
    }
    // Miss.
    let miss = Ray {
        origin: [150.0, 150.0, 500.0],
        dir: [0.0, 0.0, -1.0],
    };
    assert!(m.pick(&miss).is_none());
    // Ray pointing away from the box does not hit behind the origin.
    let away = Ray {
        origin: [50.0, 50.0, 500.0],
        dir: [0.0, 0.0, 1.0],
    };
    assert!(m.pick(&away).is_none());
}

#[test]
fn camera_ray_picks_top_face_of_box_seen_from_above() {
    let m = box_mesh([-50.0, -50.0, 0.0], [50.0, 50.0, 100.0]);
    let mut c = Camera {
        pitch: 1.2,
        ..Camera::default()
    };
    let b = m.bounds().expect("bounds");
    c.fit_sphere(b.center(), b.radius(), 1.0);
    // Centre pixel looks at the box centre from above: first hit is +Z face.
    let ray = c.ray(400.0, 400.0, 800.0, 800.0).expect("ray");
    let (_, _, g) = m.pick(&ray).expect("hit");
    assert_eq!(g, 5);
}

#[test]
fn box_mesh_winding_matches_outward_normals() {
    let m = box_mesh([0.0; 3], [1.0, 2.0, 3.0]);
    for (t, idx) in m.indices.as_chunks::<3>().0.iter().enumerate() {
        let [a, b, c] = [0, 1, 2].map(|k| m.positions[idx[k] as usize]);
        let n = math::cross(math::sub(b, a), math::sub(c, a));
        let stored = m.normals[idx[0] as usize];
        assert!(math::dot(n, stored) > 0.0, "triangle {t} winding inverted");
    }
}

#[test]
fn validate_rejects_bad_meshes() {
    let good = box_mesh([0.0; 3], [1.0; 3]);
    let mut m = good.clone();
    m.indices.push(0);
    assert!(matches!(
        m.validate(),
        Err(MeshError::IndexCountNotMultipleOf3(_))
    ));
    let mut m = good.clone();
    m.tri_group.pop();
    assert!(matches!(
        m.validate(),
        Err(MeshError::TriGroupLenMismatch { .. })
    ));
    let mut m = good.clone();
    m.indices[0] = 9999;
    assert!(matches!(
        m.validate(),
        Err(MeshError::IndexOutOfRange { .. })
    ));
    let mut m = good.clone();
    m.positions[0][1] = f32::NAN;
    assert!(matches!(m.validate(), Err(MeshError::NonFinitePosition(0))));
    let mut m = good;
    m.normals.pop();
    assert!(matches!(
        m.validate(),
        Err(MeshError::NormalLenMismatch { .. })
    ));
}

#[test]
fn expand_mesh_falls_back_to_flat_normals() {
    let mut m = box_mesh([0.0; 3], [1.0; 3]);
    m.normals.clear();
    let v = expand_mesh(&m);
    assert_eq!(v.len(), m.indices.len());
    for (t, tri) in v.as_chunks::<3>().0.iter().enumerate() {
        assert_eq!(tri[0].group, m.tri_group[t]);
        assert!(approx(math::length(tri[0].normal), 1.0, 1e-5));
    }
}

#[test]
fn grid_spacing_scales_with_view() {
    // Replaces the old mesh-sized grid test: the grid now adapts to zoom.
    let c = Camera::default();
    let near = GridSpec::new(None, &c, 600.0, 1.5);
    let far = GridSpec::new(
        None,
        &Camera {
            distance: c.distance * 100.0,
            ..c.clone()
        },
        600.0,
        1.5,
    );
    assert!(far.spacing >= near.spacing * 50.0);
    assert!(near.spacing * pixels_per_mm(&c, 600.0) >= MIN_CELL_PX);
}

/// Screen position of `p` relative to the target's (the screen centre).
fn screen_offset(c: &Camera, p: Vec3) -> [f32; 2] {
    let (w, h) = (800.0, 600.0);
    let [x, y] = c.project(p, w, h).expect("visible");
    [x - w / 2.0, y - h / 2.0]
}

/// Assert that world direction `d` (from the target) goes right (+x) or up
/// (-y on screen) and is near-perpendicular to the other screen axis.
fn assert_screen_dir(c: &Camera, d: Vec3, expect: [f32; 2], what: &str) {
    let p = math::add(c.target, math::scale(d, 10.0));
    let [x, y] = screen_offset(c, p);
    let len = (x * x + y * y).sqrt();
    assert!(len > 1.0, "{what}: projects to a point");
    let (ux, uy) = (x / len, y / len);
    assert!(
        approx(ux, expect[0], 0.01) && approx(uy, expect[1], 0.01),
        "{what}: screen dir ({ux}, {uy}), expected {expect:?}"
    );
}

const RIGHT: [f32; 2] = [1.0, 0.0];
const LEFT: [f32; 2] = [-1.0, 0.0];
const UP: [f32; 2] = [0.0, -1.0];
const DOWN: [f32; 2] = [0.0, 1.0];

#[test]
fn view_presets_have_documented_screen_axes() {
    let x = [1.0, 0.0, 0.0];
    let y = [0.0, 1.0, 0.0];
    let z = [0.0, 0.0, 1.0];
    // preset, axis A, A's screen dir, axis B, B's screen dir, view dir
    type Case = (ViewPreset, Vec3, [f32; 2], Vec3, [f32; 2], Vec3);
    let cases: [Case; 6] = [
        (ViewPreset::Top, x, RIGHT, y, UP, [0.0, 0.0, -1.0]),
        (ViewPreset::Bottom, x, RIGHT, y, DOWN, [0.0, 0.0, 1.0]),
        (ViewPreset::Front, x, RIGHT, z, UP, [0.0, 1.0, 0.0]),
        (ViewPreset::Back, x, LEFT, z, UP, [0.0, -1.0, 0.0]),
        (ViewPreset::Right, y, RIGHT, z, UP, [-1.0, 0.0, 0.0]),
        (ViewPreset::Left, y, LEFT, z, UP, [1.0, 0.0, 0.0]),
    ];
    for (preset, a, ad, b, bd, view) in cases {
        let mut c = cam(1.234, -0.4);
        let (t0, d0) = (c.target, c.distance);
        c.set_view(preset);
        assert_eq!(c.target, t0);
        assert_eq!(c.distance, d0);
        let name = preset.label();
        assert_screen_dir(&c, a, ad, name);
        assert_screen_dir(&c, b, bd, name);
        let (_, _, f) = c.basis();
        assert!(math::dot(f, view) > 0.9999, "{name}: forward {f:?}");
    }
    let mut c = cam(0.0, 0.0);
    c.set_view(ViewPreset::Iso);
    let d = Camera::default();
    assert!(approx(c.pitch, d.pitch, 1e-6));
    assert!(approx(c.basis().0[0], d.basis().0[0], 1e-6));
    assert_eq!(ViewPreset::ALL.len(), 7);
}

#[test]
fn look_along_normal_orients_to_plane() {
    // XY plane seen from +Z with x = +X: same as Top.
    let mut c = cam(2.0, 0.1);
    c.look_along_normal([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    assert_screen_dir(&c, [1.0, 0.0, 0.0], RIGHT, "xy");
    assert_screen_dir(&c, [0.0, 1.0, 0.0], UP, "xy");
    // Rotated sketch x axis on a Z plane still points right.
    let xa = [0.6, 0.8, 0.0];
    c.look_along_normal([0.0, 0.0, 1.0], xa);
    assert_screen_dir(&c, xa, RIGHT, "rotated xy");
    // Plane facing -Z (sketch on a bottom face): eye below, x right.
    c.look_along_normal([0.0, 0.0, -2.0], [1.0, 0.0, 0.0]);
    assert!(c.eye()[2] < c.target[2]);
    assert_screen_dir(&c, [1.0, 0.0, 0.0], RIGHT, "-z plane");
    // XZ plane with normal -Y, x = +X: front view.
    c.look_along_normal([0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    assert_screen_dir(&c, [1.0, 0.0, 0.0], RIGHT, "xz");
    assert_screen_dir(&c, [0.0, 0.0, 1.0], UP, "xz");
    // Tilted plane: view direction is exactly -normal.
    let n = math::normalize([1.0, 2.0, 3.0]).unwrap();
    c.look_along_normal(n, [1.0, 0.0, 0.0]);
    assert!(math::dot(c.basis().2, n) < -0.9999);
    // Degenerate normal leaves the camera alone.
    let before = c.clone();
    c.look_along_normal([0.0; 3], [1.0, 0.0, 0.0]);
    assert_eq!(c, before);
}

#[test]
fn zoom_at_keeps_point_under_cursor() {
    let (w, h) = (1000.0, 700.0);
    let mut c = cam(0.5, 0.5);
    let (px, py) = (820.0, 130.0);
    // Point under the cursor at target depth.
    let ray = c.ray(px, py, w, h).unwrap();
    let f = c.basis().2;
    let anchor = math::add(
        ray.origin,
        math::scale(ray.dir, c.distance / math::dot(ray.dir, f)),
    );
    let d0 = c.distance;
    c.zoom_at(300.0, px, py, w, h);
    assert!(c.distance < d0);
    let [x, y] = c.project(anchor, w, h).unwrap();
    assert!(approx(x, px, 1e-2) && approx(y, py, 1e-2), "({x}, {y})");
    c.zoom_at(-600.0, px, py, w, h);
    assert!(c.distance > d0);
    let [x, y] = c.project(anchor, w, h).unwrap();
    assert!(approx(x, px, 1e-2) && approx(y, py, 1e-2), "({x}, {y})");
    // Zooming at the centre does not move the target.
    let t = c.target;
    c.zoom_at(100.0, w / 2.0, h / 2.0, w, h);
    assert!(approx3(c.target, t, 1e-3));
    // Invalid viewport size falls back to plain zoom.
    c.zoom_at(100.0, 0.0, 0.0, 0.0, 0.0);
    assert!(approx3(c.target, t, 1e-3));
}

#[test]
fn pixels_per_mm_matches_projection() {
    let c = cam(0.3, 0.7);
    let h = 640.0;
    let ppm = pixels_per_mm(&c, h);
    // A 1 mm step along screen-right at target depth spans ppm pixels.
    let (r, _, _) = c.basis();
    let a = c.project(c.target, 900.0, h).unwrap();
    let b = c
        .project(math::add(c.target, math::scale(r, 10.0)), 900.0, h)
        .unwrap();
    assert!(approx((b[0] - a[0]) / 10.0, ppm, 1e-3 * ppm));
    assert_eq!(pixels_per_mm(&c, 0.0), 0.0);
    assert_eq!(pixels_per_mm(&c, f32::NAN), 0.0);
}

#[test]
fn selection_flags_mark_selected_groups() {
    let m = box_mesh([0.0; 3], [1.0; 3]);
    let v = expand_mesh(&m);
    let groups: Vec<u32> = v.iter().map(|x| x.group).collect();
    let f = selection_flags(&groups, &[1, 5]);
    assert_eq!(f.len(), v.len());
    for (g, fl) in groups.iter().zip(&f) {
        assert_eq!(*fl == FLAG_SELECTED, *g == 1 || *g == 5);
    }
    assert!(selection_flags(&groups, &[]).iter().all(|&x| x == 0));
}

#[test]
fn nav_help_mentions_all_bindings() {
    for word in ["orbit", "pan", "zoom", "Shift"] {
        assert!(NAV_HELP.contains(word));
    }
}

#[test]
fn project_is_inverse_of_ray() {
    let c = cam(0.7, 0.45);
    let (w, h) = (900.0, 600.0);
    for p in [[0.0, 0.0, 0.0], [30.0, -12.0, 10.0], [-40.0, 25.0, 0.0]] {
        let [px, py] = c.project(p, w, h).expect("in front");
        let ray = c.ray(px, py, w, h).expect("ray");
        // Distance from p to the ray line.
        let rel = math::sub(p, ray.origin);
        let along = math::dot(rel, ray.dir);
        let off = math::sub(rel, math::scale(ray.dir, along));
        assert!(
            math::length(off) < 1e-3,
            "{p:?} off by {}",
            math::length(off)
        );
    }
    // Target lands at the centre; points behind the eye do not project.
    let [cx, cy] = c.project(c.target, w, h).unwrap();
    assert!(approx(cx, 450.0, 1e-3) && approx(cy, 300.0, 1e-3));
    let behind = math::add(c.eye(), math::scale(c.basis().2, -10.0));
    assert!(c.project(behind, w, h).is_none());
    assert!(c.project([0.0; 3], 0.0, h).is_none());
}

#[test]
fn ray_plane_hits_xy_and_offset_planes() {
    let c = cam(-0.6, 0.8);
    let ray = c.ray(400.0, 300.0, 800.0, 600.0).unwrap();
    let hit = ray_plane(&ray, [0.0, 0.0, 20.0], [0.0, 0.0, 1.0]).unwrap();
    // The centre ray passes through the target, which lies on z = 20.
    assert!((hit[0] - 10.0).abs() < 1e-3 && (hit[1] + 5.0).abs() < 1e-3);
    assert!((hit[2] - 20.0).abs() < 1e-9);
    // Unnormalised normal gives the same hit.
    let hit2 = ray_plane(&ray, [0.0, 0.0, 20.0], [0.0, 0.0, 7.0]).unwrap();
    assert!((0..3).all(|k| (hit[k] - hit2[k]).abs() < 1e-9));
    // Parallel ray, plane behind the origin, and zero normal all miss.
    let flat = Ray {
        origin: [0.0, 0.0, 5.0],
        dir: [1.0, 0.0, 0.0],
    };
    assert!(ray_plane(&flat, [0.0; 3], [0.0, 0.0, 1.0]).is_none());
    let up = Ray {
        origin: [0.0, 0.0, 5.0],
        dir: [0.0, 0.0, 1.0],
    };
    assert!(ray_plane(&up, [0.0; 3], [0.0, 0.0, 1.0]).is_none());
    assert!(ray_plane(&up, [0.0, 0.0, 9.0], [0.0; 3]).is_none());
}

#[test]
fn overlay_projects_polylines_and_dashes() {
    let c = Camera::default();
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
    let square = vec![
        [-10.0, -10.0, 0.0],
        [10.0, -10.0, 0.0],
        [10.0, 10.0, 0.0],
        [-10.0, 10.0, 0.0],
    ];
    let mut ov = Overlay {
        polylines: vec![OverlayPolyline {
            points: square,
            closed: true,
            color: egui::Color32::YELLOW,
            width: 2.0,
            dashed: false,
        }],
        labels: vec![],
    };
    let solid = overlay_shapes(&c, rect, &ov);
    assert_eq!(solid.len(), 1);
    let egui::Shape::Path(p) = &solid[0] else {
        panic!("expected a path");
    };
    // Closed: first point repeated; all inside the rect around its centre.
    assert_eq!(p.points.len(), 5);
    assert_eq!(p.points[0], p.points[4]);
    assert!(p.points.iter().all(|q| rect.contains(*q)));
    ov.polylines[0].dashed = true;
    assert!(overlay_shapes(&c, rect, &ov).len() > 4);
    // A polyline entirely behind the eye draws nothing.
    let behind = math::add(c.eye(), math::scale(c.basis().2, -5.0));
    ov.polylines[0].points = vec![behind, math::add(behind, [1.0, 0.0, 0.0])];
    assert!(overlay_shapes(&c, rect, &ov).is_empty());
}

/// A fine grid surface with ridges: `n` x `n` quads (2 triangles each) over
/// [0, 100]², z a smooth bump, one group per row.
fn wavy_grid(n: usize) -> RenderMesh {
    let mut m = RenderMesh::default();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (100.0 * i as f32 / n as f32, 100.0 * j as f32 / n as f32);
            m.positions
                .push([x, y, 5.0 * (x * 0.05).sin() * (y * 0.07).cos()]);
        }
    }
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    for j in 0..n {
        for i in 0..n {
            m.indices.extend([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            m.indices.extend([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            m.tri_group.extend([j as u32, j as u32]);
        }
    }
    m
}

/// The viewport's hover pick through the hierarchy returns exactly what the
/// plain scan returns (same depth bits, triangle and group), with and without
/// a section plane, including rays through shared edges and vertices.
#[test]
fn hierarchy_face_pick_matches_the_plain_scan() {
    let m = wavy_grid(60);
    let scene = PickScene::new(&m, &[], &[]).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    let section = SectionPlane {
        normal: [0.6, 0.0, 0.8],
        offset: 40.0,
    };
    let mut hits = 0;
    for k in 0..4000 {
        // Every 4th ray straight down through a grid vertex (ties between triangles).
        let (x, y) = if k % 4 == 0 {
            (
                (rnd() * 60.0).floor() * 100.0 / 60.0,
                (rnd() * 60.0).floor() * 100.0 / 60.0,
            )
        } else {
            (rnd() * 120.0 - 10.0, rnd() * 120.0 - 10.0)
        };
        let dir = if k % 4 == 0 {
            [0.0, 0.0, -1.0]
        } else {
            let d = [rnd() - 0.5, rnd() - 0.5, -1.0];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            [d[0] / l, d[1] / l, d[2] / l]
        };
        let ray = Ray {
            origin: [x, y, 50.0],
            dir,
        };
        for s in [None, Some(section)] {
            let a = m.pick_section(&ray, s);
            let b = scene.pick_face_section(&ray, s);
            assert_eq!(
                a.map(|(d, t, g)| (d.to_bits(), t, g)),
                b.map(|(d, t, g)| (d.to_bits(), t, g)),
                "ray {ray:?} section {s:?}"
            );
            hits += usize::from(a.is_some());
        }
    }
    assert!(hits > 4000, "{hits}");
}

/// Hover-pick cost on a 2-million-triangle mesh: plain scan vs hierarchy.
/// `cargo test -p rcad-viewport --release hover_pick_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing report; run explicitly"]
fn hover_pick_timing() {
    let m = wavy_grid(1000);
    let t = std::time::Instant::now();
    let scene = PickScene::new(&m, &[], &[]).unwrap();
    let build = t.elapsed().as_secs_f64();
    let rays: Vec<Ray> = (0..200)
        .map(|k| Ray {
            origin: [
                (k % 20) as f32 * 5.0 + 1.3,
                (k / 20) as f32 * 10.0 + 2.1,
                50.0,
            ],
            dir: [0.0, 0.0, -1.0],
        })
        .collect();
    let t = std::time::Instant::now();
    let a: Vec<_> = rays.iter().map(|r| m.pick_section(r, None)).collect();
    let scan = t.elapsed().as_secs_f64() / rays.len() as f64;
    let t = std::time::Instant::now();
    let b: Vec<_> = rays
        .iter()
        .map(|r| scene.pick_face_section(r, None))
        .collect();
    let tree = t.elapsed().as_secs_f64() / rays.len() as f64;
    assert_eq!(a, b);
    eprintln!(
        "BENCH hover pick, {} triangles: plain scan {:.3} ms/pick, hierarchy {:.4} ms/pick \
         (built once per mesh in {:.1} ms)",
        m.triangle_count(),
        scan * 1e3,
        tree * 1e3,
        build * 1e3
    );
}

#[test]
fn thumbnail_camera_is_isometric_and_frames_every_corner() {
    for (min, max) in [
        ([-40.0, -30.0, 0.0], [40.0, 30.0, 25.0]),
        ([100.0, 200.0, -5.0], [101.0, 260.0, 5.0]),
        ([-500.0, -2.0, -2.0], [500.0, 2.0, 2.0]),
    ] {
        let b = Aabb { min, max };
        let cam = thumbnail_camera_for(b);
        let (yaw, pitch) = ViewPreset::Iso.yaw_pitch();
        let tau = std::f32::consts::TAU;
        assert!(
            (cam.yaw - yaw.rem_euclid(tau)).abs() < 1e-5,
            "{} {yaw}",
            cam.yaw
        );
        assert!((cam.pitch - pitch).abs() < 1e-6);
        assert_eq!(cam.target, b.center());
        for i in 0..8 {
            let corner = [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ];
            let [x, y] = cam.project(corner, 256.0, 256.0).expect("in front");
            assert!(
                (0.0..=256.0).contains(&x) && (0.0..=256.0).contains(&y),
                "{corner:?} -> {x},{y}"
            );
        }
    }
}
