# ADR 0005: Independent check of every Join/Cut result

Status: accepted.

## Context

Truck 0.6 booleans can return a single closed shell that is still the wrong solid. Before
this decision, two such cases were documented behind ignored tests and a third untested one:

* a cut that fully engulfs an existing hole keeps the old hole's walls inside the void
  (2010.6 mm³ removed instead of 1225.2; the shell reports "closed");
* a Join sketched on a face moved its start 0.5 mm into the support to avoid coplanar
  boolean faces; over a thinner skin or flange that overlap leaked into the cavity below
  (3113.2 vs 3110 mm³; 2161.6 vs 2160 with unchanged bounds);
* pattern and mirror copies reused the seed's overlap and margins without checks.

A volume check alone does not catch these reliably (compensating errors, unchanged bounds),
and validating with the same boolean is not independent.

## Decision

1. **Nominal operation.** Each Extrude Join/Cut (seed or pattern/mirror copy) has a
   *nominal* tool: the user's profile over the user's span along the sketch normal
   (`ExtrudeOut.nominal`; through-all: the target's whole extent plus margin). Workarounds
   (Join overlap into the support, blind-cut start/floor margins) only change the tool handed
   to the kernel; they never change what the result must equal:
   `expected = target ∪ prism` (Join) or `target \ prism` (Cut).
2. **One tool path.** `regen.rs::apply_tool` applies a tool for the seed and for every
   pattern/mirror copy. The workarounds are re-derived for each placement from the material
   around it, and every result goes through the check below. The legacy Cut Through All and
   Pocket paths are checked too.
3. **Independent result check** (`rcad-kernel/src/validate.rs`, `verify_prism_boolean`):
   * the result's tessellation must be closed (edge incidence / orientation);
   * for every result face near the tool actually used (largest triangles first, up to 3
     decided probes per face), a point just inside and just outside the face is classified:
     tool membership **analytically** (prism of the profile polygon, arcs subdivided to
     1e-6 relative); target membership by the **generalized winding number** of the
     target's tessellation (exact for a closed polyhedron). The inside point must be expected
     material and the outside point must not;
   * probe offsets: planar faces max(20 × `KERNEL_LINEAR`, 1e-6 × size); curved faces
     3 × the validation chord (chord = clamp(1e-4 × size, 1e-3, 0.02) mm), so both sides
     clear the chord error; points too close to a surface are skipped, not guessed.
   * every nominal tool's interior is sampled (7 × 7 profile grid × 3 depths): points of
     `tool ∩ target` must be outside the result (Cut), points of the tool inside it (Join),
     by the result's own winding number — so a tool the kernel silently did not apply fails;
   * a check in which no probe could be classified is a refusal, not a pass.
   A failure is `DocError::GeometryCheck { feature, detail }`; the transaction is refused
   and nothing is committed, saved or exported. Patterns and mirrors are checked once, on
   the final body, against the union of all their nominal tools (`verify_prisms_boolean`).
4. **Exact Join overlap.** Instead of a fixed 0.5 mm, the overlap is
   `min(0.5, distance/2, clear/2)` where `clear` (`validate.rs::clear_depth`) is the depth
   under the face to which the prism of the profile is free of any other surface of the
   target: exact for planar faces (triangle clipping against the profile footprint),
   chord-corrected for curved faces (whose triangles are ignored within 2 chords of the
   profile boundary, so a cylinder stacked on a cylinder of the same radius still works).
   Less than 2 × `MIN_LENGTH` of clear material is a `ProductLimit` refusal.
5. **Exact blind-cut margin test.** Moving a cutter end past a coplanar face is allowed only
   where the slab it sweeps is empty: `prism_is_empty` = no surface crosses the slab
   (`clear_depth`) and an interior point lies outside the body (winding number). The
   earlier test probed with a cut, which Truck could fail or panic on beside coplanar faces, refusing
   valid cuts (e.g. a notch through an enclosure wall exactly into the cavity).

## Consequences

* The engulfed-hole cut is **refused** (`GeometryCheck`, document unchanged), through Extrude
  Cut, blind cut and Cut Through All alike. Splitting the cutter into overlapping halves was
  tried and fails in Truck on the second half. The supported route is to edit the existing
  hole's dimension (control test). Kernel limitation, not fixed.
* Thin skin / flange joins and their linear, circular and mirror copies are now **exact**
  (`crates/rcad-commands/tests/geometry_gate.rs`, each with an adjacent control).
* Verification is to the validation tessellation's tolerance: a defect thinner than the probe
  offset on a curved face, or confined to a face region that no probe reaches, could pass.
  The check is a strong guard against wrong whole faces (the observed failure mode), not a
  proof of exactness. The kernel's own closed-shell test is kept.
* Cost: two extra tessellations and O(probes × triangles) winding sums per boolean (once per
  pattern); the 855-check acceptance suite runs in the same time (7.6 s before, 7.2 s
  after); the 19-hole pattern benchmark 1908 ms → 2116 ms.
* Hole "through all" goes into the material only (through everything of the target in that
  direction), so the nominal tool never includes material behind the face.
* Pocket/flush cases that Truck cannot build (cutter walls coplanar with outer walls) remain
  refusals (`ProductLimit` / kernel error), never wrong geometry.

### Amendment: target-face probes near curved tool walls
The probes on target faces near the tools (checked against the result's own winding number)
are not classified within `h_curved` (three times the check chord) of a tool wall: the result's
mesh of a curved tool wall is only chord-accurate, so a probe closer than that could disagree
with the exact tool test without any defect. Seen on a second through hole whose top point lay
on a large target triangle's centroid (test `second_hole_through_a_thin_wall_is_accepted_in_either_order`).
