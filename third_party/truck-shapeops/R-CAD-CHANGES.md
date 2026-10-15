# R-CAD fork of truck-shapeops

Upstream: https://github.com/ricosjp/truck, crate `truck-shapeops` 0.4.0 as published on crates.io
(source commit c23d2c25c1cd83b615d35db72cc5a4432c693e17, `path_in_vcs = truck-shapeops`).
License: Apache-2.0 (`LICENSE`, copied from the upstream repository at that commit; upstream has no
NOTICE file). Used through `[patch.crates-io]` in the workspace `Cargo.toml`.

Why a fork of the release and not upstream master: master (88ed0052, checked 2026-10-13) panics in
`IntersectionCurve::subs` on plain cylinder cuts that 0.4.0 handles, and it does not fix revolved-tool
booleans. Fixes here are kept small and self-contained so each can be offered upstream as a PR.

## Changes (newest last)

- Vendored verbatim (only `.cargo-ok` and `Cargo.lock` from the registry package dropped).
- `src/lib.rs`: `#![cfg_attr(not(debug_assertions), deny(warnings))]` and `clippy::all` removed from
  the deny list, `dead_code` allowed. Registry dependencies build with capped lints; a path
  dependency does not, and rustc 1.99 reports the unused `Alternative` enum. No code change.
- `Cargo.toml`: empty `[workspace]` so `cargo test` inside the crate does not attach to an enclosing
  workspace.
- `src/transversal/integrate`: ricosjp/truck PR #134 (open, unmerged), ported to 0.4.0. Faces untouched
  by intersection curves are classified against orientation-applied polygons, with threshold 0 for an
  inverted (difference) operand and a 3-ray majority. Its two tests are included. On 0.4 the test
  helper asserts closedness on the topology, because 0.4's tessellation does not weld every seam
  (`Oriented` mesh for a correct closed solid). Fixes: a revolved ring cut entirely inside a block
  panicked ("This shell is not oriented and closed"). It now gives the block plus a void shell, with
  volume within 1.3e-6 of 48000 - 160π.
- Known pre-existing failure: `healing::tests::step_import` reads a fixture that is not in the
  crates.io package (fails identically on the verbatim crate).
- `src/transversal/divide_face`: parameter loops on periodic surfaces are unwrapped. Each point
  takes the period nearest to its predecessor, and each loop is shifted by whole periods into the
  frame of the face's original outer boundary. Before this, `search_parameter` on a surface of
  revolution could return v = 2π where the walk was at v = 0, so the loop self-crossed. Its area
  sign flipped, and `divide_faces` returned `None` for every revolved Join/Cut that crossed a seam.
  Test: `groove_cut_by_a_revolved_ring`, which uses 4 seam angles and fails on the previous code.
- `src/transversal/divide_face`: each inner (negative) loop becomes a hole of the smallest outer
  loop that contains it. That loop must be strictly larger than the hole, because the test point
  lies on the hole's own curve, which the loop on the other side also runs along. Upstream took the
  first containing loop. With nested intersection loops, such as two coaxial circles in one plane,
  that was often the face's outer boundary, so pieces overlapped and `Solid::new` panicked ("This
  shell is not oriented and closed"). Test: `ring_boss_through_a_face_nests_loops`, which fails on
  the previous code.
- `src/transversal/stable.rs` (new), `integrate`, `faces_classification`: order-stable connected
  components. truck-topology's `Shell::connected_components` builds its face adjacency in a hash
  map keyed by face addresses and starts each component at the first key it iterates, so the
  order of the result solid's shells and faces, and of the unknown face groups classified by
  `integrate_by_component`, followed the allocation addresses of the process. The next boolean
  then walked faces in a different order, and the same document could be accepted in one process
  and refused in another, or give volumes differing in the last bits. The replacement is a
  union-find over face indices: components in the order of their first face, faces in input order.
  Exported as `truck_shapeops::connected_components` for callers with the same need. Tests:
  `stable::tests::components_follow_input_order` here, and R-CAD's cross-process test, which runs a
  seeded corpus of 189 cuts in five processes (87 differing results before, none after).
- `src/fast_curve.rs` (new), `divide_face`, `integrate`, `Cargo.toml`: performance only, results
  bit-identical. `IntersectionCurve::search_triple` (truck-geometry) projects every point onto a
  B-spline/NURBS surface after a 51 x 51 `presearch` grid evaluation of that surface, rebuilt for
  each point; dividing the implicit edges of an earlier boolean's result into boundary polylines
  took about a quarter of each later boolean. `fast_curve::search_triple` caches each surface's grid
  (keyed by the exact bits of its knots and control points), picks the same nearest grid point with
  the same comparisons, and passes it as the hint to truck-geometry's own `double_projection`, so
  every deciding floating-point operation is unchanged. `fast_curve::parameter_division` runs
  `algo::curve::parameter_division` on a wrapper whose `subs` uses it; `create_parameter_boundary`
  divides edges through it (it downcasts to truck-modeling's `Curve`, hence `truck-modeling` as a
  normal dependency and `'static` on `ShapeOpsCurve` and the divide-face functions). R-CAD's kernel
  samples intersection curves through `fast_curve::search_triple` as well. Test:
  `fast_curve::tests::matches_truck_bit_for_bit` (triples, `subs`, `der` and divisions compared bit
  for bit with truck-geometry on plane–cylinder and cylinder–cylinder curves).
- `src/transversal/intersection_curve`, `loops_store`, `integrate`: performance only, results
  bit-identical. `IntersectionCurveWithParameters::try_new` projected every point of each new
  intersection polyline with unhinted `double_projection`, i.e. a fresh presearch grid per surface
  per point; it now passes `fast_curve::nearest_hint`, the same presearch answer from the cached
  grid (`None` for other surface types, where Truck searches as before). `'static` added to
  `ShapeOpsSurface` and the functions that reach `try_new` (for the downcast). Test: the hinted
  projection is compared bit for bit with the unhinted one in
  `fast_curve::tests::matches_truck_bit_for_bit`.
