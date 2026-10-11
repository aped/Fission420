#!/usr/bin/env python3
"""Hand-written 3MF fixtures (Python stdlib only), written from the public 3MF Core
Specification 1.3 and ECMA-376 Part 2 (OPC), independent of Fission420's writer.

usage: gen_handwritten.py OUT_DIR

Every expected value is stated next to the fixture (and asserted in
crates/f420-exchange/tests/threemf_fixtures.rs).
"""
import os
import struct
import sys
import zipfile

CORE = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
MAT = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02"
PROD = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
CT = ('<?xml version="1.0" encoding="UTF-8"?>\n'
      '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
      '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
      '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>'
      '</Types>\n')
RELS = ('<?xml version="1.0" encoding="UTF-8"?>\n'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Target="/3D/3dmodel.model" Id="rel0" '
        'Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>\n')


def box(sx, sy, sz):
    """Axis-aligned box [0,sx]x[0,sy]x[0,sz], outward (CCW from outside) winding."""
    v = [(x * sx, y * sy, z * sz) for z in (0, 1) for y in (0, 1) for x in (0, 1)]
    # index = x + 2y + 4z
    t = [(0, 2, 3), (0, 3, 1),  # z=0, normal -z
         (4, 5, 7), (4, 7, 6),  # z=1
         (0, 1, 5), (0, 5, 4),  # y=0
         (2, 6, 7), (2, 7, 3),  # y=1
         (0, 4, 6), (0, 6, 2),  # x=0
         (1, 3, 7), (1, 7, 5)]  # x=1
    return v, t


def tet(a):
    """Right tetrahedron with legs a at the origin: V = a^3/6."""
    v = [(0, 0, 0), (a, 0, 0), (0, a, 0), (0, 0, a)]
    t = [(0, 2, 1), (0, 1, 3), (0, 3, 2), (1, 2, 3)]
    return v, t


def mesh_xml(v, t, tri_attrs=None):
    s = "<mesh><vertices>"
    s += "".join(f'<vertex x="{x!r}" y="{y!r}" z="{z!r}"/>' for x, y, z in v)
    s += "</vertices><triangles>"
    for i, (a, b, c) in enumerate(t):
        extra = (tri_attrs or {}).get(i, "")
        s += f'<triangle v1="{a}" v2="{b}" v3="{c}"{extra}/>'
    return s + "</triangles></mesh>"


def model(body, unit="millimeter", extra_ns="", req=""):
    r = f' requiredextensions="{req}"' if req else ""
    return (f'<?xml version="1.0" encoding="UTF-8"?>\n<model unit="{unit}" xml:lang="en-US" '
            f'xmlns="{CORE}"{extra_ns}{r}>{body}</model>\n')


def package(path, model_text, compression=zipfile.ZIP_DEFLATED, rels=RELS, part="3D/3dmodel.model"):
    with zipfile.ZipFile(path, "w", compression) as z:
        z.writestr("[Content_Types].xml", CT)
        if rels is not None:
            z.writestr("_rels/.rels", rels)
        z.writestr(part, model_text)


def main(out):
    os.makedirs(out, exist_ok=True)
    p = lambda n: os.path.join(out, n)

    # 1. inch units + item transform. Box 1 x 0.5 x 0.25 in, item rotated +90 deg about Z
    #    ((x,y) -> (-y,x), row-vector matrix "0 1 0 -1 0 0 0 0 1") and moved +2 in X.
    #    Expected mm: x [1.5, 2] in = [38.1, 50.8], y [0, 25.4], z [0, 6.35];
    #    V = 0.125 in^3 = 2048.383 mm^3; 12 triangles, closed.
    v, t = box(1.0, 0.5, 0.25)
    package(p("box_inch_rotated.3mf"), model(
        f'<resources><object id="1" type="model" name="Box">{mesh_xml(v, t)}</object></resources>'
        '<build><item objectid="1" transform="0 1 0 -1 0 0 0 0 1 2 0 0"/></build>', unit="inch"))

    # 2. components + a mirroring component transform, stored (uncompressed) entries.
    #    Object 1: tetrahedron legs 10 (V 1000/6). Object 2: components (1, identity) and
    #    (1, mirror X then +30 X: "-1 0 0 0 1 0 0 0 1 30 0 0") -> x in [20, 30].
    #    One build item (object 2): 2 components, closed, V = 2000/6 (winding flipped for
    #    the mirrored copy), bounds [0,30]x[0,10]x[0,10]. Item name "Pair".
    v, t = tet(10.0)
    package(p("components_mirror_stored.3mf"), model(
        f'<resources><object id="1" type="model">{mesh_xml(v, t)}</object>'
        '<object id="2" type="model" name="Pair"><components><component objectid="1"/>'
        '<component objectid="1" transform="-1 0 0 0 1 0 0 0 1 30 0 0"/></components></object>'
        '</resources><build><item objectid="2"/></build>'), compression=zipfile.ZIP_STORED)

    # 3. colours: basematerials id 5 with red 50% alpha and green; object 1 pid 5 pindex 0,
    #    object 2 pid 5 pindex 1 (box 10^3 at x 20). Triangle 0 of object 2 has p1="0":
    #    a per-triangle override that is reported, not imported. Materials-extension
    #    colorgroup id 6 (required extension "m") used by object 3: geometry imported, no
    #    colour. Expected colours [255,0,0,128], [0,255,0,255], None; 3 objects.
    v, t = box(10, 10, 10)
    v2 = [(x + 20, y, z) for x, y, z in v]
    v3 = [(x + 40, y, z) for x, y, z in v]
    package(p("colors.3mf"), model(
        '<resources><basematerials id="5"><base name="Red" displaycolor="#FF000080"/>'
        '<base name="Green" displaycolor="#00FF00"/></basematerials>'
        '<m:colorgroup id="6"><m:color color="#0000FF"/></m:colorgroup>'
        f'<object id="1" type="model" name="Red box" pid="5" pindex="0">{mesh_xml(v, t)}</object>'
        f'<object id="2" type="model" name="Green box" pid="5" pindex="1">'
        f'{mesh_xml(v2, t, {0: " p1=\"0\""})}</object>'
        f'<object id="3" type="model" name="Blue box" pid="6" pindex="0">{mesh_xml(v3, t)}</object>'
        '</resources><build><item objectid="1"/><item objectid="2"/><item objectid="3"/></build>',
        extra_ns=f' xmlns:m="{MAT}"', req="m"))

    # 4. required production extension: must be refused (multi-part models).
    v, t = box(1, 1, 1)
    package(p("requires_production.3mf"), model(
        f'<resources><object id="1">{mesh_xml(v, t)}</object></resources>'
        '<build><item objectid="1"/></build>', extra_ns=f' xmlns:p="{PROD}"', req="p"))

    # 5. triangle index out of range (8 vertices, index 8): malformed.
    v, t = box(1, 1, 1)
    package(p("bad_index.3mf"), model(
        f'<resources><object id="1">{mesh_xml(v, t[:-1] + [(1, 7, 8)])}</object></resources>'
        '<build><item objectid="1"/></build>'))

    # 6. no build items: nothing to import.
    package(p("no_build.3mf"), model(
        f'<resources><object id="1">{mesh_xml(v, t)}</object></resources><build/>'))

    # 7. DOCTYPE with an entity expansion ("billion laughs" seed): refused.
    package(p("doctype.3mf"),
            '<?xml version="1.0"?>\n<!DOCTYPE model [<!ENTITY a "aaaaaaaaaa">]>\n'
            f'<model unit="millimeter" xmlns="{CORE}"><metadata name="x">&a;</metadata>'
            f'<resources><object id="1">{mesh_xml(v, t)}</object></resources>'
            '<build><item objectid="1"/></build></model>\n')

    # 8. 2 MiB of XML comment padding (deflates to a few KB): accepted by default,
    #    refused before inflating with max_model_bytes = 1 MiB. 1 mm cube.
    pad = "<!--" + " " * (2 << 20) + "-->"
    package(p("padded_2mib.3mf"), model(
        f'{pad}<resources><object id="1">{mesh_xml(v, t)}</object></resources>'
        '<build><item objectid="1"/></build>'))

    # 9. a package whose central directory under-declares the model's size (a lying
    #    "zip bomb" header): must be refused, never inflated past the declared size.
    package(p("lying_size.3mf"), model(
        f'{pad}<resources><object id="1">{mesh_xml(v, t)}</object></resources>'
        '<build><item objectid="1"/></build>'))
    data = bytearray(open(p("lying_size.3mf"), "rb").read())
    name = b"3D/3dmodel.model"
    for sig, size_at in ((b"PK\x01\x02", 24), (b"PK\x03\x04", 22)):
        i = 0
        while (i := data.find(sig, i)) >= 0:
            nl_at = 28 if sig == b"PK\x01\x02" else 26
            hdr = 46 if sig == b"PK\x01\x02" else 30
            n = struct.unpack_from("<H", data, i + nl_at)[0]
            if bytes(data[i + hdr:i + hdr + n]) == name:
                struct.pack_into("<I", data, i + size_at, 1000)
            i += 4
    open(p("lying_size.3mf"), "wb").write(bytes(data))

    # 10. no _rels/.rels: tolerated with a warning (default part name). 1 mm cube.
    package(p("no_rels.3mf"), model(
        f'<resources><object id="1">{mesh_xml(v, t)}</object></resources>'
        '<build><item objectid="1"/></build>'), rels=None)


if __name__ == "__main__":
    main(sys.argv[1])
