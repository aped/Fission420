# ADR 0008: Exact loft between two planar profiles (narrow scope)

Status: accepted.

## Decision

`rcad_kernel::loft(a: &Profile, fa: &Frame, b: &Profile, fb: &Frame) -> Result<Body>`
(`crates/rcad-kernel/src/loft.rs`) builds a new body between profile `a` (in `fa`'s 2-D
coordinates, like `extrude`) and profile `b` (in `fb`'s 2-D coordinates).

**Placement.** The planes must be parallel (`|fa.z × fb.z| ≤ 1e-9`) and at least
`MIN_LENGTH` apart. The loft direction runs from `a`'s plane to `b`'s; both profiles
are expressed in one shared in-plane frame (`fa`'s, with z towards `b`).

**Correspondence (straight-edged loops).** Both loops are made counter-clockwise about
the loft direction; `b` is rotated so that its vertex 0 is the one closest (in the shared
in-plane frame) to `a`'s vertex 0; vertex i of `a` joins vertex i of `b`. Refused:
different vertex counts; a tie for the closest vertex (within 1e-9 × the profile size:
"ambiguous"); a self-intersecting profile; an intermediate section (t = 1/8 … 7/8) that is
not simple or not counter-clockwise ("twisted correspondence").

**Faces.** Corresponding edges must be parallel and point the same way: every side is then
an exact planar quad. Non-parallel corresponding edges would need a bilinear (twisted)
surface; Truck can build one (`builder::homotopy`), but neither our volume measurement
nor the result checks handle it, so it is refused by name ("not parallel … twisted
(non-planar) surface"). Two coaxial circles give an exact cone frustum (a cylinder for
equal radii): the rim-to-rim line revolved about the axis, capped by planar disks. Mixed
profiles (circle and polygon), arcs inside loops, and non-coaxial circles (oblique cones)
are refused.

**Checks inside `loft`.** One closed shell; the volume equals the prismatoid formula
V = h/6 (A1 + 4 Am + A2) (exact: the section area of a straight-edged loft between
parallel planes is quadratic in the height) or the frustum formula πh/3 (r1² + r1 r2 + r2²).
Cones are measured exactly by the surface-of-revolution path
(`BrepBoundaryIntegralRevolution`, `measure/revolution.rs`; the frustum test holds it to
1e-9 relative); FreeCAD confirms independently (`tools/check_loft.sh`).

## Consequences

* A 60 × 40 → 70 × 50 tray blank (30 mm) is 88000 mm³: the sections are not similar, so it
  is a prismatoid, not a pyramid frustum (h/3 (A1 + A2 + √(A1A2)) = 87982.75 would be
  wrong); 60 × 40 → 75 × 50 (similar) matches both formulas (91500 mm³).
* Not supported yet: non-parallel planes, rotated/twisted correspondences (non-parallel
  edges), different vertex counts, arcs, oblique cones. Each is a named refusal.
