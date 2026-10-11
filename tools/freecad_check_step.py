# flatpak run --env=F420_STEP=<abs path> --filesystem=<repo> --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_step.py
# Optional: --env=F420_EXPECT_SOLIDS=<N> adds "expect_solids_ok" (solid count == N).
# "solid_volumes" lists each solid's volume, sorted ascending.
import os, json, Part
s = Part.Shape()
try:
    s.read(os.environ["F420_STEP"])
    bb = s.optimalBoundingBox()
    r = {"ok": True, "solids": len(s.Solids), "faces": len(s.Faces),
        "isValid": s.isValid(), "volume": s.Volume, "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
        "surfaces": sorted({type(f.Surface).__name__ for f in s.Faces}),
        "solid_volumes": sorted(x.Volume for x in s.Solids)}
    exp = os.environ.get("F420_EXPECT_SOLIDS")
    if exp:
        r["expect_solids_ok"] = len(s.Solids) == int(exp)
    print("FREECAD_RESULT " + json.dumps(r))
except Exception as e:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(e)}))
