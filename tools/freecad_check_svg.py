# Independent read of an SVG by FreeCAD's Draft importer (not Fission420's reader).
# flatpak run --env=F420_SVG=<abs path> --filesystem=<dir> --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_svg.py
# Prints FREECAD_RESULT {ok, edges, length (sum of edge lengths, mm), size [dx, dy] of all edges}
import os, json, FreeCAD
try:
    import importSVG
    doc = FreeCAD.newDocument("svgcheck")
    importSVG.insert(os.environ["F420_SVG"], doc.Name)
    doc.recompute()
    edges = [e for o in doc.Objects if hasattr(o, "Shape") for e in o.Shape.Edges]
    bb = None
    for e in edges:
        b = e.BoundBox
        bb = b if bb is None else bb.united(b)
    r = {"ok": True, "edges": len(edges), "length": sum(e.Length for e in edges),
         "size": [bb.XLength, bb.YLength] if bb else None}
    print("FREECAD_RESULT " + json.dumps(r))
except Exception as ex:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(ex)}))
