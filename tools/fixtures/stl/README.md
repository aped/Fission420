# STL fixtures: provenance

Committed under `crates/rcad-exchange/tests/fixtures/stl/`, used by
`crates/rcad-exchange/tests/stl_fixtures.rs`. Regenerate with `tools/fixtures/stl/gen_freecad.sh`
(FreeCAD 1.1.4 flatpak `org.freecad.FreeCAD`, headless `FreeCADCmd`, plus Python stdlib).

| file | writer | content | proves |
|---|---|---|---|
| `box_freecad_ascii.stl` | FreeCAD Mesh (`"AST"`), `gen_freecad.py` | `Part.makeBox(20,10,5)` tessellated: 8 points, 12 facets | ASCII parsing, welding, closed/volume 1000/area 700 |
| `box_freecad_binary.stl` | FreeCAD Mesh (`"STL"`), same mesh | header `MESH-MESH-…` | binary parsing; identical geometry to the ASCII file |
| `box_open_freecad.stl` | FreeCAD Mesh, the two +Z facets removed | 10 facets | 4 boundary edges, `closed = false`, no volume, area 500 |
| `tet_solid_header.stl` | `gen_python.py` (`struct`) | binary, 80-byte header starts with `solid`; tetrahedron legs 10 | detection by length, not by the `solid` token; volume 1000/6 |
| `box_truncated.stl` | `gen_python.py` | `box_freecad_binary.stl` minus 20 bytes (664 bytes) | length-mismatch error |
| `ascii_malformed.stl` | `gen_python.py` | `vertex 0 0 zero` on line 4 | malformed error with line number |
| `ascii_nan.stl` | `gen_python.py` | `vertex 1 nan 0` on line 5 | non-finite error with line number |
| `ascii_unterminated.stl` | `gen_python.py` | ends inside a facet | truncated error |

FreeCAD's own report on its files (printed by `gen_freecad.sh`, 2026-10-10):
box ASCII/binary: 8 points, 12 facets, `isSolid` true, volume 1000.0, area 700.0, bounds
0..(20,10,5); open box: 10 facets, `isSolid` false, area 500.0.

Writer validation (independent reader): `tools/fixtures/stl/validate_writer.sh` writes a 10 mm
cube with `stl::write_binary` / `stl::write_ascii` and reads both with FreeCAD's Mesh module
(`tools/freecad_check_stl.py`): 8 points, 12 facets, solid, no non-manifolds, volume 1000.0 —
PASS on 2026-10-10.
