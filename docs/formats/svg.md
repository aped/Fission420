# SVG planar paths (`f420_exchange::svg`)

Status: run 3. Commands `import_svg` (new unconstrained sketch on a plane) and `export_svg`
(a sketch). Clean-room from the SVG 1.1 specification (path grammar, transform list, arc
implementation notes F.6.5); XML tokenizing by `quick-xml` (already a dependency for 3MF).

## Import subset
| SVG | Imported as | Notes |
|---|---|---|
| `line` | line | any affine transform |
| `rect` | 4 lines | `rx`/`ry` > 0 (rounded) unsupported; zero size skipped as degenerate |
| `circle`, `ellipse` with `rx == ry` | circle | similarity transforms only |
| `polyline`, `polygon` | lines | polygon closed |
| `path` `M L H V Z` (abs/rel) | lines | must start with M/m |
| `path` `A` with `rx == ry` | arc | endpoint → centre (F.6.5), radius scaled up when too small; similarity transforms only; re-oriented CCW after the full transform |
| `g`, root `svg` | transforms | `matrix translate scale rotate(a[,cx,cy]) skewX skewY` |
| `C S Q T`, elliptical `A`, `use`, `text`, `image`, nested `svg`, `foreignObject`, `switch`, `a` | **unsupported** | listed per element type with a reason; the import is refused unless `allow_partial` |
| `defs symbol clipPath mask pattern marker style script metadata title desc` gradients, filters | ignored | draw nothing by themselves |

* Units: root `width`/`height` with `mm cm in pt pc` plus `viewBox` → mm per user unit (the
  smaller ratio, as for the default `preserveAspectRatio`; other values refused). Without a
  physical size the file is unitless: `units_to_mm` is required (`UnitsRequired` otherwise).
  Only the scale is applied: drawing coordinates keep their origin.
* Orientation: sketch = (x, −y) of the drawing in mm (SVG y points down).
* Endpoints within 1e-6 mm become one point; no constraints except the origin; the sketch
  is solved once (under-constrained), like DXF.
* Limits: 64 MiB, 200 000 elements, nesting depth 256. A DOCTYPE with an internal subset
  (entity declarations) is refused; a public/system DOCTYPE (FreeCAD, older editors) is
  accepted. Malformed input is an error, never a panic (prefix fuzz in the tests).

## Export
The solved sketch's lines, arcs (`A` commands, `sweep-flag 0`, large-arc from the sweep)
and circles; construction geometry is not written. Root `width`/`height` in mm and a
`viewBox` in mm: 1 user unit = 1 mm, coordinates are the sketch's local 2D coordinates with
y negated. Stroke only, no fill. Losses: constraints, dimensions, construction geometry,
sketch placement (plane) — the same contract as DXF export.

## Evidence
* `crates/f420-exchange/tests/svg_fixtures.rs` (7 tests): hand-written fixture
  `plate_mm.svg` (arc, circle, translated group; exact region area), FreeCAD-generated
  `freecad_plate.svg` (`tools/fixtures/svg/generate_freecad_svg.py`; reflected group
  transform, arcs; analytic region area 2400 − 36π − (80 + 16π)), units/override, transforms,
  unsupported reporting and partial import, DOCTYPE, export → import round trip, prefix fuzz.
* Acceptance 3S (`f420-cli acceptance`): FreeCAD SVG → sketch with two holes → 5 mm solid
  (exact volume, bounds), own SVG export re-imported to the same solid, STEP export.
* `tools/check_run3_steps.sh`: the 3S STEP validated by FreeCAD (volume, bounds, probes), and
  the exported SVG read by FreeCAD's own `importSVG` (total edge length 220 + 20π, 60 × 40).
