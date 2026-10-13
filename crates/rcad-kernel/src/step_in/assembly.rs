//! Assembly placements and product names.
//!
//! A solid's world placement is the composition of the transforms of the
//! representation relationships from its representation up to a root
//! representation. Supported: `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` with
//! an `ITEM_DEFINED_TRANSFORMATION` between two AXIS2_PLACEMENT_3D (the AP203/AP214
//! assembly pattern written by OpenCascade/FreeCAD: rep_1 = component, rep_2 =
//! assembly; x_assembly = M(item_2) M(item_1)^-1 x_component), plain
//! (SHAPE_)REPRESENTATION_RELATIONSHIP as identity links, any nesting depth up to
//! [`MAX_DEPTH`], multiple instances of one component. MAPPED_ITEM and
//! non-rigid transforms are refused, never ignored.
use super::ast::{Index, P};
use super::geom::Geo;
use super::{StepError, fail};
use std::collections::{HashMap, HashSet};
use std::result::Result;
use truck_modeling::*;

const MAX_DEPTH: usize = 32;
const MAX_PATHS: usize = 10_000;

pub(super) struct Placements<'a> {
    idx: &'a Index,
    /// representation -> items (refs)
    rep_items: HashMap<u64, Vec<u64>>,
    /// identity links between representations (undirected)
    same: HashMap<u64, Vec<u64>>,
    /// child representation -> (parent representation, child-to-parent transform)
    up: HashMap<u64, Vec<(u64, Matrix4)>>,
}

fn is_representation(name: &str) -> bool {
    name.ends_with("REPRESENTATION") && !name.contains("RELATIONSHIP")
}

fn placement_matrix(geo: &Geo, id: u64) -> Result<Matrix4, StepError> {
    let pl = geo.placement(id)?;
    Ok(Matrix4::from_cols(
        pl.x.extend(0.0),
        pl.y.extend(0.0),
        pl.z.extend(0.0),
        pl.o.to_vec().extend(1.0),
    ))
}

impl<'a> Placements<'a> {
    pub fn new(idx: &'a Index, geo: &Geo) -> Result<Placements<'a>, StepError> {
        if let Some(&m) = idx.all_with("MAPPED_ITEM").first() {
            return Err(fail(format!(
                "assembly placements via MAPPED_ITEM (#{m}) are unsupported"
            )));
        }
        let mut rep_items = HashMap::new();
        let mut same: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut up: HashMap<u64, Vec<(u64, Matrix4)>> = HashMap::new();
        for &id in &idx.order {
            let e = &idx.map[&id];
            for (name, params) in &e.parts {
                if is_representation(name) && e.parts.len() == 1 {
                    let p = P::new(id, name, params);
                    if let Ok(items) = p.refs(1) {
                        rep_items.insert(id, items);
                    }
                }
            }
            // A simple `SHAPE_REPRESENTATION_RELATIONSHIP(...)` instance has the
            // same attributes as its supertype (run 7: OCCT and R-CAD's
            // hierarchical export link a part's B-rep representation to its
            // shape representation with it).
            let simple_srr = e.parts.len() == 1 && e.has("SHAPE_REPRESENTATION_RELATIONSHIP");
            let rr = e.part("REPRESENTATION_RELATIONSHIP").or_else(|| {
                simple_srr
                    .then(|| e.part("SHAPE_REPRESENTATION_RELATIONSHIP"))
                    .flatten()
            });
            if let Some(rr) = rr {
                let p = P::new(id, "REPRESENTATION_RELATIONSHIP", rr);
                let (r1, r2) = (p.r(2)?, p.r(3)?);
                if let Some(t) = e.part("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION") {
                    let tp = P::new(id, "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION", t);
                    let tid = tp.r(0)?;
                    let (tty, ip) = idx.simple(tid, &["ITEM_DEFINED_TRANSFORMATION"]).map_err(
                        |_| {
                            fail(format!(
                                "assembly placement #{id}: transformation {} #{tid} unsupported",
                                idx.type_name(tid)
                            ))
                        },
                    )?;
                    let _ = tty;
                    let m1 = placement_matrix(geo, ip.r(2)?)?;
                    let m2 = placement_matrix(geo, ip.r(3)?)?;
                    let inv = m1.invert().ok_or_else(|| {
                        fail(format!(
                            "ITEM_DEFINED_TRANSFORMATION #{tid}: singular placement"
                        ))
                    })?;
                    up.entry(r1).or_default().push((r2, m2 * inv));
                } else if e.parts.len() == 1 || e.has("SHAPE_REPRESENTATION_RELATIONSHIP") {
                    same.entry(r1).or_default().push(r2);
                    same.entry(r2).or_default().push(r1);
                }
            }
        }
        for id in idx.all_with("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION") {
            // Must point at a relationship we understood (with transform).
            let e = idx.get(id)?;
            let p = P::new(
                id,
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
                e.part("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
                    .unwrap_or(&[]),
            );
            let rel = p.r(0)?;
            if !idx
                .get(rel)?
                .has("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
            {
                return Err(fail(format!(
                    "assembly placement CONTEXT_DEPENDENT_SHAPE_REPRESENTATION #{id}: relationship #{rel} ({}) unsupported",
                    idx.type_name(rel)
                )));
            }
        }
        Ok(Placements {
            idx,
            rep_items,
            same,
            up,
        })
    }

    /// Representations reachable from `rep` through identity links (incl. itself).
    fn closure(&self, rep: u64) -> Vec<u64> {
        let mut seen = HashSet::from([rep]);
        let mut stack = vec![rep];
        let mut out = Vec::new();
        while let Some(r) = stack.pop() {
            out.push(r);
            for &n in self.same.get(&r).into_iter().flatten() {
                if seen.insert(n) {
                    stack.push(n);
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn reps_containing(&self, item: u64) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .rep_items
            .iter()
            .filter(|(_, items)| items.contains(&item))
            .map(|(&r, _)| r)
            .collect();
        v.sort_unstable();
        v
    }

    /// World placements of a top-level representation item: `None` = identity.
    pub fn instances_of_item(&self, item: u64) -> Result<Vec<Option<Matrix4>>, StepError> {
        let reps = self.reps_containing(item);
        let Some(&rep) = reps.first() else {
            return Ok(vec![None]); // not in any representation: as written
        };
        let mut out = Vec::new();
        self.walk(rep, None, 0, &mut HashSet::new(), &mut out)?;
        Ok(out)
    }

    fn walk(
        &self,
        rep: u64,
        acc: Option<Matrix4>,
        depth: usize,
        path: &mut HashSet<u64>,
        out: &mut Vec<Option<Matrix4>>,
    ) -> Result<(), StepError> {
        if depth > MAX_DEPTH {
            return Err(fail("assembly nesting too deep".into()));
        }
        let group = self.closure(rep);
        if group.iter().any(|r| path.contains(r)) {
            return Err(fail(format!("assembly placement cycle at #{rep}")));
        }
        let parents: Vec<(u64, Matrix4)> = group
            .iter()
            .flat_map(|r| self.up.get(r).into_iter().flatten().copied())
            .collect();
        if parents.is_empty() {
            if out.len() >= MAX_PATHS {
                return Err(fail("too many assembly instances".into()));
            }
            out.push(acc);
            return Ok(());
        }
        path.extend(group.iter().copied());
        for (parent, m) in parents {
            check_rigid(&m)?;
            let next = Some(match acc {
                Some(a) => m * a,
                None => m,
            });
            self.walk(parent, next, depth + 1, path, out)?;
        }
        for r in &group {
            path.remove(r);
        }
        Ok(())
    }

    /// PRODUCT name owning the representation that contains `item`, if found.
    pub fn product_name_of_item(&self, item: u64) -> Option<String> {
        let idx = self.idx;
        let reps = self.reps_containing(item);
        let group: HashSet<u64> = reps.iter().flat_map(|&r| self.closure(r)).collect();
        for sdr in idx.all_simple("SHAPE_DEFINITION_REPRESENTATION") {
            let (_, p) = idx.simple(sdr, &["SHAPE_DEFINITION_REPRESENTATION"]).ok()?;
            if !group.contains(&p.r(1).ok()?) {
                continue;
            }
            let (_, pds) = idx
                .simple(p.r(0).ok()?, &["PRODUCT_DEFINITION_SHAPE"])
                .ok()?;
            let (_, pd) = idx.simple(pds.r(2).ok()?, &["PRODUCT_DEFINITION"]).ok()?;
            let (_, pdf) = idx
                .simple(
                    pd.r(2).ok()?,
                    &[
                        "PRODUCT_DEFINITION_FORMATION",
                        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
                    ],
                )
                .ok()?;
            let (_, prod) = idx.simple(pdf.r(2).ok()?, &["PRODUCT"]).ok()?;
            let name = prod.opt_s(1).or(prod.opt_s(0))?.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        None
    }
}

/// Placements must be rigid (rotation + translation, det +1).
fn check_rigid(m: &Matrix4) -> Result<(), StepError> {
    let r = Matrix3::from_cols(m.x.truncate(), m.y.truncate(), m.z.truncate());
    let should_be_identity = r.transpose() * r;
    let mut err: f64 = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            let want = if i == j { 1.0 } else { 0.0 };
            err = err.max((should_be_identity[i][j] - want).abs());
        }
    }
    if err > 1e-9 || (r.determinant() - 1.0).abs() > 1e-9 {
        return Err(fail(
            "assembly placement is not a rigid motion (mirror/scale unsupported)".into(),
        ));
    }
    Ok(())
}
