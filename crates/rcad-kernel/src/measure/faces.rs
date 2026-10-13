//! Exact measurements between B-rep faces (run 6, IN01): radius/diameter of a
//! cylindrical face, distance/angle between planes, between parallel cylinder
//! axes, and between a plane and an axis — from the analytic surfaces
//! ([`crate::classify_face`]), never from a tessellation. World frame, mm,
//! degrees.
use crate::{Body, DirectFace, KernelError, Result};
use serde::{Deserialize, Serialize};

/// Directions closer than this (sine of the angle) are parallel.
pub const PARALLEL_SIN: f64 = 1.0e-9;

/// What was measured between two faces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FacePairMeasure {
    /// Two parallel planes: distance between them (mm).
    PlaneDistance { distance: f64 },
    /// Two non-parallel planes: angle between their outward normals (deg).
    PlaneAngle { degrees: f64 },
    /// Two cylinders with parallel axes: axis-to-axis distance (mm).
    AxisDistance { distance: f64 },
    /// Two cylinders with non-parallel axes: angle between the axes (deg, 0–90)
    /// and the shortest distance between the axis lines (mm).
    AxisAngle { degrees: f64, distance: f64 },
    /// A plane and a cylinder axis parallel to it: axis-to-plane distance (mm).
    PlaneAxisDistance { distance: f64 },
    /// A plane and a cylinder axis not parallel to it: angle between the axis
    /// and the plane (deg, 0–90).
    PlaneAxisAngle { degrees: f64 },
}

type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V) -> Result<V> {
    let n = norm(a);
    if !(n.is_finite() && n > 0.0) {
        return Err(KernelError::InvalidInput("zero direction".into()));
    }
    Ok(a.map(|x| x / n))
}

/// Distance from point p to the line (o, unit d).
fn point_line(p: V, o: V, d: V) -> f64 {
    norm(cross(sub(p, o), d))
}

/// Exact geometry of face `face` (B-rep face order, = display face order).
pub fn face_geometry(body: &Body, face: usize) -> Result<DirectFace> {
    crate::classify_face(body, face)
}

/// Measurement between two classified faces. Err for unsupported surfaces
/// (anything but planes and circular cylinders).
pub fn measure_face_pair(a: &DirectFace, b: &DirectFace) -> Result<FacePairMeasure> {
    use DirectFace::{Cylinder, Plane};
    let angle = |c: f64| c.clamp(-1.0, 1.0).acos().to_degrees();
    Ok(match (a, b) {
        (
            Plane {
                origin: p,
                normal: n,
            },
            Plane {
                origin: q,
                normal: m,
            },
        ) => {
            let (n, m) = (unit(*n)?, unit(*m)?);
            if norm(cross(n, m)) <= PARALLEL_SIN {
                FacePairMeasure::PlaneDistance {
                    distance: dot(sub(*q, *p), n).abs(),
                }
            } else {
                FacePairMeasure::PlaneAngle {
                    degrees: angle(dot(n, m)),
                }
            }
        }
        (
            Cylinder {
                origin: p, axis: d, ..
            },
            Cylinder {
                origin: q, axis: e, ..
            },
        ) => {
            let (d, e) = (unit(*d)?, unit(*e)?);
            let c = cross(d, e);
            if norm(c) <= PARALLEL_SIN {
                FacePairMeasure::AxisDistance {
                    distance: point_line(*q, *p, d),
                }
            } else {
                FacePairMeasure::AxisAngle {
                    degrees: angle(dot(d, e).abs()),
                    distance: dot(sub(*q, *p), unit(c)?).abs(),
                }
            }
        }
        (
            Plane {
                origin: p,
                normal: n,
            },
            Cylinder {
                origin: q, axis: d, ..
            },
        )
        | (
            Cylinder {
                origin: q, axis: d, ..
            },
            Plane {
                origin: p,
                normal: n,
            },
        ) => {
            let (n, d) = (unit(*n)?, unit(*d)?);
            let s = dot(n, d).abs();
            if s <= PARALLEL_SIN {
                FacePairMeasure::PlaneAxisDistance {
                    distance: dot(sub(*q, *p), n).abs(),
                }
            } else {
                FacePairMeasure::PlaneAxisAngle {
                    degrees: s.clamp(0.0, 1.0).asin().to_degrees(),
                }
            }
        }
        _ => {
            return Err(KernelError::InvalidInput(
                "exact face measurement supports planes and circular cylinders only".into(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(o: V, n: V) -> DirectFace {
        DirectFace::Plane {
            origin: o,
            normal: n,
        }
    }
    fn cyl(o: V, a: V, r: f64) -> DirectFace {
        DirectFace::Cylinder {
            origin: o,
            axis: a,
            radius: r,
            hole: true,
        }
    }

    #[test]
    fn pairs() {
        let m = |a, b| measure_face_pair(&a, &b).unwrap();
        assert_eq!(
            m(
                plane([0.0; 3], [0.0, 0.0, 1.0]),
                plane([3.0, 4.0, 7.5], [0.0, 0.0, -1.0])
            ),
            FacePairMeasure::PlaneDistance { distance: 7.5 }
        );
        let FacePairMeasure::PlaneAngle { degrees } = m(
            plane([0.0; 3], [0.0, 0.0, 1.0]),
            plane([0.0; 3], [0.0, 1.0, 1.0]),
        ) else {
            panic!()
        };
        assert!((degrees - 45.0).abs() < 1e-12);
        assert_eq!(
            m(
                cyl([0.0; 3], [0.0, 0.0, 1.0], 2.0),
                cyl([3.0, 4.0, 9.0], [0.0, 0.0, -1.0], 1.0)
            ),
            FacePairMeasure::AxisDistance { distance: 5.0 }
        );
        let FacePairMeasure::AxisAngle { degrees, distance } = m(
            cyl([0.0; 3], [0.0, 0.0, 1.0], 2.0),
            cyl([0.0, 0.0, 6.0], [1.0, 0.0, 0.0], 1.0),
        ) else {
            panic!()
        };
        assert!((degrees - 90.0).abs() < 1e-12 && distance.abs() < 1e-12);
        let FacePairMeasure::AxisAngle { distance, .. } = m(
            cyl([0.0; 3], [0.0, 0.0, 1.0], 2.0),
            cyl([0.0, 6.0, 0.0], [1.0, 0.0, 0.0], 1.0),
        ) else {
            panic!()
        };
        assert!((distance - 6.0).abs() < 1e-12);
        assert_eq!(
            m(
                plane([0.0, 0.0, 2.0], [0.0, 0.0, 1.0]),
                cyl([1.0, 1.0, 10.0], [1.0, 0.0, 0.0], 3.0)
            ),
            FacePairMeasure::PlaneAxisDistance { distance: 8.0 }
        );
        let FacePairMeasure::PlaneAxisAngle { degrees } = m(
            cyl([0.0; 3], [0.0, 0.0, 1.0], 1.0),
            plane([0.0; 3], [0.0, 0.0, 1.0]),
        ) else {
            panic!()
        };
        assert!((degrees - 90.0).abs() < 1e-12);
        assert!(measure_face_pair(&DirectFace::Other, &plane([0.0; 3], [0.0, 0.0, 1.0])).is_err());
    }
}
