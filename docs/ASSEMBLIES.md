# Components, occurrences and joints

Status: run 6 flat model, run 7 nesting, more joint types, UI tree, exploded view and
hierarchical STEP. Code: `crates/rcad-document/src/assembly.rs` (model, validation, tree
solver, instance expansion), `crates/rcad-commands/src/assembly.rs` (commands, BOM,
interference, export), `crates/rcad/src/assembly_ui.rs` (browser tree and actions, display
placement), `crates/rcad/src/joint_ui.rs` (joint dialog), `crates/rcad-kernel/src/step.rs`
(`export_step_assembly`). Formats: v6 for the run-6 subset (`io::v6_feature`), v7 as soon as a
run-7 addition is used (`io::assembly_v7_feature`: nesting, empty components, independent
copies, hidden occurrences, an active component, exploded offsets, the new joint kinds). A
document without them still saves as v6 (frozen fixtures in
`crates/rcad-commands/tests/fixtures/native_v6/`).

## Model

* **Component** = a definition: a named set of this document's bodies (its *definition
  bodies*, modeled by the ordinary feature history) and, since run 7, **child occurrences** of
  other components (a sub-assembly). A body belongs to at most one component. A component may
  be empty (made to be activated and modeled into). Optional part number.
* **Occurrence** = a placement `{id, component, name, placement, owner, grounded, suppressed,
  hidden}`. `owner` is `None` for the top-level assembly or the component whose definition
  contains it. The placement is relative to the owner. All occurrences of a component are
  **linked**: editing the definition (its features, its child occurrences, joints between
  them) changes every occurrence and every instance of every enclosing sub-assembly.
* **Containment is acyclic**: placing a component inside itself or inside a component it
  (transitively) contains is refused (`add_occurrence {owner}`), and documents are validated
  (also on load). Nesting depth ≤ 16; ≤ 20 000 evaluated instances.
* **Instance** = an occurrence reached through a path of occurrences from the top level,
  written `O3/O7` (stable ids; names `Wing#1/Screw#2` for display). World transform = the
  composition of the local transforms along the path. Paths are what BOM rows, interference
  results, highlighting, exploded offsets and STEP usages refer to. `Assembly::evaluate()`
  returns every instance whose component has bodies (the leaf placements used by display,
  mass, interference, export and drawings); `evaluate_tree()` also returns pure sub-assembly
  instances.
* **Independent copy** (`make_independent`): a new component (`Screw copy`, `derived_from`
  the source) whose bodies are body copies (`MoveBody` copy features appended at the end of
  history) of the source definition bodies; the occurrence switches to it. It does not follow
  features added to the source after the copy; **parameter edits of features before the copy
  point do propagate** (the copy is history-based, not a frozen snapshot). Sub-assemblies
  cannot be made independent (refused). The UI and `ComponentSummary` label linked
  components (`linked ×N`) and copies (`independent copy of …`). The run-6 `move_body {copy}`
  body copy remains a separate, non-component thing.
* **Suppression** (`set_occurrence_suppressed`, configurations): the occurrence and everything
  below it leave display, BOM, mass, interference and export; the definition is not deleted.
  A suppressed occurrence of a sub-assembly's definition is suppressed in every instance.
* **Hidden** (`set_occurrence_hidden`): display only (with everything below it).
* **Active component** (`activate_component`): bodies created while a component is active
  join its definition (`Document::add_body`), so every occurrence shows them. Features stay in
  the single document history (the definition is "the history of those bodies"). Display: the
  instances outside the active component and loose bodies are drawn dimmed. Display and derived
  bodies are not auto-joined: `Document::add_body_unowned` (display bodies such as flat
  patterns) never joins; `Document::add_derived_body(name, source)` (stitch/fill/thicken/repair
  results) takes the source's place in the source's component, or stays loose. *As of this run
  the sheet-metal/surface commands still call `add_body` (owners: sheet/surface work) — until
  they switch, their outputs join the active component.*
* **Deleting** an occurrence or component also removes it from every configuration's excluded
  occurrences; ids that an older build left behind are dropped (and reported) on open. Deleting
  the feature that creates a component body removes the body from the definition.
* **Grounding**: a grounded occurrence cannot be moved, aligned or driven by a joint.
* **External components** (`insert_external_component {path, linked}`): another native
  document is loaded and regenerated; its visible solids (placed instances named by path, plus
  loose bodies) are written to one STEP text that is **embedded** in this document as an import
  feature, and those bodies become a new component (one occurrence at the origin). The source
  file's FNV-1a 64 hash and size are pinned (`ExternalSource`; change detection, not security).
  The model never reads the source again on its own: `check_external_components` reports
  `embedded`, `up_to_date`, `modified` or `missing`; `update_external_component` (linked only)
  replaces the embedded copy from the file in one undoable step, is a no-op when unchanged, and
  is refused with the copy kept when the file is missing or its solid count changed. A document
  with external components opens without their sources. The external document's own
  components/joints are not imported as structure (its solids are). Mesh bodies are refused.

## Joints (as-built tree solver)

`L_child = L_parent ∘ M(values) ∘ rest` in the coordinates of the common owner; `rest` is
captured when the joint is made, so nothing jumps (*as-built*). Optionally the child is
**snapped** first (`child_axis_from`: a cylinder face or two-face edge of the child, `flip`): its
axis is rotated onto the joint axis (minimal rotation about the child axis point) and moved
perpendicular onto the joint line, keeping its axial position; then the joint is made (one undo
step). UI: joint dialog **Use selected face as child axis**, **Flip**. `origin`/`axis` are in the parent's definition coordinates, given
explicitly or taken exactly from the parent's faces (`axis_from`: cylinder face axis, or the
line where two planar faces meet; captured at creation, not re-derived after edits;
`set_joint_axis` redefines it later without moving the child). The joint editor also has a
drive slider between the limits (each release commits one solved value; a motion preview of
the joint solver, not a simulation).

| kind | DOF | values (units) | motion `M` |
|---|---|---|---|
| rigid | 0 | — | identity |
| revolute | 1 | angle (°) | rotation about axis through origin |
| slider | 1 | distance (mm) | translation along axis |
| cylindrical | 2 | angle (°), distance (mm) | translate along axis ∘ rotate about it |
| pin_slot | 2 | angle (°), slot distance (mm) | translate along `u` ∘ rotate about the pin axis |
| planar | 3 | u (mm), w (mm), angle (°) | translate in the plane (normal = axis) ∘ rotate about the normal |
| ball | 3 | angles about axis, u, w (°) | R(axis, v1) ∘ R(u, v2) ∘ R(w, v3) through origin with axis/u/w the parent's fixed axes: the child turns first about w, then u, then axis (fixed axes) — equivalently about axis, then the moved u, then the moved w; gimbal lock at v2 = ±90° |

`u` = `dir2` projected perpendicular to the axis (default: a fixed perpendicular), `w = axis ×
u`. Each value has its own optional limits. **Enforcement**: a commanded value (or a limit
change) that would put any value outside its limits is refused and the document is unchanged —
no clamping, no drift. After any parameter edit the evaluation recomputes every instance from
the stored values.

**Solved mechanisms**: forests of joints between sibling occurrences (same owner), each
occurrence the child of at most one joint, driven by stored values. **Refused (structured
error, document unchanged)**: loops (closed chains), a second joint on a child
(over-constraint), a grounded child, joints between occurrences of different
(sub-)assemblies, zero axes, a `dir2` parallel to the axis (singular frame), a grounded or
jointed occurrence moved directly. Loops/closed-chain mechanisms, contact, gears, motion links
and joint-driven sub-assembly flexibility are **not** solved.

**Rigid groups** (`create_rigid_group`) are rigid joints from the first occurrence to each
other one. **DOF report**: per definition-level occurrence 0 (grounded or rigid child), joint
DOF, or 6 (free); `total_dof` sums non-suppressed occurrences (children inside a sub-assembly
count once, not per instance). This counts joint freedoms in a tree; it is not a general
constraint analysis.

## BOM

* **Flattened** (`Response.assembly.bom`): one row per component with bodies that has at least
  one instance anywhere in the tree; quantity = number of instances (a part used twice in a
  sub-assembly used three times counts 6); suppressed occurrences and their subtrees are
  excluded; pure sub-assemblies (no bodies) have no row; then loose visible bodies × 1.
* **Structured** (`Response.assembly.structured_bom`): under each parent (top level, then each
  sub-assembly row's definition) one row per component used there, in first-use order, with
  the quantity per one parent (`1`, `1.1`, `1.2`, …); loose bodies at level 0.

## Mass properties

Loose visible bodies plus every instance (each counts its evaluated definition bodies placed
by the composed world transform: centre of mass transformed, inertia rotated R I Rᵀ), total by
the parallel-axis theorem. Hidden occurrences count; suppressed ones do not.
`OccurrenceMass.path` identifies the instance.

## Interference

`interference {occurrences?}` is a **static** check of every pair of instances at their current
placement — not collision avoidance and not a motion study. Per pair of placed bodies: disjoint
bounding boxes, or boxes whose common extent on some axis is ≤ 1e-6 mm (`CONTACT_TOLERANCE`:
touching), → clear; else Truck's intersection, verified against both inputs, volume from the
B-rep measurement (`exact` when the volume method is exact, otherwise an estimate) — the
**exact** scope. When Truck cannot build the intersection, a 10³ point sample of the common box
decides (the **sampled** scope): points inside both → "overlap, volume unknown"; none → omitted.
Touching contact and overlaps thinner than the sample spacing are not reported. Results carry
both instance paths; clicking a result in the browser highlights both occurrences in the view.

## Exploded view

Per-instance offsets (`set_explode_offset {path, offset}`, world mm, accumulating down the
tree) and a toggle (`set_exploded`). Display state only: geometry, joints, BOM, mass,
interference and export are unchanged. Persisted (v7) and undoable.

## Export / import

* `export_assembly {format: "step"}` / `"3mf"`: **flattened** — one solid / object per placed
  instance body named by its path, plus loose visible bodies, in one STEP product.
* `export_assembly {format: "step_assembly"}`: **product structure** — one `PRODUCT` per
  component (reused definitions written once), sub-assemblies, each occurrence a
  `NEXT_ASSEMBLY_USAGE_OCCURRENCE` with a `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`
  (`ITEM_DEFINED_TRANSFORMATION` of its local placement), parts' solids in an
  `ADVANCED_BREP_SHAPE_REPRESENTATION` related to the product's `SHAPE_REPRESENTATION`, loose
  bodies as top-level parts. Validated independently with FreeCAD 1.1.4 (OpenCascade):
  `tools/check_step_assembly.sh` (hinge example: 7 solids with analytic volumes and centres,
  FreeCAD rebuilds the `Wing` App::Part under `Assembly` with leaf B and three screws, placed
  at the hinge angle). Evidence: `artifacts/step_assembly/hinge_tree_freecad.txt`.
* **STEP import flattens**: placements of every instance are composed (including the
  simple `SHAPE_REPRESENTATION_RELATIONSHIP` part link, fixed in run 7) and bodies are named by
  product, but no components/occurrences are recreated; the import notes say so ("assembly
  structure flattened: N component usage(s) … not recreated"). A flattened import is not
  assembly fidelity.

## UI (browser "Assembly" section)

Tree of instances (sub-assemblies indented, path names, linked/independent labels,
grounded/joint/DOF state, hidden in italics, rows outside the active component weak); click a
row to select it (highlighted in the view) and get its actions: Activate / Return to top level,
Hide/Show, Ground/Unground, Suppress (restore below), Move… (X/Y/Z, rotation about Z, snap
nudges and rotation steps), Rename… (occurrence and component), Make independent, Add
occurrence, Group… (sub-assembly from checked siblings), Insert into (another component into
this definition), Joint…, Explode…, Delete. **Create empty** component; **Align from this face**
/ **Align … to this face** from viewport picks; **New joint…** dialog (kind, parent/child,
explicit / cylinder-face / two-face-edge axis, slot or in-plane direction, initial values and
limits per DOF, DOF feedback before creation); joint **Edit…** per value and limits; **Bill of
materials** (flattened), **Structured BOM**, **Check interference** (selectable results),
**Exploded view**. Evidence: `crates/rcad/tests/assembly_ui.rs`,
`crates/rcad/tests/assembly_tree_ui.rs` (egui_kittest real widgets).

## Not supported (refused or absent)

* External components keep no feature history or structure of the source (solids only);
  updates that change the number of solids are refused.
* Joints between occurrences of different (sub-)assemblies; flexible sub-assemblies (a
  sub-assembly's internal joints have one value for all its instances).
* Joint loops / closed-chain solving, over-constrained systems, contact, gear/rack/screw
  motion links, as-built joints with a separate joint origin snap, joint limits beyond scalar
  value limits.
* Independent copies of sub-assemblies; frozen (non-history) copies.
* Picking or sketching on a placed copy: face selection, measurement and sketch frames refer to
  the definition bodies (the picked instance path is known and used for alignment and
  highlighting).
* STEP import preserving hierarchy (it flattens, see above); 3MF assembly structure (objects
  are flat).
* Collision avoidance, dynamic simulation, motion studies.

## Evidence

`crates/rcad-commands/tests/assembly.rs` (run 6 flat model), `nested_assembly.rs` (nesting,
paths, cyclic refusal, BOM, mass, joints inside sub-assemblies, activation, hidden, explode,
independent copy, alignment, cylindrical/pin-slot/planar/ball joints, v7 save/reopen),
`hinge_acceptance.rs` (acceptance 1, flat and nested, plus the committed
`examples/hinge_assembly_workflow.json` replay), `slider_acceptance.rs` (acceptance 2),
`crates/rcad-kernel/tests/step_export_assembly.rs`, `tools/check_step_assembly.sh` (FreeCAD).
