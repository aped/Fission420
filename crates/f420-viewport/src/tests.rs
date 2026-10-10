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
fn grid_spacing_scales_with_model() {
    let small = Aabb::from_points([[0.0; 3], [10.0; 3]]);
    let big = Aabb::from_points([[0.0; 3], [1000.0; 3]]);
    let (_, bs) = grid_lines(small);
    let (_, bb) = grid_lines(big);
    assert!(bs.max[0] >= 10.0 && bs.max[0] < 100.0);
    assert!(bb.max[0] >= 1000.0 && bb.max[0] < 10000.0);
}
