# Associative drawings

Status: run 6, foundation of SESSION_7 §4 (coverage DW01 base/projected views and sheet,
DW02 driven dimensions: partial). Code: `crates/rcad-document/src/drawing.rs` (model,
projection, hidden lines, dimensions), `crates/rcad-kernel/src/edges.rs` (exact B-rep edges),
`crates/rcad-exchange/src/drawing.rs` (sheet primitives, SVG writer),
`crates/rcad-exchange/src/pdf.rs` (PDF writer), `crates/rcad-exchange/src/drawing_dxf.rs` (DXF writer), `crates/rcad-commands/src/drawings.rs`
(commands). Tests: `crates/rcad-commands/tests/drawings.rs`.

## What is stored and what is computed

A drawing is part of the document (`Document.drawings`, native format v6, written only when a
document has one; `io::v6_feature` "drawings", so an older build refuses the file instead of
dropping it). It stores only:

* sheet size (`a4_landscape` 297 × 210 mm, `a3_landscape` 420 × 297 mm), projection
  convention (`third_angle`, the default, or `first_angle`; printed in the title block),
  title block fields (title; part number and material, empty = taken from the model);
* views: a **base view** names bodies by `BodyId`, an orientation (`front`, `back`, `top`,
  `bottom`, `right`, `left`, `iso`; Z up, the same directions as the viewport presets), a
  scale (sheet mm per model mm) and the sheet position of its centre; a **projected view**
  names its parent view and the side it is seen from (`above`, `below`, `left`, `right`);
  its scale is the parent's and its position follows from the parent's outline and the
  convention (third angle: the view from above is placed above, from the right to the
  right; first angle: mirrored). Changing the convention moves the projected views;
* semantic annotations: a **hole callout** names a Hole feature by `FeatureId`.

Nothing geometric is stored: no edges, no tessellation indices, no dimension values. Every
export renders from the current regenerated model, so a dimension cannot go stale. The drawn
values are *driven* annotations (they are measured from the model and never change it); the
model's driving dimensions stay in sketches and features.

**References that break are reported, never redrawn from old data.** A view whose body no
longer exists (deleted creating feature, consumed by a combine, rolled back) draws a red
flag box with "body Bn missing" instead of geometry, reports `status: missing_bodies` and
an issue in `Response.drawing_report.issues`. A hole callout whose feature was deleted or
suppressed, whose hole is not seen end-on in that view, or whose circle cannot be found at
the hole's current centre and diameter is drawn in red as `Ø? Fn: <reason>` without a
value and reported as dangling. Automatic dimensions simply follow the geometry (a deleted
hole's Ø disappears because its circle is gone). Mesh (STL/3MF) bodies are not drawn (no
exact edges); the view is flagged.

## Rendering

* **Edges.** Every B-rep edge of a body (`rcad_kernel::drawing_edges`) is classified once as a
  straight segment, a circle/arc (samples on one circle within 1e-6 mm; the kernel's rational
  arcs and the exactified boolean edges are exact) or, for anything else, a 33-point polyline
  (stated as sampled). Lines and arcs are projected exactly: a line stays a line, an arc whose
  plane faces the view is drawn as a true arc/circle; arcs seen edge-on become lines; other
  arcs (ellipses in the iso view) are drawn as polylines.
* **Smooth edges** (seams of closed faces, tangent edges between faces with equal normals) are
  not drawn, except where they lie exactly on the outline (surface normal perpendicular to the
  view at both ends and the middle). Tangent edges are therefore omitted rather than drawn
  thin.
* **Silhouettes of curved faces**: mesh edges inside one B-rep face between a front- and a
  back-facing triangle of the occlusion tessellation. On a cylinder whose axis is perpendicular
  to the view the segment is moved onto the exact generator (axis ± r, perpendicular to the
  view), so cylinder outlines are exact; on other curved faces the silhouette is accurate to the
  chord tolerance below.
* **Hidden-line removal is sampled.** Each edge/silhouette is sampled at
  `sample_spacing = max(diag / 400, 0.02 mm)` (diag: bounding diagonal of the drawing's bodies)
  and each sample is tested against the front-facing triangles of a tessellation of the view's
  bodies (`hlr_chord = max(2e-4 × diag, kernel minimum)`): hidden when strictly inside a
  triangle's projection by more than `1.5 × chord + 1e-6 × diag` and behind it by more than
  `2 × chord + 1e-6 × diag`. Visibility changes between samples are located by bisection to
  1e-4 mm; runs shorter than twice the inside margin (artefacts where an edge ends on an
  occluding outline) take their neighbour's visibility. The drawn curves are the exact
  projected curves cut at those parameters. Visible curves are solid 0.5 mm, hidden curves
  dashed 0.35 mm (3/1.5 mm). Coincident curves are merged (collinear segments and concentric
  equal circles are united; a hidden piece under a visible one is dropped). Iso (pictorial)
  views draw visible edges only. Consequences: a
  feature thinner than the sampling spacing can be misclassified, and visibility exactly at an
  occluding outline is decided within the margin above.
* **Dimensions** (when `auto_dimensions`, default on): each world axis is dimensioned once, on
  the first orthographic view (in view order) whose horizontal or vertical axis it is, from the
  exact bounding box of the view's bodies (`rcad_kernel::bounds`): horizontal below the view,
  vertical to its right. Iso views get none. Every visible full circle seen end-on gets a
  centre mark (chain lines r + 2 mm past the circle) and circles of equal diameter in a view
  share one `Ø` callout (`n× Øx` for several). Values: up to two decimals, trailing zeros
  removed, model millimetres.
* **Title block** (bottom right, 130 × 32 mm): title, part number (the title block's, else the
  active configuration's, else "-"), material (the title block's, else the distinct materials
  of the drawn bodies, else "-"), scale (common base-view scale or "AS SHOWN"), sheet size,
  projection convention as text. No date (exports are byte-for-byte deterministic). The ISO
  projection symbol is not drawn.
* A view whose outline extends past the 10 mm border is reported as an issue (it is still
  written; nothing is clipped silently).

## Section views

A section view (`{"kind":"section","parent","from","offset","label"}`) is a projected view
of its parent with the material between the cutting plane and the viewer removed. The plane
contains the parent's view direction and a cutting line in the parent: horizontal at `offset`
along the parent's up axis for `from` above/below, vertical along its right axis for `from`
left/right (`offset` is a world coordinate along that axis, e.g. x = 20 on a front view). The
cut is the exact kernel cut used by Split Body (the part behind the plane; a plane that misses
the part, or a cut that falls apart, flags the view with the reason). The cut body is drawn like
any view (exact edges, sampled hidden lines); its faces lying in the plane are hatched with thin
45° lines 2.5 mm apart on the sheet, clipped to the faces' tessellation (exact for straight
boundaries, within the chord tolerance on arcs). The parent shows the cutting line (chain line
past the outline, thick ends, arrows in the direction of sight, the label at both ends); the
section is labelled `SECTION A-A` and gets no automatic dimensions. Labels are 1-3 letters or
numbers, unique per drawing; sections of sections are refused. Projected and section views are
placed aligned with their parent on the side the convention gives, moved further out past any
view already there.

## Detail views

A detail view (`add_drawing_view` with a `Detail` spec: parent view, label, centre, radius in model
mm, scale) shows the parent's projected geometry inside a circle of that radius around the centre,
enlarged by its own scale and clipped to the circle; the parent shows the labelled circle. Its
orientation follows the parent (it cannot be set separately). Like every view it is recomputed from
the regenerated model at export; a deleted parent or body flags it. Evidence: `drawings.rs`
(clipping test). Not yet: breaks of hidden lines at the clip boundary are drawn as clipped
segments without end treatment.

## Export

`export_drawing {drawing, path, format?}` writes one sheet:

* **SVG**: `width`/`height` in mm, `viewBox` in mm (one user unit = 1 mm), y flipped for SVG.
  Groups `border`, `title`, `view-N` (geometry, centre marks, label) and `dims-N`
  (dimensions and callouts); every curve carries `class` (`visible`, `hidden`, `center`,
  `thin`, `frame`, `flag`) and explicit stroke attributes.
* **PDF**: one page, PDF 1.4, uncompressed content, MediaBox = sheet size in points; the
  content stream is scaled to millimetres (`cm`), text is the standard Helvetica font
  (WinAnsiEncoding; `Ø` is code 0xD8), each group is a marked-content sequence
  `/RCADGroup <</Name (view-1)>> BDC … EMC`. Arcs and circles are cubic Béziers of at most 90°
  (radial error < 2.8e-4 × r); lines are exact.
* **DXF** (`.dxf`): R12 ASCII (`crates/rcad-exchange/src/drawing_dxf.rs`), sheet mm,
  `$INSUNITS` 4 (mm), layers `VISIBLE`, `HIDDEN` (linetype DASHED), `CENTER` (CENTER),
  `THIN`, `FRAME`, `FLAG` (red). Exact `LINE`/`ARC`/`CIRCLE`; polylines as consecutive
  `LINE`s, arrowheads as `SOLID`, text as `TEXT` with `Ø` written as `%%c`. View grouping is
  not represented (R12 has no groups without blocks); layers carry the line style.

All three are deterministic: identical documents give identical bytes (tested after
save/reopen for SVG/PDF, twice for DXF). Numbers are written with 3 (SVG), 4 (PDF) or 6 (DXF)
decimals.

## Verification

`crates/rcad-commands/tests/drawings.rs` reads its own output back with two small readers
written in the test (an XML tag/attribute scanner and a PDF xref + content-stream
interpreter), independent of the writer's types:

* 40 × 30 × 10 block with a Ø8 through hole: front/top/right views are rectangles of 40 × 10,
  40 × 30, 30 × 10 at scale; the hole's silhouettes are exactly two dashed lines in the front
  and right views at the hole's x / y ± 4, 10 long; the top view has one visible circle r = 4
  at the hole centre with a centre mark; the views are aligned and placed per the convention;
  dimension texts are exactly {40, 10, 30, Ø8}; SVG and PDF agree.
* Changing the hole to Ø12 at (12, 10) moves and resizes the hidden lines and the circle and
  gives Ø12; switching to first angle swaps the view placement and the title block text;
  deleting the hole removes its Ø and hidden lines and reports its callout as dangling;
  deleting the body flags every view (no geometry drawn); undo restores the drawing of it.
* Ø20 × 30 cylinder: side views are 20 × 30 rectangles whose vertical sides are the exact
  silhouettes; the top view is one circle; dimensions {20, 20, 30, Ø20}.
* Section A-A of the block through the hole axis, seen from the right: 30 × 10 outline and
  the two hole walls at y = cy ± d/2 (6 visible lines, nothing hidden), every 45° hatch line
  inside the cut face (none across the hole), "SECTION A-A", the cutting line on the front view
  at x = 20 extending past it, labelled A twice, the view placed past the right view; it
  follows Ø8 → Ø12 at a new position, shows one solid face after the hole is deleted, and a
  cutting plane that misses the part is flagged ("section failed"), not drawn.
* Invalid commands (unknown body, duplicate projected side, NaN scale, scaling a projected
  view, removing a parent view, unknown extension, unknown drawing) are refused unchanged;
  delete/undo/redo of a drawing.
* External reader: poppler's `pdfinfo`/`pdftotext`/`pdftoppm` read the exported PDF (page size
  A4 841.89 × 595.276 pt, all texts extracted, rendered for visual inspection;
  `tools/validate_drawing_pdf.sh`).
* DXF: the test reads the group codes back (12 visible / 4 hidden lines, hidden total 40 mm,
  one Ø8 circle, texts `40`, `30`, `10`, `%%c8`); FreeCAD's DXF importer reads the same file
  with the same counts and lengths per layer (`tools/validate_drawing_dxf.sh`, via
  `tools/freecad_check_drawing_dxf.py`).
* Title block from the model: material PLA on the body and an active configuration with part
  number PN-STD-40 appear in the title block; an A3 sheet at 2:1 doubles every view while the
  dimension values stay 40/30/10/Ø8.

## Not supported (yet)

Half/offset/aligned/broken-out sections and hatch patterns per material, detail views, broken/auxiliary views, user-placed linear/angular
dimensions, tolerances and fits, notes and leaders other than hole callouts, thread
representation, BOM tables and balloons (no assembly model to count yet), multiple sheets per
drawing, view grouping in DXF, a drawing editor canvas (views are placed numerically),
tangent edges drawn as thin lines, the ISO projection symbol, dangling-reference repair UI.
