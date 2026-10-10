# Fission420 — morning report (run 1)

Run window: 2026-10-10 00:34:03 → deadline 08:34:03 PDT (8 h, per the /goal instruction; GOAL.md's
default was 6 h). Branch `work/run-1`. See `plan/STATUS.md` for the task log and current commit.

## 1. What works / what does not

Works (verified, evidence below):
- **Kernel:** Truck 0.6 (pure-Rust B-rep, Apache-2.0) behind `crates/f420-kernel`. Covers extrusion of a
  closed profile, through-all cut (Truck boolean), and B-rep measurements: shell count, closedness,
  bounds, and a boundary-integral volume over exact edge curves. Also watertight tessellation, binary STL,
  and **STEP that OpenCascade (FreeCAD) imports as one valid solid with volume error 5e-7 mm³**.
- **Sketch:** a Levenberg–Marquardt constraint solver with SVD rank. It reports residual, DOF, and
  redundant or conflicting constraints. Dimensioned centred-rectangle and centred-circle templates.
- **Document:** typed parameters and a stable-ID feature history with a dependency graph and cycle
  detection. A **semantic face reference** `FaceRef{feature, role: ExtrudeEndCap|ExtrudeStartCap}` is
  resolved from the feature definition, then validated as exactly one B-rep face, otherwise reported
  as a broken reference.
- **Document lifecycle:** transactional edits with full rollback, undo/redo of whole-document
  snapshots, and a versioned native JSON format with atomic save.
- **Command API:** one typed JSON API (`f420-commands`) used by the CLI, the UI, and the tests.
- **Native app:** egui/eframe + wgpu on Wayland. It has:
  - a 3D viewport (orbit, pan, zoom, fit, face hover highlight);
  - a history tree showing the semantic reference and constraint state;
  - an inspector with Apply / Cancel edits;
  - create forms, undo/redo, save/open/close, STL/STEP export, and diagnostics.
  - Regeneration runs on a worker thread.
- **Stretch work, all verified:**
  - **Interactive placement:** drag a rectangle on world XY, or drag a hole on the semantic end cap,
    with a live overlay preview; it opens a pre-filled dialog and creates the feature through the
    same commands. Centres are editable as `center_x` / `center_y`.
  - **Revolve:** a WorldXZ profile about world Z, checked against Pappus' theorem.
  - **Bracket example:** a 120×60 plate with five holes, with STL and STEP exported and validated
    by FreeCAD.
  - **Regeneration cache:** undo/redo and downstream edits reuse upstream B-reps.
- **Acceptance:** all 10 GOAL.md steps pass headlessly, 96/96 checks. Steps 1–7 and 10 also pass by
  driving the real UI widgets with egui_kittest.

Does not work / not done:
- **UI scope:**
  - Placement creates fixed-template sketches (rectangle, circle). There is no freehand sketching
    or dragging of existing geometry.
  - There are no dimension annotations drawn in the 3D view.
  - Selecting a history feature does not highlight its faces in 3D; only pointer hover highlights a
    face.
- **Modeling scope:**
  - One base body (extrude or revolve) per document. Cuts are only supported on extruded bodies.
  - Cuts must be circular, parallel to the extrusion, and strictly inside the base profile.
  - Face roles exist only for extrusion caps.
  - No fillets, revolve, or assemblies.
- **Undo history** is not persisted, so Undo is empty after Open (by design).
- **Real-window interaction:** no human has clicked through the app, and mouse/keyboard behaviour
  in the live window was not exercised.
  - I tried `xdotool` against the app running under XWayland. The X pointer warps and clicks never
    reached the window: GNOME's XWayland pointer state was stale, and XTEST may require an
    input-emulation permission prompt.
  - I stopped there rather than trigger or bypass prompts, or inject input into the whole desktop
    with `ydotool`.
  - What is verified: the live window was run scripted (`--demo`, `--demo-place`) for screenshots.
    egui_kittest drives the same `F420App::ui` code with real egui pointer and key events, including
    viewport drags.
  - To record your own walkthrough: `target/release/f420-app --shot-dir /tmp/shots`, then press F12
    at each step.
- **Truck boolean fragility:** some multi-hole configurations make `truck_shapeops::and` return
  None (example: a corner Ø6.5 hole followed by a centred Ø20).
  - The adapter retries with the same exact geometry: 4 circle seam angles × 3 boolean tolerances.
    That fixed every case observed.
  - If every attempt fails, the cut is reported as a `Kernel` error and the document is preserved.
- **Reference validation:** no Fusion comparison was done (no access), so it is pending.

## 2. Commands that were run

```bash
cd <repo>
cargo xtask check                     # fmt --check, clippy -D warnings, test --release, acceptance
target/release/f420-cli acceptance --out artifacts/acceptance
target/release/f420-cli run examples/plate_workflow.json
tools/external_validation.sh artifacts/acceptance   # python STL check + FreeCAD STL/STEP import
cargo build --release -p f420-app
target/release/f420-app                              # interactive
timeout 90 target/release/f420-app --demo --screenshot artifacts/ui_demo.png
timeout 90 target/release/f420-app --demo-place --screenshot artifacts/ui_place_done.png
timeout 90 target/release/f420-app --demo-place-preview --screenshot artifacts/ui_place_preview.png
target/release/f420-cli run examples/bracket_workflow.json
```

## 3. Evidence and observed results

| check | result | evidence |
|---|---|---|
| `cargo xtask check` | all stages pass (fmt, clippy, tests, acceptance, plate and bracket examples); 78 tests (app 8 unit + 4 placement + 7 workflow, commands 5, document 11, kernel 11, sketch 15, viewport 17) | `artifacts/xtask_check_full.log` |
| headless acceptance | 96 passed, 0 failed | `artifacts/acceptance/acceptance_report.json` |
| 80×50×10, Ø10 volume | 39214.601825 vs 39214.601837 (err 1.2e-5; tol 3.9e-3) | report step 4 |
| 100×50×10, Ø10 volume | 49214.601834 (err 2.8e-6; tol 4.9e-3) | report step 5 |
| 100×50×12, Ø10 volume | 59057.522193 (err 1.1e-5; tol 5.9e-3) | report step 6 |
| bounds | max error 5.1e-10 mm (tol 1e-6) | report |
| hole axis | centroid of hole loops on z=0 and z=d at |xy| ≤ 1.5e-8 mm; equivalent diameter err ≤ 7.5e-8 | report |
| top-face attachment | hole sketch frame z = 12 after depth 12, back to 10 after restore | report step 6, UI test |
| undo/redo, save/close/reload | document equality after reload; regenerated volume = saved cache | report step 7 |
| invalid edits | thickness −10, Ø60, Ø50 (tangent), height 8 all rejected; document and revision unchanged | report step 10, UI test |
| STL (chord 0.01 mm) | watertight (Rust check, independent Python parser, FreeCAD `isSolid`, no non-manifolds); mesh volume rel err 1.7e-5 | `artifacts/acceptance/stl_check_*` |
| STEP | FreeCAD/OCC: 1 solid, 9 faces, `isValid`, volume 49214.6018371 (err 5e-7) | `artifacts/acceptance/step_check_freecad.txt` |
| UI screenshot | plate with hole, history with semantic ref, DOF 0, inspector, rejected −10 shown | `artifacts/ui_demo.png` |
| placement | mid-drag preview "Ø12 mm @ (20, 8)" on Plate.EndCap; committed off-centre hole, volume 38869.026652 = 40000 − 360π | `artifacts/ui_place_preview.png`, `artifacts/ui_place_done.png` |
| revolve | washer 360° and 90° match Pappus within 1e-3 mm³, watertight; also driven through UI forms | kernel/commands/app tests |
| bracket | 120×60×6, 4×Ø6.5 + Ø20: B-rep volume 40518.650659 vs 40518.650670 analytic; FreeCAD STEP 1 valid solid, 21 faces, err 2e-6 | `artifacts/bracket_workflow.jsonl`, `artifacts/bracket_step_check_freecad.txt` |
| external validation | Python STL check + FreeCAD STL/STEP pass | `artifacts/external_validation.log` |

Notes:
- **Volume method:** the "B-rep volume" is a boundary integral over the exact Truck curves (chord sums
  plus Richardson extrapolation), not a closed-form analytic value. It reports `Unavailable` for faces
  that are not planar or Z-ruled. The error estimate is reported alongside the value.
- **FreeCAD's STL volume varies between runs** (49214.67 / 49214.86) for the same file. It appears to
  use float32 accumulation. The independent Python and Rust mesh volumes agree to 1e-9.

Checks not run:
- Manual live-window interaction.
- Fusion reference behaviour.
- Other Linux distributions or GPUs.
- Debug-build performance, beyond noting it's slow; dependencies are now optimised in the dev
  profile.

## 4. Kernel choice and observed limitations

Truck 0.6. vcad 0.10 was rejected: it pulls in C++ via clipper-sys, its volume comes only from the mesh,
and its STEP facade is broken. Full detail in `docs/adr/0001-kernel.md`. Observed Truck limitations:
- **Coplanar boolean faces fail.** Through-all cutters therefore overshoot the body.
- **No mass properties.** The volume method was added in the adapter.
- **`IntersectionCurve::subs` can panic.** The adapter wraps all Truck calls in `catch_unwind`.
- **STEP writer bug:** truck-stepio 0.3 writes duplicate entity ids for `INTERSECTION_CURVE`. It is
  worked around by rebuilding exact NURBS edges, for the plane ∩ Z-extrusion case only.
- **Performance:**
  - A full plate regeneration takes about 1.3 s in release builds, dominated by the exact-curve
    volume passes (now 3 threads, final body only).
  - The prefix cache makes undo/redo and downstream-only edits much cheaper.
  - The bracket script still takes about 70 s, because every new hole needs a real boolean.
- **Cylinder export:** cylinders go to STEP as rational B-spline surfaces, not `CYLINDRICAL_SURFACE`.
- **Transitive C code:** liblzma is compiled via truck-meshalgo's `vtk` feature. It isn't geometry;
  documented in `docs/DEPENDENCIES.md`.

## 5. Five-minute walkthrough

1. `cargo build --release -p f420-app && target/release/f420-app`
2. **New**, then **Rectangle on XY…**: width 80, height 50, OK. The history shows F1, "FullyConstrained · DOF 0".
3. **Extrude…**: sketch F1, distance 10, OK. The status bar shows volume 40000 mm³.
4. **Circle on face…**: "End cap of extrusion" = F2 Plate, diameter 10, OK. F3 shows
   "on Plate.EndCap (semantic: F2 ExtrudeEndCap)". **Cut through all…**: sketch F3, OK.
   Volume ≈ 39214.6018.
5. Select F1, set width to 100, **Apply**. Volume ≈ 49214.6018; the hole stays centred.
6. Select F2, set distance to 12, **Apply**. Volume ≈ 59057.5222; F3's frame follows to z = 12. Set it
   back to 10.
7. **Undo** and **Redo** from the toolbar. **Save As…** `/tmp/plate.f420.json`, **Close**, **Open…** the
   same path.
8. **Export STL…** `/tmp/plate.stl`: the dialog reports watertight. **Export STEP…** `/tmp/plate.step`.
9. Set F2 distance to −10, then set F3 diameter to 60. Both show red errors, and the model is
   unchanged.

10. Optional: **Place hole**, choose F2 in the combo, press on the top face, drag outward, release,
    then OK. **Rectangle on XZ…** then **Revolve…** makes a washer.

Headless equivalent: `target/release/f420-cli run examples/plate_workflow.json`.

## 6. Next three tasks (by value) and open decisions

1. **Freeform sketching.** Lines and arcs with interactive constraints (coincident, equal, tangent)
   using the existing solver, plus dimension annotations drawn in 3D. Template placement already
   exists.
2. **Generalise semantic references.** Add roles for side faces (keyed by profile edge) and faces
   created by cuts. Allow more than one body or feature per document, using union and blind cuts.
   This needs a topological-naming design (ADR 0002).
3. **Kernel hardening.** Upstream or `[patch]` the truck-stepio INTERSECTION_CURVE fix and the boolean
   fragility repro (`examples/bracket_workflow.json` without retries). Faster volume on unchanged
   faces.

Open decisions:
- Whether to vendor or patch Truck. That would remove liblzma and fix the STEP writer at the source.
- Whether undo history should be saved with the file.
- Scale-aware face-match and weld tolerances for very large or small parts.
