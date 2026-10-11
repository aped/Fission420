#!/usr/bin/env python3
"""Hand-built STL fixtures (Python stdlib `struct`, written from the STL format description,
independent of the Rust writer).

usage: gen_python.py OUT_DIR   (needs box_freecad_binary.stl in OUT_DIR for the truncated copy)

Outputs:
  tet_solid_header.stl  binary STL whose 80-byte header starts with "solid": the tetrahedron
                        (0,0,0) (10,0,0) (0,10,0) (0,0,10), outward-facing. Volume 1000/6 mm^3.
  box_truncated.stl     box_freecad_binary.stl minus its last 20 bytes (length mismatch).
  ascii_malformed.stl   ASCII STL with a bad token on line 4.
  ascii_nan.stl         ASCII STL with a NaN vertex coordinate on line 5.
  ascii_unterminated.stl ASCII STL that ends inside a facet (missing endloop/endfacet/endsolid).
"""
import os
import struct
import sys

out = sys.argv[1]

# Tetrahedron, faces wound counter-clockwise seen from outside (right-hand rule -> outward normal).
o, x, y, z = (0.0, 0.0, 0.0), (10.0, 0.0, 0.0), (0.0, 10.0, 0.0), (0.0, 0.0, 10.0)
faces = [
    ((0, 0, -1), o, y, x),  # bottom, normal -Z
    ((0, -1, 0), o, x, z),  # front y=0, normal -Y
    ((-1, 0, 0), o, z, y),  # left x=0, normal -X
    ((1 / 3**0.5,) * 3, x, y, z),  # slanted, normal (1,1,1)/sqrt3
]
header = b"solid tetra_but_binary (header deliberately starts with solid)".ljust(80, b" ")
with open(os.path.join(out, "tet_solid_header.stl"), "wb") as f:
    f.write(header)
    f.write(struct.pack("<I", len(faces)))
    for n, a, b, c in faces:
        f.write(struct.pack("<12fH", *n, *a, *b, *c, 0))

with open(os.path.join(out, "box_freecad_binary.stl"), "rb") as f:
    data = f.read()
with open(os.path.join(out, "box_truncated.stl"), "wb") as f:
    f.write(data[:-20])

with open(os.path.join(out, "ascii_malformed.stl"), "w") as f:
    f.write(
        "solid bad\n"
        "  facet normal 0 0 1\n"
        "    outer loop\n"
        "      vertex 0 0 zero\n"
        "      vertex 1 0 0\n"
        "      vertex 0 1 0\n"
        "    endloop\n"
        "  endfacet\n"
        "endsolid bad\n"
    )

with open(os.path.join(out, "ascii_nan.stl"), "w") as f:
    f.write(
        "solid nan\n"
        "  facet normal 0 0 1\n"
        "    outer loop\n"
        "      vertex 0 0 0\n"
        "      vertex 1 nan 0\n"
        "      vertex 0 1 0\n"
        "    endloop\n"
        "  endfacet\n"
        "endsolid nan\n"
    )

with open(os.path.join(out, "ascii_unterminated.stl"), "w") as f:
    f.write(
        "solid cut\n"
        "  facet normal 0 0 1\n"
        "    outer loop\n"
        "      vertex 0 0 0\n"
        "      vertex 1 0 0\n"
    )
print("python fixtures written to", out)
