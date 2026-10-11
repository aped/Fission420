#!/usr/bin/env python3
"""Independent 3MF reader (Python stdlib zipfile + xml.etree; shares no code with
Fission420). Prints one JSON line per file:
  {"file", "unit", "objects": [{"id", "name", "vertices", "triangles", "closed",
   "volume", "color"}], "items": [objectid...]}
Volumes are in file units (divergence theorem over each object's triangles); "closed"
means every undirected edge is used exactly twice with opposite directions.
usage: check_3mf.py FILE.3mf...
"""
import json
import sys
import xml.etree.ElementTree as ET
import zipfile
from collections import Counter

CORE = "{http://schemas.microsoft.com/3dmanufacturing/core/2015/02}"


def check(path):
    with zipfile.ZipFile(path) as z:
        rels = ET.fromstring(z.read("_rels/.rels"))
        target = [r.get("Target") for r in rels
                  if r.get("Type") == "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"]
        model = ET.fromstring(z.read(target[0].lstrip("/")))
    colors = {}
    for bm in model.iter(CORE + "basematerials"):
        colors[bm.get("id")] = [b.get("displaycolor") for b in bm.findall(CORE + "base")]
    objs = []
    for o in model.iter(CORE + "object"):
        vs = [(float(v.get("x")), float(v.get("y")), float(v.get("z")))
              for v in o.iter(CORE + "vertex")]
        ts = [(int(t.get("v1")), int(t.get("v2")), int(t.get("v3")))
              for t in o.iter(CORE + "triangle")]
        edges = Counter()
        vol = 0.0
        for a, b, c in ts:
            for u, w in ((a, b), (b, c), (c, a)):
                edges[(u, w)] += 1
            (x1, y1, z1), (x2, y2, z2), (x3, y3, z3) = vs[a], vs[b], vs[c]
            vol += (x1 * (y2 * z3 - y3 * z2) - x2 * (y1 * z3 - y3 * z1) + x3 * (y1 * z2 - y2 * z1)) / 6
        closed = all(n == 1 and edges.get((w, u)) == 1 for (u, w), n in edges.items())
        pid, pindex = o.get("pid"), o.get("pindex")
        color = colors.get(pid, [None])[int(pindex or 0)] if pid else None
        objs.append({"id": o.get("id"), "name": o.get("name"), "vertices": len(vs),
                     "triangles": len(ts), "closed": closed, "volume": vol, "color": color})
    items = [i.get("objectid") for i in model.iter(CORE + "item")]
    return {"file": path.rsplit("/", 1)[-1], "unit": model.get("unit", "millimeter"),
            "objects": objs, "items": items}


for p in sys.argv[1:]:
    print(json.dumps(check(p)))
