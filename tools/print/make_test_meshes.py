#!/usr/bin/env python3
"""Generate STL/3MF fixtures for tools/print/check_print_mesh.py (stdlib only).

usage: python3 -I tools/print/make_test_meshes.py OUT_DIR
Writes the fixtures plus spec.json (expected values) into OUT_DIR.
"""
import json
import math
import os
import struct
import sys
import zipfile


def box(x0, y0, z0, x1, y1, z1):
    """Outward-wound triangles of an axis-aligned box."""
    quads = [
        [(x0, y0, z0), (x0, y1, z0), (x1, y1, z0), (x1, y0, z0)],  # -z
        [(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)],  # +z
        [(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)],  # -y
        [(x0, y1, z0), (x0, y1, z1), (x1, y1, z1), (x1, y1, z0)],  # +y
        [(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)],  # -x
        [(x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)],  # +x
    ]
    tris = []
    for a, b, c, d in quads:
        tris.append((a, b, c))
        tris.append((a, c, d))
    return tris


def cylinder(r, h, n):
    """Side quads, bottom cap fanned from a centre vertex, top cap fanned from
    ring vertex 0 (no centre) so it contains chords spanning the circle."""
    ang = [2 * math.pi * k / n for k in range(n)]
    bot = [(r * math.cos(a), r * math.sin(a), 0.0) for a in ang]
    top = [(r * math.cos(a), r * math.sin(a), h) for a in ang]
    tris = []
    for k in range(n):
        k1 = (k + 1) % n
        tris.append((bot[k], bot[k1], top[k1]))
        tris.append((bot[k], top[k1], top[k]))
        tris.append(((0.0, 0.0, 0.0), bot[k1], bot[k]))
    for k in range(1, n - 1):
        tris.append((top[0], top[k], top[k + 1]))
    return tris


def write_stl(path, tris, header=b"make_test_meshes fixture"):
    with open(path, "wb") as f:
        f.write(header.ljust(80, b"\0")[:80])
        f.write(struct.pack("<I", len(tris)))
        for t in tris:
            f.write(struct.pack("<3f", 0.0, 0.0, 0.0))
            for p in t:
                f.write(struct.pack("<3f", *p))
            f.write(b"\0\0")


def write_ascii_stl(path, tris):
    with open(path, "w") as f:
        f.write("solid cube\n")
        for t in tris:
            f.write(" facet normal 0 0 0\n  outer loop\n")
            for p in t:
                f.write("   vertex %r %r %r\n" % p)
            f.write("  endloop\n endfacet\n")
        f.write("endsolid cube\n")


def mesh_xml(tris):
    idx, verts, out = {}, [], []
    for t in tris:
        ids = []
        for p in t:
            if p not in idx:
                idx[p] = len(verts)
                verts.append(p)
            ids.append(idx[p])
        out.append(ids)
    vs = "".join('<vertex x="%r" y="%r" z="%r"/>' % p for p in verts)
    ts = "".join('<triangle v1="%d" v2="%d" v3="%d"/>' % tuple(t) for t in out)
    return "<mesh><vertices>%s</vertices><triangles>%s</triangles></mesh>" % (vs, ts)


def write_3mf(path, unit, objects, items, metadata=None):
    """objects: list of (id, name, tris | [(objectid, transform|None)]);
    items: list of (objectid, transform|None); unit None omits the attribute."""
    ua = ' unit="%s"' % unit if unit else ""
    md = "".join('<metadata name="%s">%s</metadata>' % kv for kv in (metadata or {}).items())
    res = []
    for oid, name, body in objects:
        if body and isinstance(body[0], tuple) and isinstance(body[0][0], tuple):
            inner = mesh_xml(body)
        else:
            inner = "<components>%s</components>" % "".join(
                '<component objectid="%s"%s/>' % (c, ' transform="%s"' % tr if tr else "")
                for c, tr in body)
        res.append('<object id="%s" name="%s" type="model">%s</object>' % (oid, name, inner))
    build = "".join('<item objectid="%s"%s/>' % (o, ' transform="%s"' % tr if tr else "")
                    for o, tr in items)
    model = ('<?xml version="1.0" encoding="UTF-8"?>\n'
             '<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"%s>'
             '%s<resources>%s</resources><build>%s</build></model>' % (ua, md, "".join(res), build))
    ct = ('<?xml version="1.0" encoding="UTF-8"?>\n'
          '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
          '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
          '<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>'
          '</Types>')
    rels = ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Target="/3D/3dmodel.model" Id="rel0" '
            'Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>')
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", ct)
        z.writestr("_rels/.rels", rels)
        z.writestr("3D/3dmodel.model", model)


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    P = lambda n: os.path.join(out, n)

    cube = box(0, 0, 0, 10, 10, 10)
    write_stl(P("cube.stl"), cube)
    write_ascii_stl(P("cube_ascii.stl"), cube)
    write_stl(P("cube_open.stl"), cube[:-1])
    flipped = list(cube)
    a, b, c = flipped[3]
    flipped[3] = (a, c, b)
    write_stl(P("cube_flipped.stl"), flipped)
    write_stl(P("cube_inward.stl"), [(a, c, b) for a, b, c in cube])
    write_stl(P("two_cubes_edge.stl"), cube + box(10, 10, 0, 20, 20, 10))
    write_stl(P("two_cubes_vertex.stl"), cube + box(10, 10, 10, 20, 20, 20))
    with open(P("cube.stl"), "rb") as f:
        data = f.read()
    with open(P("cube_truncated.stl"), "wb") as f:
        f.write(data[:-10])

    R, H = 10.0, 10.0
    n_cyl = {}
    for n in (16, 64):
        name = "cylinder_r10_h10_n%d.stl" % n
        write_stl(P(name), cylinder(R, H, n))
        n_cyl[name] = n

    # plate: Base 20x10x5 at origin; Lid 20x10x2 rotated +90 deg about z and moved.
    # Row-vector 3MF transform: x' = x*m00 + y*m10 + z*m20 + m30 ... so a +90 deg
    # z rotation (x' = -y, y' = x) is m00=0 m01=1 m10=-1 m11=0.
    lid_tr = "0 1 0 -1 0 0 0 0 1 40 5 0"
    write_3mf(P("plate_mm.3mf"), "millimeter",
              [(1, "Base", box(0, 0, 0, 20, 10, 5)), (2, "Lid", box(0, 0, 0, 20, 10, 2))],
              [(1, None), (2, lid_tr)], {"Title": "validator fixture", "Designer": "make_test_meshes"})
    write_3mf(P("plate_inch.3mf"), "inch", [(1, "InchCube", box(0, 0, 0, 1, 1, 1))], [(1, None)])
    # nested components, no unit attribute (defaults to mm, flagged), item lifted to z=5
    write_3mf(P("plate_components.3mf"), None,
              [(1, "Unit", box(0, 0, 0, 10, 10, 10)),
               (2, "Pair", [(1, None), (1, "1 0 0 0 1 0 0 0 1 20 0 0")]),
               (3, "Assembly", [(2, "1 0 0 0 1 0 0 0 1 5 5 0")])],
              [(3, "1 0 0 0 1 0 0 0 1 0 0 5")])

    # malformed 3MF: triangle index out of range
    write_3mf(P("bad_index.3mf"), "millimeter", [(1, "Bad", box(0, 0, 0, 1, 1, 1))], [(1, None)])
    with zipfile.ZipFile(P("bad_index.3mf")) as z:
        parts = {n: z.read(n) for n in z.namelist()}
    parts["3D/3dmodel.model"] = parts["3D/3dmodel.model"].replace(b'v3="7"', b'v3="99"', 1)
    with zipfile.ZipFile(P("bad_index.3mf"), "w") as z:
        for n, b in parts.items():
            z.writestr(n, b)

    spec = {"files": {
        "cube.stl": {"objects": 1, "triangles": 12, "volume": [1000, 1e-6], "area": [600, 1e-6],
                     "dims": [[10, 10, 10], 1e-6], "min_z": [0, 1e-4], "closed": True},
        "cube_ascii.stl": {"objects": 1, "volume": [1000, 1e-6], "closed": True},
        "plate_mm.3mf": {"unit": "millimeter", "objects": 2, "names": ["Base", "Lid"],
                         "volume": [1400, 1e-6],
                         "per_object": {
                             "Base": {"volume": [1000, 1e-6], "dims": [[20, 10, 5], 1e-6],
                                      "bounds": [[0, 0, 0], [20, 10, 5], 1e-6], "min_z": [0, 1e-4]},
                             "Lid": {"volume": [400, 1e-6], "dims": [[10, 20, 2], 1e-6],
                                     "bounds": [[30, 5, 0], [40, 25, 2], 1e-6], "min_z": [0, 1e-4]}},
                         "within_bed": [[0, 0], [256, 256], 256]},
        "plate_inch.3mf": {"unit": "inch", "objects": 1, "closed": True,
                           "per_object": {"InchCube": {"volume": [25.4 ** 3, 1e-6],
                                                       "dims": [[25.4, 25.4, 25.4], 1e-9]}}},
        "plate_components.3mf": {"unit": "millimeter", "objects": 1, "names": ["Assembly"],
                                 "per_object": {"Assembly": {
                                     "volume": [2000, 1e-6], "bounds": [[5, 5, 5], [35, 15, 15], 1e-9]}}},
    }}
    for name, n in n_cyl.items():
        sag = R * (1 - math.cos(math.pi / n))
        vol = 0.5 * n * R * R * math.sin(2 * math.pi / n) * H
        spec["files"][name] = {
            "objects": 1, "closed": True, "volume": [vol, 1e-3],
            "dims": [[2 * R, 2 * R, H], 1e-4],  # N divisible by 4: ring hits +/-x and +/-y
            "circle": {"center": [0, 0], "r": R, "z": [0, H], "chord": sag * 1.0001},
            "_expected_sagitta": sag, "_segments": n}
    with open(P("spec.json"), "w") as f:
        json.dump(spec, f, indent=1)
    print("wrote fixtures to %s" % out)
    for name, n in n_cyl.items():
        print("  %s: expected sagitta R(1-cos(pi/N)) = %.9g, volume %.9g"
              % (name, R * (1 - math.cos(math.pi / n)), 0.5 * n * R * R * math.sin(2 * math.pi / n) * H))
    return 0


if __name__ == "__main__":
    sys.exit(main())
