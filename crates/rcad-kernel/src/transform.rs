//! Rigid transforms of bodies and frames (run 6: body Move/Copy, SESSION_6
//! §B "Move/Copy/Align"). Rotation about an axis through a point, then a
//! translation; exact for every surface the kernel builds (planes, cylinders,
//! cones, tori, B-splines are mapped by their control points).
use crate::{Body, Frame, KernelError, Result, guarded};
use truck_modeling::*;

/// `p ↦ R·p + t` with `R` a rotation (orthonormal, det +1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rigid {
    pub r: [[f64; 3]; 3],
    pub t: [f64; 3],
}

impl Rigid {
    pub const IDENTITY: Rigid = Rigid {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    /// Rotation by `angle` rad (right-handed) about the axis through `origin`
    /// along `axis`, followed by the translation `translate`. Refused for a
    /// zero axis with a non-zero angle or non-finite input.
    pub fn new(translate: [f64; 3], origin: [f64; 3], axis: [f64; 3], angle: f64) -> Result<Rigid> {
        if !translate
            .iter()
            .chain(origin.iter())
            .chain(axis.iter())
            .chain(std::iter::once(&angle))
            .all(|v| v.is_finite())
        {
            return Err(KernelError::InvalidInput(
                "transform: all values must be finite".into(),
            ));
        }
        let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        let r = if angle == 0.0 {
            Rigid::IDENTITY.r
        } else {
            if n < 1e-12 {
                return Err(KernelError::InvalidInput(
                    "transform: the rotation axis has zero length".into(),
                ));
            }
            let (x, y, z) = (axis[0] / n, axis[1] / n, axis[2] / n);
            let (s, c) = angle.sin_cos();
            let v = 1.0 - c;
            [
                [c + x * x * v, x * y * v - z * s, x * z * v + y * s],
                [y * x * v + z * s, c + y * y * v, y * z * v - x * s],
                [z * x * v - y * s, z * y * v + x * s, c + z * z * v],
            ]
        };
        // p' = R (p − o) + o + translate.
        let ro = mul(&r, origin);
        let t = [
            origin[0] - ro[0] + translate[0],
            origin[1] - ro[1] + translate[1],
            origin[2] - ro[2] + translate[2],
        ];
        Ok(Rigid { r, t })
    }

    /// Uniform scaling by `factor` (> 0) about `center` (run 6: body Scale).
    /// The matrix is then `factor · I`; [`Rigid::frame`] keeps frame axes unit.
    pub fn scaling(center: [f64; 3], factor: f64) -> Result<Rigid> {
        if !(factor.is_finite() && factor > 0.0) || !center.iter().all(|v| v.is_finite()) {
            return Err(KernelError::InvalidInput(format!(
                "scale: the factor must be a finite number > 0, got {factor}"
            )));
        }
        Ok(Rigid {
            r: [[factor, 0.0, 0.0], [0.0, factor, 0.0], [0.0, 0.0, factor]],
            t: [
                center[0] * (1.0 - factor),
                center[1] * (1.0 - factor),
                center[2] * (1.0 - factor),
            ],
        })
    }

    pub fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let q = mul(&self.r, p);
        [q[0] + self.t[0], q[1] + self.t[1], q[2] + self.t[2]]
    }

    pub fn vector(&self, v: [f64; 3]) -> [f64; 3] {
        mul(&self.r, v)
    }

    /// `self` after `first` (apply `first`, then `self`).
    pub fn after(&self, first: &Rigid) -> Rigid {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, x) in row.iter_mut().enumerate() {
                *x = (0..3).map(|k| self.r[i][k] * first.r[k][j]).sum();
            }
        }
        Rigid {
            r,
            t: self.point(first.t),
        }
    }

    pub fn frame(&self, f: &Frame) -> Frame {
        let unit = |v: [f64; 3]| {
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / n, v[1] / n, v[2] / n]
        };
        Frame {
            origin: self.point(f.origin),
            x: unit(self.vector(f.x)),
            y: unit(self.vector(f.y)),
            z: unit(self.vector(f.z)),
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Rigid::IDENTITY
    }
}

fn mul(r: &[[f64; 3]; 3], p: [f64; 3]) -> [f64; 3] {
    [
        r[0][0] * p[0] + r[0][1] * p[1] + r[0][2] * p[2],
        r[1][0] * p[0] + r[1][1] * p[1] + r[1][2] * p[2],
        r[2][0] * p[0] + r[2][1] * p[1] + r[2][2] * p[2],
    ]
}

/// `body` moved by `t` (a new, independent body).
pub fn transform_body(body: &Body, t: &Rigid) -> Result<Body> {
    let m = Matrix4::from_cols(
        Vector4::new(t.r[0][0], t.r[1][0], t.r[2][0], 0.0),
        Vector4::new(t.r[0][1], t.r[1][1], t.r[2][1], 0.0),
        Vector4::new(t.r[0][2], t.r[1][2], t.r[2][2], 0.0),
        Vector4::new(t.t[0], t.t[1], t.t[2], 1.0),
    );
    let solid = guarded("transform body", || builder::transformed(&body.solid, m))?;
    Ok(Body { solid })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Profile, extrude, measure};

    #[test]
    fn rotate_and_translate_a_block() {
        let b = extrude(
            &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
            &Frame::XY,
            10.0,
        )
        .unwrap();
        // 90° about Z through the origin, then +100 X.
        let t = Rigid::new(
            [100.0, 0.0, 0.0],
            [0.0; 3],
            [0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap();
        let m = transform_body(&b, &t).unwrap();
        let bb = crate::bounds(&m).unwrap();
        let want_min = [70.0, 0.0, 0.0];
        let want_max = [100.0, 40.0, 10.0];
        for i in 0..3 {
            assert!((bb.min[i] - want_min[i]).abs() < 1e-9, "{bb:?}");
            assert!((bb.max[i] - want_max[i]).abs() < 1e-9, "{bb:?}");
        }
        let v = |b: &Body| match measure(b).unwrap().volume {
            crate::VolumeResult::Value { volume, .. } => volume,
            v => panic!("{v:?}"),
        };
        assert!((v(&m) - 12000.0).abs() < 1e-6);
        // Composition matches applying both.
        let u = Rigid::new([0.0, 0.0, 5.0], [0.0; 3], [1.0, 0.0, 0.0], 0.0).unwrap();
        let p = [1.0, 2.0, 3.0];
        assert_eq!(u.after(&t).point(p), u.point(t.point(p)));
        assert!(Rigid::new([0.0; 3], [0.0; 3], [0.0; 3], 1.0).is_err());
    }
}
