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
    /// Run 7: relationship entity -> child-to-parent transform.
    rel: HashMap<u64, Matrix4>,
}

/// Run 7: the product structure of a STEP file (products = product
/// definitions, usages = NEXT_ASSEMBLY_USAGE_OCCURRENCE with their
/// CONTEXT_DEPENDENT_SHAPE_REPRESENTATION placement).
pub(super) struct Structure {
    pub products: Vec<SProduct>,
    pub roots: Vec<usize>,
}

pub(super) struct SProduct {
    pub name: String,
    /// Representations of the product (identity-linked group).
    pub reps: HashSet<u64>,
    /// (child product, usage name, child-to-parent transform).
    pub children: Vec<(usize, String, Matrix4)>,
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
        let mut rel: HashMap<u64, Matrix4> = HashMap::new();
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
                    rel.insert(id, m2 * inv);
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
            rel,
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

    /// Representations (identity groups) containing `item`.
    pub fn item_reps(&self, item: u64) -> HashSet<u64> {
        self.reps_containing(item)
            .into_iter()
            .flat_map(|r| self.closure(r))
            .collect()
    }

    /// Run 7: the product structure, when the file has assembly usages
    /// (`None` without NEXT_ASSEMBLY_USAGE_OCCURRENCE). Refused (error) when a
    /// usage's placement cannot be resolved or usage is cyclic.
    pub fn structure(&self) -> Result<Option<Structure>, StepError> {
        let idx = self.idx;
        let nauos = idx.all_simple("NEXT_ASSEMBLY_USAGE_OCCURRENCE");
        if nauos.is_empty() {
            return Ok(None);
        }
        let pds = idx.all_simple("PRODUCT_DEFINITION");
        let index: HashMap<u64, usize> = pds.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let mut products: Vec<SProduct> = Vec::with_capacity(pds.len());
        for &pd in &pds {
            let (_, p) = idx.simple(pd, &["PRODUCT_DEFINITION"])?;
            let name = idx
                .simple(
                    p.r(2)?,
                    &[
                        "PRODUCT_DEFINITION_FORMATION",
                        "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
                    ],
                )
                .ok()
                .and_then(|(_, f)| f.r(2).ok())
                .and_then(|prod| idx.simple(prod, &["PRODUCT"]).ok())
                .and_then(|(_, prod)| {
                    prod.opt_s(1)
                        .or(prod.opt_s(0))
                        .map(|s| s.trim().to_string())
                })
                .unwrap_or_default();
            products.push(SProduct {
                name,
                reps: HashSet::new(),
                children: vec![],
            });
        }
        // Shape of each product: SHAPE_DEFINITION_REPRESENTATION(PDS(pd), rep).
        let mut pds_def: HashMap<u64, u64> = HashMap::new();
        for pds_id in idx.all_simple("PRODUCT_DEFINITION_SHAPE") {
            if let Ok((_, p)) = idx.simple(pds_id, &["PRODUCT_DEFINITION_SHAPE"])
                && let Ok(d) = p.r(2)
            {
                pds_def.insert(pds_id, d);
            }
        }
        for sdr in idx.all_simple("SHAPE_DEFINITION_REPRESENTATION") {
            let (_, p) = idx.simple(sdr, &["SHAPE_DEFINITION_REPRESENTATION"])?;
            let (Ok(pds_id), Ok(rep)) = (p.r(0), p.r(1)) else {
                continue;
            };
            if let Some(pd) = pds_def.get(&pds_id)
                && let Some(&i) = index.get(pd)
            {
                products[i].reps.extend(self.closure(rep));
            }
        }
        // Usages and their placements (CDSR(rel, PDS(nauo))).
        let mut cdsr_of: HashMap<u64, u64> = HashMap::new();
        for c in idx.all_with("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION") {
            let e = idx.get(c)?;
            let p = P::new(
                c,
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
                e.part("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
                    .unwrap_or(&[]),
            );
            let (rel, pds_id) = (p.r(0)?, p.r(1)?);
            if let Some(n) = pds_def.get(&pds_id) {
                cdsr_of.insert(*n, rel);
            }
        }
        let mut is_child = vec![false; products.len()];
        for &n in &nauos {
            let (_, p) = idx.simple(n, &["NEXT_ASSEMBLY_USAGE_OCCURRENCE"])?;
            let (parent, child) = (p.r(3)?, p.r(4)?);
            let (Some(&pi), Some(&ci)) = (index.get(&parent), index.get(&child)) else {
                return Err(fail(format!(
                    "NEXT_ASSEMBLY_USAGE_OCCURRENCE #{n}: unknown product definition"
                )));
            };
            let name = p
                .opt_s(1)
                .or(p.opt_s(0))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let m = cdsr_of
                .get(&n)
                .and_then(|rel| self.rel.get(rel))
                .copied()
                .ok_or_else(|| {
                    fail(format!(
                        "NEXT_ASSEMBLY_USAGE_OCCURRENCE #{n}: no placement \
                         (CONTEXT_DEPENDENT_SHAPE_REPRESENTATION with a transformation)"
                    ))
                })?;
            check_rigid(&m)?;
            products[pi].children.push((ci, name, m));
            is_child[ci] = true;
        }
        // Acyclic, bounded depth (memoized longest path; recursion depth is
        // bounded by MAX_DEPTH).
        fn longest(
            p: &[SProduct],
            i: usize,
            state: &mut [u8],
            memo: &mut [usize],
            level: usize,
        ) -> Result<usize, StepError> {
            match state[i] {
                2 => return Ok(memo[i]),
                1 => return Err(fail("cyclic product usage".into())),
                _ => {}
            }
            if level > MAX_DEPTH {
                return Err(fail("assembly nesting too deep".into()));
            }
            state[i] = 1;
            let mut best = 0;
            for (c, _, _) in &p[i].children {
                best = best.max(1 + longest(p, *c, state, memo, level + 1)?);
            }
            state[i] = 2;
            memo[i] = best;
            Ok(best)
        }
        let roots: Vec<usize> = (0..products.len())
            .filter(|&i| !is_child[i] && !products[i].children.is_empty())
            .collect();
        let mut state = vec![0u8; products.len()];
        let mut memo = vec![0usize; products.len()];
        for i in 0..products.len() {
            if longest(&products, i, &mut state, &mut memo, 0)? > MAX_DEPTH {
                return Err(fail("assembly nesting too deep".into()));
            }
        }
        Ok(Some(Structure { products, roots }))
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
