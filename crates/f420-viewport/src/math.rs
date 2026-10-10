//! Minimal f32 vector/matrix helpers for display-only math.
//!
//! Authoritative geometry is f64 elsewhere; everything here is derived
//! display state, so f32 is appropriate. Matrices are column-major
//! (`m[col][row]`), matching WGSL `mat4x4<f32>` layout.

pub type Vec3 = [f32; 3];
pub type Mat4 = [[f32; 4]; 4];

#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// Normalize, returning `None` for zero-length or non-finite input.
#[inline]
pub fn normalize(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    if l.is_finite() && l > f32::MIN_POSITIVE {
        Some(scale(a, 1.0 / l))
    } else {
        None
    }
}

/// `a * b` (column-major).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (c, out_col) in out.iter_mut().enumerate() {
        for (r, out_val) in out_col.iter_mut().enumerate() {
            *out_val = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

#[cfg(test)]
/// Transform a point (w = 1) and return homogeneous clip coordinates.
pub fn transform_point4(m: &Mat4, p: Vec3) -> [f32; 4] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut out = [0.0f32; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    out
}

/// Right-handed perspective projection with wgpu's 0..1 depth range.
pub fn perspective_rh_zo(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (0.5 * fov_y).tan();
    let nf = near - far;
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / nf, -1.0],
        [0.0, 0.0, near * far / nf, 0.0],
    ]
}

/// Right-handed view matrix from an orthonormal camera basis.
/// `right`, `up`, `forward` must be orthonormal.
pub fn view_from_basis(eye: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    [
        [right[0], up[0], -forward[0], 0.0],
        [right[1], up[1], -forward[1], 0.0],
        [right[2], up[2], -forward[2], 0.0],
        [-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0],
    ]
}
