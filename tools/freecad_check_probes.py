# Independent STEP check (FreeCAD/OpenCascade), with point probes.
# flatpak run --env=RCAD_STEP=<abs path> --env=RCAD_PROBES='[[x,y,z],...]' --filesystem=<dir> \
#   --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_probes.py
# Prints FREECAD_RESULT {ok, solids, isValid, solid_volumes (sorted), bounds, inside: [bool per probe]}
# A probe is "inside" when it lies strictly inside some solid (tolerance 1e-6 mm).
import os, json, Part, FreeCAD
s = Part.Shape()
try:
    s.read(os.environ["RCAD_STEP"])
    bb = s.optimalBoundingBox()
    probes = json.loads(os.environ.get("RCAD_PROBES", "[]"))
    inside = [any(x.isInside(FreeCAD.Vector(*p), 1e-6, False) for x in s.Solids) for p in probes]
    r = {"ok": True, "solids": len(s.Solids), "isValid": s.isValid(),
         "solid_volumes": sorted(x.Volume for x in s.Solids),
         "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax], "inside": inside}
    print("FREECAD_RESULT " + json.dumps(r))
except Exception as e:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(e)}))
