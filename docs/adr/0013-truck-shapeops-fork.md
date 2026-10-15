# ADR 0013: Fork truck-shapeops 0.4.0 for boolean fixes

Status: accepted (2026-10-13)

## Context
Revolved Join/Cut failed in Truck on every case tried. Truck returned `None` or
panicked, and the adapter refused the operation. The last crates.io release, truck-shapeops 0.4.0,
is from 2024-09. Upstream master (88ed0052, checked 2026-10-13) was evaluated and rejected:
- It does not fix revolved booleans.
- It panics in `IntersectionCurve::subs` on plain cylinder cuts that 0.4.0 handles.
- It changes `rsweep`'s signature.

Evidence is in a local spike that compares 0.6/0.4 with master on 8 cases and 2 controls.

## Decision
Vendor truck-shapeops 0.4.0 (Apache-2.0, source commit c23d2c25) in
`third_party/truck-shapeops`, used through `[patch.crates-io]`. Keep each change small and
self-contained so it can be offered upstream. `third_party/truck-shapeops/R-CAD-CHANGES.md`
lists them:
1. Port of the open upstream PR #134: faces untouched by intersection curves are classified
   against orientation-applied polygons, with threshold 0 for an inverted operand.
2. Periodic parameter unwrapping when dividing faces. On a surface of revolution, a boundary
   that crossed the seam jumped 2π, so face division failed.
3. Inner loops nest in the smallest strictly larger outer loop. Upstream used the first
   containing loop, which broke two coaxial circles in one plane.

The product side needed three related fixes:
- T-junction repair in the check tessellation, within the weld tolerance.
- An exact-volume test for "cut removed no material" when the face count does not grow.
- STEP export of revolved faces with a bounded generatrix. Previously an unbounded `LINE`
  crossed the axis, and OpenCascade read most seam angles as invalid faces. This bug predates
  the fork.

## Verification
Each fork change has a test that fails on the previous code. Product-path cases
(`crates/rcad-kernel/tests/it/revolved_booleans.rs`) go through `combine`: Truck, then the
independent membership check, then exact volume within 1e-6 of the closed form. The cases are a
groove on 4 seams, a collar, a countersink cone and a ring boss. FreeCAD reads all of them as one
valid solid with zero symmetric difference against its own primitive-built solids
(`tools/check_revolved.sh`). The full workspace suite passes.

## Consequences
- We own a small boolean fork. Upstream releases must be re-diffed before adopting them.
- Corner blends and exact helicoids are not addressed. Upstream has a single-edge fillet
  prototype on master, and nothing for helices.
- The revolved-case speed limit is the exact volume over the intersection edges plus the
  membership check, not the boolean (about 1 s).
