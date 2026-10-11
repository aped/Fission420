# ADR 0001 — Geometry kernel: Truck 0.6 (pure Rust B-rep)

Status: accepted (run 1, 2026-10-10)

## Decision
Use the Truck crates from crates.io (Apache-2.0): truck-modeling 0.6.0, truck-shapeops 0.4.0,
truck-meshalgo 0.4.0 (plus their truck-* deps). All Truck types stay behind `crates/f420-kernel`.

## Evidence (spike: `spikes/truck_spike`, `cargo run --release` there)
Box built by `tsweep` of a vertex/edge/face; cutter = `rsweep` circle -> `try_attach_plane` -> `tsweep`;
cut = `truck_shapeops::and(plate, !cutter, 0.05)`.

| case | result |
|---|---|
| 80x50x10, Ø10, cutter overshoots by 1 mm | 1 shell, Closed, 9 faces (6 planes + 3 NURBS cylinder patches), ~0.2-0.3 s |
| 100x50x10, Ø10 | same; mesh vol rel err 1.7e-5 at chord 0.01 |
| 100x50x12, Ø10 | same |
| cutter caps coplanar with plate faces (no overshoot) | **`and` returns None** -> cutter must overshoot; kernel adapter always does |
| r = 24.999 / r = 25 (tangent to plate sides) | Truck returns a Closed single shell; tangency is outside product scope and rejected by document validation |

Volume: Truck has no analytic mass properties. The adapter computes a **B-rep boundary-integral volume**
(divergence theorem with F=(0,0,z); planar face integrals via Green's theorem over exact edge curves,
chord sums at 512/1024/2048 chords per edge + Richardson extrapolation). Faces must be planes or Z-ruled
surfaces (n_z = 0), otherwise the volume is reported **Unavailable**. Observed error vs analytic:
4e-7 (80 wide), 8e-7 (100 wide), 9e-6 (depth 12) mm³; target is max(0.001, V*1e-7). Points on
intersection curves come from Truck's Newton double projection, converged to Truck's 1e-6 linear
tolerance, which bounds this method's accuracy (~1e-4 mm³ worst case for these sizes).

## Alternative evaluated: vcad 0.10.0 (MIT)
Real analytic B-rep with Cylinder surfaces and genuine STEP output via `vcad-kernel`, but: pulls
`clipper-sys` (C++ via `cc`) through `vcad-kernel-cam` -> violates the pure-Rust kernel policy;
volume only from f32 tessellation; the `vcad` facade fails to compile with `step` on;
`difference()` silently degrades to mesh CSG. Rejected for tonight; revisit if Truck blocks.

## Limitations observed
- Coplanar boolean faces fail (handled by overshoot for through-all; blind cuts flush with faces would fail).
- `IntersectionCurve::subs` unwraps internally; adapter evaluates with `search_triple` and wraps every
  Truck call in `catch_unwind` (`guarded`) so a kernel panic becomes `KernelError::Internal`.
- Boolean results carry IntersectionCurve edges. **truck-stepio 0.3 writes INTERSECTION_CURVE with
  the second surface at the first surface's entity index** (`out/geometry.rs`, `DisplayByStep for
  IntersectionCurve`: `self.surface1().fmt(surface0_idx, f)`), giving duplicate ids. Observed: the
  cut plate exported as-is imports in FreeCAD with volume 39194.58 (expected 39214.60) and z bounds
  [-1, 11] (`spikes/truck_spike/src/bin/step.rs`, artifacts/step_probe_cut.step).
  Mitigation in `f420-kernel/src/step.rs`: before writing, every plane ∩ Z-extruded-NURBS intersection
  edge is rebuilt as the exact rational B-spline generating curve at the plane's z, trimmed between
  the edge vertices, and verified to lie on both surfaces (1e-7 mm) and on the same branch as Truck's
  edge; any other intersection type is refused. Output is checked for unique entity ids.
  Independent check: FreeCAD 1.1.4 (OpenCascade) imports 1 valid solid, 9 faces, volume
  49214.6018371 (err 5e-7 mm³) for the 100x50x10 plate (`tools/external_validation.sh`).
  Cylinder faces are exported as rational B-spline surfaces (exact circle representation), not
  CYLINDRICAL_SURFACE.
  Run 2: the same rebuild (generalised to any plane normal, with stronger verification) now runs
  once after every boolean, so stored bodies carry exact explicit curves; see ADR 0002.
- No persistent topological naming in Truck; the document layer resolves semantic references
  from feature parameters and validates them against the B-rep (see docs/behavior/plate-workflow.md).

## Adapter API (crates/f420-kernel)
`extrude(profile, frame, distance)`, `cut_through_all(body, profile, frame)`, `measure(body)`,
`planar_faces_on(body, frame)`, `tessellate(body, chord)` (welded), `tessellate_display`,
`check_mesh`, `write_stl_binary`. All return `Result<_, KernelError>`.
