# DXF fixtures: provenance

Committed under `crates/f420-exchange/tests/fixtures/dxf/`, used by
`crates/f420-exchange/tests/dxf_fixtures.rs`. Regenerate with `tools/fixtures/dxf/gen_freecad.sh`
(FreeCAD 1.1.4 flatpak, C++ writer `Import.writeDXFShape`) which also runs
`gen_handwritten.py` (Python stdlib, written from the public DXF reference group codes).
Expected values are derived analytically in the generator comments and the tests.

| file | writer | content | expected (mm) |
|---|---|---|---|
| `slot_freecad_r14.dxf` | FreeCAD, AC1014, `$INSUNITS 4` | 3 LINE + ARC centre (30,10) r 10, −90°→90° | closed, area 600 + 50π, bounds (0,0)–(40,20) |
| `notch_freecad_r12.dxf` | FreeCAD, AC1009, **no `$INSUNITS`** | 40×20 rectangle with semicircular notch r 5 at (20,20); ARC 180°→0° | unit required; with 1 mm: area 800 − 12.5π |
| `lwpoly_bulge.dxf` | hand-written, AC1015 | closed LWPOLYLINE (0,0) (20,0) b=1 (20,20) (0,20) b=−tan(π/8) | arcs centre (20,10) r 10 sweep π and centre (−10,10) r 10√2 sweep π/2; area 400 + 50π − (50π − 100) = 500 |
| `polyline_bulge_r12_unitless.dxf` | hand-written, AC1009, no units | same contour as POLYLINE/VERTEX/SEQEND | unit required; same geometry × chosen scale |
| `rect_inch.dxf` | hand-written, `$INSUNITS 1`, `$MEASUREMENT 0` | 2 × 1 rectangle of LINEs | 50.8 × 25.4, area 1290.32 |
| `slot_mirrored_arc.dxf` | hand-written | the slot, ARC in OCS with extrusion (0,0,−1): OCS centre (−30,10), 90°→270° | WCS centre (30,10), −90°→90°, area 600 + 50π |
| `unsupported_entities.dxf` | hand-written | PROFILE: 10×10 LINE square; HOLES: CIRCLE (5,5) r 2; NOTES: TEXT, SPLINE, INSERT of block `B`, LINE at z = 5; BLOCKS: `B` with a LINE | default: error listing TEXT/SPLINE/INSERT/LINE(not XY); partial: 5 entities, 4 skipped; layer PROFILE: area 100 |

## Independent validation (2026-10-10)

`tools/fixtures/dxf/validate_export.sh` exports sketches with `dxf::export`
(`examples/export_samples.rs`: the imported slot, bulge square, notch, and a circle + construction
line) and reads them, plus the hand-written fixtures, with FreeCAD's C++ DXF importer
(`freecad_read_dxf.py`), comparing edge types, arc centre/radius/sweep, bounds and closed-face
area against the analytic values above. Result: **all PASS**, with two FreeCAD importer
deviations asserted as observed (not as correct):

* `slot_mirrored_arc.dxf`: FreeCAD ignores the ARC's (0,0,−1) extrusion and places it at
  (−30,10). The DXF arbitrary-axis algorithm gives WCS centre (30,10); ezdxf 1.4.2 agrees
  (`ezdxf_check.py`: centre (30,10), start (30,20), end (30,0), i.e. clockwise in the WCS).
* `polyline_bulge_r12_unitless.dxf`: FreeCAD drops the closing (bulge) segment of the closed R12
  POLYLINE (2 lines + 1 arc). ezdxf 1.4.2 reads all four segments; FreeCAD reads the equivalent
  LWPOLYLINE completely (area 500).

FreeCAD applied `$INSUNITS = 1` to `rect_inch.dxf` (area 1290.32 mm²), matching our scaling.
The "First reference to missing Layer '0'" messages it prints come from the hand-written
fixtures, which have no TABLES section (allowed by the format); our exports define layer `0`.

Second reader: `ezdxf_check.py` (ezdxf 1.4.2, MIT) run from a throwaway venv under
`artifacts/venv` (`python3 -m venv artifacts/venv && artifacts/venv/bin/pip install ezdxf==1.4.2`;
not a project dependency). It reads every export and fixture, with WCS arc data matching the
expectations above.
