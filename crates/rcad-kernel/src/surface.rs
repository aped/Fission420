//! Exact surface bodies from sketches (docs/SURFACES.md): a planar
//! patch bounded by a closed profile (holes allowed), and the extrusion of a
//! profile or of an open polyline into a ruled surface (planar strips for
//! lines, exact cylindrical patches for arcs). Results are open shells.
use crate::{Body, Frame, KernelError, Profile, Result, guarded, tol};
use truck_modeling::*;

/// What a surface extrusion sweeps.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfacePath {
    /// A closed profile (lines, arcs, circle).
    Closed(Profile),
    /// An open chain of points joined by lines (at least two).
    Open(Vec<[f64; 2]>),
}

fn p3(a: [f64; 3]) -> Point3 {
    Point3::new(a[0], a[1], a[2])
}

/// The planar face bounded by `outer` minus `holes` on `frame`, as an open
/// surface body (normal along `frame.z`).
pub fn planar_patch(outer: &Profile, holes: &[Profile], frame: &Frame) -> Result<Body> {
    let face = crate::build::region_face(outer, holes, frame, 0.0)?;
    Ok(Body::from_shell_unchecked(Shell::from(vec![face])))
}

/// `path` on `frame` swept `distance` along `frame.z` (against it when
/// `reverse`) as an open ruled surface body.
///
/// Orientation: every face's normal is the path tangent ×
/// `frame.z`, i.e. to the right of the direction of travel seen from +z. A
/// closed profile (counter-clockwise, as profiles are given) therefore
/// faces outward; an open chain faces to the right of its point order (the
/// caller fixes that order; documents use `sheet::surface_chain`).
/// `reverse` changes only the side of the sketch plane the surface lies on,
/// never its normal: the wire is run backwards for the opposite sweep.
pub fn extrude_surface(
    path: &SurfacePath,
    frame: &Frame,
    distance: f64,
    reverse: bool,
) -> Result<Body> {
    if !(distance.is_finite() && distance >= tol::MIN_LENGTH) {
        return Err(KernelError::InvalidInput(format!(
            "surface extrusion distance must be >= {} mm, got {distance}",
            tol::MIN_LENGTH
        )));
    }
    let wire = match path {
        SurfacePath::Closed(p) => crate::build::profile_wire(p, frame, 0.0)?,
        SurfacePath::Open(pts) => {
            if pts.len() < 2 {
                return Err(KernelError::InvalidInput(
                    "an open surface path needs at least two points".into(),
                ));
            }
            for w in pts.windows(2) {
                if (w[0][0] - w[1][0]).hypot(w[0][1] - w[1][1]) < tol::MIN_LENGTH {
                    return Err(KernelError::InvalidInput(
                        "an open surface path has a degenerate segment".into(),
                    ));
                }
            }
            let v: Vec<Vertex> = pts
                .iter()
                .map(|q| builder::vertex(p3(frame.to_world(*q))))
                .collect();
            v.windows(2).map(|w| builder::line(&w[0], &w[1])).collect()
        }
    };
    // tsweep's faces have normal (wire tangent) × (sweep vector): sweeping
    // the reversed wire against z keeps tangent × z.
    let wire = if reverse { wire.inverse() } else { wire };
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2])
        * if reverse { -distance } else { distance };
    let shell: Shell = guarded("extrude surface", || builder::tsweep(&wire, z))?;
    Ok(Body::from_shell_unchecked(shell))
}
