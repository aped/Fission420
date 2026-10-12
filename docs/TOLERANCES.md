# Tolerance policy

Units: millimetres and radians internally; f64 everywhere authoritative.

| name | value | kind | used for |
|---|---|---|---|
| `tol::KERNEL_LINEAR` | 1e-6 mm | geometric | Truck's internal `TOLERANCE`; points closer are identical |
| `tol::BOOLEAN` | 0.05 mm | geometric | `truck_shapeops::and` intersection polyline accuracy (exact points are re-projected) |
| `tol::FACE_MATCH` | 1e-7 mm | geometric | matching a semantic face reference to a B-rep plane |
| `tol::MIN_LENGTH` | 1e-3 mm | validation | smallest accepted length/radius/distance |
| `tol::THROUGH_ALL_MARGIN_*` | 5 % of bbox diagonal, >= 1 mm | construction | cutter overshoot (avoid coplanar boolean faces) |
| join overlap `JOIN_OVERLAP` (f420-document `regen.rs`) | min(0.5 mm, distance / 2, clear / 2) | construction | a Join sketched on a face of its target starts this far inside the material (Truck 0.6 booleans fail on coplanar contact, ADR 0003); `clear` = depth under the profile free of other surfaces (`validate.rs::clear_depth`, run 4, ADR 0005); refused below 2 × `MIN_LENGTH` |
| result-check validation chord (`validate.rs::check_chord`) | clamp(1e-4 × size, 1e-3, 0.02) mm | mesh | tessellation of target and result for the independent boolean check (ADR 0005) |
| result-check probe offsets | planar: max(20 × `KERNEL_LINEAR`, 1e-6 × size); curved: 3 × validation chord | geometric | distance of the inside/outside probe points from a result face; winding numbers within 0.1 of 0/1 classify, others are skipped |
| result-check tool neighbourhood | max(1 mm, 2 % × size) around the tool actually used | geometric | faces outside are untouched target faces and are not probed |
| clear-depth boundary slack | planar: max(10 × `KERNEL_LINEAR`, 1e-7 × size); curved: 2 × validation chord | geometric | surfaces this close to the profile boundary do not count as crossing the overlap slab (a wall that shares the boundary) |
| face-region containment slack (`ops.rs` `profile_within_face`) | max(10 × `KERNEL_LINEAR`, 1e-6 × extent) | geometric | sampled profile points may lie this far outside the support face region (chord error of curved face boundaries, 64 chords per edge) |
| blind-cut floor on a face | `THROUGH_ALL_MARGIN_*` | construction | a blind cut whose floor lands exactly on a face whose material is on the tool's side is extended past it by the margin (same material removed, no coplanar faces) |
| tool/target miss test (join, cut) | `KERNEL_LINEAR` on axis-aligned bounds | validation | "does not touch / does not intersect the target" |
| STEP import validation (`step_in`) | max(1e-5 mm, 1e-9 × bounding diagonal) | geometric | vertex-on-curve / curve-on-surface checks of imported topology (docs/formats/step.md) |
| DXF endpoint join (`DxfImportOptions::join_tolerance_mm`) | 1e-6 mm after scaling | geometric | DXF entity endpoints within this share one sketch point (docs/formats/dxf.md) |
| STL vertex weld | exact coordinate equality | mesh | STL corners are welded only when bit-identical; near-coincident corners stay separate and show up as boundary edges in the diagnostics (docs/formats/stl.md) |
| `WELD_TOLERANCE` (mesh.rs) | 1e-5 mm | mesh | welding tessellation vertices across faces |
| `tol::MIN_MESH_CHORD_*` | max(1e-3 mm, 1e-5 × bbox diagonal) | mesh | lower bound on chord (prevents runaway allocation) |
| `tol::MESH_CHORD` | 0.01 mm | mesh | default chordal tolerance for display/STL |
| `tol::ARC_RADIUS_MATCH` | 1e-6 mm (= `KERNEL_LINEAR`) | geometric | max start/end radius mismatch of an arc in a kernel `Profile::Loop`; the arc's last control point is placed exactly on the end vertex, so a mismatch distorts only the last span by at most this; sketches are solved far tighter |
| arc span slack (f420-kernel `build.rs` `ARC_SPAN_SLACK`) | 1e-9 quarter turns | construction | an arc becomes `ceil(sweep / (π/2) − slack)` single-span edges, so a solved quarter arc (sweep π/2 + rounding) stays one edge; spans stay < π (positive middle weight) |
| B-rep volume chords (f420-kernel `measure.rs` `VOLUME_CHORDS`) | 512 / 1024 / 2048 chords per edge, Richardson extrapolation | numerical | exact-path boundary integral (`BrepBoundaryIntegral{Z,X,Y}`); error estimate = difference of the two extrapolants |
| volume axis classification (`measure.rs` `classify_by_sampling`) | \|unit normal · a\| ≤ 1e-12 (ruled), or \|other two normal components\| ≤ 1e-12 and position along a within 1e-9 mm (perpendicular), on a 17×17 parameter grid | numerical | picks the first axis a in Z, X, Y for which every curved face is ruled along or perpendicular to a; planar faces: perpendicular if the two off-axis normal components sum < 1e-12, ignored if \|n · a\| ≤ 1e-12, otherwise integrated in the plane's own frame (tilted) |
| tessellation volume fallback (`measure.rs` `FALLBACK_CHORD_REL`, `FALLBACK_REFINE`) | chord c = max(1e-4 × bbox diagonal, 4 × `MIN_MESH_CHORD_*`) and c / 4 | numerical | `TessellationEstimate` when no axis applies (e.g. torus): fine-mesh signed volume, error estimate \|V(c) − V(c/4)\| (≈ 3 × the fine error when O(chord) convergence holds; an estimate, not a bound). Torus R 40 r 10: error −3.3e-4 relative, estimate 1.0e-3 relative |
| volume test thresholds (`tests/volume_general.rs`) | exact path: max(0.001 mm³, 1e-7 × V); fallback: \|V − analytic\| ≤ error estimate and estimate < 1 % of V | acceptance | side holes along X/Y, tilted planar faces, torus |
| bounds extremum refinement (f420-kernel `measure.rs`) | 60 golden-section iterations between neighbouring samples | numerical | exact axis extrema of explicit curved edges; skipped for boolean intersection curves (they keep sampling error; since ADR 0002 only unsupported cases such as cylinder ∩ cylinder remain implicit) |
| exact boolean edge rebuild `exact::REBUILD_TOL` (f420-kernel `exact.rs`) | 1e-7 mm | geometric | an exact plane ∩ extrusion curve replaces a boolean intersection edge only if its ends are within this of the edge vertices and 7 interior samples are within this of both surfaces (ADR 0002); also used by STEP export |
| exact boolean edge agreement `exact::VERIFY_TOL` | 1e-5 mm (10 × `KERNEL_LINEAR`) | geometric | 9 points of Truck's original intersection curve (converged to 1e-6) must lie on the rebuilt curve within this, at non-decreasing parameter (same arc, same direction); otherwise the original curve is kept |
| extrusion test for the rebuild (`exact.rs` `is_extrusion_along`) | \|unit normal · n\| < 1e-12 on a 9×9 parameter grid | numerical | identifies a surface extruded along the plane normal n |
| hole containment clearance (f420-document `hole.rs`) | `tol::MIN_LENGTH` | validation | a circular cut is accepted only if every host line segment / arc is farther than `r + MIN_LENGTH` from its centre (and, for a line/arc loop, the centre is inside) |
| sketch solver tolerance | see crates/f420-sketch | numerical | Newton residual convergence |
| sketch profile tolerance `tol = solve_tolerance(scale)` | max(1e-9 mm, 1e-12 × sketch scale) | geometric | zero-length lines / zero-sweep arcs (chord ≤ tol), touching or crossing segments (line–line orientation at tol × length; line–arc and arc–arc exact circle intersections, within tol; angular span test widened by tol / r), zero area (\|area\| ≤ tol × perimeter / 2) |
| three-point arc degeneracy (f420-sketch `arc.rs`) | any two points ≤ tol apart, or triangle height over its longest side ≤ tol; `MAX_ARC_RADIUS_RATIO` = 1e4 (radius ≤ 1e4 × longest side) | validation | `add_arc_3pt` / `arc_through_3_points`; flatter arcs are near-lines and badly conditioned |
| `DIST_GRAD_FLOOR` (solver.rs) | 1e-12 mm | numerical | below this length the `Distance` gradient uses a fixed unit direction (coincident start points); the same floor applies to the unit directions of `Tangent`, `Parallel`, `Perpendicular`, `PointOnLine` (gradient through the direction is zero below it) |
| `Parallel` / `Perpendicular` row scaling (solver.rs `Eq::LineAngle`) | residual `S · sin θ` / `S · cos θ`, S = sketch scale (≥ 1 mm, fixed per solve) | numerical | puts the angle rows in mm so the common residual tolerance applies: the effective angular tolerance is `solve_tolerance(S) / S = max(1e-9 mm / S, 1e-12)` rad. Jacobian entries are `S / line length` (dimensionless); a line much shorter than the sketch (ratio > ~1e6) weakens the rank test (`RANK_REL_TOL`). Unit directions (not raw cross/dot products) so a collapsed line is never a spurious solution |
| `EqualLength` / `PointOnLine` rows | `|a| − |b|`, `u × (p − a)` (mm) | numerical | plain length difference and signed distance from the infinite line; Jacobian entries O(1) (PointOnLine: plus `distance / length` through the direction) |
| viewport pick tolerance | pixels | viewport | picking only; never touches model; convert with `Viewport::pixels_per_mm()` |
| feature-edge quantum (f420-viewport `edges.rs`) | max(1e-5 × mesh diagonal, 4e-7 × max abs coord, 1e-9) | display | matching duplicated tessellation positions when deriving display edges; display cache only |
| grid min cell (`f420_viewport::MIN_CELL_PX`) | 14 points | viewport | adaptive grid spacing from {1,2,5}×10^k mm; snapping step via `Viewport::grid_spacing()` |
| sketch editor pick / snap / alignment (`f420-app sketch_edit.rs`) | 8 / 10 / 8 points | viewport | screen-space; converted to sketch mm with the local px-per-mm at the cursor; never stored |
| face analysis plane / cylinder (`f420-app view3d.rs`) | max(1e-3 mm, 1e-5 × mesh radius); + display chord 0.05 mm for cylinders | display | maps a picked display face to a semantic cap reference or cut wall; the regenerator re-validates references on the exact B-rep |

Acceptance tolerances (GOAL.md) live in tests and the headless demo, not in model code.

Solver convergence is judged on equation residuals (`max(1e-9 mm, 1e-12 × sketch scale)`), not
on coordinates. Through nonlinear rows (the `Distance` constraint) solved coordinates can differ
from the exact solution by a few residuals (observed 4e-9 mm on a 3-4-5 triangle), so tests and
the acceptance scenarios compare solved coordinates at 1e-8 mm (10× the residual tolerance);
volumes and bounds keep the GOAL.md tolerances.

Known deviation: `FACE_MATCH` and `WELD_TOLERANCE` are absolute, not scale-aware; fine for parts of
~1 mm to ~10 m, unverified beyond (review finding 6).

## Added 2026-10-12

| name | value | kind | meaning |
|---|---|---|---|
| `prismatic::GRID_STEPS_PER_MM` | 1e9 (1e-9 mm grid) | construction | integer grid of the slab construction's 2-D booleans (ADR 0006); result coordinates are on it in the tool frame |
| slab level / wall plane merge | `FACE_MATCH` (1e-7 mm) | geometric | cap levels, wall planes and vertices of the slab construction closer than this are one |
| `profile_within_face` area test | `slack × perimeter` | geometric | the profile's area outside the face regions (exact 2-D boolean) must not exceed this |
| `CANONICAL_GRID` (f420-kernel `mesh.rs`) | 1e-6 mm | export | key resolution of the canonical vertex / triangle order of print exports |
| 3MF coordinates | 1e-9 mm in the file unit | export | fixed decimals (9 in mm, 11 in inch), so float noise does not change the text |
| STEP REALs, native-file cache | 1e-9 | export | rounded so recomputed geometry gives the same text |
| planar re-triangulation ties | in-circle `|det| <= 1e-9 L^4` | export/display | cocircular quads (rectangles) get the diagonal avoiding the smallest-key vertex |

