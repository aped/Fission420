# ADR 0002 — Store boolean intersection edges as verified exact explicit curves

Status: accepted (run 2, 2026-10-10)

## Context
Truck 0.6 booleans (`truck_shapeops::and`) return every new edge as an `IntersectionCurve`:
two surfaces plus a polyline leader. Evaluating a point runs `double_projection` with no
parameter hints (a nearest-point search on each surface, then Newton refinement), measured at
~118 µs per evaluation on the plate's NURBS cylinder patch. The B-rep boundary-integral volume,
bounds sampling and the tessellation chord guard evaluate edges thousands of times, so after one
Ø10 hole `measure` took ~1.8 s and `tessellate_display` ~0.38 s (profile in
docs/behavior/timings.md, "Kernel hardening: intersection-curve cost").

## Decision
After each successful boolean (`cut_through_all`, `cut_blind`; `build.rs::with_exact_edges`),
rebuild the solid with every *supported* intersection edge replaced by an explicit exact curve
(`crates/rcad-kernel/src/exact.rs`). Topology, vertices and surfaces are unchanged (compress →
replace curve → extract), and the result must again be one closed shell, else the original body
is kept.

Supported case: a plane (normal n, point o) meeting a NURBS surface extruded along n (normals
perpendicular to n at a 9×9 parameter grid, to 1e-12). This covers hole and pocket rims on the
faces they are drilled through, from any sketch plane.

## Exactness argument
Every point of a surface extruded along n is g(t) + s·n for the generating curve g. Its
intersection with the plane (p − o)·n = 0 is exactly {P(g(t))} with P(p) = p − ((p − o)·n) n, the
projection along n. P is affine, and an affine map applied to the Euclidean part of each
homogeneous control point (weights unchanged) maps a rational B-spline onto exactly the image
curve. So the rebuilt curve is the true intersection (a circle stays a rational quadratic circle);
no fitting or approximation is involved. It is trimmed between the edge's parameters found by
nearest-point search for the edge vertices.

## Verification (replacement rejected, original kept, on any failure)
- trimmed curve end points within `REBUILD_TOL` = 1e-7 mm of the boolean's vertices;
- 7 interior samples within 1e-7 mm of the plane and of the NURBS surface;
- 9 points of the *original* intersection curve (both ends and 7 interior, at increasing leader
  parameter) lie on the rebuilt curve within `VERIFY_TOL` = 1e-5 mm (10× Truck's 1e-6
  projection convergence) at non-decreasing curve parameter, so it is the same arc (not the
  complementary one) traversed in the same direction;
- Truck panics during the check are caught and keep the original.

Cylinder ∩ cylinder and planes oblique to the extrusion are never replaced; they stay as
`IntersectionCurve` (correct, slow), and STEP export still refuses them as before.

## Consequences
- Plate with Ø10 hole: measure 1818 → 0.9 ms, tessellate 380 → 1.6 ms; volume error vs analytic
  1.2e-5 → 4e-11 mm³. Bracket workflow 68.9 → 2.7 s, acceptance 76.5 → 2.4 s.
- Each boolean pays roughly 10 ms for one through hole (6 edges), estimated from the
  `regen_profile` boolean-time delta (183/156 ms before, 194/167 ms after).
- Subsequent booleans see explicit NURBS edges instead of intersection curves. The preserved
  fragile case (120×60×8 plate, corner Ø6.5 then centred Ø20) fails on the first seam angle at
  all three tolerances either way and succeeds with the existing retries
  (`build/tests.rs::fragile_corner_then_centre_hole_needs_retry`).
- STEP export reuses `exact_edge`; side-face holes (non-Z planes) now export too.
- No Truck sources were patched.
