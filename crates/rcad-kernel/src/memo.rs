//! Small memo tables for values derived from one B-rep shape (bounds,
//! tessellations), which regeneration and the viewport ask for repeatedly.
//!
//! An entry is returned only for the very same shape: identical Truck handles in
//! the same order and orientation (`crate::same_solid`) and bit-identical vertex
//! points. The table holds a clone of the shape, so its handles stay alive and no
//! other shape can be allocated at the same addresses while the entry exists. A
//! hit is therefore exactly the value recomputing would return: these values are
//! deterministic functions of the shape (and of the key, e.g. a chord).
use std::sync::Mutex;
#[cfg(test)]
use truck_modeling::Point3;
use truck_modeling::Solid;

/// Vertex points of `solid`, bit for bit, in its vertex order.
fn fingerprint(solid: &Solid) -> Vec<[u64; 3]> {
    solid
        .vertex_iter()
        .map(|v| {
            let p = v.point();
            [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
        })
        .collect()
}

struct Entry<K, V> {
    solid: Solid,
    points: Vec<[u64; 3]>,
    key: K,
    value: V,
}

/// Most recent `cap` (shape, key) -> value pairs.
pub(crate) struct ShapeMemo<K, V> {
    cap: usize,
    entries: Mutex<Vec<Entry<K, V>>>,
}

impl<K: PartialEq, V: Clone> ShapeMemo<K, V> {
    pub(crate) const fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// The remembered value for (`solid`, `key`), or `compute()` (remembered
    /// when it succeeds).
    pub(crate) fn get_or_try<E>(
        &self,
        solid: &Solid,
        key: K,
        compute: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        let points = fingerprint(solid);
        if let Ok(entries) = self.entries.lock()
            && let Some(e) = entries
                .iter()
                .find(|e| e.key == key && e.points == points && crate::same_solid(&e.solid, solid))
        {
            return Ok(e.value.clone());
        }
        let value = compute()?;
        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() >= self.cap {
                entries.remove(0);
            }
            entries.push(Entry {
                solid: solid.clone(),
                points,
                key,
                value: value.clone(),
            });
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, Profile, extrude};

    #[test]
    fn served_only_for_the_same_shape_and_key() {
        let p = Profile::Polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 6.0], [0.0, 6.0]]);
        let a = extrude(&p, &Frame::XY, 4.0).unwrap();
        let memo: ShapeMemo<u8, usize> = ShapeMemo::new(4);
        let calls = std::cell::Cell::new(0);
        let get = |s: &Solid, k: u8| {
            memo.get_or_try(s, k, || {
                calls.set(calls.get() + 1);
                Ok::<_, ()>(calls.get())
            })
            .unwrap()
        };
        assert_eq!(get(&a.solid, 0), 1);
        // A clone shares the handles: served.
        assert_eq!(get(&a.clone().solid, 0), 1);
        // Another key, an equal but separate shape, the inverted shape: computed.
        assert_eq!(get(&a.solid, 1), 2);
        assert_eq!(get(&a.deep_copy().unwrap().solid, 0), 3);
        let mut inv = a.clone();
        inv.solid.not();
        assert_eq!(get(&inv.solid, 0), 4);
        // A moved vertex (same handles, new point) is not served the old value.
        let moved = a.clone();
        let v = moved.solid.vertex_iter().next().unwrap();
        let q = v.point();
        v.set_point(Point3::new(q.x + 1.0, q.y, q.z));
        assert_eq!(get(&moved.solid, 0), 5);
        v.set_point(q);
        assert_eq!(calls.get(), 5);
    }
}
