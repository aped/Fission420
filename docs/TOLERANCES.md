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
| `tol::MESH_CHORD` | 0.01 mm | mesh | default chordal tolerance for display/STL |
| sketch solver tolerance | see crates/f420-sketch | numerical | Newton residual convergence |
| viewport pick tolerance | pixels | viewport | picking only; never touches model |

Acceptance tolerances (GOAL.md) live in tests and the headless demo, not in model code.
