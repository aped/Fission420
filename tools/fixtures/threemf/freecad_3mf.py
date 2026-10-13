# FreeCAD (FreeCADCmd) helper for 3MF fixtures and validation; no R-CAD code.
# RCAD_MODE=generate: write RCAD_OUT/freecad_two_boxes.3mf (Mesh module 3MF writer):
#   object A = Part.makeBox(10,10,10) at the origin, object B = Part.makeBox(20,10,5) at
#   (50,0,0), both tessellated (12 triangles each); unit millimeter.
# RCAD_MODE=read: read every ':'-separated path in RCAD_3MF with Mesh.Mesh() (FreeCAD's
#   3MF reader; it merges all build items into one mesh) and print one line per file:
#   FREECAD_RESULT {"file", "points", "facets", "volume", "isSolid", "bounds"} or {"error"}.
import json
import os

import FreeCAD
import Mesh
import Part

mode = os.environ.get("RCAD_MODE", "read")
if mode == "generate":
    doc = FreeCAD.newDocument()
    a = doc.addObject("Mesh::Feature", "A")
    a.Mesh = Mesh.Mesh(Part.makeBox(10, 10, 10).tessellate(0.1))
    b = doc.addObject("Mesh::Feature", "B")
    b.Mesh = Mesh.Mesh(Part.makeBox(20, 10, 5, FreeCAD.Vector(50, 0, 0)).tessellate(0.1))
    path = os.path.join(os.environ["RCAD_OUT"], "freecad_two_boxes.3mf")
    Mesh.export([a, b], path)
    print("FREECAD_RESULT " + json.dumps({"written": path, "bytes": os.path.getsize(path)}))
else:
    for path in os.environ["RCAD_3MF"].split(":"):
        r = {"file": os.path.basename(path)}
        try:
            m = Mesh.Mesh(path)
            bb = m.BoundBox
            r.update({"points": m.CountPoints, "facets": m.CountFacets, "volume": m.Volume,
                      "isSolid": m.isSolid(),
                      "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax]})
        except Exception as e:  # report, never crash the harness
            r["error"] = str(e)
        print("FREECAD_RESULT " + json.dumps(r))
