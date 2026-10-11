# FreeCAD cross-check for print exports (STL / 3MF), independent of check_print_mesh.py.
# Run with (from the repo root; F420_PRINT = absolute path of the .stl or .3mf):
#   tools/heavy.sh timeout 180 flatpak run --env=F420_PRINT=/abs/file.stl \
#     --filesystem=/abs/repo --command=FreeCADCmd org.freecad.FreeCAD tools/print/freecad_check_print.py
# Prints one line "FREECAD_RESULT {json}" with per-object mesh facts as FreeCAD's Mesh
# module sees them. 3MF goes through Mesh.insert (FreeCAD's own 3MF reader, if present);
# an import failure is reported as {"error": ...}.
# Observed with FreeCAD 1.1.4 (artifacts/run4/print/validator/freecad_*.txt): its 3MF reader
# merges all build items into ONE mesh object (build transforms applied) and IGNORES the
# model unit attribute (a unit="inch" 1-inch cube reports volume 1.0). Per-object and
# non-millimeter 3MF results must therefore come from check_print_mesh.py, not FreeCAD.
import os, json, FreeCAD, Mesh

path = os.environ["F420_PRINT"]
out = {"file": path, "freecad": ".".join(FreeCAD.Version()[:3])}
try:
    doc = FreeCAD.newDocument("f420print")
    Mesh.insert(path, doc.Name)
    objs = []
    for o in doc.Objects:
        if not hasattr(o, "Mesh"):
            continue
        m = o.Mesh
        if hasattr(o, "Placement") and not o.Placement.isIdentity():
            m = m.copy()
            m.transform(o.Placement.toMatrix())
        bb = m.BoundBox
        objs.append({"name": o.Label, "points": m.CountPoints, "facets": m.CountFacets,
                     "isSolid": m.isSolid(), "hasNonManifolds": m.hasNonManifolds(),
                     "hasSelfIntersections": m.hasSelfIntersections(),
                     "components": m.countComponents(), "volume": m.Volume, "area": m.Area,
                     "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax]})
    out["objects"] = objs
    if not objs:
        out["error"] = "no mesh objects imported"
except Exception as e:  # report, don't crash: the caller greps FREECAD_RESULT
    out["error"] = "%s: %s" % (type(e).__name__, e)
print("FREECAD_RESULT " + json.dumps(out))
