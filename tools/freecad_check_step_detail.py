# Independent STEP solid check with FreeCAD/OpenCascade (no R-CAD code involved).
# flatpak run --env=RCAD_STEP=<abs path> [--env=RCAD_PROBES='[[x,y,z],...]'] \
#   --filesystem=<dir> --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_step_detail.py
# Prints one line "FREECAD_RESULT {json}" with:
#   solids, isValid, volume, solid_volumes (sorted), bounds [xmin,ymin,zmin,xmax,ymax,zmax]
#   (optimalBoundingBox), surfaces (OCC surface type names),
#   z_inner_loops: for every planar face with normal +-Z, each non-outer wire as
#                  {"z", "cx", "cy", "area"} (centroid/area of the planar face it bounds),
#   probes: [[x, y, z, inside]] from Shape.isInside(point, 1e-7, True) per RCAD_PROBES point.
import json
import os

import FreeCAD
import Part

res = {"ok": False}
try:
    s = Part.Shape()
    s.read(os.environ["RCAD_STEP"])
    bb = s.optimalBoundingBox()
    res.update({
        "ok": True, "solids": len(s.Solids), "faces": len(s.Faces), "isValid": s.isValid(),
        "volume": s.Volume, "solid_volumes": sorted(x.Volume for x in s.Solids),
        "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
        "surfaces": sorted({type(f.Surface).__name__ for f in s.Faces}),
    })
    loops = []
    for f in s.Faces:
        if type(f.Surface).__name__ != "Plane":
            continue
        u0, u1, v0, v1 = f.ParameterRange
        n = f.normalAt(0.5 * (u0 + u1), 0.5 * (v0 + v1))
        if abs(n.x) + abs(n.y) > 1e-9:
            continue
        outer = f.OuterWire
        for w in f.Wires:
            if w.isSame(outer):
                continue
            inner = Part.Face(w)
            c = inner.CenterOfMass
            loops.append({"z": c.z, "cx": c.x, "cy": c.y, "area": inner.Area})
    res["z_inner_loops"] = sorted(loops, key=lambda l: l["z"])
    probes = json.loads(os.environ.get("RCAD_PROBES", "[]"))
    res["probes"] = [[x, y, z, s.isInside(FreeCAD.Vector(x, y, z), 1e-7, True)]
                     for x, y, z in probes]
except Exception as e:  # report, never crash the harness
    res["error"] = str(e)
print("FREECAD_RESULT " + json.dumps(res))
