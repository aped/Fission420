# UI notes (run 3)

The UI shows the last `DocSummary` from the engine and changes the document only through
`Command`s. Run-3 additions, the module that implements each one, and the test that drives it
through the real widgets:

| Area | Behaviour | Code | UI test |
|---|---|---|---|
| Multi-body view | Every visible body is tessellated into one display scene. Each body gets its own group range and tint, and mesh bodies get their own colour. Hidden bodies are not drawn, picked or fitted. | `engine.rs` (`display_scene`), `rcad-viewport` `set_group_tints` | `multibody.rs` |
| Face picking | A planar face matches a document face candidate on the same body that is coplanar and has the same outward normal (caps, line side faces, cut floors, imported planar faces), and it can host a sketch. Curved faces, mesh bodies and planar faces with no unique reference show the reason. | `view3d.rs` (`analyse_faces`) | `sketch_workflow.rs` `selection_sync…`, `reattach.rs` |
| Bodies browser | You can select a body (this highlights all its faces), rename it inline, and hide or show it (each is an undoable command). The inspector shows the body's kind, where it came from, and either its measurements or mesh diagnostics. | `bodies.rs` | `multibody.rs` |
| Extrude | Operations are New Body, Join and Cut, with a target body. Through All is available for Cut only and goes through both sides of the sketch plane. Reverse and units are also available. A translucent preview is computed in the UI, and the target is highlighted. A Cut from a face of the target goes into the material. | `extrude.rs` | `multibody.rs`, `reattach.rs` |
| Pattern / mirror | For an Extrude Join or Cut: a linear pattern (count, spacing, sketch X or Y), a circular pattern (count, angle between instances, centre in sketch coordinates) or a mirror across a world plane. | `panels.rs`, `app.rs` | `multibody.rs` |
| Angle dimension | In the sketch editor, select two lines and press Angle. The angle is measured counter-clockwise and oriented to be positive. The value is edited inline in degrees. Parallel lines are refused. | `sketch_edit.rs`, `sketch_view.rs`, `panels.rs` | `sketch_edit.rs` unit test |
| Trim (sketch, T) | Click the part of a curve to remove: it is cut at the nearest intersections with other lines, circles or arcs on either side of the click (a curve with none is deleted). A line's middle part splits it in two (Horizontal/Vertical copied); a circle needs two crossings and becomes an arc (its diameter dimension becomes a radius dimension); an arc's middle part splits it into two arcs with Equal radius. Existing points never move; equal-length/tangent constraints that no longer apply and constraints of detached unused endpoints are removed (reported); a Tangent stays with the piece that keeps its tangent point, and construction geometry stays construction. Each new endpoint is joined to the curve it was cut at: an existing point there is reused (shared vertex, so trimmed outlines close into profiles), otherwise a point on a line gets a Point-on-line constraint; a point on a circle/arc is left free (no point-on-arc constraint yet; reported). |
| Extend (sketch, E) | Click a line near a free end (or the end point itself): the end moves along the line to the nearest line, circle or arc its extension crosses. Connected ends are refused. The old end point is replaced (its constraints removed, reported); the new end is joined to the curve it reached as for Trim. Lines only. | `sketch_edit.rs` | `sketch_edit.rs` `extend_free_ends_to_the_nearest_curve` | `sketch_edit.rs` | `sketch_edit.rs` `trim_overhangs_split_and_delete` |
| Chamfer | Select a planar face, choose **Chamfer edge…**, choose the second face of the edge (planar faces of the same body that are not parallel to the first) and a distance. | `panels.rs`, `app.rs` | `multibody.rs` `chamfer_edge_from_a_picked_face` |
| Import | STEP, STL, 3MF, DXF and SVG import runs on the worker. The units choice appears only when needed. For DXF and SVG you also choose a plane (world plane or the selected face) and whether to allow a partial import. On success the view fits to the new bodies and an import summary opens. A failure leaves the document unchanged. | `exchange.rs` | `exchange_ui.rs` |
| Export | STL, STEP and 3MF export either the selected bodies or all visible bodies (N). Bodies are written separately and never fused. Hidden bodies are omitted unless selected. STEP refuses an explicit mesh selection; with all visible bodies it skips mesh bodies and says so. A sketch can be exported to DXF (R12) or SVG in local 2D coordinates in mm. | `exchange.rs` | `exchange_ui.rs` |
| Re-attach | After a `BrokenReference` refusal, choose a world plane or an earlier face for the sketch. A dashed outline previews the result, and one `SetSketchPlane` applies it as a single undo step. | `reattach.rs` | `reattach.rs` |
| Frame pacing | Each worker reply calls the repaint callback. An idle UI requests no repaint. While work is pending the UI polls every 50 ms. | `engine.rs`, `app.rs` | `frame_pacing.rs` |

Screenshots: `tools/screenshots.sh [binary] [out-dir] [scenes...]` renders the `--scene`
states (`crates/rcad/src/scenes.rs`) in a private headless mutter at 1280×800 and
1920×1080, into `artifacts/screenshots/` (not tracked). Curated copies are in
`docs/screenshots/`.

Cancelling the Import dialog while an import is running only closes the dialog. The import cannot
be interrupted and still completes, and Undo removes it in one step.

## Run 4 additions

* **Browser → Parameters (N)**: named parameters with expression, value and unit; click a
  name to edit, rename or delete (disabled with the reason while used); bottom row adds one.
  Refusals are shown under the list with the model unchanged. docs/PARAMETERS.md.
* **Inspector parameter fields** take a number or an expression; a bound field shows its
  expression with `= value` beside the label; a plain number unbinds it.
* **Measure**: select a planar face → *Measure from this face* → select another planar
  face: exact distance between parallel faces (wall, floor, cavity, lip clearance) or the
  angle between their normals, from the B-rep planes (not the display mesh). The body
  inspector shows *Overall size (bounding box)*, labelled as such (it is not a wall
  thickness), and a *tessellation estimate* marker when the volume is an estimate.
* **Extrude preview**: resolves the direction from every planar face of the target (also
  faces made by patterns, mirrors, chamfers, or split faces) like regeneration; Through
  All uses exact body bounds (hidden targets too). The label "Preview (display only; the
  exact solid is built and checked on OK)" sits at the bottom of the view, clear of the
  dialog.
* **Templates → Printable parts**: *Parametric enclosure + lid*, *Clearance test coupon*,
  *Mounting bracket* start a new document with editable parameters.
* **Export STL… / Export 3MF…**: body table, tessellation, export-only placement (selected
  face down), layout, bed fit, per-body report (docs/PRINT_EXPORT.md).
* **Autosave / recovery** (`crates/rcad/src/recovery.rs`): while a document has
  unsaved changes, a copy is written every 60 s (`RCAD_AUTOSAVE_SECS`) to
  `~/.local/state/r-cad/recovery` (`RCAD_RECOVERY_DIR`); Save or deliberately leaving
  the document removes it; after a crash the next start offers *Open copy* (opens as an
  unsaved, untitled document) or *Discard*. Scripted scenes, demos and tests never write
  recovery files. Copies left by a pre-rename build (`…/fission420/recovery`,
  `*.autosave.f420.json`) are still offered, and `F420_RECOVERY_DIR` /
  `F420_AUTOSAVE_SECS` still apply when the `RCAD_*` variables are unset.
* Dialogs holding feature/body ids close on Undo/Redo (ids may be reused afterwards) and
  when their ids disappear; import replies resolve only the form that submitted them.
