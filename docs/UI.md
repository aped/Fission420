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
| Trim (sketch, T) | Click the part of a curve to remove: it is cut at the nearest intersections with other lines, circles or arcs on either side of the click (a curve with none is deleted). A line's middle part splits it in two (Horizontal/Vertical copied); a circle needs two crossings and becomes an arc (its diameter dimension becomes a radius dimension); an arc's middle part splits it into two arcs with Equal radius. Existing points never move; equal-length/tangent constraints that no longer apply and constraints of detached unused endpoints are removed (reported); a Tangent stays with the piece that keeps its tangent point, and construction geometry stays construction. Each new endpoint is joined to the curve it was cut at: an existing point there is reused (shared vertex, so trimmed outlines close into profiles), otherwise a point on a line gets a Point-on-line constraint; a point on a circle/arc is left free (no point-on-arc constraint yet; reported). | `sketch_edit.rs` | `sketch_edit.rs` `trim_overhangs_split_and_delete` |
| Extend (sketch, E) | Click a line near a free end (or the end point itself): the end moves along the line to the nearest line, circle or arc its extension crosses. Connected ends are refused. The old end point is replaced (its constraints removed, reported); the new end is joined to the curve it reached as for Trim. Lines only. | `sketch_edit.rs` | `sketch_edit.rs` `extend_free_ends_to_the_nearest_curve` |
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

The Import dialog's Cancel stops its own import, queued or running, and nothing is committed;
the toolbar Cancel stops whatever is running (docs/FORMAT_SUPPORT.md, "Qualification scenario:
external exchange"; tests `crates/rcad/tests/import_cancel_ui.rs`, `stale_state.rs`).

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
  recovery files.
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
  (`text_*`), `text_ui.rs`, `sketch_view.rs`; UI test `crates/rcad/tests/it/sketch_text_ui.rs`.
* **Slot / Polygon tools:** Slot = three clicks (centre, centre, width point); Polygon = centre +
  vertex, with *Sides* and *Across flats* shown in the toolbar while the tool is active. Dashed
  previews follow the cursor. Both create constrained, dimensioned ordinary geometry
  (docs/behavior/sketching.md "Slot and regular polygon"). UI test
  `crates/rcad/tests/it/sketch_shapes_ui.rs`.
* **Pattern… (sketch toolbar):** enabled when curves are selected; dialog *Rectangular* /
  *Circular*, *Instances*, *Step x/y (mm)* (+ *Second direction*), or *Angle between (°)* and
  *Centre X/Y*; *Apply pattern* / *Cancel pattern*. Copies are constrained to their sources and
  the spacing/angle become dimensions. UI test `crates/rcad/tests/it/sketch_pattern_ui.rs`.

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
* **File ▸ Quit (Ctrl+Q):** the same unsaved-changes guard as closing the window.
* **Esc** closes an open About, Help or Preferences window first; in model mode, after a
  placement, dialog or dimension edit, it clears a refusal notice before the selection.
* **Plain names:** faces and sketch planes are named "Body1: Extrude1 end cap" / "on Extrude1 end
  cap" (the reference is on hover in the inspector); the status bar volume line omits an error
  bound that shows nothing at its precision and counts solids in words.
* **Units:** dialog value fields say `(mm)` or `(deg)` (test `tests/it/value_labels.rs`).
* **Tooltips:** workspace tabs and the Loft/Sweep, Planes and Templates dropdowns explain
  themselves.
* **Column widths:** a browser or inspector width the user drags is saved in the preferences and
  restored at the next start.
* **Scenes** open repository files by paths relative to the working directory when it is the
  repository, so screenshots show `examples/…`.
* **Glyphs:** `tests/it/glyphs.rs` fails on any character in the app's string literals that the UI
  font cannot draw (it would show as an empty box).
UI tests: `crates/rcad/tests/it/first_run_ui.rs`, `edit_feature_ui.rs`, `glyphs.rs`, `polish_ui.rs`, `value_labels.rs`.

## Cloud (optional server tier)

R-CAD is complete without a server. The `cloud` feature of the `rcad` crate (on by default;
`cargo build -p rcad --no-default-features` leaves it out entirely) adds a client for an R-CAD
sync server: organizations, projects, document versions, edit locks, share links and comments.
The workflow follows Fusion's Data Panel where that fits a local-first app.

* **Nothing without an account.** At startup the app reads only the account file
  (`<config>/r-cad/cloud.json`). With no account it starts no service thread, opens no socket,
  writes no cache and shows no prompt. Scripted runs (`--scene`, `--demo`) never read it.
* **Data panel** (application bar **Data**, left of File): a column left of the browser.
  Signed out, it explains what a server adds and offers **Connect to server…** (server URL,
  username, password; the password field is masked, Enter connects, Esc cancels, errors show in
  the dialog and the message log). Once the typed server URL has been unchanged for about
  0.6 s, the dialog reads the server's public information (`GET /info` only, no credentials;
  this starts the cloud service thread) to learn its sign-in methods. A server with single
  sign-on gets a **Sign in with <provider>** button; a server without password sign-in shows
  only that button, without username and password fields. If the information cannot be read,
  a quiet note says so and the password form stays. The single sign-on button opens the
  provider's page in the system browser (`xdg-open`, `open` or the Windows URL handler) and
  the dialog shows **Waiting for the browser…** with the address (selectable, for when no
  browser opens), **Open the page again** and **Cancel**, which stops the wait. A refusal,
  a timeout (five minutes) or a mismatched answer shows in the dialog; success closes it as
  a password sign-in does. The unencrypted-connection consent applies to both methods.
  Signed in: server name, user, **Refresh**, **Sign out**, the
  organization selector (with **Organization…** for organization administrators), then four
  sections. **Browse** lists projects (**New project…**), and a selected project's folders and
  documents with their latest version, who saved it and a lock mark (**New folder…**,
  **Upload current document…**, **Open** or double-click). Clicking a folder selects it (click
  again to clear): new folders and uploads go into the selected folder, and **Rename folder…**
  and **Delete folder…** (empty folders only) act on it. A selected document offers **Rename…**,
  **Move…** (folder combo) and **Move to trash…** (with confirmation); **Trash (N)** switches to
  the project's trash with **Restore** per document. Controls an editor needs are disabled for
  viewers and commenters. **History** and **Comments** belong to the open cloud document.
  **Activity** lists the selected project's recent activity, newest first (what happened, who,
  when; at most 100 entries); an entry about a document opens it when clicked. It loads when
  the section is shown and refreshes every 60 s while it stays shown.
* **Members** (**Members…** next to the project name, for everyone who can see the project):
  the explicit members and their roles. Project administrators get a role combo and **Remove**
  per member and **Add member** (username and role, default Editor). The server's refusals show
  in the window, with the usual cause spelled out (a user outside the organization: "the user
  must be a member of the organization first").
* **Organization…** (organization Admins and Owners): storage used against the quota (a bar,
  or "no quota"), document and version counts, and the members with role combos, **Remove** and
  **Add to organization**. Owners assign every role; Admins manage Guests and Members. Its
  **Recent activity** list shows the whole organization's activity (clicking a document entry
  opens that document and closes the window); **Reload** refreshes it with the members.
* **Offline queue:** while versions wait to be sent the panel shows the count with **Sync now**
  and a **Queued versions** list (document, description, when it was queued). **Discard** asks
  for confirmation and then drops that version from the queue; the local file is not changed.
  A refusal that may pass later (a full storage quota) keeps the queue blocked until the version
  is discarded or the cause is fixed.
* **Working copies.** Opening a cloud document downloads it into the cache and opens that file
  through the normal Open path (with the unsaved-changes guard). A cloud document is an
  ordinary local file: Save, autosave and recovery work as for any document, offline too.
* **Status chip** (right side of the application bar, only for a cloud document):
  `☁ Bracket v3 · Synced`, `Modified` (local edits since v3), `Offline · 2 pending`,
  `Conflict`, `Locked by sam` or `Read-only`. Clicking it opens the version history (and retries
  queued versions when offline).
* **Newer version on the server** (Fusion's "out of date" indicator): while signed in with a
  cloud document open, R-CAD asks the server for the document's latest version every 60 s, at
  once when the Data panel opens or the window regains focus (never without an account; after
  the server was unreachable it waits a full interval). When someone saved a newer version the
  chip shows `☁ Bracket v3 · v4 available` with an **Update** button if the working copy has no
  local edits (Update downloads v4 into the working copy and reopens it), or
  `Newer v4 on server` if it has (its tooltip explains that Save Version will report a
  conflict). With the Data panel open the project list is re-read at the same pace, and a
  project whose revision changed has its tree reloaded.
* **Thumbnails:** saving a version or uploading renders the model offscreen and stores a
  256 px PNG with the version. The render is a clean model view, like a product shot: all
  visible bodies fitted in the isometric direction on a light background, with feature
  edges, and without sketches, dimensions, captions, selection or hover colours, a section
  plane, the grid or the axis triad. It is drawn into a separate target (rendered at 512 px
  and scaled down), so the window's view and camera do not change and nothing flickers.
  Without a GPU view (tests, headless runs) or without a body, the version is saved without
  a thumbnail. Document rows in the Data
  panel show the latest version's thumbnail small, and the version history shows each
  version's thumbnail larger. Thumbnails are fetched once per version, when first shown, and
  kept for the session; one that does not decode as a PNG is simply not shown.
* **File menu:** **Save to Cloud Project…** for an open document that is not on the server
  (Fusion's first Save into a project): pick a project you can edit, a folder and a name; a
  document never saved locally goes through Save As first, and unsaved changes are saved
  locally before the upload. The file becomes the cloud document's working copy.
  **Save Version to Cloud…** (description and optional milestone; unsaved changes
  are saved locally first), **Version History**, **Share Link…** (expiry in days, empty = never;
  allow download; latest or a fixed version; the URL is shown once with **Copy**; below, the
  document's existing links with version, expiry, download and creator, each with **Revoke…**,
  which asks before the link stops working), **Comments**
  and **Sync Now** (versions saved while the server was unreachable). They are registry commands
  (command search finds them) and are disabled with a reason until they apply.
* **Version history:** newest first, with author, UTC date and age, description and a star for
  milestones. **Open vN** opens a read-only copy (Save Version is disabled for it), **Promote vN**
  asks before making that version the latest, and milestones can be set or cleared.
* **Conflicts:** when someone saved a newer version since the one the edit started from, a window
  says so and offers to save as the next version anyway, to save as a new document
  (`<name> (copy)`) or to cancel. The local file is never touched.
* **Edit lock** (Fusion's reservation, simplified): with Preferences ▸ Cloud ▸ "Lock cloud
  documents while I edit them" (default on), the first edit of a cloud document asks for its lock;
  saving a version, opening another document or quitting releases it. A lock held by someone else
  is only shown (chip and message log): local editing and local saving always work, and Save
  Version fails until the lock is released.
* **Upload a version on every save** (Preferences ▸ Cloud, default off; Fusion saves every
  save as a version): when on, a successful local save of a cloud document's working copy also
  saves it as a new version described "Saved from R-CAD", with the model thumbnail. It goes
  through the same path as Save Version: a newer version on the server opens the conflict
  window, and offline the version is queued. Read-only copies of older versions and documents
  that are not on the server are never uploaded, and the local save that Save Version or an
  upload makes first does not add a second version.
* **Messages:** every service error is logged as `Cloud: <message>` (errors from background work
  such as refreshing or locking do not raise the error banner). Tokens and passwords are never
  logged.
* **Layout:** the Data panel narrows the 3D view rather than covering it; at 1280 px wide with
  the panel open the view keeps at least 380 px and the application bar stays one row. The
  navigation bar under the view (Orbit, Pan, Zoom, Fit view, Look At, display style) shows
  labels next to the timeline when they fit, one glyph per button (same tooltips and
  accessible names) when the view is narrow, and moves to its own wrapping row when even that
  does not fit; its buttons never overlap or leave the view's width.

Code `crates/rcad/src/cloud_ui.rs` (the calls that construct `rcad-cloud` objects are in its
`adapter` module); screenshot scenes `cloud_browse` and `cloud_history` (a sample server's
events, no network); UI tests `crates/rcad/tests/it/cloud_ui.rs` drive the widgets against a
detached service, asserting the requests sent and the state after injected events. The live
script `tools/live/cloud/u_cloud_roundtrip.txt` runs against a real server and is not part of the
default walkthrough, and neither is `tools/live/cloud/v_cloud_members.txt` (members and the
organization window; needs users alice, owner of organization Home, and bob, a member, and
project Parts). The live-test probe reports the cloud state under `doc.cloud` (`signed_in`,
`linked`, `version`, `state`, `pending`, `versions_listed`, `share_url`, `dialog_kind`,
`selected_document`, `selected_folder`, `documents_listed`, `folders_listed`, `trash_listed`,
`members_listed`, `member_roles` (username → role), `org_members_listed`, `usage_loaded`,
`usage_bytes`, `admin_error`, `admin_error_code`, `queue_listed`, `server_head`,
`thumbnails_loaded`, `shares_listed`, `activity_listed`, `org_activity_listed`, …; `state` adds `update_available` and `newer_on_server`). Thumbnail
cropping and decoding are in `crates/rcad/src/cloud_ui/thumb.rs`. The administration
windows are in `crates/rcad/src/cloud_ui/admin.rs`, the activity and share link lists in
`crates/rcad/src/cloud_ui/activity.rs`; the navigation bar layout test is
`crates/rcad/tests/it/toolbar_layout.rs`.
