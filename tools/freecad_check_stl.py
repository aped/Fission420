# Run with: flatpak run --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_stl.py  (STL path via RCAD_STL env)
import os, json, Mesh
m = Mesh.Mesh(os.environ["RCAD_STL"])
bb = m.BoundBox
print("FREECAD_RESULT " + json.dumps({
    "points": m.CountPoints, "facets": m.CountFacets, "isSolid": m.isSolid(),
    "hasNonManifolds": m.hasNonManifolds(), "hasSelfIntersections": m.hasSelfIntersections(),
    "volume": m.Volume, "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax]}))
