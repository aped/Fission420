# ADR 0006: Exact slab construction for coplanar Join/Cut on prismatic bodies

Status: accepted (2026-10-12, after run 4).

## Context

Truck 0.6 booleans fail (panic, return `None`) or return a non-closed shell when a tool
face coincides with a face of the target. Run 4's overlap fuzz (`gate_fuzz.rs`, 64 random
overlapping rectangular pockets on a 2 mm grid) refused 21 of 64 for this reason: 19 Truck
failures and 2 results rejected by the result check (ADR 0005). Pockets flush with an outer wall, a
pocket continuing an earlier pocket's walls or floor, and a boss sketched exactly on a face
(no overlap into the support) are all ordinary CAD operations that Truck cannot build.

Splitting or nudging the tool cannot remove the coincidence: the result itself contains
coplanar neighbouring faces (a wall that continues an older wall), so any 3-D boolean that
produces it must resolve that coincidence.

## Decision

For a **prismatic** case, build the result directly instead of calling Truck:

* every face of the target is planar and either perpendicular (a *cap*) or parallel (a
  *wall*) to the tool axis, and
* the profile has straight edges only (no arcs or circles).

Then the body is a stack of slabs whose cross-section is constant between consecutive cap
levels, and a Join/Cut is a 2-D region boolean per slab (`f420-kernel/src/prismatic.rs`):

1. `S_k = (S_{k-1} \ Up_k) ∪ Down_k` from the cap faces (up-/down-facing at level `k`); the
   region above the top level must come out empty (consistency check);
2. the tool's two ends are added as levels, and slabs inside the tool span become
   `S_k \ P` (Cut) or `S_k ∪ P` (Join);
3. the result is rebuilt: caps `S_{k-1} \ S_k` (up) and `S_k \ S_{k-1}` (down) per level, and
   walls by sweeping each boundary edge of `S_k` over its slab and uniting the rectangles of
   one wall plane, so **a wall is one face however many slabs it spans** (face references
   resolve by plane and refuse ambiguity, ADR 0003);
4. vertices are shared (merged within `FACE_MATCH`), T-junctions are split, every edge must
   bound exactly two faces in opposite directions, and the shell must be connected. Otherwise
   the operation is refused ("disconnected solids" or "not a manifold solid").

The 2-D booleans use `i_overlay` 8.1 (pure Rust, the boolean engine of `geo`) with `i64`
coordinates on an absolute 1e-9 mm grid, so coincident and collinear edges are resolved
exactly rather than by floating-point accident. Inputs and outputs are snapped to that grid.
The adapter is centred on an integer millimetre with a power-of-two half-size, so every call
maps a grid value to the same integer. (`i_overlay`'s own fixed-scale adapter centres on the
input bounds, so two calls could round the same noisy value one step apart and leave
1e-9 mm slivers; `grid_snapping_is_consistent_across_calls` tests this.)

**Where it is used** (`f420-document/src/regen.rs::apply_tool`): Truck stays first. If
Truck fails, or its result fails the ADR 0005 check, and the case is prismatic, the slab
result is built and goes through **the same independent result check**. If that check
fails, the error says it is the slab construction's. A Join sketched on a face whose
profile leaves the face is refused before any boolean for non-prismatic targets. For
prismatic targets it is built by the slab construction with the nominal span, so no overlap
into the support is needed.

Fixed along the way: `profile_within_face` sampled only profile and face-boundary points, and
accepted a profile running along the face's edge across two notches. That extended a blind
cut's start margin into a boss. It now also requires `area(profile \ face regions)` to vanish
(exact 2-D boolean, `prismatic::area_outside`).

## Consequences

* Overlap fuzz: 63/64 accepted, all exact (was 43). The one refusal is a sketch whose face
  reference became ambiguous (the top face split in two), a document rule.
* New fuzz `flush_coplanar_pockets_and_bosses_are_exact_or_refused` (24 cases × 8 features,
  pockets and bosses flush with outer walls, through-depth, coplanar contact) checks every
  result against an exact 2 mm occupancy grid. 141 were accepted, all exact. All 51 refusals
  are **proven** legitimate by the grid: 33 ambiguous material side, 4 disconnected,
  4 non-manifold, 10 no-op.
* Results are exact up to the 1e-9 mm grid of the tool frame. Coordinates of untouched
  faces are re-snapped to that grid; this is far below `KERNEL_LINEAR`.
* Not covered: arcs in the profile and curved faces on the target (round holes), and tool
  axes oblique to the body's faces. Those cases keep Truck's behaviour and errors; the
  error names the reason the slab construction did not apply. Extending the slabs to
  line+arc regions is the natural next step. It needs an exact 2-D arc boolean, since
  polygonizing would silently facet cylinders.
