# FreeCAD (independent reader) check of a drawing DXF exported by R-CAD.
# flatpak run --env=RCAD_DXF=<abs path> --filesystem=<repo> --command=FreeCADCmd \
#   org.freecad.FreeCAD tools/freecad_check_drawing_dxf.py
# Prints "FREECAD_RESULT {json}": per layer the number of straight edges, circles, arcs
# and the total length of the straight edges, as FreeCAD's built-in DXF importer reads them.
import os, json
import FreeCAD as App
try:
    import Import
    doc = App.newDocument("drawing")
    Import.readDXF(os.environ["RCAD_DXF"])
    layers = {}
    texts = []
    # Layer membership: FreeCAD puts imported entities into layer objects
    # (members in their Group) under a "Layers" container.
    member_of = {}
    for o in doc.Objects:
        if hasattr(o, "Group") and o.Label != "Layers":
            for m in o.Group:
                member_of[m.Name] = o.Label
    for o in doc.Objects:
        if hasattr(o, "Group"):
            continue
        layer = member_of.get(o.Name) or getattr(o, "Layer", None) or "?"
        e = layers.setdefault(layer, {"lines": 0, "circles": 0, "arcs": 0, "line_length": 0.0})
        if hasattr(o, "Shape") and not o.Shape.isNull():
            for ed in o.Shape.Edges:
                t = type(ed.Curve).__name__
                if t in ("Line", "LineSegment"):
                    e["lines"] += 1
                    e["line_length"] += ed.Length
                elif t == "Circle":
                    if ed.isClosed():
                        e["circles"] += 1
                    else:
                        e["arcs"] += 1
        if hasattr(o, "LabelText"):
            texts.extend(list(o.LabelText))
        elif hasattr(o, "Text"):
            texts.extend(list(o.Text) if isinstance(o.Text, (list, tuple)) else [str(o.Text)])
    print("FREECAD_RESULT " + json.dumps({"ok": True, "layers": layers, "texts": texts,
        "objects": len(doc.Objects)}))
except Exception as ex:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": repr(ex)}))
