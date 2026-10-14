//! Separating a boolean result that fell apart into one body per piece.
//!
//! A cut through the whole width of a part leaves several separate solids. The
//! opt-in `*_pieces` operations return them as one `Solid` with one closed
//! shell per piece; [`split_pieces`] turns that into one [`Body`] per piece.
//! Pieces are ordered by the lowest corner of their bounding box
//! (lexicographically x, then y, then z, compared exactly), so the order is a
//! deterministic function of the geometry. A shell enclosing negative volume
//! is an internal void, not a piece, and is refused: a body is one connected
//! solid without cavities (ADR 0003).
use crate::measure::raw_bounds;
use crate::{Body, KernelError, Result, check_mesh, tessellate, tol};
use truck_modeling::*;

/// One body per closed shell of `body`, in the documented order. A body with
/// one shell is returned unchanged (as the only piece).
pub fn split_pieces(body: &Body) -> Result<Vec<Body>> {
    let shells = body.solid.boundaries();
    if shells.len() == 1 {
        return Ok(vec![body.clone()]);
    }
    if shells.is_empty() {
        return Err(KernelError::InvalidResult("the result has no shell".into()));
    }
    let mut pieces: Vec<(Bounds3, Body)> = Vec::with_capacity(shells.len());
    for (i, sh) in shells.iter().enumerate() {
        let cond = sh.shell_condition();
        if cond != ShellCondition::Closed {
            return Err(KernelError::InvalidResult(format!(
                "piece {} of the result is a {cond:?} shell, not Closed",
                i + 1
            )));
        }
        let piece = Body {
            solid: Solid::try_new(vec![sh.clone()])
                .map_err(|e| KernelError::Internal(format!("piece {}: {e:?}", i + 1)))?,
        };
        let b = raw_bounds(&piece.solid)?;
        let diag = (0..3)
            .map(|k| (b.max[k] - b.min[k]).powi(2))
            .sum::<f64>()
            .sqrt();
        // The sign of the enclosed volume tells a piece (outward shell) from
        // a void (inward shell); any chord that keeps the mesh closed gives
        // the sign, the size only bounds the cost.
        let chord = tol::MESH_CHORD.max(1e-3 * diag);
        let check = check_mesh(&tessellate(&piece, chord)?);
        if !check.watertight {
            return Err(KernelError::InvalidResult(format!(
                "piece {} of the result does not tessellate to a closed mesh",
                i + 1
            )));
        }
        if check.mesh_volume <= 0.0 {
            return Err(KernelError::InvalidResult(
                "the result would enclose an internal void (a cavity closed on all sides); \
                 enclosed internal voids are not supported (leave the cavity open, e.g. with \
                 a vent hole)"
                    .into(),
            ));
        }
        pieces.push((Bounds3(b.min), piece));
    }
    pieces.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pieces.into_iter().map(|(_, p)| p).collect())
}

/// Lowest bounding-box corner, compared lexicographically with `total_cmp`.
struct Bounds3([f64; 3]);

impl Bounds3 {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (0..3)
            .map(|i| self.0[i].total_cmp(&o.0[i]))
            .find(|c| c.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}
