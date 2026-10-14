#!/usr/bin/env python3
"""Second set of hand-authored qualification corpus files (Python stdlib only),
written from the public format specifications, independent of R-CAD's writers.
Expected values are stated here and in MANIFEST.md / manifest.json.

* c1: colours added to FreeCAD's c0 STEP (AP214 STYLED_ITEM chain), because
  headless FreeCAD writes no colours.
* h6: 3MF in centimetres, three build items, one object made of two components.
* h7: DXF with a hole and a counter (island inside a hole).
* h8: SVG with a hole and a counter.
* x6..x11: truncated / malformed / unsupported variants.

Zip entries carry a fixed date so the output is byte-for-byte reproducible.

usage: gen_handwritten_more.py OUT_DIR
"""
import math
import os
import re
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


# ---------------------------------------------------------------- c1 colours
# Red on the large block, blue on the small one, attached to each
# MANIFOLD_SOLID_BREP through STYLED_ITEM (ISO 10303-214 presentation).
src = open(p("c0_blocks_uncoloured_mm.step"), encoding="latin-1").read()
breps = re.findall(r"^#(\d+) = MANIFOLD_SOLID_BREP", src, re.M)
ctx = re.search(r"^#\d+ = SHAPE_REPRESENTATION\([^;]*,#(\d+)\);", src, re.M).group(1)
nid = max(int(x) for x in re.findall(r"^#(\d+) =", src, re.M)) + 1
lines, styled = [], []
for brep, rgb in zip(breps, [(1.0, 0.0, 0.0), (0.0, 0.0, 1.0)]):
    c, fasc, fas, ssfa, sss, ssu, psa, si = range(nid, nid + 8)
    nid += 8
    lines += [
        f"#{c} = COLOUR_RGB('',{rgb[0]:.1f},{rgb[1]:.1f},{rgb[2]:.1f});",
        f"#{fasc} = FILL_AREA_STYLE_COLOUR('',#{c});",
        f"#{fas} = FILL_AREA_STYLE('',(#{fasc}));",
        f"#{ssfa} = SURFACE_STYLE_FILL_AREA(#{fas});",
        f"#{sss} = SURFACE_SIDE_STYLE('',(#{ssfa}));",
        f"#{ssu} = SURFACE_STYLE_USAGE(.BOTH.,#{sss});",
        f"#{psa} = PRESENTATION_STYLE_ASSIGNMENT((#{ssu}));",
        f"#{si} = STYLED_ITEM('color',(#{psa}),#{brep});",
    ]
    styled.append(f"#{si}")
lines.append(f"#{nid} = MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({','.join(styled)}),#{ctx});")
head, sep, tail = src.rpartition("ENDSEC;")
coloured = head.rstrip("\n") + "\n" + "\n".join(lines) + "\n" + sep + tail
coloured = coloured.replace("c0_blocks_uncoloured_mm.step", "c1_coloured_blocks_mm.step")
open(p("c1_coloured_blocks_mm.step"), "w", encoding="latin-1", newline="").write(coloured)

# ----------------------------------------------------------------- 3MF helpers
CORE = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
CT = ('<?xml version="1.0" encoding="UTF-8"?>\n<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
      '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
      '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>\n')
RELS = ('<?xml version="1.0" encoding="UTF-8"?>\n<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>\n')


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


def write_3mf(name, model):
    with zipfile.ZipFile(p(name), "w", zipfile.ZIP_DEFLATED) as z:
        for n, data in (("[Content_Types].xml", CT), ("_rels/.rels", RELS), ("3D/3dmodel.model", model)):
            info = zipfile.ZipInfo(n, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)


def rot_z(deg, t):
    """3MF row-vector matrix: rotation about Z by deg, then translation t."""
    c, s = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    m = [c, s, 0, -s, c, 0, 0, 0, 1, *t]
    return " ".join(f"{v:.12g}" for v in m)


def apply(mstr, v):
    m = [float(x) for x in mstr.split()]
    x, y, z = v
    return (x * m[0] + y * m[3] + z * m[6] + m[9],
            x * m[1] + y * m[4] + z * m[7] + m[10],
            x * m[2] + y * m[5] + z * m[8] + m[11])


# ------------------------------------------------------------------- h6 (cm)
# Object 1: 1 cm cube. Object 2: two components of object 1 (identity, and
# rotated 90 deg about Z then moved 3 cm in X). Object 3: 0.5 cm cube.
# Items: 1 at +3 cm Y; 2 at +2 cm Z; 3 rotated 45 deg about Z at +5 cm X.
# Volume (1 + 2 + 0.125) cm^3 = 3125 mm^3.
comp2 = rot_z(90, (3, 0, 0))
items = [(1, rot_z(0, (0, 3, 0))), (2, rot_z(0, (0, 0, 2))), (3, rot_z(45, (5, 0, 0)))]
model = (f'<?xml version="1.0" encoding="UTF-8"?>\n<model unit="centimeter" xml:lang="en-US" xmlns="{CORE}">'
         f'<resources><object id="1" type="model" name="Cube 1cm">{mesh_xml(cube((0, 0, 0), 1))}</object>'
         f'<object id="2" type="model" name="Pair"><components><component objectid="1"/>'
         f'<component objectid="1" transform="{comp2}"/></components></object>'
         f'<object id="3" type="model" name="Cube 5mm">{mesh_xml(cube((0, 0, 0), 0.5))}</object></resources><build>'
         + "".join(f'<item objectid="{o}" transform="{m}"/>' for o, m in items)
         + '</build></model>\n')
write_3mf("h6_three_items_cm.3mf", model)
pts = []
for o, m in items:
    base = cube((0, 0, 0), 0.5 if o == 3 else 1)
    sets = [base] if o != 2 else [base, [tuple(apply(comp2, v) for v in t) for t in base]]
    for tris in sets:
        for t in tris:
            for v in t:
                pts.append(tuple(10 * c for c in apply(m, v)))
lo = [min(q[i] for q in pts) for i in range(3)]
hi = [max(q[i] for q in pts) for i in range(3)]
print("h6 bbox mm", [round(x, 9) for x in lo + hi])

# ------------------------------------------------------------- h7 DXF counters
# Outer 100 x 60 rectangle, a circular hole r 10 at (20, 30), a 40 x 40 square
# hole (50..90, 10..50) and a 20 x 20 counter (island) in it (60..80, 20..40);
# 13 entities. Area 6000 - 100 pi - 1600 + 400 = 4800 - 100 pi.


def dxf(entities, insunits):
    head = f"0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{insunits}\n0\nENDSEC\n"
    return head + "0\nSECTION\n2\nENTITIES\n" + "".join(entities) + "0\nENDSEC\n0\nEOF\n"


def line(a, b):
    return f"0\nLINE\n8\n0\n10\n{a[0]}\n20\n{a[1]}\n30\n0\n11\n{b[0]}\n21\n{b[1]}\n31\n0\n"


def rect(x0, y0, x1, y1):
    c = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    return [line(c[i], c[(i + 1) % 4]) for i in range(4)]


ents = rect(0, 0, 100, 60) + ["0\nCIRCLE\n8\n0\n10\n20\n20\n30\n30\n0\n40\n10\n"]
ents += rect(50, 10, 90, 50) + rect(60, 20, 80, 40)
open(p("h7_counters_mm.dxf"), "w", newline="\n").write(dxf(ents, 4))

# ------------------------------------------------------------- h8 SVG counters
# 50 x 50 mm square with a 30 x 30 square hole and a r 5 circle counter in it.
# Area 2500 - 900 + 25 pi. Sketch = (x, -y).
open(p("h8_counters_mm.svg"), "w").write(
    '<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60">'
    '<path fill-rule="evenodd" d="M 5 5 H 55 V 55 H 5 Z M 15 15 V 45 H 45 V 15 Z"/>'
    '<circle cx="30" cy="30" r="5"/></svg>\n')

# --------------------------------------------------- malformed / unsupported
asc = open(p("h1_two_cubes_disconnected.stl")).read()
cut = asc.index("vertex", len(asc) // 2)
open(p("x6_truncated_ascii.stl"), "w", newline="\n").write(asc[:cut + 10])
with open(p("x7_nan_binary.stl"), "wb") as f:
    f.write(b"binary stl with a NaN".ljust(80, b" "))
    tris = cube((0, 0, 0), 1)
    f.write(struct.pack("<I", len(tris)))
    for i, t in enumerate(tris):
        f.write(struct.pack("<3f", *normal(t)))
        for k, v in enumerate(t):
            f.write(struct.pack("<3f", *((float("nan"), v[1], v[2]) if (i, k) == (5, 1) else v)))
        f.write(b"\0\0")
bogus = model.replace('unit="centimeter"', 'unit="parsec"')
write_3mf("x8_unit_bogus.3mf", bogus)
beam = model.replace(
    f'xmlns="{CORE}">',
    f'xmlns="{CORE}" xmlns:b="http://schemas.microsoft.com/3dmanufacturing/beamlattice/2017/02" '
    'requiredextensions="b">')
write_3mf("x9_required_beamlattice.3mf", beam)
sing = model.replace(rot_z(0, (0, 0, 2)), "1 0 0 0 1 0 0 0 0 0 0 2")
write_3mf("x10_singular_transform.3mf", sing)
dx = open(p("h7_counters_mm.dxf")).read()
open(p("x11_truncated.dxf"), "w", newline="\n").write(dx[: dx.index("CIRCLE") + 12])
print("handwritten corpus (second set) written")
