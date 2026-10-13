//! Tapered (drafted) extrusion tools (run 6, SESSION_6 §B): the exact frustum
//! of a profile along a frame normal, and its use as a Join/Cut tool.
//!
//! Section law: the profile is exact at `w = 0` of `frame` (its plane) and the
//! section at height `w` along `frame.z` is the profile offset *inward* by
//! `w · tan(taper)` (positive taper shrinks along +z; negative grows). Every
//! side face is exact:
//! * polygons: the mitred offset (each edge moved parallel to itself), so the
//!   sides are planar quads (lofted, ADR 0008: corresponding edges parallel);
//! * circles: radius `r − w·tan(taper)`, a cone frustum (lofted).
//!
//! Line/arc loops (rounded profiles) and regions with inner loops are refused
//! by name (the loft has no exact construction for them).
//!
//! As a Join/Cut tool the frustum goes through Truck's boolean (the slab
//! construction, ADR 0006/0007, only handles walls parallel to the axis), and
//! the result must pass [`crate::verify_tapered_boolean`] against the nominal
//! tapered tool (the ADR 0005 check with the section law applied). Truck 0.6
//! fails on revolved cones (run-4 spike), so round tapered Join/Cut is
//! expected to be refused with Truck's error; polygons work.
use crate::build::{BOOLEAN_TOLS, check_single_closed, with_exact_edges};
use crate::validate::{BooleanCheck, Prism, PrismOp};
use crate::{Body, Frame, KernelError, Profile, Result, guarded, tol};

fn refuse<T>(m: impl Into<String>) -> Result<T> {
    Err(KernelError::InvalidInput(format!("taper: {}", m.into())))
}

/// Mitred inward offset of a counter-clockwise polygon by `d` (negative:
/// outward): every edge moves `d` along its inward normal; the vertices are
/// the intersections of consecutive moved edges. None when an edge would
/// shrink below `MIN_LENGTH` or reverse (the topology of the section would
/// change), or two consecutive edges are (anti)parallel.
pub(crate) fn offset_polygon(v: &[[f64; 2]], d: f64) -> Option<Vec<[f64; 2]>> {
    let n = v.len();
    if n < 3 {
        return None;
    }
    let normal = |i: usize| {
        let (a, b) = (v[i], v[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        [-dy / l, dx / l]
    };
    let mut out = Vec::with_capacity(n);
    for (i, vi) in v.iter().enumerate() {
        let (np, nc) = (normal((i + n - 1) % n), normal(i));
        let den = 1.0 + np[0] * nc[0] + np[1] * nc[1];
        if den < 1e-9 {
            return None;
        }
        let m = [(np[0] + nc[0]) / den, (np[1] + nc[1]) / den];
        out.push([vi[0] + d * m[0], vi[1] + d * m[1]]);
    }
    for i in 0..n {
        let (a, b) = (v[i], v[(i + 1) % n]);
        let (p, q) = (out[i], out[(i + 1) % n]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let f = [q[0] - p[0], q[1] - p[1]];
        if e[0] * f[0] + e[1] * f[1] <= 0.0 || f[0].hypot(f[1]) < tol::MIN_LENGTH {
            return None;
        }
    }
    // Simple: no two non-adjacent edges cross.
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (a, b, c, e) = (out[i], out[(i + 1) % n], out[j], out[(j + 1) % n]);
            let (d1, d2) = (cross(a, b, c), cross(a, b, e));
            let (d3, d4) = (cross(c, e, a), cross(c, e, b));
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return None;
            }
        }
    }
    Some(out)
}

/// The section of `profile` offset inward by `d` (see the module docs).
pub fn taper_section(profile: &Profile, d: f64) -> Result<Profile> {
    if !d.is_finite() {
        return refuse("non-finite offset");
    }
    match profile {
        Profile::Polygon(v) => match offset_polygon(v, d) {
            Some(p) => Ok(Profile::Polygon(p)),
            None => refuse(format!(
                "the polygon offset by {d:.6} mm loses or crosses an edge; reduce the taper or \
                 the length"
            )),
        },
        Profile::Circle { center, radius } => {
            let r = radius - d;
            if r < tol::MIN_LENGTH {
                return refuse(format!(
                    "the circle shrinks to radius {r:.6} mm; reduce the taper or the length"
                ));
            }
            Ok(Profile::Circle {
                center: *center,
                radius: r,
            })
        }
        Profile::Loop(_) => refuse(
            "rounded (line/arc) profiles cannot be tapered exactly yet; polygons and circles only",
        ),
    }
}

/// Exact tapered tool: the frustum of `profile` (exact at `frame`'s plane,
/// `w = 0`) between `w = lo` and `w = hi` along `frame.z`, with draft angle
/// `taper` (radians, |taper| < 89°).
pub fn tapered_tool(
    profile: &Profile,
    frame: &Frame,
    (lo, hi): (f64, f64),
    taper: f64,
) -> Result<Body> {
    if !(taper.is_finite() && taper.abs() < 89f64.to_radians()) {
        return refuse("draft angle must be finite and below 89 degrees");
    }
    if !(lo.is_finite() && hi.is_finite()) || hi - lo < tol::MIN_LENGTH {
        return refuse("empty span");
    }
    let t = taper.tan();
    let (a, b) = (
        taper_section(profile, lo * t)?,
        taper_section(profile, hi * t)?,
    );
    crate::loft(&a, &frame.offset(lo), &b, &frame.offset(hi))
}

/// `target` joined with / cut by the tapered tool of `profile` (exact at
/// `frame`'s plane) whose *nominal* span is `(0, length)` along `frame.z`;
/// `used` is the span actually handed to the boolean (it may extend past the
/// nominal one to avoid coplanar faces; the section law continues there).
/// The result must pass the independent check against the nominal tool.
pub fn tapered_boolean(
    target: &Body,
    profile: &Profile,
    frame: &Frame,
    length: f64,
    used: (f64, f64),
    taper: f64,
    op: PrismOp,
) -> Result<(Body, BooleanCheck)> {
    if !(length.is_finite() && length >= tol::MIN_LENGTH) {
        return refuse("length must be at least the smallest feature length");
    }
    let tool = tapered_tool(profile, frame, used, taper)?;
    let what = match op {
        PrismOp::Join => "tapered join",
        PrismOp::Cut => "tapered cut",
    };
    let mut failures = Vec::new();
    let mut result = None;
    for &btol in &BOOLEAN_TOLS {
        let r = guarded(what, || match op {
            PrismOp::Join => truck_shapeops::or(&target.solid, &tool.solid, btol),
            PrismOp::Cut => {
                let mut t = tool.solid.clone();
                t.not();
                truck_shapeops::and(&target.solid, &t, btol)
            }
        });
        let solid = match r {
            Ok(Some(s)) => s,
            Ok(None) => {
                failures.push(format!("tol {btol}: the Truck boolean returned nothing"));
                continue;
            }
            Err(e) => {
                failures.push(format!("tol {btol}: {e}"));
                continue;
            }
        };
        let body = Body { solid };
        if body.solid.boundaries().len() > 1 {
            return Err(KernelError::InvalidResult(format!(
                "{what}: the result would be {} disconnected solids",
                body.solid.boundaries().len()
            )));
        }
        match check_single_closed(&body, what) {
            Ok(()) => {
                result = Some(with_exact_edges(body));
                break;
            }
            Err(e) => failures.push(format!("tol {btol}: {e}")),
        }
    }
    let Some(out) = result else {
        return Err(KernelError::BooleanFailed(format!(
            "{what}: {}",
            failures.join("; ")
        )));
    };
    let prism = Prism {
        profile,
        holes: &[],
        start: frame,
        length,
    };
    let check = crate::verify_tapered_boolean(target, &out, op, &prism, taper, used)?;
    Ok((out, check))
}

#[cfg(test)]
mod tests;
