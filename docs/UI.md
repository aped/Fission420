# UI notes

The UI shows the last `DocSummary` from the engine and changes the document only through
`Command`s. UI areas, the module that implements each one, and the test that drives it
through the real widgets:

| Area | Behaviour | Code | UI test |
|---|---|---|---|
| Multi-body view | Every visible body is tessellated into one display scene. Each body gets its own group range and tint, and mesh bodies get their own colour. Hidden bodies are not drawn, picked or fitted. | `engine.rs` (`display_scene`), `rcad-viewport` `set_group_tints` | `multibody.rs` |
| Face picking | A planar face matches a document face candidate on the same body that is coplanar and has the same outward normal (caps, line side faces, cut floors, imported planar faces), and it can host a sketch. Curved faces, mesh bodies and planar faces with no unique reference show the reason. | `view3d.rs` (`analyse_faces`) | `sketch_workflow.rs` `selection_sync…`, `reattach.rs` |
| Bodies browser | You can select a body (this highlights all its faces), rename it inline, and hide or show it (each is an undoable command). The inspector shows the body's kind, where it came from, and either its measurements or mesh diagnostics. Right-click → **Isolate** draws only that body (view state, not saved, no undo step; "Show all" above the list ends it). | `bodies.rs` | `multibody.rs`, `isolate_ui.rs` |
| Extrude | Operations are New Body, Join and Cut, with a target body. Through All is available for Cut only and goes through both sides of the sketch plane. Reverse and units are also available. A translucent preview is computed in the UI, and the target is highlighted. A Cut from a face of the target goes into the material. | `extrude.rs` | `multibody.rs`, `reattach.rs` |
| Pattern / mirror | For an Extrude Join or Cut: a linear pattern (count, spacing, sketch X or Y), a circular pattern (count, angle between instances, centre in sketch coordinates) or a mirror across a world plane. | `panels.rs`, `app.rs` | `multibody.rs` |
| Angle dimension | In the sketch editor, select two lines and press Angle. The angle is measured counter-clockwise and oriented to be positive. The value is edited inline in degrees. Parallel lines are refused. | `sketch_edit.rs`, `sketch_view.rs`, `panels.rs` | `sketch_edit.rs` unit test |
| Trim (sketch, T) | Click the part of a curve to remove: it is cut at the nearest intersections with other lines, circles or arcs on either side of the click (a curve with none is deleted). A line's middle part splits it in two (Horizontal/Vertical copied); a circle needs two crossings and becomes an arc (its diameter dimension becomes a radius dimension); an arc's middle part splits it into two arcs with Equal radius. Existing points never move; equal-length/tangent constraints that no longer apply and constraints of detached unused endpoints are removed (reported); a Tangent stays with the piece that keeps its tangent point, and construction geometry stays construction. Each new endpoint is joined to the curve it was cut at: an existing point there is reused (shared vertex, so trimmed outlines close into profiles), otherwise a point on a line gets a Point-on-line constraint; a point on a circle/arc is left free (no point-on-arc constraint yet; reported). |
| Extend (sketch, E) | Click a line near a free end (or the end point itself): the end moves along the line to the nearest line, circle or arc its extension crosses. Connected ends are refused. The old end point is replaced (its constraints removed, reported); the new end is joined to the curve it reached as for Trim. Lines only. | `sketch_edit.rs` | `sketch_edit.rs` `extend_free_ends_to_the_nearest_curve` | `sketch_edit.rs` | `sketch_edit.rs` `trim_overhangs_split_and_delete` |
| Chamfer | Select a planar face, choose **Chamfer edge…**, choose the second face of the edge (planar faces of the same body that are not parallel to the first) and a distance. | `panels.rs`, `app.rs` | `multibody.rs` `chamfer_edge_from_a_picked_face` |
| Import | STEP, STL, 3MF, DXF and SVG import runs on the worker. The units choice appears only when needed. For DXF and SVG you also choose a plane (world plane or the selected face) and whether to allow a partial import. On success the view fits to the new bodies and an import summary opens. A failure leaves the document unchanged. | `exchange.rs` | `exchange_ui.rs` |
| Export | STL, STEP and 3MF export either the selected bodies or all visible bodies (N). Bodies are written separately and never fused. Hidden bodies are omitted unless selected. STEP refuses an explicit mesh selection; with all visible bodies it skips mesh bodies and says so. A sketch can be exported to DXF (R12) or SVG in local 2D coordinates in mm. | `exchange.rs` | `exchange_ui.rs` |
| Re-attach | After a `BrokenReference` refusal, choose a world plane or an earlier face for the sketch. A dashed outline previews the result, and one `SetSketchPlane` applies it as a single undo step. | `reattach.rs` | `reattach.rs` |
| File dialogs | Each path field has a **Browse…** button, or a compact **…** in the narrow panels, beside its text box. It opens the platform file dialog: on Linux this is the XDG desktop portal, so it works on Wayland and inside Flatpak. The dialog opens in the folder of the path already in the box, and Save dialogs suggest that file name. The chosen path replaces the text, and typing a path still works. The fields are Open, Save As, Import, STEP, STL, 3MF, sketch SVG/DXF, flat-pattern DXF, drawing export, BOM CSV, External document / STEP assembly, and the variant export folder. The dialog runs off the UI thread and only one can be open at a time. | `file_pick.rs`, `panels.rs` | `file_picker_ui.rs` (scripted dialog backend) |
| Frame pacing | Each worker reply calls the repaint callback. An idle UI requests no repaint. While work is pending the UI polls every 50 ms. | `engine.rs`, `app.rs` | `frame_pacing.rs` |

Screenshots: `tools/screenshots.sh [binary] [out-dir] [scenes...]` renders the `--scene`
states (`crates/rcad/src/scenes.rs`) in a private headless mutter at 1280×800 and
1920×1080, into `artifacts/screenshots/` (not tracked). Curated copies are in
`docs/screenshots/`.

Cancelling the Import dialog while an import is running only closes the dialog. The import cannot
be interrupted and still completes, and Undo removes it in one step.

## Parameters, measure, printable templates, export and recovery

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
  Each copy has a side file naming the saved document it came from; if that document
  was saved after the copy (e.g. by another instance), the offer says the copy holds
  older work and offers **Open saved document**. Opening a copy never writes to the
  saved file (it opens without a path).
* Dialogs holding feature/body ids close on Undo/Redo (ids may be reused afterwards) and
  when their ids disappear; import replies resolve only the form that submitted them.

## Modifier faces, section view, mirror, suppression and dialogs

* **Sketch on modifier faces:** faces made by linear/circular patterns, mirrors and chamfers
  are semantic references (`FaceRole::Copy { instance, tool, of }`, `FaceRole::Chamfer`) and can
  host sketches, holes, mirrors and chamfers like extrusion faces. When several faces share a
  plane (pattern floors), the picked face is matched by plane plus a point inside that face's own
  tool footprint; ambiguous faces show the reason instead of guessing. A sketch on occurrence *k*
  follows that occurrence when the count changes and is refused (BrokenReference, model
  unchanged) when occurrence *k* disappears.
* **View › Section view…** (display only): a clipping plane parallel to world YZ/XZ/XY or the
  selected planar face, offset field and slider, Flip. Material beyond the plane is neither drawn
  nor pickable; exposed inside surfaces are tinted. Not capped; the model, measurements (exact
  B-rep planes) and exports never change. Probe key `section`.
* **Mirror dialog:** *Mirror face* combo (any referenceable planar face) and *Mirror plane offset*
  (a construction plane; `plane_offset` bindable). **Hole dialog:** *Start plane offset*
  (a construction plane that must hold a face of the target).
* **Suppression:** inspector *Suppress* / *Suppress with dependents* / *Restore* / *Restore with
  dependents*; History strikes suppressed features through. Plain *Suppress* is disabled for a
  feature others use (the reason is in the tooltip).
* **Sketch editor:** the Fillet tool also rounds a line–arc corner; dimensioning the same two
  points again gives the other axis (horizontal ↔ vertical).
* **Parameters panel:** the open edit's Apply / Cancel / Delete sit on their own row (they were
  clipped off a narrow browser by long expressions); a rename is applied before the value change
  and a refused rename sends nothing else; long messages wrap (they widened the browser).
* **Extrude dialog:** a Join/Cut sketched on a plane holding target faces facing both ways shows
  no preview and says OK will be refused.
* **Templates › Organizer tray** (after the blank-document workflow, which is the acceptance path).
* Live walkthrough of the tray from New: `tools/live/t_organizer_tray.txt`.

## Sketch tools

* **Text tool (sketch toolbar "Text"):** click to place a text (on an existing point the text is
  anchored to it and follows it), or click an existing text to edit it (double-click in Select
  does the same). The dialog sets *Text string*, *Font family* (typed, or picked from the
  installed families), Bold/Italic, *Height (mm, cap height)*, *Rotation (°)* and the
  horizontal/vertical alignment; *Apply text* adds/changes it as one local undo step, *Delete
  text* removes it, *Cancel text* closes the dialog. A refusal (font not installed, character
  without a glyph, outline touching other geometry) keeps the dialog open with the reason and
  the sketch unchanged. For stored text whose font is missing the dialog says so (placement edits
  still work). Glyph outlines are drawn as thin closed curves and become profile regions
  (counters are holes; docs/behavior/sketching.md "Sketch text"). Code: `sketch_edit.rs`
  (`text_*`), `text_ui.rs`, `sketch_view.rs`; UI test `crates/rcad/tests/sketch_text_ui.rs`.
* **Slot / Polygon tools:** Slot = three clicks (centre, centre, width point); Polygon = centre +
  vertex, with *Sides* and *Across flats* shown in the toolbar while the tool is active. Dashed
  previews follow the cursor. Both create constrained, dimensioned ordinary geometry
  (docs/behavior/sketching.md "Slot and regular polygon"). UI test
  `crates/rcad/tests/sketch_shapes_ui.rs`.
* **Pattern… (sketch toolbar):** enabled when curves are selected; dialog *Rectangular* /
  *Circular*, *Instances*, *Step x/y (mm)* (+ *Second direction*), or *Angle between (°)* and
  *Centre x/y*; *Apply pattern* / *Cancel pattern*. Copies are constrained to their sources and
  the spacing/angle become dimensions. UI test `crates/rcad/tests/sketch_pattern_ui.rs`.

## Edit Feature, messages and first run

* **Edit Feature** (History double-click or right-click ▸ Edit Feature) reopens the feature's
  dialog prefilled for Extrude, Revolve, Hole, Fillet / Chamfer (edge and loop), Shell (face and
  scoped), Linear / Circular pattern, Mirror, Loft, Sweep, Thread and Align. A bound value shows its
  expression; OK is one `RedefineFeature` (same id and history position, one undo step), and values
  the dialog does not show (created bodies, a nut's clearance, a mirror plane's tilt, an align
  offset) are kept. The pattern type is fixed while editing. A definition a dialog cannot show (a
  pattern along a slanted direction, a loft with several middle sections) and the remaining kinds
  say to use the Inspector. Code `dialogs/edit_feature.rs`; tests `edit_feature_ui.rs` and the
  module's round-trip unit tests (an unchanged dialog gives back the stored definition).
* **Refusal notice:** a key press or menu action that cannot run (E with no sketch) shows why above
  the bottom of the view, with × to dismiss; the next command or 8 s clear it. The Messages log
  keeps every message.
* **Error banner:** leads with what did not happen in plain words ("Shell face was not applied: not
  supported yet"), then the reason and next step; the log keeps the technical text and error kind
  (`messages.rs`).
* **Save state:** the window title is `Name — R-CAD` with `● ` while there are unsaved changes (an
  open sketch with edits counts); the status bar shows the name (Untitled until saved) and one
  state: not saved yet, unsaved changes, or saved.
* **Shortcuts:** menu items show the active key layer's shortcut at the right (also exposed as the
  accessible keyboard shortcut); hovering a disabled ribbon button shows its key and why it is
  disabled. F1 lists every command and key.
* **Start card** (empty view): New Sketch on XY, Open Document…, Start from a template (the
  printable-part templates), how to sketch on another plane or a face, and the navigation keys.
* **File ▸ About R-CAD:** package version, license and source repository.
* **Glyphs:** `tests/glyphs.rs` fails on any character in the app's string literals that the UI
  font cannot draw (it would show as an empty box).
UI tests: `crates/rcad/tests/first_run_ui.rs`, `edit_feature_ui.rs`, `glyphs.rs`.
