#!/usr/bin/env python3
"""Independent binary-STL checker (does not share code with the Rust writer).

Welds vertices by exact float32 equality, then checks every undirected edge is
used by exactly two triangles in opposite directions, counts connected
components, and computes the divergence-theorem volume and bounds.
usage: check_stl.py FILE.stl [expected_volume]
"""
import struct, sys, json
from collections import defaultdict

def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    n = struct.unpack_from("<I", data, 80)[0]
    if len(data) != 84 + 50 * n:
        print(json.dumps({"ok": False, "error": f"size {len(data)} != 84+50*{n}"})); return 1
    idx, verts, tris = {}, [], []
    for i in range(n):
        vals = struct.unpack_from("<12f", data, 84 + 50 * i)
        t = []
        for k in range(3):
            p = vals[3 + 3 * k: 6 + 3 * k]
            if p not in idx:
                idx[p] = len(verts); verts.append(p)
            t.append(idx[p])
        tris.append(t)
    edges = defaultdict(int)
    for a, b, c in tris:
        for u, v in ((a, b), (b, c), (c, a)):
            edges[(u, v)] += 1
    bad = sum(1 for (u, v), c in edges.items() if c != 1 or edges.get((v, u), 0) != 1)
    # components via union-find
    parent = list(range(len(verts)))
    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]; x = parent[x]
        return x
    for a, b, c in tris:
        for u in (b, c):
            ra, ru = find(a), find(u)
            if ra != ru: parent[ra] = ru
    comps = len({find(t[0]) for t in tris})
    vol = 0.0
    for a, b, c in tris:
        p, q, r = verts[a], verts[b], verts[c]
        vol += (p[0]*(q[1]*r[2]-q[2]*r[1]) - p[1]*(q[0]*r[2]-q[2]*r[0]) + p[2]*(q[0]*r[1]-q[1]*r[0])) / 6.0
    lo = [min(v[i] for v in verts) for i in range(3)]
    hi = [max(v[i] for v in verts) for i in range(3)]
    out = {"file": path, "triangles": n, "vertices": len(verts), "bad_edges": bad,
           "watertight": bad == 0, "components": comps, "volume": vol, "min": lo, "max": hi}
    ok = bad == 0 and comps == 1
    if len(sys.argv) > 2:
        exp = float(sys.argv[2]); out["expected_volume"] = exp
        out["rel_err"] = abs(vol - exp) / exp; ok = ok and out["rel_err"] <= 1e-3
    out["ok"] = ok
    print(json.dumps(out))
    return 0 if ok else 1

sys.exit(main())
