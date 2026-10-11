# Fission420 — morning report, run 2

Spec: `SESSION_2.md` (Night 2). Branch **`work/run-2`** (from run 1's `work/run-1` @ 68aba09).
Final commit: **TBD** (see `git log -1`). Run window: started 2026-10-10 16:23:57 PDT,
deadline 2026-10-11 00:23:57 PDT; finished **TBD** (elapsed **TBD**).
Machine: AMD Ryzen 7 9800X3D (16 threads), RX 7900 GRE, 32 GB, Bazzite (Fedora 44 atomic),
kernel 7.2.7, GNOME/Wayland, rustc 1.99.0, release builds unless noted.

## 1. Summary

The prototype now has one coherent sketch → dimension → extrude → edit workflow driven from
the mouse and keyboard, and the original run-1 modelling (templates, cuts, revolve, exports,
bracket) still works and still passes its tests.

* **Sketch editor** (new): Create Sketch on world XY or on a selected extrusion cap (semantic
  `FaceRef`), Edit Sketch for existing sketches (including run-1 templates and old files). Tools:
  Select, Line/polyline, Center Rect, 2-Point Rect, Circle, 3-Point Arc, Dimension. Snapping to
  points (shared endpoints), H/V alignment (adds visible H/V constraints) and an adaptive grid;
  screen-space tolerances; constraint-respecting drags with explanations; inline dimension
  editing with units (Enter applies, Esc cancels); Horizontal / Vertical / Coincident / Tangent /
  Equal radius; local undo; Finish = one document undo step, Cancel discards.
  Behaviour spec: `docs/behavior/sketching.md`.
* **Profiles**: any single closed loop of lines and arcs (or one circle) extrudes as an exact
  B-rep; open / self-intersecting / zero-length geometry is kept but refused with a reason.
* **UI**: command bar, browser (left), contextual inspector (right), status bar; tree ↔ view
  selection with GPU highlight of the selected feature's faces; contextual commands disabled
  with reasons; view presets (Top/Front/Right/Iso, keys 7/1/3/0, F fit) and an axis triad;
  Help (F1); unsaved-changes guard on New/Open/Close/window close; Ctrl+S/Ctrl+Shift+S/Ctrl+O/
  Ctrl+Z/Ctrl+Shift+Z; shortcuts ignored while typing; error banner with a next step; empty
  state; dimensions of the selected sketch drawn in 3D and editable in place.
* **Responsiveness**: exact B-rep measurement (75 % of a cut-body regeneration) is deferred to a
  background thread; a plate edit now shows new geometry in **575 ms** instead of 2395 ms, with
  measurements following and tagged by document id + revision (never shown as current for
  newer geometry). Sketch interaction is local and sub-millisecond.
* **Stretch**: (1) blind circular pockets with an explicit Through all / Blind depth choice;
  (2) three-point arcs with Radius / Tangent / Equal radius and line+arc profiles;
  (3) kernel bottleneck work: **TBD**.
* **Evidence of real-window behaviour**: the desktop session was locked all night
  (`loginctl … LockedHint=yes`), so all window evidence comes from a *private headless GNOME
  compositor* (`mutter --headless`) running the real release binary, with **real Wayland
  pointer/keyboard input** injected through that compositor's own RemoteDesktop D-Bus API.
  Four scripted walkthroughs assert document state after every step and pass
  (`tools/live_walkthrough.sh`).

## 2. Build, launch, demo, test commands (all run tonight)

```bash
cd <repo>
cargo xtask check                         # fmt, clippy -D warnings, tests (release), acceptance, 5 example replays
cargo build --release -p f420-app
target/release/f420-app                   # interactive (F1 = help)
target/release/f420-cli acceptance --out artifacts/acceptance
target/release/f420-cli run examples/scenario_a_workflow.json   # also plate, bracket, scenario_b, scenario_r
tools/external_validation.sh artifacts/acceptance               # Python STL + FreeCAD/OCC STL+STEP
tools/live_walkthrough.sh artifacts/live_walkthrough            # real-input walkthroughs in a private headless compositor
cargo run --release -p f420-app --example regen_profile        # kernel stage timings
cargo run --release -p f420-app --example drag_latency         # sketch drag solver cost
cargo test --release -p f420-app --lib engine::tests::timings -- --ignored --nocapture
```

## 3. Acceptance matrix (SESSION_2)

Legend: **UI** = egui_kittest driving the real `F420App::ui` with real egui pointer/key events
(GPU-less, fallback camera, same projection code); **Live** = real binary in a private headless
compositor with real Wayland input (`tools/live/*.txt`, assertions on the app's own state);
**Headless** = shared command API (`f420-cli acceptance`); **Ext** = FreeCAD/OpenCascade + Python.

| # | item | result | evidence |
|---|---|---|---|
| A.1 | New → Create Sketch XY → centred rectangle → dimension 80×50 → Finish → Extrude 10 | PASS (UI, Live, Headless) | `crates/f420-app/tests/sketch_workflow.rs::scenario_a_*`; `tools/live/a_plate_with_hole.txt`; acceptance A.1 |
| A.2 | select top face → Create Sketch → centred Ø10 → Finish → Cut Through All | PASS (UI, Live, Headless) | same; face pick resolves `F2 ExtrudeEndCap` |
| A.3 | width → 100 through the visible dimension; hole stays centred; thickness 12, sketch follows | PASS (UI, Live, Headless) | volume 49214.6018 / 59057.5222; hole frame origin (0,0,12) |
| A.4 | undo/redo, save/close/reopen, STL watertight, STEP valid | PASS (UI, Live, Headless, Ext) | `artifacts/run2/external_validation.log` (scenario_a STEP err 6.1e-7) |
| A.5 | volume = w·h·t − π·5²·t, bounds, topology | PASS | acceptance report; UI asserts |
| B.1 | line loop (0,0)(60,0)(60,30)(50,40)(0,40), extrude 5 → 2350 mm², 11750 mm³, closed | PASS (UI, Live, Headless, Ext) | STEP err 1.8e-12 |
| B.2 | edit a vertex / a driving dimension → correct regeneration | PASS (UI: drag → 11500, width 45 → 11625; Live same; Headless chamfer length) | `scenario_b_*`, `tools/live/b_line_loop.txt` |
| B.3 | reject self-intersecting / open loop, document unchanged | PASS (UI, Headless; Live: open loop kept & not extrudable) | |
| C.1 | drag unconstrained vertex/circle; Esc cancels; one undo per drag | PASS (unit, UI, Live) | `sketch_edit.rs` tests; `scenario_c_*`; `tools/live/c_failure_paths.txt` |
| C.2 | conflicting dimension, invalid numeric input, invalid profile: visible, actionable, last valid kept | PASS (unit, UI, Live) | live shots `c_failure_paths/shots/` |
| C.3 | tree ↔ view selection, highlight + inspector; camera gestures don't place geometry | PASS (UI, Live) | `selection_sync_and_contextual_commands`; live MMB orbit in Line tool |
| C.4 | quick successive edits, undo, open another document while busy; no late results | PASS (engine tests, UI threaded test) | `engine.rs` tests; `queued_edits_and_document_switch_while_busy` |
| C.5 | unsaved changes on close; failed save path; focus blocks shortcuts | PASS (UI; Live window-close via Alt+F4 → confirm) | |
| S1 | blind circular pocket, depth vs through-all, volume/closed/STEP | PASS (UI, Headless, Ext) | `blind_pocket_through_cut_dialog`; pocket STEP err 3.5e-7 |
| S2 | three-point arcs, minimum constraints, profile validation | PASS (unit, UI, Live, Headless, Ext) | `three_point_arc_*`; acceptance R.1–R.7; R STEP err ≤ 1.4e-6 |
| S3 | measured kernel hardening | **TBD** | |
| run 1 | GOAL.md acceptance, plate/bracket examples, revolve, placement | PASS | `cargo xtask check`, `ui_workflow.rs`, `placement.rs` |

Not run / not proven: a human clicking on a physical display (the session was locked);
non-Linux builds; Fusion reference comparison; very large/small parts (absolute tolerances).

## 4. What a user can do now; limitations; regressions

Can do: everything in §1, plus run-1 features (templates dialog menu, placement drag tools,
revolve, STL/STEP export, bracket example).

Limitations (honest list):
* One base body per document (extrude or revolve). Cuts/pockets are circular, parallel to the
  extrusion, from a cap sketch, and must lie inside the host profile (exact containment check
  for line/arc hosts).
* Only extrusion caps can host sketches; side faces and cut faces explain that they cannot.
* No construction geometry, no point-on-line / midpoint / parallel / perpendicular / angle
  constraints; arc radius changes only through its points or a Radius dimension.
* Sketch overlays are drawn on top of the body (no depth test) in the model view.
* STEP writes cylinders/arcs as exact rational B-splines (other tools show splines).
* Display tessellation after booleans is slow (~0.4 s); measurements of cut bodies arrive ~2 s
  after the geometry.
* Observed once, not reproduced (live or in a kittest pointer test): a toolbar click right after
  dismissing the unsaved-changes dialog did nothing; a second click worked
  (`crates/f420-app/tests/dialog_click.rs` guards it).
* The glyphs ▾ ◐ → ● are missing from egui's default fonts and were replaced with ASCII.

Known regressions vs run 1: none known. Behaviour changes: template dialogs moved into a
Templates menu, exports into an Export menu; Undo/Redo show the action in the tooltip (fixed
labels so the bar never shifts); commands queue while the worker is busy instead of being
disabled; the native format is now v2 (v1 files load unchanged; saves write v2, which run-1
binaries refuse).

## 5. Five-minute walkthrough (actual labels and bindings)

1. `cargo build --release -p f420-app && target/release/f420-app`. Empty state: click
   **New Sketch on XY** (or **New**, then **Create Sketch**).
2. Press **R** (Center Rect). Click the origin (the ring snaps), then click a corner. Press
   **D** (Dimension), click the bottom edge, type `80`, **Enter**; click the right edge, type
   `50`, **Enter**. The chip turns to `✔ Fully constrained (DOF 0)` and the lines turn white.
3. **Finish Sketch**. Click **Extrude…** (the sketch is preselected), type `10`, **Enter**.
4. Click the top face (it highlights; the inspector says `Extrude1 end cap (semantic F2
   ExtrudeEndCap)`), click **Create Sketch**. Press **C**, click the origin, click a rim point;
   **D**, click the circle, type `10`, **Enter**. **Finish Sketch**.
5. **Cut through all…**, **Enter** (or choose **Blind depth** for a pocket).
6. Click **Sketch1** in the browser: its dimensions appear in 3D. Click `80 mm`, type `100`,
   **Enter**. The status bar shows the geometry immediately, then the volume
   (49214.6018 mm³). Click **Extrude1**, set `distance (mm)` to 12, **Apply**.
7. **Undo** / **Redo** (Ctrl+Z / Ctrl+Shift+Z). **Ctrl+Shift+S** to save, **Close**, **Ctrl+O**.
8. Try failures: in a sketch, dimension the top edge of the rectangle again (rejected, culprit
   named); type `abc` into a dimension (stays open, error); drag a dimensioned corner (blocked,
   explained). Close the window with unsaved changes → **Save / Discard changes / Keep editing**.
9. Arcs: **Create Sketch**, **L** line from (−20,0) to (20,0), **Esc**; **A**, click the two ends,
   then a point above; **D** on the arc → radius; **Finish Sketch**, **Extrude…**.
10. Navigation: middle-drag orbit, right-drag pan, wheel zoom to cursor, **F** fit, **7/1/3/0**
    views, **F1** help. In sketch mode the left button always belongs to the tool.

## 6. Screenshots (capture method)

All "after" images are F12 captures from the real release binary in a private headless mutter
(virtual monitor 1280×800 unless noted), driven by real compositor input.

| image | shows |
|---|---|
| `artifacts/run2/before_empty.png`, `before_demo.png` | run-1 UI at the start (real desktop, before the lock) |
| `artifacts/run2/live_a/shots/shot_008.png` | active sketch with dimensions, inline editor |
| `artifacts/run2/live_a/shots/shot_011.png` | selected top face with semantic description |
| `artifacts/run2/live_a/shots/shot_014.png` | plate with hole, selected sketch's dimensions in 3D |
| `artifacts/run2/live_a/shots/shot_016.png` | pending regeneration indicator (last valid model shown) |
| `artifacts/run2/live_a/shots/shot_022.png` | unsaved-changes guard on window close |
| `artifacts/run2/live_a/shots/shot_032.png` | recoverable error: redundant dimension rejected with culprits |
| `artifacts/run2/live_auto/d_three_point_arc/shots/shot_001.png` | three-point arc with radius dimension |
| `artifacts/run2/live_big/shots/shot_003.png` | 1920×1080 at 1.2× UI zoom |

## 7. Timings (method: release build, this machine)

| measurement | value | method |
|---|---|---|
| sketch drag step (free vertex / resize / blocked + diagnosis) | 2 µs / 5 µs / 0.12 ms | `--example drag_latency` |
| UI CPU per frame (status bar EMA) | 0.2–0.5 ms | live sessions |
| plate+hole edit → new geometry | 575 ms (was 2395) | engine timing harness, 7 runs |
| plate+hole edit → measurements | 2394 ms | same |
| box / scenario-B edit round trip | 1.1 ms | same |
| boolean / measure / display tessellation of the plate | 170 / 1805 / 377 ms | `--example regen_profile` |
| TBD kernel stretch numbers | | |

## 8. Examples and migration

* Native documents: `examples/plate.f420.json`, `bracket.f420.json`, `scenario_a.f420.json`,
  `scenario_b.f420.json`, `scenario_r.f420.json` (v2). Frozen v1 fixture:
  `crates/f420-document/tests/fixtures/plate_v1.f420.json`.
* Replayable commands: `examples/*_workflow.json` (`f420-cli run …`, exit 0).
* Migration: format v2 adds sketch `origin`, `Distance`, arcs, `Radius`/`Tangent`/`EqualRadius`,
  pockets; it is additive, v1 loads unchanged (identity migration), saves write v2.
  `serde_json` `float_roundtrip` makes stored solved coordinates reload bit-exactly.

## 9. Tests and checks run

* `cargo xtask check`: **TBD final** (last run: 200 passed, 0 failed, 2 ignored timing harnesses;
  acceptance 440/440; 5 example replays) — `artifacts/run2/xtask_arcs.log`.
* External: `tools/external_validation.sh` — 8 models, all PASS (Python STL, FreeCAD STL, FreeCAD
  STEP: 1 valid solid each, volume error ≤ 1.4e-6 mm³) — `artifacts/run2/external_validation.log`.
* Live real-input walkthroughs: 4/4 PASS — `artifacts/run2/live_auto_all.log`,
  `artifacts/run2/live_auto/*/{script.txt,result.log,app.log,shots}`.
* Separation of evidence: kittest = widget/input logic with real egui events (no compositor);
  live = real window + compositor input routing (no human); scripted `--screenshot` = appearance.

## 10. Next three improvements (by practical value) and open decisions

1. **TBD**
2. **TBD**
3. **TBD**

Decisions for you: **TBD**
