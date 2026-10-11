# Read DXF files with FreeCAD's C++ DXF importer (Import.readDXF) and report what it found.
# Run inside FreeCADCmd; F420_DXF is a ':'-separated list of absolute paths.
# One line per file: FREECAD_RESULT {"file", "edges": {type: n}, "layers", "bounds", "area",
# "closed", "arcs": [[cx, cy, r, sweep_deg]]}
import json
import math
import os

import FreeCAD
import Import
import Part

for path in os.environ["F420_DXF"].split(":"):
    doc = FreeCAD.newDocument()
    res = {"file": os.path.basename(path)}
    try:
        Import.readDXF(path, doc.Name, False)
        doc.recompute()
        edges = []
        layers = set()
        for o in doc.Objects:
            shape = getattr(o, "Shape", None)
            if shape is not None and not shape.isNull():
                edges.extend(shape.Edges)
            layers.add(o.Label)
        kinds = {}
        arcs = []
        for e in edges:
            k = type(e.Curve).__name__
            kinds[k] = kinds.get(k, 0) + 1
            if k == "Circle":
                c = e.Curve
                arcs.append([c.Center.x, c.Center.y, c.Radius,
                             math.degrees(e.LastParameter - e.FirstParameter)])
        res["edges"] = kinds
        res["arcs"] = sorted(arcs)
        res["objects"] = sorted(layers)
        if edges:
            comp = Part.Compound(edges)
            bb = comp.BoundBox
            res["bounds"] = [bb.XMin, bb.YMin, bb.XMax, bb.YMax]
            try:
                wire = Part.Wire(Part.__sortEdges__(edges))
                res["closed"] = wire.isClosed()
                res["area"] = Part.Face(wire).Area if wire.isClosed() else None
            except Exception as ex:  # several loops / open curves
                res["closed"] = False
                res["area"] = None
                res["wire_error"] = str(ex)
    except Exception as ex:
        res["error"] = str(ex)
    print("FREECAD_RESULT " + json.dumps(res))
    FreeCAD.closeDocument(doc.Name)
