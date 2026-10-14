#![allow(clippy::many_single_char_names)]

use super::faces_classification::FacesClassification;
use super::loops_store::*;
use rustc_hash::FxHashMap as HashMap;
use std::ops::Deref;
use truck_meshalgo::prelude::*;
use truck_topology::*;

/// R-CAD fork: moves each periodic coordinate of `p` by whole periods to the representative nearest
/// `near`. `search_parameter` on a periodic surface (e.g. a surface of revolution, v period 2π)
/// returns a canonical representative, so a boundary walking across the seam could jump a full
/// period (0 -> 2π), giving a self-crossing parameter loop with the wrong area sign.
fn nearest_period(mut p: Point2, near: Point2, periods: (Option<f64>, Option<f64>)) -> Point2 {
    if let Some(up) = periods.0 {
        p.x += ((near.x - p.x) / up).round() * up;
    }
    if let Some(vp) = periods.1 {
        p.y += ((near.y - p.y) / vp).round() * vp;
    }
    p
}

/// R-CAD fork: shifts a closed parameter loop by whole periods so its bounding-box centre is nearest
/// `centre` (the centre of the face's original outer boundary). Each loop starts without a hint, so
/// loops of one face could otherwise land in different periods, breaking hole containment.
fn align_loop(poly: &mut PolylineCurve<Point2>, centre: Point2, periods: (Option<f64>, Option<f64>)) {
    if periods == (None, None) || poly.is_empty() {
        return;
    }
    let (mut min, mut max) = (poly[0], poly[0]);
    poly.iter().for_each(|p| {
        min = Point2::new(min.x.min(p.x), min.y.min(p.y));
        max = Point2::new(max.x.max(p.x), max.y.max(p.y));
    });
    let mid = min.midpoint(max);
    let shift = nearest_period(mid, centre, periods) - mid;
    poly.iter_mut().for_each(|p| *p += shift);
}

fn surface_periods<S: ParametricSurface>(surface: &S) -> (Option<f64>, Option<f64>) {
    (surface.u_period(), surface.v_period())
}

fn create_parameter_boundary<P, C, S>(
    face: &Face<P, C, S>,
    wire: &Wire<P, C>,
    polys: &mut HashMap<EdgeID<C>, PolylineCurve<P>>,
    tol: f64,
) -> Option<PolylineCurve<Point2>>
where
    P: Copy,
    C: BoundedCurve<Point = P> + ParameterDivision1D<Point = P>,
    S: Clone + SearchParameter<D2, Point = P> + ParametricSurface,
{
    let surface = face.surface();
    let periods = surface_periods(&surface);
    let pt = wire.front_vertex().unwrap().point();
    let p: Point2 = surface.search_parameter(pt, None, 100)?.into();
    let vec = wire.edge_iter().try_fold(vec![p], |mut vec, edge| {
        let poly = polys.entry(edge.id()).or_insert_with(|| {
            let curve = edge.curve();
            let div = curve.parameter_division(curve.range_tuple(), tol).1;
            PolylineCurve(div)
        });
        let mut p = *vec.last().unwrap();
        let closure = |q: &P| -> Option<Point2> {
            let found: Point2 = surface.search_parameter(*q, Some(p.into()), 100)?.into();
            p = nearest_period(found, p, periods);
            Some(p)
        };
        let add: Option<Vec<Point2>> = match edge.orientation() {
            true => poly.iter().skip(1).map(closure).collect(),
            false => poly.iter().rev().skip(1).map(closure).collect(),
        };
        vec.append(&mut add?);
        Some(vec)
    })?;
    Some(PolylineCurve(vec))
}

#[derive(Clone, Debug)]
struct WireChunk<'a, C> {
    poly: PolylineCurve<Point2>,
    wire: &'a BoundaryWire<Point3, C>,
}

type FaceWithShapesOpStatus<C, S> = (Face<Point3, C, S>, ShapesOpStatus);
fn divide_one_face<C, S>(
    face: &Face<Point3, C, S>,
    loops: &Loops<Point3, C>,
    tol: f64,
) -> Option<Vec<FaceWithShapesOpStatus<C, S>>>
where
    C: BoundedCurve<Point = Point3> + ParameterDivision1D<Point = Point3>,
    S: Clone + SearchParameter<D2, Point = Point3> + ParametricSurface,
{
    let (mut pre_faces, mut negative_wires) = (Vec::new(), Vec::new());
    let mut map = HashMap::default();
    let periods = surface_periods(&face.surface());
    // Reference frame for periodic surfaces: the centre of the face's original outer boundary.
    let centre = match periods {
        (None, None) => None,
        _ => {
            let outer = face.absolute_boundaries().first()?;
            let poly = create_parameter_boundary(face, outer, &mut map, tol)?;
            let (mut min, mut max) = (poly[0], poly[0]);
            poly.iter().for_each(|p| {
                min = Point2::new(min.x.min(p.x), min.y.min(p.y));
                max = Point2::new(max.x.max(p.x), max.y.max(p.y));
            });
            Some(min.midpoint(max))
        }
    };
    loops.iter().try_for_each(|wire| {
        let mut poly = create_parameter_boundary(face, wire, &mut map, tol)?;
        if let Some(centre) = centre {
            align_loop(&mut poly, centre, periods);
        }
        match poly.area() > 0.0 {
            true => pre_faces.push(vec![WireChunk { poly, wire }]),
            false => negative_wires.push(WireChunk { poly, wire }),
        }
        Some(())
    })?;
    negative_wires.into_iter().try_for_each(|chunk| {
        let pt = chunk.poly.front();
        let op = pre_faces.iter_mut().find(|face| face[0].poly.include(pt))?;
        op.push(chunk);
        Some(())
    })?;
    let vec: Vec<_> = pre_faces
        .into_iter()
        .map(|pre_face| {
            let surface = face.surface();
            let op = pre_face
                .iter()
                .find(|chunk| chunk.wire.status() != ShapesOpStatus::Unknown);
            let status = match op {
                Some(chunk) => chunk.wire.status(),
                None => ShapesOpStatus::Unknown,
            };
            let wires: Vec<Wire<Point3, C>> = pre_face
                .into_iter()
                .map(|chunk| chunk.wire.deref().clone())
                .collect();
            let mut new_face = Face::debug_new(wires, surface);
            if !face.orientation() {
                new_face.invert();
            }
            (new_face, status)
        })
        .collect();
    Some(vec)
}

pub fn divide_faces<C, S>(
    shell: &Shell<Point3, C, S>,
    loops_store: &LoopsStore<Point3, C>,
    tol: f64,
) -> Option<FacesClassification<Point3, C, S>>
where
    C: BoundedCurve<Point = Point3> + ParameterDivision1D<Point = Point3>,
    S: Clone + SearchParameter<D2, Point = Point3> + ParametricSurface,
{
    let mut res = FacesClassification::<Point3, C, S>::default();
    shell
        .iter()
        .zip(loops_store)
        .try_for_each(|(face, loops)| {
            if loops
                .iter()
                .all(|wire| wire.status() == ShapesOpStatus::Unknown)
            {
                res.push(face.clone(), ShapesOpStatus::Unknown);
            } else {
                let vec = divide_one_face(face, loops, tol)?;
                vec.into_iter()
                    .for_each(|(face, status)| res.push(face, status));
            }
            Some(())
        })?;
    Some(res)
}

#[cfg(test)]
mod tests;
