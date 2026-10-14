//! Sketch patterns: rectangular (linear in one or two
//! directions) and circular copies of selected sketch entities, kept as
//! **constrained** ordinary geometry so they follow the source.
//!
//! Every point of the selection gets one copy per instance, tied to its source
//! by [`Constraint::PatternStep`] (`to − from = k · step`, the step being a
//! construction line from the first source point, whose length is the
//! `pattern_spacing` dimension) or [`Constraint::PatternRotate`] (rotation by
//! k · `pattern_angle` about a centre point). Copies are exact rigid images of
//! the sources, so source constraints are not duplicated. Circles copy their
//! radius with `EqualRadius`. A copied arc's three points would give one
//! equation more than needed (the arc implies |start − c| = |end − c|): for each
//! copied arc one point used by that arc alone (preferably the centre) is tied
//! along one axis only, the axis most perpendicular to the direction its
//! remaining equation constrains, chosen when the pattern is made.
//!
//! Remaining freedom: a linear pattern's step line can still turn (1 DOF per
//! direction; constrain it horizontal/vertical or by an angle); circular
//! patterns add none. Editing `pattern_spacing` / `pattern_angle` moves every
//! copy; changing the count means a new pattern (delete the copies).

use crate::{Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchError};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Most instances (including the source) per pattern direction.
pub const MAX_PATTERN_COUNT: usize = 100;

/// What a pattern added.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternResult {
    /// Copied entities, instance by instance in source order.
    pub entities: Vec<EntityId>,
    /// Linear: the construction step line(s); circular: none.
    pub step_lines: Vec<EntityId>,
    /// Dimension names (spacing per direction, or the angle).
    pub dimensions: Vec<String>,
}

enum Map {
    /// Step vector `a -> b` times k.
    Step { a: PointId, b: PointId },
    /// Rotation about `center` by k θ (θ degrees).
    Rot {
        center: PointId,
        theta: f64,
        name: String,
    },
}

fn check_selection(s: &Sketch, ents: &[EntityId]) -> Result<Vec<Entity>, SketchError> {
    if ents.is_empty() {
        return Err(SketchError::InvalidValue(
            "pattern: select at least one line, arc or circle".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    ents.iter()
        .filter(|e| seen.insert(**e))
        .map(|e| s.entity(*e).cloned().ok_or(SketchError::MissingEntity(*e)))
        .collect()
}

/// Source points in first-use order.
fn source_points(src: &[Entity]) -> Vec<PointId> {
    let mut seen = BTreeSet::new();
    let mut v = Vec::new();
    for e in src {
        for p in e.points() {
            if seen.insert(p) {
                v.push(p);
            }
        }
    }
    v
}

/// Per copied arc, the (source) point tied along one axis only, with the
/// direction its remaining equation leaves free (sketch mm, source pose).
fn one_axis_points(s: &Sketch, src: &[Entity]) -> HashMap<PointId, [f64; 2]> {
    let mut uses: HashMap<PointId, usize> = HashMap::new();
    for e in src {
        for p in e.points() {
            *uses.entry(p).or_default() += 1;
        }
    }
    let xy = |p: PointId| s.point_xy(p).unwrap_or([0.0, 0.0]);
    let mut out = HashMap::new();
    for e in src {
        let Entity::Arc {
            center, start, end, ..
        } = e
        else {
            continue;
        };
        let (c, a, b) = (xy(*center), xy(*start), xy(*end));
        // Remaining equation of each candidate: the centre lies on the chord's
        // perpendicular bisector (direction ⟂ chord); an end lies on the circle
        // (radial normal). Tie the axis most along that free direction's normal.
        let cands = [
            (*center, [b[0] - a[0], b[1] - a[1]]),
            (*start, [a[1] - c[1], -(a[0] - c[0])]),
            (*end, [b[1] - c[1], -(b[0] - c[0])]),
        ];
        // The free direction of the centre is ⟂ chord, i.e. along (−dy, dx):
        // its x component is −chord.y; tie the axis where |free| is largest.
        for (p, d) in cands {
            if uses[&p] == 1 && !out.contains_key(&p) {
                let free = if p == *center { [-d[1], d[0]] } else { d };
                out.insert(p, free);
                break;
            }
        }
    }
    out
}

impl Sketch {
    fn add_pattern(
        &mut self,
        src: &[Entity],
        maps: &[(Map, usize)],
    ) -> Result<Vec<EntityId>, SketchError> {
        let pts = source_points(src);
        let single = one_axis_points(self, src);
        let construction: BTreeSet<EntityId> = src
            .iter()
            .map(Entity::id)
            .filter(|e| self.is_construction(*e))
            .collect();
        let xy = |s: &Sketch, p: PointId| s.point_xy(p).ok_or(SketchError::MissingPoint(p));
        // Instances: every combination of steps (two linear directions give a grid).
        let mut instances: Vec<Vec<(usize, u32)>> = vec![vec![]];
        for (mi, (_, count)) in maps.iter().enumerate() {
            let mut next = Vec::new();
            for inst in &instances {
                for k in 0..*count as u32 {
                    let mut v = inst.clone();
                    if k > 0 {
                        v.push((mi, k));
                    }
                    next.push(v);
                }
            }
            instances = next;
        }
        let mut copies = Vec::new();
        // Copy of source point p in an earlier instance.
        let mut index: HashMap<(PointId, Vec<(usize, u32)>), PointId> = HashMap::new();
        for inst in instances.into_iter().filter(|v| !v.is_empty()) {
            // Position of each copy point: apply each map in turn.
            let mut map_pt: BTreeMap<PointId, PointId> = BTreeMap::new();
            for &p in &pts {
                let mut q = xy(self, p)?;
                for &(mi, k) in &inst {
                    q = match &maps[mi].0 {
                        Map::Step { a, b } => {
                            let (pa, pb) = (xy(self, *a)?, xy(self, *b)?);
                            let kf = f64::from(k);
                            [q[0] + kf * (pb[0] - pa[0]), q[1] + kf * (pb[1] - pa[1])]
                        }
                        Map::Rot { center, theta, .. } => {
                            let c = xy(self, *center)?;
                            let (sn, cs) = (f64::from(k) * theta.to_radians()).sin_cos();
                            let (dx, dy) = (q[0] - c[0], q[1] - c[1]);
                            [c[0] + cs * dx - sn * dy, c[1] + sn * dx + cs * dy]
                        }
                    };
                }
                let id = self.add_point(q[0], q[1]);
                map_pt.insert(p, id);
                // Tie the copy: a grid instance (two steps) is tied to the
                // instance one step back in the last direction (a chain), so
                // each point has exactly one tie.
                let (mi, k) = *inst.last().expect("non-empty");
                let (from, kk) = (p, k);
                // Tie the axis along which the copy's free direction (rotated
                // with the instance) is largest: well conditioned.
                let turn: f64 = inst
                    .iter()
                    .map(|&(mi, k)| match &maps[mi].0 {
                        Map::Rot { theta, .. } => f64::from(k) * theta.to_radians(),
                        Map::Step { .. } => 0.0,
                    })
                    .sum();
                let axis = single.get(&p).map(|f| {
                    let (sn, cs) = turn.sin_cos();
                    let g = [cs * f[0] - sn * f[1], sn * f[0] + cs * f[1]];
                    u8::from(g[0].abs() < g[1].abs())
                });
                let prior: Vec<(usize, u32)> = inst[..inst.len() - 1].to_vec();
                let from = if prior.is_empty() {
                    from
                } else {
                    // Find the copy of `p` in the instance `prior` (made earlier).
                    *index.get(&(p, prior.clone())).ok_or_else(|| {
                        SketchError::InvalidValue("pattern: internal order".into())
                    })?
                };
                index.insert((p, inst.clone()), id);
                self.constraints.push(match &maps[mi].0 {
                    Map::Step { a, b } => Constraint::PatternStep {
                        from,
                        to: id,
                        a: *a,
                        b: *b,
                        k: kk,
                        axis,
                    },
                    Map::Rot {
                        center,
                        theta,
                        name,
                    } => Constraint::PatternRotate {
                        from,
                        to: id,
                        center: *center,
                        k: kk,
                        dim: Dimension::new(name.clone(), *theta),
                        axis,
                    },
                });
            }
            for e in src {
                let id = match e {
                    Entity::Line { a, b, .. } => self.add_line(map_pt[a], map_pt[b]),
                    Entity::Circle {
                        id: sid,
                        center,
                        radius,
                    } => {
                        let c = self.add_circle(map_pt[center], *radius);
                        self.constraints
                            .push(Constraint::EqualRadius { a: *sid, b: c });
                        c
                    }
                    Entity::Arc {
                        center, start, end, ..
                    } => {
                        let id = self.next_entity_id();
                        self.entities.push(Entity::Arc {
                            id,
                            center: map_pt[center],
                            start: map_pt[start],
                            end: map_pt[end],
                        });
                        id
                    }
                };
                if construction.contains(&e.id()) {
                    self.set_construction(id, true);
                }
                copies.push(id);
            }
        }
        Ok(copies)
    }

    /// Rectangular pattern: `counts[i]` instances (source included) along each
    /// step vector `steps[i]` (sketch mm; one or two directions).
    pub fn pattern_linear(
        &mut self,
        ents: &[EntityId],
        steps: &[([f64; 2], usize)],
    ) -> Result<PatternResult, SketchError> {
        let src = check_selection(self, ents)?;
        if steps.is_empty() || steps.len() > 2 {
            return Err(SketchError::InvalidValue(
                "pattern: one or two directions".into(),
            ));
        }
        let mut s = self.clone();
        let anchor = source_points(&src)[0];
        let a = s
            .point_xy(anchor)
            .ok_or(SketchError::MissingPoint(anchor))?;
        let mut maps = Vec::new();
        let mut lines = Vec::new();
        let mut names = Vec::new();
        for (i, (v, n)) in steps.iter().enumerate() {
            let len = v[0].hypot(v[1]);
            if !(2..=MAX_PATTERN_COUNT).contains(n) || !len.is_finite() || len <= 1e-6 {
                return Err(SketchError::InvalidValue(format!(
                    "pattern direction {}: count 2..={MAX_PATTERN_COUNT} and a non-zero step",
                    i + 1
                )));
            }
            let b = s.add_point(a[0] + v[0], a[1] + v[1]);
            let l = s.add_line(anchor, b);
            s.set_construction(l, true);
            let name = s.unique_dimension_name("pattern_spacing");
            s.constraints.push(Constraint::Distance {
                a: anchor,
                b,
                dim: Dimension::new(name.clone(), len),
            });
            maps.push((Map::Step { a: anchor, b }, *n));
            lines.push(l);
            names.push(name);
        }
        let entities = s.add_pattern(&src, &maps)?;
        s.validate()?;
        *self = s;
        Ok(PatternResult {
            entities,
            step_lines: lines,
            dimensions: names,
        })
    }

    /// Circular pattern: `count` instances (source included) about `center`
    /// (an existing point, or a new one at `at`), `angle` degrees apart.
    pub fn pattern_circular(
        &mut self,
        ents: &[EntityId],
        center: Option<PointId>,
        at: [f64; 2],
        count: usize,
        angle: f64,
    ) -> Result<PatternResult, SketchError> {
        let src = check_selection(self, ents)?;
        if !(2..=MAX_PATTERN_COUNT).contains(&count)
            || !(angle > 0.0 && angle < 360.0)
            || (count as f64 - 1.0) * angle > 360.0 + 1e-9
        {
            return Err(SketchError::InvalidValue(format!(
                "circular pattern: count 2..={MAX_PATTERN_COUNT}, angle in (0, 360) and the \
                 instances within one turn (count {count}, angle {angle}°)"
            )));
        }
        let mut s = self.clone();
        let c = match center {
            Some(p) if s.point(p).is_some() => p,
            Some(p) => return Err(SketchError::MissingPoint(p)),
            None => s.add_point(at[0], at[1]),
        };
        let name = s.unique_dimension_name("pattern_angle");
        let maps = vec![(
            Map::Rot {
                center: c,
                theta: angle,
                name: name.clone(),
            },
            count,
        )];
        let entities = s.add_pattern(&src, &maps)?;
        s.validate()?;
        *self = s;
        Ok(PatternResult {
            entities,
            step_lines: vec![],
            dimensions: vec![name],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PolygonFit, SolveStatus};
    use std::f64::consts::PI;

    /// Slot (3 DOF) from (0,0) to (10,0), width 4, pinned at the origin and
    /// horizontal: fully constrained.
    fn pinned_slot(s: &mut Sketch) -> Vec<EntityId> {
        let o = s.ensure_origin();
        let r = s
            .add_slot([0.0, 0.0], [10.0, 0.0], 4.0, [Some(o), None])
            .unwrap();
        s.constraints.push(Constraint::Horizontal {
            line: r.entities[0],
        });
        r.entities
    }

    fn assert_regions(s: &Sketch, n: usize, area: f64) {
        let rs = s.regions().unwrap();
        assert_eq!(rs.len(), n);
        for r in rs {
            assert!((r.area - area).abs() < 1e-8 * area, "{} vs {area}", r.area);
        }
    }

    fn has_point(s: &Sketch, x: f64, y: f64) -> bool {
        s.points
            .iter()
            .any(|p| (p.x - x).abs() < 1e-7 && (p.y - y).abs() < 1e-7)
    }

    #[test]
    fn linear_pattern_of_a_slot_follows_its_spacing() {
        let mut s = Sketch::default();
        let src = pinned_slot(&mut s);
        assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
        let r = s.pattern_linear(&src, &[([0.0, 10.0], 3)]).unwrap();
        assert_eq!(r.entities.len(), 8);
        let rep = s.solve();
        // The step line may still turn: 1 DOF; no redundancy from the arcs.
        assert_eq!(rep.status, SolveStatus::UnderConstrained, "{rep:?}");
        assert_eq!(rep.dof, 1);
        s.constraints.push(Constraint::Vertical {
            line: r.step_lines[0],
        });
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        assert_regions(&s, 3, 10.0 * 4.0 + PI * 4.0);
        // Spacing edit moves every copy; a slot length edit changes all three.
        s.set_dimension(&r.dimensions[0], 12.5).unwrap();
        s.set_dimension("slot_length", 16.0).unwrap();
        assert!(s.solve().is_success());
        assert_regions(&s, 3, 16.0 * 4.0 + PI * 4.0);
        // Second arc centres at (16, 12.5 k).
        assert!(has_point(&s, 16.0, 12.5) && has_point(&s, 16.0, 25.0));
    }

    #[test]
    fn grid_pattern_of_a_hexagon() {
        let mut s = Sketch::default();
        let o = s.ensure_origin();
        let h = s
            .add_regular_polygon([0.0, 0.0], [3.0, 0.0], 6, PolygonFit::Inscribed, Some(o))
            .unwrap();
        let r = s
            .pattern_linear(&h.entities, &[([10.0, 0.0], 3), ([0.0, 10.0], 2)])
            .unwrap();
        assert_eq!(r.entities.len(), 6 * 5);
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::UnderConstrained, "{rep:?}");
        // Hexagon rotation (1) + two step lines (1 each).
        assert_eq!(rep.dof, 3);
        assert_regions(&s, 6, 1.5 * 3f64.sqrt() * 9.0);
    }

    #[test]
    fn circular_pattern_of_holes_and_of_a_slot() {
        let mut s = Sketch::default();
        let o = s.ensure_origin();
        let c = s.add_point(20.0, 0.0);
        let hole = s.add_circle(c, 2.5);
        s.constraints.push(Constraint::FixedPoint {
            p: c,
            x: 20.0,
            y: 0.0,
        });
        s.constraints.push(Constraint::Diameter {
            circle: hole,
            dim: Dimension::new("hole_d", 5.0),
        });
        let r = s
            .pattern_circular(&[hole], Some(o), [0.0, 0.0], 6, 60.0)
            .unwrap();
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        let angles = s
            .dimensions()
            .into_iter()
            .filter(|(n, _)| n == "pattern_angle")
            .count();
        assert_eq!(angles, 1, "one shared angle parameter");
        assert_regions(&s, 6, PI * 6.25);
        // Angle edit: copies at k * 45 degrees.
        s.set_dimension(&r.dimensions[0], 45.0).unwrap();
        assert!(s.solve().is_success());
        s.validate().unwrap();
        for k in 1..6 {
            let a = (45.0 * k as f64).to_radians();
            assert!(has_point(&s, 20.0 * a.cos(), 20.0 * a.sin()), "copy {k}");
        }
        assert!(s.set_dimension(&r.dimensions[0], 400.0).is_err());

        // A slot turned in 90 degree steps: the arcs' single-axis ties are
        // chosen per copy (a 90 degree copy would otherwise be singular).
        let mut s = Sketch::default();
        let src = pinned_slot(&mut s);
        let o = s.origin.unwrap();
        s.pattern_circular(&src, Some(o), [0.0, 0.0], 4, 90.0)
            .unwrap();
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        assert!(has_point(&s, 0.0, 10.0) && has_point(&s, -10.0, 0.0) && has_point(&s, 0.0, -10.0));
    }

    #[test]
    fn pattern_refusals_leave_the_sketch_unchanged() {
        let mut s = Sketch::default();
        let src = pinned_slot(&mut s);
        let before = s.clone();
        assert!(s.pattern_linear(&[], &[([1.0, 0.0], 2)]).is_err());
        assert!(s.pattern_linear(&src, &[([1.0, 0.0], 1)]).is_err());
        assert!(s.pattern_linear(&src, &[([0.0, 0.0], 3)]).is_err());
        assert!(
            s.pattern_linear(&[EntityId(999)], &[([1.0, 0.0], 2)])
                .is_err()
        );
        assert!(s.pattern_circular(&src, None, [0.0, 0.0], 3, 0.0).is_err());
        assert!(
            s.pattern_circular(&src, None, [0.0, 0.0], 5, 120.0)
                .is_err()
        );
        assert_eq!(s, before);
    }
}
