#!/usr/bin/env python3
"""Independent checks of the mesh-edit exports of tools/fixtures/mesh_edit_workflow.json
(Python stdlib only; shares no code with R-CAD). The source is FreeCAD's ASCII sphere
tools/fixtures/corpus/m2_sphere_ascii.stl.

* plane cut at z = 2, both sides: each half watertight; lower max z = 2, upper min z = 2;
  volumes sum to the source volume.
* decimation (max deviation 0.05 mm): watertight; fewer triangles; the deviation measured
  here by brute-force exact point-to-triangle distance (source vertices and centroids to
  the result, result vertices, edge midpoints and centroids to the source) is <= 0.05.
* transform (scale 2, 1, 0.5; 30 deg about Z; +10 X): watertight; volume = source x 1;
  bounds equal the bounds of the source vertices transformed here (float32 tolerance).

usage: check_mesh_edit.py SOURCE.stl LOWER.stl UPPER.stl DECIMATED.stl TRANSFORMED.stl
"""
import json
import math
import struct
import sys
from collections import defaultdict


def read_stl(path):
    data = open(path, "rb").read()
    if data[:5] == b"solid" and b"facet" in data[:400]:
        tris, cur = [], []
        for line in data.decode().splitlines():
            w = line.split()
            if w and w[0] == "vertex":
                cur.append(tuple(float(x) for x in w[1:4]))
                if len(cur) == 3:
                    tris.append(tuple(cur))
                    cur = []
        return tris
    n = struct.unpack_from("<I", data, 80)[0]
    assert len(data) == 84 + 50 * n, "binary size mismatch"
    out = []
    for i in range(n):
        v = struct.unpack_from("<12f", data, 84 + 50 * i)
        out.append((v[3:6], v[6:9], v[9:12]))
    return out


def watertight(tris):
    edges = defaultdict(int)
    for t in tris:
        for k in range(3):
            edges[(t[k], t[(k + 1) % 3])] += 1
    return all(c == 1 and edges.get((b, a), 0) == 1 for (a, b), c in edges.items())


def volume(tris):
    v = 0.0
    for p, q, r in tris:
        v += (p[0] * (q[1] * r[2] - q[2] * r[1]) - p[1] * (q[0] * r[2] - q[2] * r[0])
              + p[2] * (q[0] * r[1] - q[1] * r[0])) / 6.0
    return v


def bounds(tris):
    pts = [v for t in tris for v in t]
    return [min(p[i] for p in pts) for i in range(3)], [max(p[i] for p in pts) for i in range(3)]


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def seg_dist(p, a, b):
    ab = sub(b, a)
    l2 = dot(ab, ab)
    t = 0.0 if l2 == 0 else max(0.0, min(1.0, dot(sub(p, a), ab) / l2))
    q = (a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2])
    d = sub(p, q)
    return math.sqrt(dot(d, d))


def point_tri(p, t):
    """Exact distance: inside the prism -> plane distance, else nearest edge."""
    a, b, c = t
    ab, ac = sub(b, a), sub(c, a)
    n = (ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0])
    nn = dot(n, n)
    if nn > 0:
        # Barycentric coordinates of the projection.
        ap = sub(p, a)
        d00, d01, d11 = dot(ab, ab), dot(ab, ac), dot(ac, ac)
        d20, d21 = dot(ap, ab), dot(ap, ac)
        den = d00 * d11 - d01 * d01
        v = (d11 * d20 - d01 * d21) / den
        w = (d00 * d21 - d01 * d20) / den
        if v >= 0 and w >= 0 and v + w <= 1:
            return abs(dot(ap, n)) / math.sqrt(nn)
    return min(seg_dist(p, a, b), seg_dist(p, b, c), seg_dist(p, c, a))


def dist_to(p, tris):
    return min(point_tri(p, t) for t in tris)


def centroid(t):
    return tuple(sum(v[i] for v in t) / 3 for i in range(3))


def main():
    src, lower, upper, dec, tr = (read_stl(f) for f in sys.argv[1:6])
    res, ok = {}, True

    def check(name, cond, detail):
        nonlocal ok
        res[name] = {"pass": bool(cond), "detail": detail}
        ok &= bool(cond)

    v0 = volume(src)
    check("source watertight", watertight(src), len(src))
    vl, vu = volume(lower), volume(upper)
    (llo, lhi), (ulo, uhi) = bounds(lower), bounds(upper)
    check("cut halves watertight", watertight(lower) and watertight(upper), [len(lower), len(upper)])
    check("cut volumes sum to the source", abs(vl + vu - v0) < 1e-3, [vl, vu, v0])
    check("lower max z = 2, upper min z = 2", abs(lhi[2] - 2) < 1e-5 and abs(ulo[2] - 2) < 1e-5,
          [lhi[2], ulo[2]])

    check("decimated watertight", watertight(dec), len(dec))
    check("decimated has fewer triangles", len(dec) < len(src), [len(dec), len(src)])
    s_samples = list({v for t in src for v in t}) + [centroid(t) for t in src]
    r_samples = list({v for t in dec for v in t}) + [centroid(t) for t in dec]
    r_samples += [tuple((t[k][i] + t[(k + 1) % 3][i]) / 2 for i in range(3)) for t in dec for k in range(3)]
    d1 = max(dist_to(p, dec) for p in s_samples)
    d2 = max(dist_to(p, src) for p in r_samples)
    # float32 storage of both files adds ~1e-6 mm.
    check("decimation deviation <= 0.05 (sampled, brute force)", max(d1, d2) <= 0.05 + 1e-5,
          {"source_to_result": d1, "result_to_source": d2})
    check("decimated volume within area x deviation", abs(volume(dec) - v0) <= 810 * 0.05,
          [volume(dec), v0])

    c, s = math.cos(math.radians(30)), math.sin(math.radians(30))
    moved = [(2 * x * c - y * s + 10, 2 * x * s + y * c, 0.5 * z) for t in src for (x, y, z) in t]
    mlo = [min(p[i] for p in moved) for i in range(3)]
    mhi = [max(p[i] for p in moved) for i in range(3)]
    tlo, thi = bounds(tr)
    check("transformed watertight", watertight(tr), len(tr))
    check("transformed volume = source x (2 x 1 x 0.5)", abs(volume(tr) - v0) < 1e-3, [volume(tr), v0])
    check("transformed bounds = source transformed here",
          max(abs(a - b) for a, b in zip(tlo + thi, mlo + mhi)) < 1e-4, [tlo, thi, mlo, mhi])
    for k, v in res.items():
        print(("PASS: " if v["pass"] else "FAIL: ") + k + "  " + json.dumps(v["detail"]))
    return 0 if ok else 1


sys.exit(main())
