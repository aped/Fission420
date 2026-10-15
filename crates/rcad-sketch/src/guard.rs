//! Branch-flip guard for dimension edits.
//!
//! Arcs are stored centre/start/end counter-clockwise, so a large dimension
//! edit solved in one jump can move an arc's centre across its chord: the
//! arc silently becomes the complementary one (a 60° fillet turns into a
//! 300° arc) and the profile changes shape with no error. A dimension edit
//! here is solved directly first; when an arc's sweep jumps by more than
//! [`MAX_SWEEP_JUMP`] it is re-solved by continuation (the value is moved in
//! [`CONTINUATION_STEPS`] steps, each solved from the previous result). If a
//! step still jumps, the edit is refused naming the arcs. A sweep that passes
//! smoothly through 180° (a real shape change) is not a jump.

use crate::{Entity, EntityId, Sketch, SketchError, SolveReport};
use std::f64::consts::FRAC_PI_2;

/// Largest sweep change (radians) of one arc accepted between two solves.
pub const MAX_SWEEP_JUMP: f64 = FRAC_PI_2;
/// Steps of the continuation retry.
pub const CONTINUATION_STEPS: usize = 16;

/// Curves (present in both) that jumped to another branch: arcs whose sweep
/// changed by more than the jump limit, and lines whose direction reversed
/// (a fillet or trimmed corner pushed past its neighbour).
pub fn sweep_jumps(before: &Sketch, after: &Sketch) -> Vec<EntityId> {
    let dir = |s: &Sketch, e: &Entity| match e {
        Entity::Line { a, b, .. } => {
            let (p, q) = (s.point_xy(*a)?, s.point_xy(*b)?);
            Some([q[0] - p[0], q[1] - p[1]])
        }
        _ => None,
    };
    before
        .entities
        .iter()
        .filter(|e| match e {
            Entity::Arc { id, .. } => match (before.arc_geometry(*id), after.arc_geometry(*id)) {
                (Some(g0), Some(g1)) => (g1.sweep - g0.sweep).abs() > MAX_SWEEP_JUMP,
                _ => false,
            },
            Entity::Line { id, .. } => match (
                dir(before, e),
                after.entity(*id).and_then(|x| dir(after, x)),
            ) {
                (Some(u), Some(v)) => u[0] * v[0] + u[1] * v[1] < 0.0,
                _ => false,
            },
            _ => false,
        })
        .map(Entity::id)
        .collect()
}

impl Sketch {
    /// Set dimension `name` to `value` and solve, guarding arcs against a
    /// branch flip (module docs). On success the solved sketch replaces
    /// `self`. When the direct solve fails, `self` holds the unsolved change
    /// and the failed report is returned (the caller reports it as for any
    /// failed solve). A flip that continuation cannot avoid is refused and
    /// `self` is unchanged.
    pub fn set_dimension_solved(
        &mut self,
        name: &str,
        value: f64,
    ) -> Result<SolveReport, SketchError> {
        let old = self
            .dimensions()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
            .ok_or_else(|| SketchError::UnknownDimension(name.to_owned()))?;
        let mut direct = self.clone();
        direct.set_dimension(name, value)?;
        let unsolved = direct.clone();
        let r = direct.solve();
        if !r.is_success() {
            *self = unsolved;
            return Ok(r);
        }
        if sweep_jumps(self, &direct).is_empty() {
            *self = direct;
            return Ok(r);
        }
        let mut cur = self.clone();
        let mut last = r;
        for k in 1..=CONTINUATION_STEPS {
            let v = old + (value - old) * k as f64 / CONTINUATION_STEPS as f64;
            let mut next = cur.clone();
            next.set_dimension(name, v)?;
            last = next.solve();
            let jumped = sweep_jumps(&cur, &next);
            if !last.is_success() || !jumped.is_empty() {
                let arcs = if jumped.is_empty() {
                    sweep_jumps(self, &direct)
                } else {
                    jumped
                };
                return Err(SketchError::Refused(format!(
                    "Rejected: {name} = {} would flip {} (an arc to the other side of its chord or a line reversed: a \
                     different shape); change it in smaller steps or add a constraint that \
                     keeps the arc's side",
                    crate::fmt_mm(value),
                    arcs.iter()
                        .map(|a| match self.entity(*a) {
                            Some(Entity::Line { .. }) => format!("line {}", a.0),
                            _ => format!("arc {}", a.0),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            cur = next;
        }
        *self = cur;
        Ok(last)
    }
}
