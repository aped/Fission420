# STL support (`f420_exchange::stl`, `f420_exchange::mesh`)

## Supported subset
* **Read**: ASCII and binary STL. ASCII keywords are case-insensitive; several `solid … endsolid`
  blocks in one file are concatenated (the name is the first one's). Facets must have exactly
  3 vertices.
* **Write**: `write_binary` (80-byte header `binary STL from Fission420: <name>`, never starting
  with `solid`; `f32` coordinates; attribute 0) and `write_ascii` (`f64`, shortest round-trip
  exponent form). A missing or zero stored normal is replaced by the right-hand-rule unit normal.

## Encoding detection
Binary when `len >= 84` and `84 + 50·n == len` (`n` = little-endian `u32` at byte 80), even if the
header starts with `solid`. Otherwise ASCII when the text starts with `solid` and is UTF-8.
Everything else is a length-mismatch (`Truncated`) error that states the expected and actual size.

## Vertex welding
Corners with exactly equal coordinates (bitwise `f64`, `-0.0 == 0.0`) become one vertex, numbered
in first-use order. No tolerance: near-coincident corners stay separate and appear as boundary
edges in the diagnostics.

## Units
STL is unitless. `read` returns file numbers; the caller applies `StlMesh::scale(to_mm)` after
asking the user (ADR 0004; default offered: mm). The scale must be finite and > 0.

## Limits and errors
`StlLimits { max_bytes (512 MiB), max_triangles (10 M) }`, checked before allocating. Errors
(`ExchangeError`, serializable): `LimitExceeded`, `Truncated` (length mismatch, missing
`endsolid`), `Malformed { line }` (unexpected keyword, bad number, wrong vertex count),
`NonFinite { line }` (NaN/inf coordinate; a non-finite *normal* is stored as `[0,0,0]`).
No input panics.

## Mesh diagnostics (`mesh::diagnose`)
Topology by vertex index: boundary (1 use), non-manifold (≥ 3 uses), inconsistently oriented
(2 uses, same direction) edges; degenerate (repeated index, or area ≤ 1e-12 × longest edge²)
and invalid (index out of range) triangles; vertex-connected components; area. `closed` means
every edge is shared by exactly two consistently oriented triangles (and no invalid/repeated-index
triangle). `signed_volume` is reported **only** for a closed mesh (positive for outward winding);
an open mesh never gets a volume.

## Known losses
* STL carries no units, colour (the attribute word is ignored), topology or exact geometry: an
  imported mesh is a display/measurement body, never an exact B-rep (ADR 0003: exact operations
  on mesh bodies are refused).
* Binary STL is `f32`: writing rounds coordinates to ~7 significant digits.
* Mixed or non-UTF-8 ASCII STL is rejected rather than guessed.

## Fixture evidence
`crates/f420-exchange/tests/stl_fixtures.rs` with fixtures from FreeCAD 1.1.4 and Python
(`tools/fixtures/stl/README.md`): ASCII = binary geometry, box closed with volume 1000 / area 700,
open box 4 boundary edges and no volume, `solid`-header binary detected, truncated / malformed /
NaN / unterminated / over-limit inputs rejected with the right error. Writer output validated by
FreeCAD (`tools/fixtures/stl/validate_writer.sh`: solid, volume 1000, PASS).
