#!/usr/bin/env python3
"""Second independent DXF reader (ezdxf, MIT; installed only in a throwaway venv, never a
project dependency): report WCS geometry of every LINE/ARC/CIRCLE/LWPOLYLINE/POLYLINE in the
given files, including OCS -> WCS conversion for arcs with extrusion (0,0,-1).

usage: <venv>/bin/python -I tools/fixtures/dxf/ezdxf_check.py FILE...
"""
import json
import sys

import ezdxf
from ezdxf import path as ezpath

for fn in sys.argv[1:]:
    doc = ezdxf.readfile(fn)
    out = {"file": fn.rsplit("/", 1)[-1], "insunits": doc.header.get("$INSUNITS"), "arcs": []}
    paths = []
    for e in doc.modelspace():
        t = e.dxftype()
        if t == "ARC":
            ocs = e.ocs()
            c = ocs.to_wcs(e.dxf.center)
            out["arcs"].append({
                "center_wcs": [round(c.x, 9), round(c.y, 9)],
                "start_wcs": [round(e.start_point.x, 9), round(e.start_point.y, 9)],
                "end_wcs": [round(e.end_point.x, 9), round(e.end_point.y, 9)],
                "extrusion": list(e.dxf.extrusion),
            })
        if t in ("LINE", "ARC", "LWPOLYLINE", "POLYLINE", "CIRCLE"):
            paths.append(ezpath.make_path(e))
    lo, hi = None, None
    for p in paths:
        for q in p.flattening(1e-6):
            lo = (q.x, q.y) if lo is None else (min(lo[0], q.x), min(lo[1], q.y))
            hi = (q.x, q.y) if hi is None else (max(hi[0], q.x), max(hi[1], q.y))
    out["bounds"] = [lo[0], lo[1], hi[0], hi[1]] if lo else None
    print("EZDXF_RESULT " + json.dumps(out))
