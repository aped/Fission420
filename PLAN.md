# Suggested overnight execution plan

This is an execution order and a set of time boxes, not a claim about what an agent will finish. Reallocate remaining time when evidence warrants it; protect the final integration reserve. Favor demonstrated behavior over feature count.

## Phase 0 — establish the runway (about 15 minutes)

- Inspect existing repo instructions, changes, toolchains, available disk/memory, network, and display access. Preserve unrelated work.
- Create a work branch when possible. Record the start/deadline, actual environment, and goal in plan/STATUS.md.
- Verify Rust and a minimal native window can build/run if the environment permits. Record missing native prerequisites without installing system-wide packages unattended.
- Create a small acceptance checklist from GOAL.md, marking everything pending.
- Avoid an elaborate crate workspace before the core dependency works.

Deliverable: a known build baseline or precise blockers with independent work identified.

## Phase 1 — prove the kernel (45-minute maximum)

Start with https://github.com/ricosjp/truck and its official examples. Record an exact revision/version and license. Build a small standalone experiment behind the intended adapter boundary:

1. Create the rectangular prism and cylindrical cutter.
2. Cut the hole; inspect validity, connected-solid count, bounds, and available volume computation.
3. Tessellate and export, retaining an actual B-rep as source of truth.
4. Repeat with the width/depth edits in GOAL.md.
5. Probe a coincident or near-tangent boundary and record unsupported behavior.

If a concrete required operation fails, save the reproduction before evaluating https://github.com/ecto/vcad. Do not spend the whole night comparing libraries. A second candidate shares this same phase budget. Verify the license and the actual public crate APIs rather than assuming the web app's features are exposed as a reusable library.

Write docs/adr/0001-kernel.md with the tested choice, exact evidence, unsupported cases, and adapter API. If neither passes, explicitly narrow the demo and preserve the failure; do not silently switch to a C++ kernel or fake the hole with a mesh.

Deliverable: a runnable geometry spike, reproducible measurements, and a choice or documented blocker.

## Phase 2 — headless parametric workflow (about 90 minutes)

- Define typed dimensions and stable feature IDs, then a minimal dependency graph: rectangle sketch → extrusion → attached circle sketch → through-all cut.
- Implement semantic face references and recomputation. For this first shape, the extrusion's named top-face role may be sufficient; document its limits.
- Expose create/edit/inspect/save/load via typed commands with a JSON CLI. Keep the command schema small and documented.
- Keep feature parameters authoritative. Cache geometry only as a disposable result of evaluation.
- Implement transactional failure and the negative-input cases before adding more features.
- Add independent analytic checks for the acceptance model, then depth/width regeneration tests. If analytic volume is unavailable, report that missing check separately from mesh-volume validation.

Deliverable: one command reproduces the complete headless model and emits measurements, with nonzero exit on a failed acceptance check.

## Phase 3 — persistence and editing (about 45 minutes)

- Implement undo/redo around committed document changes; exclude preview changes from history.
- Implement a versioned native format and atomic save. Round-trip the graph, dimensions, references, and geometry regeneration.
- Test invalid/corrupt native input, invalid operations, and unchanged prior state on failure.
- Add a checked-in tiny original example document and demo script.

Deliverable: the editable model survives history operations and reload.

## Phase 4 — connect the native app (about 90 minutes)

- Add orbit/pan/zoom and a fit-to-model action; show the actual kernel tessellation.
- Add a model tree/history list, dimensions inspector, selected-feature highlighting, and clear diagnostics.
- Connect dimensioned rectangle, extrusion, hole, edit, commit/cancel, undo/redo, and save/load to shared commands.
- For the initial demo, tools may use dimension forms rather than full sketch dragging. State that limitation visibly in the report.
- Keep heavy geometry evaluation from freezing interaction; do not build a distributed job system.
- Walk through the acceptance sequence in the actual UI when possible. Save screenshots and a short written reproduction. Otherwise ship the launch command and mark interaction validation pending.

Deliverable: a usable native workflow, or a runnable shell with precisely identified integration gaps.

## Phase 5 — verify and hand off (last 30–45 minutes)

- Stop feature work. Run the acceptance demo, owned-code formatting/lints/tests, a native build, and UI smoke checks if supported.
- Verify STL watertightness and meshing accuracy when export exists. Never label STL as STEP.
- Check failure rollback, save/reload, and the top-face attachment after changing extrusion depth.
- Record actual measurements, test totals/outcomes, and unrun checks. Avoid unsupported parity percentages.
- Commit the verified state where possible and write MORNING_REPORT.md.

## Suggested parallel tasks

Only after the kernel and command interfaces are settled: one worker owns document/history, another owns the native UI, and the coordinator owns kernel integration and acceptance tests. Alternatively run a reviewer against the completed geometry code. No two workers edit shared API files concurrently; integration remains sequential. If serial, simply follow phases in order.

## Useful persistent files to create during execution

- docs/adr/0001-kernel.md — evidence and dependency choice.
- docs/DEPENDENCIES.md — sources, versions, and license decisions.
- docs/behavior/plate-workflow.md — observed/documented/assumed interactions.
- docs/COMMANDS.md — actual headless API and examples.
- plan/STATUS.md — current truth and the next bounded task.
- MORNING_REPORT.md — user-facing handoff.

## References and their role

- https://github.com/storytold/cadcraft/blob/main/AGENTS.md — inspiration for command-driven architecture and persistent agent state; these instructions are original, not copied from that repository.
- https://github.com/storytold/cadcraft/blob/main/ROADMAP.md — example of tracking incomplete features; do not inherit its time/parity estimates.
- https://github.com/ricosjp/truck — kernel candidate; verify current APIs locally.
- https://github.com/ecto/vcad — alternative candidate; benchmark actual behavior.

Reference projects are evidence and potential dependencies, not authorities over this repository's instructions.
