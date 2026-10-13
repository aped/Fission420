# ADR 0011: Modeled threads as faceted solid geometry

Status: accepted (run 6, 2026-10-13).

## Context

SESSION_6 asks for modeled external and internal threads as real solid geometry (not a
cosmetic texture), for a stated nonproprietary test profile: 60° symmetric V, nominal
Ø20, pitch 2, length 12, handedness, lead-in/runout, clearance; exact, or faceted with a
declared chord tolerance and clearly labelled.

Exact options considered:

* A helicoid (thread flank) is not a rational surface, so it has no exact NURBS form;
  Truck 0.6 has no helical sweep primitive. Any NURBS helicoid is itself an approximation.
* Building the thread as a boolean of a helical tool with a cylinder: Truck 0.6 booleans
  fail on revolved cones (run-4 spike) and on many-faced coplanar-heavy tools; a thread
  cut by boolean would need thousands of intersection curves. Not viable.

## Decision

Faceted, built directly (no booleans), `rcad-kernel/src/thread.rs`:

* **Profile** (stated test profile, not a certified standard): fundamental triangle
  height `H = p·√3/2`; over one pitch along the axis: root flat `p/4` at the core
  radius, flank `5p/16`, crest flat `p/8` at the major radius, flank `5p/16`; depth
  `5H/8` (1.0825 mm for p = 2, core Ø17.835).
* **Surface:** a helical ribbon of cells between `n` angle samples per turn and the four
  profile vertices; each cell is two planar triangles whose vertices lie exactly on the
  ideal surface. `n = max(24, ceil(π / acos(1 − tol / r_major)))`, so the chords of the
  major circle have sagitta ≤ `tol` (default 0.01 mm → 71 per turn). The kernel reports
  the measured deviation of the ideal surface from the facets at cell centres
  (0.0098 mm for the test thread) and labels the result `faceted: true`; the document
  adds a feature note ("faceted thread … not an exact helicoid").
* **Lead-in / runout:** the ridge height grows linearly from 0 over the first turn and
  shrinks over the last; at both ends the surface meets the core cylinder, so the solid
  closes with faceted walls (planar quads between vertical lines) and flat end caps.
  Length must be a whole number (≥ 3) of pitches.
* **Handedness:** a left-hand thread is the mirror image (angles clockwise, loops
  reversed).
* **Internal thread / clearance:** a square nut whose void is the rod shape offset
  radially by `clearance` (default 0.1 mm): the void's lateral faces reversed, the box
  sides, and end faces with the void's cap polygons as inner loops.
* **Document:** `FeatureKind::Thread { plane, center, diameter, pitch, length, left_hand,
  chord_tol, nut: Option<NutSpec { width, clearance }>, body }` creates a new body;
  parameters `diameter`, `pitch`, `length`, `nut_width`, `clearance`; command `thread`;
  native format v6.

## Evidence

* Kernel tests (`thread::tests`): the faceted rod's exact volume (3215.0959 mm³) against
  the ideal thread volume integrated from its definition, V = ∫∫ R(θ, z)²/2 dθ dz
  (3219.1698 mm³): the faceted solid is inscribed, 4.07 mm³ (0.13 %) smaller, within the
  bound deviation × area (14.8 mm³). Probes: crest material and root air at known
  heights; left hand crest a quarter pitch lower at 90°; the nut's volume against the
  box minus the ideal void; no sampled point inside both rod and nut.
* FreeCAD 1.1.4 (`tools/check_thread.sh`): both STEP exports are single valid solids
  (2984 / 2988 faces) with volumes equal to ours to 1e-7; OpenCascade's rod ∩ nut is
  empty (0.0 mm³), while the control (nut moved half a pitch) clashes (149.0 mm³).

## Consequences

* Threads are real solids for printing, measurement and STEP export, faceted at a
  declared tolerance; they are not exact helicoids and are labelled as such.
* Threaded bodies are new bodies only; joining a thread to another body or cutting an
  internal thread into an arbitrary body would need booleans on ~3000-face polyhedra
  (not attempted; Truck's performance and robustness there are unknown).
* Each thread is ~3000 planar faces; display and export sizes grow accordingly (STEP
  ~3 MB).
