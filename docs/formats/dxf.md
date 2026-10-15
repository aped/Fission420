# DXF support (`rcad_exchange::dxf`)

Clean-room implementation from the public AutoCAD DXF Reference (group-code format, HEADER
variables, entity group codes, the arbitrary-axis algorithm). ASCII DXF only (R12/AC1009 …
R2018/AC1032 group-code files); binary DXF is rejected with a clear error.

## Group codes relied on
| code | meaning |
|---|---|
| 0 | structure marker / entity type (`SECTION`, `ENDSEC`, `EOF`, `LINE`, …) |
| 2 | section / table / block name |
| 9 | header variable name (`$ACADVER`, `$INSUNITS`, `$MEASUREMENT`) |
| 1, 70 | `$ACADVER` string; integer header values, entity flags (polyline bit 1 closed; POLYLINE bits 2/4 fit, 8/16/64 3D/mesh; VERTEX bits 1/8/16/32/64/128) |
| 8 | layer (default `0`) |
| 10/20/30, 11/21/31 | points (LINE: WCS; ARC/CIRCLE/polylines: OCS) |
| 38 | LWPOLYLINE elevation |
| 39 | thickness (ignored with a warning) |
| 40, 41, 43 | radius (ARC/CIRCLE); polyline widths (ignored with a warning) |
| 42 | bulge = tan(sweep/4), positive = counter-clockwise |
| 50, 51 | ARC start/end angle, degrees, CCW |
| 66 | POLYLINE "vertices follow" |
| 67 | paper space (1 = not imported) |
| 90 | LWPOLYLINE vertex count (checked) |
| 210/220/230 | extrusion direction (default (0,0,1)) |
| 999 | comment (skipped) |

## Import (`dxf::import(bytes, &DxfImportOptions) -> Result<DxfImport>`)
* **Supported**: LINE, ARC, CIRCLE, LWPOLYLINE, 2D POLYLINE/VERTEX/SEQEND (straight and bulge
  segments; closed flag). Bulges become exact arcs (centre = chord midpoint + left normal ×
  d(1 − b²)/(4b)).
* **Plane**: only geometry in the WCS XY plane. LINE z, OCS elevation must be
  ≤ 1e-9 × max(1, |coordinates|). Extrusion must be (0,0,1) or (0,0,−1); for (0,0,−1) OCS
  points map to WCS (−x, y) and the arc is stored with swapped ends so it stays CCW (verified
  against ezdxf; FreeCAD 1.1.4 gets this case wrong, see fixtures README).
* **ELLIPSE / SPLINE**: a full ELLIPSE (parameters 0 to 2π, in the XY plane) becomes an exact
  sketch ellipse; a SPLINE becomes a sketch spline only when it is exactly one: open,
  non-rational (all weights 1), degree 3, control points given, clamped uniform knots (any
  affine knot scaling), XY plane. Its end control points join other curves like line ends.
  Elliptical arcs, fit-point, rational, periodic, other-degree or non-uniform splines are
  unsupported (reported with the specific reason), never approximated.
* **Unsupported, always reported** with type, count, reason and first line: SPLINE / ELLIPSE
  other than the above,
  INSERT (blocks are never expanded; BLOCKS definitions are ignored), TEXT/MTEXT/ATTDEF/ATTRIB,
  DIMENSION/LEADER/…, HATCH/SOLID/TRACE, POINT, 3DFACE/3DSOLID/REGION/…, RAY/XLINE, VIEWPORT,
  IMAGE…, 3D/mesh/fit polylines, tilted extrusion, non-zero Z, paper-space entities, any other
  type.
* **Policy**: all-or-nothing. Any unsupported entity on a selected layer →
  `ExchangeError::Unsupported { entities }`, unless `allow_partial`, which skips them and lists
  them in `DxfImport::skipped` and `warnings`. Degenerate entities (zero length/radius/sweep after
  scaling and joining) carry no geometry: they are listed in `skipped` and `warnings` but do not
  trigger the error.
* **Layers**: `layers: Option<Vec<String>>` (case-insensitive). The result lists every layer
  (from TABLES and from entities) with entity counts and whether it was selected; a requested
  layer that does not exist is a warning.
* **Units**: `$INSUNITS` → mm per unit: 1 in 25.4, 2 ft 304.8, 3 mi 1 609 344, 4 mm 1,
  5 cm 10, 6 m 1000, 7 km 1e6, 8 µin 2.54e-5, 9 mil 0.0254, 10 yd 914.4, 11 Å 1e-7, 12 nm 1e-6,
  13 µm 1e-3, 14 dm 100, 15 dam 1e4, 16 hm 1e5, 21–24 US survey ft/in/yd/mi (1 ft = 1200/3937 m).
  17–20 (Gm, AU, ly, pc) → `UnsupportedUnits`. Missing or 0 → `unit_override_to_mm` is required,
  else `UnitsRequired { measurement }` (the UI asks; `$MEASUREMENT` 0 imperial / 1 metric is passed
  as a hint). A declared unit always wins over the override. Reported as
  `LengthUnit { name, to_mm, declared, insunits }`.
* **Sketch**: `ensure_origin()` (fixed origin, not shared with geometry); line/arc endpoints
  within `join_tolerance_mm` (default 1e-6 mm, after scaling) share one point (first occurrence's
  coordinates; grid hash, O(n)); centres are never shared. `Entity::Arc` is CCW start→end, the
  same as DXF ARC. No constraints other than the origin's `FixedPoint`. The sketch is solved once
  so the implied arc radius equation holds (moves points by at most the join tolerance / file
  rounding; a larger shift is a warning); `solve.status` is `UnderConstrained`. Open endpoints
  are reported as a warning (a profile needs a closed loop).
* **Limits**: `max_bytes` (64 MiB), `max_entities` (200 000), pair count ≤ bytes/4, sketch size
  ≤ `rcad_sketch::MAX_UNKNOWNS` (2048 unknowns, ≈ 1000 points; larger drawings return
  `LimitExceeded`). Single linear pass, no recursion. Non-finite numbers → `NonFinite { line }`,
  syntax errors → `Malformed { line }`, missing ENDSEC/values → `Truncated`.

## Export (`dxf::export(&Sketch, &DxfExportOptions) -> Result<String>`)
R12 (AC1009) ASCII: HEADER (`$ACADVER`, `$INSUNITS = 4`, `$MEASUREMENT = 1`, `$EXTMIN/MAX`),
TABLES (LTYPE CONTINUOUS, LAYER), ENTITIES (LINE, ARC, CIRCLE). Local sketch coordinates in mm
(the sketch plane placement is not written). Construction entities are omitted unless
`include_construction` (then on `construction_layer`, default `CONSTRUCTION`). R12 was chosen
because its entities need no handles, owners, subclass markers or OBJECTS dictionaries, so a
small writer cannot produce an inconsistent R2000+ database; it is the most widely read flavour.
Numbers use the shortest exact `f64` decimal form.

## Known losses
* Import: only exactly representable ellipses/splines (see above); no text/dimensions/hatches/blocks.
* Export (R12): lines, arcs and circles only; a sketch with an ellipse or spline is refused
  (R12 has no ELLIPSE/SPLINE; use SVG or STEP).
* Import creates no constraints: design intent (dimensions, tangency) is not recovered.
* Width, thickness, linetype, colour are not imported.
* Export writes no constraints/dimensions and no sketch placement (2D local coordinates only).
* The sketch solver's size limit bounds the importable drawing size.

## Fixture evidence
`crates/rcad-exchange/tests/it/dxf_fixtures.rs` with FreeCAD-written and hand-written fixtures
(`tools/fixtures/dxf/README.md`): arc centres/radii/sweeps, bounds, `Sketch::profile()` closure
and analytic area (600 + 50π, 800 − 12.5π, 500, 1290.32), unit scaling (in, override), mirrored
OCS arc, unsupported diagnostics (all-or-nothing, partial, layer filter), join tolerance,
malformed/limit inputs and a garbage-input no-panic sweep; export → reimport round trip
(supplementary). Writer validated with FreeCAD 1.1.4 and ezdxf 1.4.2
(`tools/fixtures/dxf/validate_export.sh`, `ezdxf_check.py`): all PASS.

## Command-level evidence
Acceptance scenario D (`crates/rcad-commands/src/acceptance/formats.rs`, steps "D.7"-"D.11"): FreeCAD contour -> sketch -> edit -> extrude -> STEP (FreeCAD-validated), DXF export read back by our importer and by FreeCAD (`tools/check_formats.sh`), declared inch and unitless files, reload without the sources; DXF onto an imported STEP face: `crates/rcad-commands/tests/it/imports.rs`. Summary matrix: `docs/FORMAT_SUPPORT.md`.
