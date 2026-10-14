#!/usr/bin/env python3
"""Hand-authored edge cases of the qualification corpus (Python stdlib
only), written from the public format specifications, independent of R-CAD's
writers. Expected values are stated here and in MANIFEST.md / manifest.json.

usage: gen_handwritten.py OUT_DIR
"""
import os
import struct
import sys
import zipfile

out = sys.argv[1]
p = lambda n: os.path.join(out, n)


def cube(o, s):
    """Closed cube [o, o+s]^3 as outward triangles (12)."""
    x0, y0, z0 = o
    v = [(x0 + x * s, y0 + y * s, z0 + z * s) for z in (0, 1) for y in (0, 1) for x in (0, 1)]
    t = [(0, 2, 3), (0, 3, 1), (4, 5, 7), (4, 7, 6), (0, 1, 5), (0, 5, 4),
         (2, 6, 7), (2, 7, 3), (0, 4, 6), (0, 6, 2), (1, 3, 7), (1, 7, 5)]
    return [(v[a], v[b], v[c]) for a, b, c in t]


def normal(t):
    (ax, ay, az), (bx, by, bz), (cx, cy, cz) = t
    u = (bx - ax, by - ay, bz - az)
    w = (cx - ax, cy - ay, cz - az)
    n = (u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0])
    l = sum(c * c for c in n) ** 0.5
    return tuple(c / l for c in n)


# h1: ASCII STL, two disconnected 10 mm cubes (V = 2000, 2 components).
tris = cube((0, 0, 0), 10) + cube((20, 0, 0), 10)
with open(p("h1_two_cubes_disconnected.stl"), "w", newline="\n") as f:
    f.write("solid two_cubes\n")
    for t in tris:
        n = normal(t)
        f.write(f"  facet normal {n[0]} {n[1]} {n[2]}\n    outer loop\n")
        for v in t:
            f.write(f"      vertex {v[0]} {v[1]} {v[2]}\n")
        f.write("    endloop\n  endfacet\n")
    f.write("endsolid two_cubes\n")

# h2: 3MF in inches, two objects; the second placed by an item transform
# (+3 in X). Cube 1 in and cube 0.5 in: V = (1 + 0.125) in^3 = 18435.447 mm^3.
CORE = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"


def mesh_xml(tris):
    verts, idx = [], []
    for t in tris:
        ids = []
        for v in t:
            if v not in verts:
                verts.append(v)
            ids.append(verts.index(v))
        idx.append(ids)
    s = "<mesh><vertices>" + "".join(f'<vertex x="{x}" y="{y}" z="{z}"/>' for x, y, z in verts)
    s += "</vertices><triangles>" + "".join(f'<triangle v1="{a}" v2="{b}" v3="{c}"/>' for a, b, c in idx)
    return s + "</triangles></mesh>"


model = (f'<?xml version="1.0" encoding="UTF-8"?>\n<model unit="inch" xml:lang="en-US" xmlns="{CORE}">'
         f'<resources><object id="1" type="model" name="Cube 1in">{mesh_xml(cube((0, 0, 0), 1))}</object>'
         f'<object id="2" type="model" name="Cube half">{mesh_xml(cube((0, 0, 0), 0.5))}</object></resources>'
         f'<build><item objectid="1"/><item objectid="2" transform="1 0 0 0 1 0 0 0 1 3 0 0"/></build></model>\n')
CT = ('<?xml version="1.0" encoding="UTF-8"?>\n<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
      '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
      '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>\n')
RELS = ('<?xml version="1.0" encoding="UTF-8"?>\n<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>\n')
with zipfile.ZipFile(p("h2_two_objects_inch.3mf"), "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("[Content_Types].xml", CT)
    z.writestr("_rels/.rels", RELS)
    z.writestr("3D/3dmodel.model", model)


def dxf(entities, insunits):
    s = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n%d\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n" % insunits
    return s + entities + "0\nENDSEC\n0\nEOF\n"


def line(a, b):
    return f"0\nLINE\n8\n0\n10\n{a[0]}\n20\n{a[1]}\n30\n0.0\n11\n{b[0]}\n21\n{b[1]}\n31\n0.0\n"


# h3: DXF in inches ($INSUNITS 1): 2 x 1 in rectangle, a Ø0.5 in circle at (0.5, 0.5)
# and an arc r 0.25 in about (1.5, 0.5) from 0° to 180°: 4 lines, 1 circle, 1 arc.
ents = "".join(line(a, b) for a, b in [((0, 0), (2, 0)), ((2, 0), (2, 1)), ((2, 1), (0, 1)), ((0, 1), (0, 0))])
ents += "0\nCIRCLE\n8\n0\n10\n0.5\n20\n0.5\n30\n0.0\n40\n0.25\n"
ents += "0\nARC\n8\n0\n10\n1.5\n20\n0.5\n30\n0.0\n40\n0.25\n50\n0.0\n51\n180.0\n"
open(p("h3_profile_inch.dxf"), "w", newline="\n").write(dxf(ents, 1))

# h4: DXF (mm) with a SPLINE: refused unless partial import is allowed.
spl = ("0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n"
       + "".join(f"40\n{k}\n" for k in (0, 0, 0, 0, 1, 1, 1, 1))
       + "".join(f"10\n{x}\n20\n{y}\n30\n0.0\n" for x, y in ((0, 0), (10, 10), (20, -10), (30, 0))))
open(p("h4_spline_unsupported.dxf"), "w", newline="\n").write(dxf(line((0, 0), (30, 0)) + spl, 4))

# h5: SVG 100 x 50 mm (viewBox 1 unit = 1 mm): a rect, a circle and a path with
# a circular arc: 4 + 1 + (1 line + 1 arc) entities.
open(p("h5_shapes_mm.svg"), "w").write(
    '<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">'
    '<rect x="5" y="5" width="30" height="20"/><circle cx="60" cy="20" r="8"/>'
    '<path d="M 70 40 L 90 40 A 10 10 0 0 0 70 40 Z"/></svg>\n')

# Malformed / truncated inputs (each must fail cleanly, nothing imported).
src = open(p("p1_bracket_holes_mm.step"), encoding="latin-1").read()
open(p("x1_truncated.step"), "w", encoding="latin-1", newline="").write(src[: len(src) // 2])
with open(p("x2_bad_count.stl"), "wb") as f:
    f.write(b"binary stl header".ljust(80, b" "))
    f.write(struct.pack("<I", 1000))  # claims 1000 triangles, has 2
    for t in cube((0, 0, 0), 1)[:2]:
        f.write(struct.pack("<3f", *normal(t)))
        for v in t:
            f.write(struct.pack("<3f", *v))
        f.write(b"\0\0")
data = open(p("h2_two_objects_inch.3mf"), "rb").read()
open(p("x3_truncated.3mf"), "wb").write(data[: len(data) // 2])
open(p("x4_garbage.dxf"), "w", newline="\n").write(
    "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\nnot-a-number\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nEOF\n")
open(p("x5_unclosed.svg"), "w").write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x="1"')
print("handwritten corpus written")
