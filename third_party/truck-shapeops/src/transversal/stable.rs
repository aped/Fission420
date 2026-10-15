//! R-CAD fork: order-stable replacements for topology walks whose output order upstream derives
//! from pointer-keyed hash maps.
//!
//! `Shell::connected_components` (truck-topology 0.6) builds a face adjacency map keyed by face
//! references (hashed by address) and starts each component at the first key it iterates, so the
//! order of the components, and of the faces inside each, follows the allocation addresses of the
//! process. Booleans fed such shells walk faces in a different order in every process, and the
//! intersection tracing, loop splitting and classification can then take different branches on the
//! same geometry. The version here is a union-find over face indices: components appear in the
//! order of their first face in the input shell, and each keeps the input face order.
use rustc_hash::FxHashMap as HashMap;
use truck_topology::*;

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Connected components of `shell` (faces sharing an edge), in input order: components by their
/// first face, faces in shell order. Deterministic replacement for `Shell::connected_components`.
pub fn connected_components<P, C, S>(shell: &Shell<P, C, S>) -> Vec<Shell<P, C, S>> {
    let n = shell.len();
    let mut parent: Vec<usize> = (0..n).collect();
    // Edge id -> first face that uses it. The map is only looked up, never iterated.
    let mut first: HashMap<EdgeID<C>, usize> = HashMap::default();
    for (i, face) in shell.face_iter().enumerate() {
        for edge in face.absolute_boundaries().iter().flatten() {
            let j = *first.entry(edge.id()).or_insert(i);
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            if a != b {
                // The smaller index stays the root, so a component is named by its first face.
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut slot: Vec<Option<usize>> = vec![None; n];
    let mut out: Vec<Shell<P, C, S>> = Vec::new();
    for (i, face) in shell.face_iter().enumerate() {
        let r = find(&mut parent, i);
        let k = *slot[r].get_or_insert_with(|| {
            out.push(Shell::new());
            out.len() - 1
        });
        out[k].push(face.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two separate triangles and a third sharing an edge with the first: components follow the
    /// input order whatever the allocation addresses.
    #[test]
    fn components_follow_input_order() {
        let v = Vertex::news([(); 7]);
        let e = [
            Edge::new(&v[0], &v[1], ()),
            Edge::new(&v[1], &v[2], ()),
            Edge::new(&v[2], &v[0], ()),
            Edge::new(&v[3], &v[4], ()),
            Edge::new(&v[4], &v[5], ()),
            Edge::new(&v[5], &v[3], ()),
            Edge::new(&v[1], &v[6], ()),
            Edge::new(&v[6], &v[2], ()),
        ];
        let f0 = Face::new(
            vec![Wire::from(vec![e[0].clone(), e[1].clone(), e[2].clone()])],
            0,
        );
        let f1 = Face::new(
            vec![Wire::from(vec![e[3].clone(), e[4].clone(), e[5].clone()])],
            1,
        );
        let f2 = Face::new(
            vec![Wire::from(vec![e[6].clone(), e[7].clone(), e[1].inverse()])],
            2,
        );
        for order in [[0, 1, 2], [1, 2, 0], [2, 1, 0]] {
            let faces = [&f0, &f1, &f2];
            let shell: Shell<(), (), i32> = order.iter().map(|&k| faces[k].clone()).collect();
            let comps = connected_components(&shell);
            let tags: Vec<Vec<i32>> = comps
                .iter()
                .map(|c| c.face_iter().map(|f| f.surface()).collect())
                .collect();
            let want: Vec<Vec<i32>> = match order {
                [0, 1, 2] => vec![vec![0, 2], vec![1]],
                [1, 2, 0] => vec![vec![1], vec![2, 0]],
                _ => vec![vec![2, 0], vec![1]],
            };
            assert_eq!(tags, want, "order {order:?}");
        }
    }
}
