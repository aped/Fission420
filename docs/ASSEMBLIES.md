# Components, occurrences and joints

Status: run 6 (pull-forward of Run-7 §1-2). Code: `crates/rcad-document/src/assembly.rs`
(model, validation, tree solver), `crates/rcad-commands/src/assembly.rs` (commands, BOM,
interference, export), `crates/rcad/src/assembly_ui.rs` (browser section, placed display).
Format v6 (`document.assembly`; `io::v6_feature` "components, occurrences and joints").

## What is supported

* **Component** = a named set of this document's bodies (its *definition bodies*), with an
  optional part number. A body belongs to at most one component. The definition is the
  ordinary feature history of those bodies.
* **Occurrence** = a placement of a component: `{id, component, name, placement, grounded}`.
  `create_component` makes the first occurrence (`Name#1`) at identity, so nothing moves.
  More occurrences (`add_occurrence`) are **linked**: editing any feature of the definition
  changes every occurrence. An **independent copy** is a different thing: `move_body` with
  `copy` makes a new body with its own history from that point; it is not an occurrence, is
  not counted in the component's BOM quantity and does not follow later definition edits.
* While a body belongs to a component that has occurrences, it is displayed, measured
  (`mass_properties` defaults) and exported (`export_assembly`) **once per occurrence**,
  placed. A component with no occurrences shows its bodies in place.
* **Joints** (tree solver): `rigid` (0 DOF), `revolute` (value in degrees about an axis
  through `origin`), `slider` (value in mm along `axis`), each with optional `min`/`max`
  limits. `origin`/`axis` are in the parent occurrence's definition coordinates (read a
  cylinder axis with `measure_faces` if needed). The child follows
  `T_child = T_parent ∘ M(value) ∘ rest`, `rest` captured when the joint is added (the child
  does not jump). Each occurrence is the child of at most one joint; a joint that would close
  a loop, a second joint on the same child (over-constraint), a grounded child, a value or
  limits excluding the current value are **refused** (structured error, document unchanged).
  A joint child cannot be moved directly or grounded. Deleting a joint keeps the child where
  it is (its pose becomes its free placement).
* **DOF report**: per occurrence 0 (grounded or rigid child), 1 (revolute/slider child),
  6 (free); `AssemblySummary.total_dof`. This counts joint freedoms in a tree; it is not a
  general constraint analysis.
* **Grounding**: a grounded occurrence cannot be moved or driven.
* **BOM** (`bom`): flat. One row per component with at least one occurrence, quantity =
  number of its occurrences, part number, occurrence names and the definition bodies'
  materials; then one row per visible evaluated body that is in no component (quantity 1,
  "loose body"). Suppressed definition bodies do not remove the row (the occurrences still
  exist); hidden loose bodies are not listed.
* **Mass properties** (`mass_properties` without lists): loose visible bodies plus every
  occurrence (each counts its visible evaluated definition bodies, placed: centre of mass
  transformed, inertia rotated R I Rᵀ), total by the parallel-axis theorem. Component bodies
  are not also counted as loose bodies.
* **Interference** (`interference {occurrences?}`): a *static* check of every pair of
  occurrences at their current placement — not collision avoidance and not a motion study.
  Per pair of placed bodies: disjoint bounding boxes → clear; else Truck's intersection,
  verified against both inputs (`rcad_kernel::verify_boolean_by_membership`), volume from the
  B-rep measurement (`exact` when the volume method is exact, otherwise an estimate). When
  Truck cannot build the intersection, a 10³ point sample of the common box decides: points
  inside both → "overlap, volume unknown"; none → "none found" (omitted from the list).
  Touching contact (coincident faces) and overlaps thinner than the sample spacing are not
  reported.
* **Export** (`export_assembly {path, format: step|3mf}`): one STEP solid / 3MF object per
  placed body, named by the occurrence (`Block#2`, or `Block#2/Body1` for multi-body
  components), plus loose visible B-rep bodies. The ordinary `export_step` / `export_stl` /
  `export_3mf` commands still export **bodies** at their definition placement (one per body,
  not per occurrence).
* **UI**: browser **Components** section — create a component from the selected body (name,
  part number), add occurrence, Move… (numeric X/Y/Z and rotation about Z), Ground/Unground,
  Delete, joint Edit… (value, limits), **Bill of materials**, **Check interference**. Joints
  are created through the command API (`add_joint`); there is no joint-creation dialog yet.

## Not supported (refused or absent)

* Nested sub-assemblies (components containing occurrences), external/linked files,
  per-occurrence suppression, per-occurrence configurations.
* Joint loops, over-constrained joints, cylindrical / pin-slot / planar / ball joints,
  as-built joints, contact or motion limits beyond the scalar value limits.
* Picking or sketching on a placed copy: the display draws every occurrence, but face
  selection, measurement and sketch frames refer to the definition bodies at their definition
  placement.
* Collision avoidance or dynamic simulation.

Evidence: `crates/rcad-commands/tests/assembly.rs` (linked vs. copy, mass totals across
occurrences, BOM, undo, save/reopen, refusals, interference volume, STEP/3MF export with
placed objects, slider fixture 0–40 → 60 with reopen, revolute hinge with limits, loop
refusal, joint deletion), `crates/rcad/tests/assembly_ui.rs` (kittest widgets, placed display).
