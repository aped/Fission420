# UI notes (run 3)

The UI shows the last `DocSummary` from the engine and changes the document only through
`Command`s. Run-3 additions, the module that implements each one, and the test that drives it
through the real widgets:

| Area | Behaviour | Code | UI test |
|---|---|---|---|
| Multi-body view | Every visible body is tessellated into one display scene. Each body gets its own group range and tint, and mesh bodies get their own colour. Hidden bodies are not drawn, picked or fitted. | `engine.rs` (`display_scene`), `f420-viewport` `set_group_tints` | `multibody.rs` |
| Face picking | A planar face matches a document face candidate on the same body that is coplanar and has the same outward normal (caps, line side faces, cut floors, imported planar faces), and it can host a sketch. Curved faces, mesh bodies and planar faces with no unique reference show the reason. | `view3d.rs` (`analyse_faces`) | `sketch_workflow.rs` `selection_sync…`, `reattach.rs` |
| Bodies browser | You can select a body (this highlights all its faces), rename it inline, and hide or show it (each is an undoable command). The inspector shows the body's kind, where it came from, and either its measurements or mesh diagnostics. | `bodies.rs` | `multibody.rs` |
| Extrude | Operations are New Body, Join and Cut, with a target body. Through All is available for Cut only and goes through both sides of the sketch plane. Reverse and units are also available. A translucent preview is computed in the UI, and the target is highlighted. A Cut from a face of the target goes into the material. | `extrude.rs` | `multibody.rs`, `reattach.rs` |
| Pattern | A linear pattern of an Extrude Cut (count, spacing, sketch X or Y). | `panels.rs`, `app.rs` | `multibody.rs` |
| Import | STEP, STL, 3MF, DXF and SVG import runs on the worker. The units choice appears only when needed. For DXF and SVG you also choose a plane (world plane or the selected face) and whether to allow a partial import. On success the view fits to the new bodies and an import summary opens. A failure leaves the document unchanged. | `exchange.rs` | `exchange_ui.rs` |
| Export | STL, STEP and 3MF export either the selected bodies or all visible bodies (N). Bodies are written separately and never fused. Hidden bodies are omitted unless selected. STEP refuses an explicit mesh selection; with all visible bodies it skips mesh bodies and says so. A sketch can be exported to DXF (R12) or SVG in local 2D coordinates in mm. | `exchange.rs` | `exchange_ui.rs` |
| Re-attach | After a `BrokenReference` refusal, choose a world plane or an earlier face for the sketch. A dashed outline previews the result, and one `SetSketchPlane` applies it as a single undo step. | `reattach.rs` | `reattach.rs` |
| Frame pacing | Each worker reply calls the repaint callback. An idle UI requests no repaint. While work is pending the UI polls every 50 ms. | `engine.rs`, `app.rs` | `frame_pacing.rs` |

Screenshots: `tools/run3_screenshots.sh` renders the `--scene` states
(`crates/f420-app/src/scenes.rs`) in a private headless mutter at 1280×800 and 1920×1080.
Curated copies are in `docs/screenshots/12-17`. The full set is written to
`artifacts/run3/screenshots/` (not tracked).

Cancelling the Import dialog while an import is running only closes the dialog. The import cannot
be interrupted and still completes, and Undo removes it in one step.
