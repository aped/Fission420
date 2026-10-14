# FreeCAD (independent reader) check of a sheet-metal flat-pattern DXF.
# flatpak run --env=RCAD_DXF=<abs path> --filesystem=<repo> --command=FreeCADCmd \
#   org.freecad.FreeCAD tools/freecad_check_flat_dxf.py
# Prints "FREECAD_RESULT {json}": per layer the straight-edge count, total length and
# x/y extent as FreeCAD's DXF importer reads them; the CUT edges sorted into closed
# wires and their face area (outer minus holes); the header $INSUNITS.
import os, json, re
import FreeCAD as App
try:
    import Import, Part
    path = os.environ["RCAD_DXF"]
    head = open(path).read()
    m = re.search(r"\$INSUNITS\s*\n\s*70\s*\n\s*(\d+)", head)
    doc = App.newDocument("flat")
    Import.readDXF(path)
    member_of = {}
    for o in doc.Objects:
        if hasattr(o, "Group") and o.Label != "Layers":
            for mm in o.Group:
                member_of[mm.Name] = o.Label
    layers, edges = {}, {}
    for o in doc.Objects:
        if hasattr(o, "Group") or not hasattr(o, "Shape") or o.Shape.isNull():
            continue
        layer = member_of.get(o.Name) or getattr(o, "Layer", None) or "?"
        e = layers.setdefault(layer, {"lines": 0, "arcs": 0, "length": 0.0,
                                      "xmin": 1e300, "xmax": -1e300, "ymin": 1e300, "ymax": -1e300})
        for ed in o.Shape.Edges:
            edges.setdefault(layer, []).append(ed)
            if type(ed.Curve).__name__ in ("Line", "LineSegment"):
                e["lines"] += 1
            else:
                e["arcs"] += 1
            e["length"] += ed.Length
            bb = ed.BoundBox
            e["xmin"] = min(e["xmin"], bb.XMin); e["xmax"] = max(e["xmax"], bb.XMax)
            e["ymin"] = min(e["ymin"], bb.YMin); e["ymax"] = max(e["ymax"], bb.YMax)
    area = None
    if "CUT" in edges:
        wires = [Part.Wire(w) for w in Part.sortEdges(edges["CUT"])]
        closed = all(w.isClosed() for w in wires)
        wires.sort(key=lambda w: -Part.Face(w).Area)
        area = Part.Face(wires[0]).Area - sum(Part.Face(w).Area for w in wires[1:])
        layers["CUT"]["wires"] = len(wires)
        layers["CUT"]["closed"] = closed
    print("FREECAD_RESULT " + json.dumps({"ok": True, "layers": layers, "cut_area": area,
        "insunits": m.group(1) if m else None}))
except Exception as ex:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": repr(ex)}))
