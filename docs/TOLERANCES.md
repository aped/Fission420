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
| sketch solver tolerance | see crates/f420-sketch | numerical | Newton residual convergence |
| `DIST_GRAD_FLOOR` (solver.rs) | 1e-12 mm | numerical | below this length the `Distance` gradient uses a fixed unit direction (coincident start points) |
| viewport pick tolerance | pixels | viewport | picking only; never touches model |

Acceptance tolerances (GOAL.md) live in tests and the headless demo, not in model code.

Solver convergence is judged on equation residuals (`max(1e-9 mm, 1e-12 × sketch scale)`), not
on coordinates. Through nonlinear rows (the `Distance` constraint) solved coordinates can differ
from the exact solution by a few residuals (observed 4e-9 mm on a 3-4-5 triangle), so tests and
the acceptance scenarios compare solved coordinates at 1e-8 mm (10× the residual tolerance);
volumes and bounds keep the GOAL.md tolerances.

Known deviation: `FACE_MATCH` and `WELD_TOLERANCE` are absolute, not scale-aware; fine for parts of
~1 mm to ~10 m, unverified beyond (review finding 6).
