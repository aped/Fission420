# ADR 0007: Slab booleans with exact arcs, cylindrical walls and holed tools

Status: accepted (2026-10-12). Extends ADR 0006; ADR 0005 (independent result check)
applies unchanged to every result.

## Context

ADR 0006 built coplanar Join/Cut results exactly from slab regions, but only for straight
profiles on all-planar targets. The remaining ordinary refusals were round: the canonical
engulfed-hole cut (Ø10 hole, concentric Ø16 cut: Truck keeps the old wall; ADR 0005 refuses
it), round cuts overlapping earlier ones with a shared floor (Truck panics), rounded pockets,
and any Join/Cut whose sketch region has inner loops (blanket refusal in `regen.rs`).
Polygonizing arcs in the output would silently facet cylinders, which is not acceptable.

## Decision

`rcad-kernel/src/prismatic{.rs,/curves.rs,/solid.rs}`; `prism_region_boolean` (outer + holes),
`prism_boolean` (no holes).

**Applicability.** A target is prismatic along the tool axis z when every face is either a
plane perpendicular to z (*cap*) or a surface whose unit normal is perpendicular to z on a
9 × 9 grid of its parameter domain to 1e-9 (*wall*: planes, extruded arcs, revolutions about z),
and every cap edge is straight or a circular arc (16 samples, circle fit through three of
them, all within `KERNEL_LINEAR`). The tool is any valid `Profile` (polygon, circle, line/arc
loop) with optional inner loops. A left-handed frame (mirror copies) is handled by negating
its y axis and mirroring the profile.

**2-D booleans, exactly.** Polygons decide topology only; geometry stays exact:

1. Circles are collected once per construction and deduplicated (centre and radius within
   `KERNEL_LINEAR`). Loop vertices within `KERNEL_LINEAR` of one circle are placed on it (of
   two: on their nearest intersection); then all 2-D vertices within `FACE_MATCH` of each
   other are merged (B-rep copies of one point at different levels differ by ~1e-8 mm).
2. *Special points* of a circle: its arc ends, loop vertices on it, and the exact
   line–circle and circle–circle intersections. Each circle gets one *ring*: its special
   points plus equal subdivisions (at least two chords between consecutive special points,
   at most 2π/64 rad per chord at first), all snapped to the 1e-9 mm grid. Every loop through
   that circle uses the same ring, so coincident arcs give identical chains; lines are
   split at special points on them. A line through two ring vertices of a convex inscribed
   polygon meets it only there, so a line–circle crossing is resolved exactly at any angle.
3. Checks before and after every polygon boolean: no input/ring vertex inside a circle may
   lie outside its ring polygon (sag band) nor on a circle without being its ring vertex;
   every vertex a boolean creates must lie clear of every circle's sag band. Violations
   retry with rings of 128, 256, 512 chords per turn, then refuse ("ambiguous at the finest
   arc resolution"). Tangency (line or circle within `KERNEL_LINEAR` of tangent) is refused
   ("tangent contact is ambiguous") unless it is a junction of one loop: between the two
   arcs meeting there, or along the line of a line/arc junction (any segment collinear with
   that line touches the ring polygon only at the junction vertex; e.g. a rounded corner, or
   a fillet tool's arc meeting a face trace).
4. Outputs are tagged back: a segment between two ring-adjacent points of one circle is an
   arc of it, anything else a line; arc pieces of one circle merge; a full circle keeps its
   ring's first special point as seam. Boolean outputs are cleaned at `FACE_MATCH`
   (near-duplicate, collinear and spike vertices, sliver contours; the rim-notch repro).

**3-D rebuild.** Slabs as in ADR 0006. Each circle gets one global partition: every exact
vertex on it at any level, each gap split into equal spans of at most a quarter turn
(`ARC_SPAN_SLACK` as in `build.rs`). Cap faces get atomic arc edges (rational quadratic
Bézier, weights 1, cos(φ/2), 1, end control points exactly on the vertices); a cylinder wall is
one exact patch (the homotopy of its bottom and top arcs, the surface `tsweep` builds) per
partition span and contiguous run of slabs, inverted when the material is outside the
circle. Planar walls are unchanged (one face per plane region). Straight edges are split at
T-junctions; every edge must be used twice in opposite directions; one connected shell.

**Where it is used** (`regen.rs::apply_tool`):

* as before, the fallback when Truck fails or its result fails the result check;
* when Truck's checked result keeps implicit intersection edges without an exact replacement
  (`has_implicit_edges`, e.g. cylinder ∩ cylinder; such results are approximations and STEP
  export refuses them), the slab result replaces it if it passes the result check on its own;
* Join/Cut tools with inner loops: only the slab construction (over the nominal span); the
  result check's `Prism` carries the holes, so a hole's interior is never probed as tool
  material. Not applicable (a countersink cone, a hole across the tool axis, ...): Truck's
  boolean with the holed prism (our truck-shapeops fork builds such tools; Truck 0.6
  panicked), placed with the single-loop margins and checked the same way; if that fails too,
  `ProductLimit` naming the inner loops, the slab reason and the boolean's failure;
* a Join sketched on a face that its profile leaves: slab construction, else the
  product limit with the slab reason appended;
* `regen.rs::legacy_cut` (legacy Cut Through All / Pocket): the same fallback and check;
* `fillet.rs::fillet_edge`: the exact edge fillet is a slab cut of the corner region
  bounded by the two faces and the tangent arc (refused when the body is not prismatic
  along the edge).

## Consequences

* Canonical engulfed-hole cut exact: 40000 − 640π mm³, 4 cylinder patches all of radius 8,
  no radius-5 surface (`prismatic::arc_tests`, `tests/slab_arcs.rs`, `tests/geometry_gate.rs`).
  Off-centre partial overlap (Ø16 4 mm off the Ø10) exact, also through the legacy
  `CutThroughAll` command.
* A cylindrical wall is split into ≤ quarter-turn faces, and at every vertex any level has on
  that circle (a Truck-built Ø10 hole rebuilt by a slab result has 6 faces instead of 3).
  Cylinder faces are not referenced by plane; planar wall references are unaffected.
* Results are exact to the 1e-9 mm grid of the tool frame, plus: vertices within
  `KERNEL_LINEAR` of a circle are moved onto it, and 2-D vertices within `FACE_MATCH` merge.
* Refused by name: tangent contacts away from loop junctions, configurations ambiguous at 512
  chords per turn, cap edges that are neither straight nor circular, oblique or non-ruled
  walls, disconnected or non-manifold results.
* Tangency is checked over the whole cross-section, not only where the tool is: a body that
  already contains a tangent contact (e.g. a hole touching a pocket wall, built earlier by
  Truck) is refused by the slab construction for every later tool. Conservative.
* Determinism: the construction itself is deterministic (no hash-order dependence in its
  output). Truck's boolean, tried first, is not: the same seeded corpus gave different
  Truck outcomes in different processes (156 vs 161 accepted of ~190 on one stream).

## Addendum to ADR 0005: target faces are probed too

The arc corpus found a closed Truck result that the result check accepted although it
was wrong: a Ø11 cut externally tangent to the wall of an earlier 1 mm pocket returned the
body with that pocket *filled again* (+72.27 mm³, `tests/arc_fuzz.rs::
tangent_round_cut_beside_a_shallow_rounded_pocket`). The check probed result faces only,
and a refilled void leaves no result face. `verify_prisms_boolean` now also probes both
sides of the target's faces whose triangles overlap the tool neighbourhood (largest
triangles first, up to 3 decided per face, same offsets as result faces) against the
result's own winding number; any disagreement with the expected solid is a refusal
("beside a face of the target … existing geometry away from the tool was changed"). The
whole test suite and corpora pass unchanged under the stronger check.
