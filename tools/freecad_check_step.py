# flatpak run --env=F420_STEP=<abs path> --filesystem=<repo> --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_step.py
import os, json, Part
s = Part.Shape()
try:
    s.read(os.environ["F420_STEP"])
    bb = s.optimalBoundingBox()
    print("FREECAD_RESULT " + json.dumps({"ok": True, "solids": len(s.Solids), "faces": len(s.Faces),
        "isValid": s.isValid(), "volume": s.Volume, "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
        "surfaces": sorted({type(f.Surface).__name__ for f in s.Faces})}))
except Exception as e:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(e)}))
