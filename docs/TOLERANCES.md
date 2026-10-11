# Tolerance policy

Units: millimetres and radians internally; f64 everywhere authoritative.

| name | value | kind | used for |
|---|---|---|---|
| `tol::KERNEL_LINEAR` | 1e-6 mm | geometric | Truck's internal `TOLERANCE`; points closer are identical |
| `tol::BOOLEAN` | 0.05 mm | geometric | `truck_shapeops::and` intersection polyline accuracy (exact points are re-projected) |
| `tol::FACE_MATCH` | 1e-7 mm | geometric | matching a semantic face reference to a B-rep plane |
| `tol::MIN_LENGTH` | 1e-3 mm | validation | smallest accepted length/radius/distance |
| `tol::THROUGH_ALL_MARGIN_*` | 5 % of bbox diagonal, >= 1 mm | construction | cutter overshoot (avoid coplanar boolean faces) |
| `WELD_TOLERANCE` (mesh.rs) | 1e-5 mm | mesh | welding tessellation vertices across faces |
| `tol::MIN_MESH_CHORD_*` | max(1e-3 mm, 1e-5 × bbox diagonal) | mesh | lower bound on chord (prevents runaway allocation) |
| `tol::MESH_CHORD` | 0.01 mm | mesh | default chordal tolerance for display/STL |
| `tol::ARC_RADIUS_MATCH` | 1e-6 mm (= `KERNEL_LINEAR`) | geometric | max start/end radius mismatch of an arc in a kernel `Profile::Loop`; the arc's last control point is placed exactly on the end vertex, so a mismatch distorts only the last span by at most this; sketches are solved far tighter |
| arc span slack (f420-kernel `build.rs` `ARC_SPAN_SLACK`) | 1e-9 quarter turns | construction | an arc becomes `ceil(sweep / (π/2) − slack)` single-span edges, so a solved quarter arc (sweep π/2 + rounding) stays one edge; spans stay < π (positive middle weight) |
| bounds extremum refinement (f420-kernel `measure.rs`) | 60 golden-section iterations between neighbouring samples | numerical | exact axis extrema of explicit curved edges; skipped for boolean intersection curves (they keep sampling error; since ADR 0002 only unsupported cases such as cylinder ∩ cylinder remain implicit) |
| exact boolean edge rebuild `exact::REBUILD_TOL` (f420-kernel `exact.rs`) | 1e-7 mm | geometric | an exact plane ∩ extrusion curve replaces a boolean intersection edge only if its ends are within this of the edge vertices and 7 interior samples are within this of both surfaces (ADR 0002); also used by STEP export |
| exact boolean edge agreement `exact::VERIFY_TOL` | 1e-5 mm (10 × `KERNEL_LINEAR`) | geometric | 9 points of Truck's original intersection curve (converged to 1e-6) must lie on the rebuilt curve within this, at non-decreasing parameter (same arc, same direction); otherwise the original curve is kept |
| extrusion test for the rebuild (`exact.rs` `is_extrusion_along`) | \|unit normal · n\| < 1e-12 on a 9×9 parameter grid | numerical | identifies a surface extruded along the plane normal n |
| hole containment clearance (f420-document `hole.rs`) | `tol::MIN_LENGTH` | validation | a circular cut is accepted only if every host line segment / arc is farther than `r + MIN_LENGTH` from its centre (and, for a line/arc loop, the centre is inside) |
| sketch solver tolerance | see crates/f420-sketch | numerical | Newton residual convergence |
| sketch profile tolerance `tol = solve_tolerance(scale)` | max(1e-9 mm, 1e-12 × sketch scale) | geometric | zero-length lines / zero-sweep arcs (chord ≤ tol), touching or crossing segments (line–line orientation at tol × length; line–arc and arc–arc exact circle intersections, within tol; angular span test widened by tol / r), zero area (\|area\| ≤ tol × perimeter / 2) |
| three-point arc degeneracy (f420-sketch `arc.rs`) | any two points ≤ tol apart, or triangle height over its longest side ≤ tol; `MAX_ARC_RADIUS_RATIO` = 1e4 (radius ≤ 1e4 × longest side) | validation | `add_arc_3pt` / `arc_through_3_points`; flatter arcs are near-lines and badly conditioned |
| `DIST_GRAD_FLOOR` (solver.rs) | 1e-12 mm | numerical | below this length the `Distance` gradient uses a fixed unit direction (coincident start points) |
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
